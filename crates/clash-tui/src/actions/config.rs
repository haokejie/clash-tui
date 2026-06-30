use std::{net::SocketAddr, str::FromStr as _, sync::Arc, time::Duration};

use anyhow::{Result, bail};
use clash_core::{AppPathSummary, AppSettings, BaseConfig, KernelState, RuleProviderDownloadProxy, constants::network};
use serde::Serialize;
use serde_yaml_ng::{Mapping, Value};
use tokio::{
    net::TcpStream,
    time::{sleep, timeout},
};

use crate::{
    mihomo_controller::{MihomoController, Mode},
    platform::{self, SystemProxyDiagnostics, TunDiagnostics},
    state::AppState,
    system_info::{self, SystemInfoPayload},
    terminal_display::{
        self, TuiDisplayMode, TuiDisplayModeSummary, TuiPunctuationMode, TuiPunctuationModeSummary, TuiTheme,
        TuiThemeSummary,
    },
};

const EXTERNAL_CONTROLLER_APPLY_RETRIES: usize = 25;
const EXTERNAL_CONTROLLER_APPLY_INTERVAL: Duration = Duration::from_millis(200);
const EXTERNAL_CONTROLLER_PORT_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSummary {
    pub paths: AppPathSummary,
    pub mixed_port: u16,
    pub dns_enabled: bool,
    pub ipv6: bool,
    pub allow_lan: bool,
    pub unified_delay: bool,
    pub log_level: String,
    pub core_log_enabled: bool,
    pub rule_provider_download_proxy: RuleProviderDownloadProxy,
    pub tun_enabled: bool,
    pub tun_diagnostics: TunDiagnostics,
    pub system_proxy_enabled: bool,
    pub system_proxy_diagnostics: SystemProxyDiagnostics,
    pub controller_endpoint: String,
    pub controller_timeout_millis: u64,
    pub controller_secret_configured: bool,
    pub external_controller: ExternalControllerSummary,
    pub tui_display_mode: TuiDisplayModeSummary,
    pub tui_punctuation_mode: TuiPunctuationModeSummary,
    pub tui_theme: TuiThemeSummary,
    pub system: SystemInfoPayload,
    pub network_interface_detail_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalControllerSummary {
    pub configured_enabled: bool,
    pub configured_port: u16,
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub source: String,
    pub runtime_bind: Option<String>,
    pub unsafe_bind: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalControllerApplyStatus {
    pub settings: SettingsSummary,
    pub config_saved: bool,
    pub runtime_generated: bool,
    pub runtime_applied: Option<bool>,
    pub requires_core_restart: bool,
    pub core_restarted: bool,
    pub core_state: Option<KernelState>,
    pub runtime_path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreLogApplyStatus {
    pub settings: SettingsSummary,
    pub config_saved: bool,
    pub requires_core_restart: bool,
    pub core_restarted: bool,
    pub core_state: Option<KernelState>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeExternalController {
    enabled: bool,
    host: Option<String>,
    port: Option<u16>,
    bind: Option<String>,
    safe_local_bind: bool,
}

pub fn paths(state: &AppState) -> AppPathSummary {
    AppPathSummary::from(state.store.paths())
}

pub async fn settings(state: &AppState) -> Result<SettingsSummary> {
    let clash = state.store.load_clash().await?;
    let mut app_settings = state.store.load_app_settings().await?;
    let mut runtime_external_controller_error = None;
    let runtime_external_controller = match fetch_running_external_controller(state).await {
        Some(Ok(runtime)) => {
            if runtime.enabled {
                if let Some(patch) = sync_external_controller_patch(&app_settings, &runtime) {
                    app_settings = patch_app_settings(state, &patch).await?;
                }
                Some(runtime)
            } else if app_settings.enable_external_controller.unwrap_or(false) {
                runtime_external_controller_error = Some("运行配置未回显外部控制器".into());
                if local_external_controller_port_open(configured_external_controller_port(&app_settings)).await {
                    Some(configured_runtime_external_controller(&app_settings))
                } else {
                    None
                }
            } else {
                if let Some(patch) = sync_external_controller_patch(&app_settings, &runtime) {
                    app_settings = patch_app_settings(state, &patch).await?;
                }
                Some(runtime)
            }
        }
        Some(Err(err)) => {
            runtime_external_controller_error = Some(short_controller_error(&err.to_string()));
            if app_settings.enable_external_controller.unwrap_or(false)
                && local_external_controller_port_open(configured_external_controller_port(&app_settings)).await
            {
                Some(configured_runtime_external_controller(&app_settings))
            } else {
                None
            }
        }
        None => None,
    };
    let paths = paths(state);
    Ok(settings_from_config(
        state,
        paths,
        &clash,
        &app_settings,
        runtime_external_controller.as_ref(),
        runtime_external_controller_error,
    ))
}

pub async fn patch_app_settings(state: &AppState, patch: &AppSettings) -> Result<AppSettings> {
    let app_settings = state.store.patch_app_settings(patch).await?;
    state.config.write().await.app_settings = app_settings.clone();
    Ok(app_settings)
}

pub async fn patch_clash(state: &AppState, patch: &Mapping) -> Result<BaseConfig> {
    let clash = state.store.patch_clash(patch).await?;
    state.config.write().await.clash = clash.clone();
    Ok(clash)
}

pub async fn get_mode(state: &AppState) -> Result<Mode> {
    let controller = MihomoController::new(state.mihomo.clone());
    match controller.get_mode().await {
        Ok(mode) => Ok(mode),
        Err(_) => {
            let clash = state.store.load_clash().await?;
            mode_from_clash(&clash)
        }
    }
}

pub async fn set_mode(state: &AppState, mode: Mode) -> Result<Mode> {
    let mut patch = Mapping::new();
    patch.insert(Value::from("mode"), Value::from(mode.as_str()));
    patch_clash(state, &patch).await?;

    let controller = MihomoController::new(state.mihomo.clone());
    let _ = controller.set_mode(mode).await;
    Ok(mode)
}

pub async fn set_dns_enabled(state: Arc<AppState>, enabled: bool) -> Result<SettingsSummary> {
    let previous = state.store.load_app_settings().await?;
    let patch = AppSettings {
        enable_dns_settings: Some(enabled),
        ..AppSettings::default()
    };
    patch_app_settings(&state, &patch).await?;
    if let Err(err) = crate::validation::apply_dns_config(Arc::clone(&state), enabled).await {
        let rollback = AppSettings {
            enable_dns_settings: previous.enable_dns_settings,
            ..AppSettings::default()
        };
        let _ = patch_app_settings(&state, &rollback).await;
        bail!("failed to apply DNS config and rolled back switch: {err}");
    }
    settings(&state).await
}

pub async fn set_ipv6(state: &AppState, enabled: bool) -> Result<SettingsSummary> {
    patch_clash_key(state, "ipv6", Value::from(enabled)).await
}

pub async fn set_allow_lan(state: &AppState, enabled: bool) -> Result<SettingsSummary> {
    patch_clash_key(state, "allow-lan", Value::from(enabled)).await
}

pub async fn set_unified_delay(state: &AppState, enabled: bool) -> Result<SettingsSummary> {
    patch_clash_key(state, "unified-delay", Value::from(enabled)).await
}

pub async fn set_log_level(state: &AppState, level: &str) -> Result<SettingsSummary> {
    let normalized = level.to_ascii_lowercase();
    match normalized.as_str() {
        "debug" | "info" | "warning" | "error" | "silent" => {
            patch_clash_key(state, "log-level", Value::from(normalized)).await
        }
        _ => bail!("unsupported log level: {level}"),
    }
}

pub async fn set_core_log_enabled(state: &AppState, enabled: bool) -> Result<SettingsSummary> {
    let patch = AppSettings {
        enable_core_log: Some(enabled),
        ..AppSettings::default()
    };
    patch_app_settings(state, &patch).await?;
    settings(state).await
}

pub async fn set_rule_provider_download_proxy(
    state: &AppState,
    strategy: RuleProviderDownloadProxy,
) -> Result<SettingsSummary> {
    let previous = state.store.load_app_settings().await?;
    let patch = AppSettings {
        rule_provider_download_proxy: Some(strategy),
        ..AppSettings::default()
    };
    patch_app_settings(state, &patch).await?;

    if let Err(err) = super::runtime_apply::generate_validate_and_apply(state).await {
        let rollback = AppSettings {
            rule_provider_download_proxy: Some(previous.rule_provider_download_proxy.unwrap_or_default()),
            ..AppSettings::default()
        };
        let _ = patch_app_settings(state, &rollback).await;
        let _ = super::runtime_apply::generate_validate_and_apply(state).await;
        bail!("规则 Provider 下载策略应用失败，已回滚：{err}");
    }

    settings(state).await
}

pub async fn apply_core_log_enabled(state: &AppState, enabled: bool) -> Result<CoreLogApplyStatus> {
    let snapshot = state.kernel.external_snapshot().await;
    let running = matches!(
        snapshot.state,
        KernelState::Running | KernelState::Unhealthy | KernelState::Starting | KernelState::Restarting
    );
    let summary = set_core_log_enabled(state, enabled).await?;

    let mut status = CoreLogApplyStatus {
        settings: summary,
        config_saved: true,
        requires_core_restart: running,
        core_restarted: false,
        core_state: Some(snapshot.state),
        message: String::new(),
    };

    if running {
        let operation = super::core::restart(state).await?;
        status.core_state = Some(operation.state);
        status.core_restarted = operation.accepted
            && matches!(
                operation.state,
                KernelState::Running | KernelState::Starting | KernelState::Restarting | KernelState::Unhealthy
            );
        status.message = if status.core_restarted {
            format!("核心日志已{}，Core 已重启生效", core_log_state_label(enabled))
        } else {
            format!(
                "核心日志已{}，但 Core 正忙未重启；下次启动生效",
                core_log_state_label(enabled)
            )
        };
    } else {
        status.message = format!("核心日志已{}，下次启动 Core 生效", core_log_state_label(enabled));
    }

    status.settings = settings(state).await?;
    Ok(status)
}

pub async fn set_mixed_port(state: &AppState, mixed_port: u16) -> Result<SettingsSummary> {
    if mixed_port == 0 {
        bail!("mixed port must be greater than 0");
    }
    patch_clash_key(state, "mixed-port", Value::from(mixed_port)).await
}

pub async fn set_external_controller_enabled(state: &AppState, enabled: bool) -> Result<ExternalControllerApplyStatus> {
    apply_external_controller_patch(
        state,
        AppSettings {
            enable_external_controller: Some(enabled),
            ..AppSettings::default()
        },
    )
    .await
}

pub async fn set_external_controller_port(state: &AppState, port: u16) -> Result<ExternalControllerApplyStatus> {
    if port == 0 {
        bail!("external controller port must be greater than 0");
    }
    let app_settings = state.store.load_app_settings().await?;
    let runtime = match fetch_running_external_controller(state).await {
        Some(Ok(runtime)) => Some(runtime),
        Some(Err(_)) | None => None,
    };
    let enable_external_controller = preserve_external_controller_enabled(&app_settings, runtime.as_ref());
    apply_external_controller_patch(
        state,
        AppSettings {
            enable_external_controller,
            external_controller_port: Some(port),
            ..AppSettings::default()
        },
    )
    .await
}

pub async fn set_tui_display_mode(state: &AppState, mode: TuiDisplayMode) -> Result<SettingsSummary> {
    patch_app_settings(
        state,
        &AppSettings {
            tui_display_mode: Some(mode.config_value().to_owned()),
            ..AppSettings::default()
        },
    )
    .await?;
    settings(state).await
}

pub async fn set_tui_punctuation_mode(state: &AppState, mode: TuiPunctuationMode) -> Result<SettingsSummary> {
    patch_app_settings(
        state,
        &AppSettings {
            tui_punctuation_mode: Some(mode.config_value().to_owned()),
            ..AppSettings::default()
        },
    )
    .await?;
    settings(state).await
}

pub async fn set_tui_theme(state: &AppState, theme: TuiTheme) -> Result<SettingsSummary> {
    patch_app_settings(
        state,
        &AppSettings {
            tui_theme: Some(theme.config_value().to_owned()),
            ..AppSettings::default()
        },
    )
    .await?;
    settings(state).await
}

async fn patch_clash_key(state: &AppState, key: &str, value: Value) -> Result<SettingsSummary> {
    let mut patch = Mapping::new();
    patch.insert(Value::from(key), value);
    patch_clash(state, &patch).await?;
    settings(state).await
}

fn settings_from_config(
    state: &AppState,
    paths: AppPathSummary,
    clash: &BaseConfig,
    app_settings: &AppSettings,
    runtime_external_controller: Option<&RuntimeExternalController>,
    runtime_external_controller_error: Option<String>,
) -> SettingsSummary {
    let system = system_info::collect(state.started_at);
    let network_interface_detail_count = system_info::network_interfaces_info()
        .map(|interfaces| interfaces.len())
        .unwrap_or_default();
    SettingsSummary {
        paths,
        mixed_port: clash.get_client_info().mixed_port,
        dns_enabled: app_settings.enable_dns_settings.unwrap_or(false),
        ipv6: bool_value(clash, "ipv6", true),
        allow_lan: bool_value(clash, "allow-lan", false),
        unified_delay: bool_value(clash, "unified-delay", true),
        log_level: string_value(clash, "log-level").unwrap_or_else(|| "info".into()),
        core_log_enabled: core_log_enabled(app_settings),
        rule_provider_download_proxy: app_settings.rule_provider_download_proxy.unwrap_or_default(),
        tun_enabled: app_settings.enable_tun_mode.unwrap_or(false),
        tun_diagnostics: platform::tun_diagnostics(
            app_settings.enable_tun_mode.unwrap_or(false),
            &state.options.resolved_mihomo_bin(state.store.paths()),
        ),
        system_proxy_enabled: app_settings.enable_system_proxy.unwrap_or(false),
        system_proxy_diagnostics: platform::system_proxy_diagnostics(app_settings),
        controller_endpoint: format!("{:?}", state.mihomo_config.endpoint),
        controller_timeout_millis: state.mihomo_config.timeout_millis,
        controller_secret_configured: state.mihomo_config.secret.is_some(),
        external_controller: external_controller_summary(
            app_settings,
            runtime_external_controller,
            runtime_external_controller_error,
        ),
        tui_display_mode: terminal_display::summary(app_settings),
        tui_punctuation_mode: terminal_display::punctuation_summary(app_settings),
        tui_theme: terminal_display::theme_summary(app_settings),
        system,
        network_interface_detail_count,
    }
}

pub fn core_log_enabled(app_settings: &AppSettings) -> bool {
    app_settings.enable_core_log.unwrap_or(false)
}

const fn core_log_state_label(enabled: bool) -> &'static str {
    if enabled { "开启" } else { "关闭" }
}

async fn apply_external_controller_patch(
    state: &AppState,
    patch: AppSettings,
) -> Result<ExternalControllerApplyStatus> {
    let previous = state.store.load_app_settings().await?;
    let app_settings = patch_app_settings(state, &patch).await?;
    let runtime = match state.runtime.generate().await {
        Ok(runtime) => runtime,
        Err(err) => {
            let rollback = AppSettings {
                enable_external_controller: Some(previous.enable_external_controller.unwrap_or(false)),
                external_controller_port: Some(configured_external_controller_port(&previous)),
                ..AppSettings::default()
            };
            let _ = patch_app_settings(state, &rollback).await;
            return Err(err);
        }
    };

    let snapshot = state.kernel.external_snapshot().await;
    let running = matches!(
        snapshot.state,
        KernelState::Running | KernelState::Unhealthy | KernelState::Starting | KernelState::Restarting
    );
    let target_enabled = app_settings.enable_external_controller.unwrap_or(false);
    let restart_needed = running && (target_enabled || patch.enable_external_controller.is_some());

    let mut status = ExternalControllerApplyStatus {
        settings: settings_from_saved_app_settings(state, &app_settings).await?,
        config_saved: true,
        runtime_generated: true,
        runtime_applied: Some(false),
        requires_core_restart: false,
        core_restarted: false,
        core_state: Some(snapshot.state),
        runtime_path: Some(runtime.path),
        message: String::new(),
    };

    if restart_needed {
        let operation = super::core::restart(state).await?;
        let core_restarted = operation.accepted
            && matches!(
                operation.state,
                KernelState::Running | KernelState::Starting | KernelState::Restarting | KernelState::Unhealthy
            );
        let runtime_applied = if core_restarted {
            wait_for_external_controller_runtime(state, &app_settings).await
        } else {
            false
        };
        status.core_restarted = core_restarted;
        status.runtime_applied = Some(runtime_applied);
        status.requires_core_restart = !runtime_applied;
        status.core_state = Some(operation.state);
        status.settings = settings_from_saved_app_settings(state, &app_settings).await?;
        status.message = if runtime_applied {
            "外部控制器配置已保存，runtime 已重新生成，Core 已重启并应用".into()
        } else if core_restarted {
            "外部控制器配置已保存，runtime 已重新生成，Core 已重启；等待运行态确认超时，稍后刷新或重启 Core".into()
        } else {
            "外部控制器配置已保存，runtime 已重新生成；Core 正忙，稍后重启后生效".into()
        };
    } else if running {
        status.runtime_applied = Some(true);
        status.message = "外部控制器配置已保存，runtime 已重新生成；当前无需重启 Core".into();
    } else {
        status.message = "外部控制器配置已保存，runtime 已重新生成；Core 未运行，下次启动生效".into();
    }

    Ok(status)
}

async fn wait_for_external_controller_runtime(state: &AppState, app_settings: &AppSettings) -> bool {
    for _ in 0..EXTERNAL_CONTROLLER_APPLY_RETRIES {
        if let Some(Ok(runtime)) = fetch_running_external_controller(state).await
            && external_controller_runtime_matches_target(app_settings, &runtime)
        {
            return true;
        }
        if external_controller_port_matches_target(app_settings).await {
            return true;
        }
        sleep(EXTERNAL_CONTROLLER_APPLY_INTERVAL).await;
    }
    false
}

async fn external_controller_port_matches_target(app_settings: &AppSettings) -> bool {
    let port_open = local_external_controller_port_open(configured_external_controller_port(app_settings)).await;
    if app_settings.enable_external_controller.unwrap_or(false) {
        port_open
    } else {
        !port_open
    }
}

async fn local_external_controller_port_open(port: u16) -> bool {
    timeout(
        EXTERNAL_CONTROLLER_PORT_PROBE_TIMEOUT,
        TcpStream::connect((network::DEFAULT_EXTERNAL_CONTROLLER_HOST, port)),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

fn external_controller_runtime_matches_target(app_settings: &AppSettings, runtime: &RuntimeExternalController) -> bool {
    if app_settings.enable_external_controller.unwrap_or(false) {
        return runtime.enabled
            && runtime.safe_local_bind
            && runtime.port == Some(configured_external_controller_port(app_settings));
    }
    !runtime.enabled
}

fn preserve_external_controller_enabled(
    app_settings: &AppSettings,
    runtime: Option<&RuntimeExternalController>,
) -> Option<bool> {
    let configured_enabled = app_settings.enable_external_controller.unwrap_or(false);
    let runtime_enabled = runtime
        .filter(|runtime| runtime.enabled && runtime.safe_local_bind)
        .is_some();
    (configured_enabled || runtime_enabled).then_some(true)
}

fn configured_runtime_external_controller(app_settings: &AppSettings) -> RuntimeExternalController {
    let port = configured_external_controller_port(app_settings);
    RuntimeExternalController {
        enabled: true,
        host: Some(network::DEFAULT_EXTERNAL_CONTROLLER_HOST.into()),
        port: Some(port),
        bind: Some(format!("{}:{port}", network::DEFAULT_EXTERNAL_CONTROLLER_HOST)),
        safe_local_bind: true,
    }
}

async fn settings_from_saved_app_settings(state: &AppState, app_settings: &AppSettings) -> Result<SettingsSummary> {
    let clash = state.store.load_clash().await?;
    let paths = paths(state);
    Ok(settings_from_config(state, paths, &clash, app_settings, None, None))
}

async fn fetch_running_external_controller(state: &AppState) -> Option<Result<RuntimeExternalController>> {
    let snapshot = state.kernel.external_snapshot().await;
    if !matches!(snapshot.state, KernelState::Running | KernelState::Unhealthy) {
        return None;
    }
    let controller = MihomoController::new(state.mihomo.clone());
    Some(
        controller
            .runtime_controller_config()
            .await
            .map(|config| runtime_external_controller(config.external_controller.as_deref())),
    )
}

fn sync_external_controller_patch(
    app_settings: &AppSettings,
    runtime: &RuntimeExternalController,
) -> Option<AppSettings> {
    let mut patch = AppSettings::default();
    let mut changed = false;

    if runtime.enabled {
        if !runtime.safe_local_bind {
            return None;
        }
        let port = runtime.port?;
        if app_settings.enable_external_controller != Some(true) {
            patch.enable_external_controller = Some(true);
            changed = true;
        }
        if configured_external_controller_port(app_settings) != port {
            patch.external_controller_port = Some(port);
            changed = true;
        }
    }

    changed.then_some(patch)
}

fn external_controller_summary(
    app_settings: &AppSettings,
    runtime: Option<&RuntimeExternalController>,
    runtime_error: Option<String>,
) -> ExternalControllerSummary {
    let configured_enabled = app_settings.enable_external_controller.unwrap_or(false);
    let configured_port = configured_external_controller_port(app_settings);
    if let Some(runtime) = runtime {
        if runtime.enabled {
            let host = runtime.host.clone().unwrap_or_else(|| "未知".into());
            let port = runtime.port.unwrap_or(configured_port);
            let unsafe_bind = !runtime.safe_local_bind;
            return ExternalControllerSummary {
                configured_enabled,
                configured_port,
                enabled: true,
                host: host.clone(),
                port,
                source: "mihomo".into(),
                runtime_bind: runtime.bind.clone(),
                unsafe_bind,
                warning: unsafe_bind.then(|| format!("当前运行绑定 {host}:{port} 不是本机安全绑定，未自动写回配置")),
            };
        }
        return ExternalControllerSummary {
            configured_enabled,
            configured_port,
            enabled: false,
            host: network::DEFAULT_EXTERNAL_CONTROLLER_HOST.into(),
            port: configured_port,
            source: "mihomo".into(),
            runtime_bind: None,
            unsafe_bind: false,
            warning: None,
        };
    }

    ExternalControllerSummary {
        configured_enabled,
        configured_port,
        enabled: configured_enabled,
        host: network::DEFAULT_EXTERNAL_CONTROLLER_HOST.into(),
        port: configured_port,
        source: "config".into(),
        runtime_bind: None,
        unsafe_bind: false,
        warning: runtime_error.map(|error| format!("运行配置读取失败，显示本地配置：{error}")),
    }
}

fn runtime_external_controller(value: Option<&str>) -> RuntimeExternalController {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return RuntimeExternalController {
            enabled: false,
            host: None,
            port: None,
            bind: None,
            safe_local_bind: true,
        };
    };

    let (host, port, bind) = parse_external_controller_bind(value)
        .map(|(host, port)| {
            let bind = format!("{host}:{port}");
            (Some(host), Some(port), Some(bind))
        })
        .unwrap_or_else(|| (Some(value.to_owned()), None, Some(value.to_owned())));
    let safe_local_bind = matches!(host.as_deref(), Some("127.0.0.1" | "::1"));
    RuntimeExternalController {
        enabled: true,
        host,
        port,
        bind,
        safe_local_bind,
    }
}

fn parse_external_controller_bind(value: &str) -> Option<(String, u16)> {
    let value = value.trim();
    let normalized = if value.starts_with(':') {
        format!("{}{}", network::DEFAULT_EXTERNAL_CONTROLLER_HOST, value)
    } else {
        value.to_owned()
    };
    SocketAddr::from_str(&normalized)
        .ok()
        .map(|socket| (socket.ip().to_string(), socket.port()))
}

fn configured_external_controller_port(app_settings: &AppSettings) -> u16 {
    app_settings
        .external_controller_port
        .filter(|port| *port > 0)
        .unwrap_or(network::DEFAULT_EXTERNAL_CONTROLLER_PORT)
}

fn short_controller_error(message: &str) -> String {
    if message.contains("failed to connect mihomo unix socket") {
        "未连接".into()
    } else if message.contains("timed out") || message.contains("timeout") || message.contains("超时") {
        "超时".into()
    } else if message.contains("Permission denied") || message.contains("权限") {
        "权限不足".into()
    } else {
        "不可用".into()
    }
}

fn mode_from_clash(clash: &BaseConfig) -> Result<Mode> {
    let mode = string_value(clash, "mode").unwrap_or_else(|| "rule".into());
    mode.parse()
}

fn bool_value(clash: &BaseConfig, key: &str, default: bool) -> bool {
    clash.0.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn string_value(clash: &BaseConfig, key: &str) -> Option<String> {
    clash.0.get(key).and_then(Value::as_str).map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        configured_external_controller_port, configured_runtime_external_controller, core_log_enabled,
        external_controller_runtime_matches_target, external_controller_summary, parse_external_controller_bind,
        preserve_external_controller_enabled, runtime_external_controller, short_controller_error,
        sync_external_controller_patch,
    };
    use clash_core::{AppSettings, constants::network};

    #[test]
    fn runtime_external_controller_parses_local_and_remote_binds() {
        let local = runtime_external_controller(Some("127.0.0.1:19097"));
        assert!(local.enabled);
        assert_eq!(local.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(local.port, Some(19097));
        assert!(local.safe_local_bind);

        let remote = runtime_external_controller(Some("0.0.0.0:9097"));
        assert!(remote.enabled);
        assert_eq!(remote.host.as_deref(), Some("0.0.0.0"));
        assert_eq!(remote.port, Some(9097));
        assert!(!remote.safe_local_bind);

        let disabled = runtime_external_controller(None);
        assert!(!disabled.enabled);
        assert!(disabled.safe_local_bind);
    }

    #[test]
    fn core_log_defaults_to_disabled_for_legacy_config() {
        assert!(!core_log_enabled(&AppSettings::default()));
        assert!(core_log_enabled(&AppSettings {
            enable_core_log: Some(true),
            ..AppSettings::default()
        }));
        assert!(!core_log_enabled(&AppSettings {
            enable_core_log: Some(false),
            ..AppSettings::default()
        }));
    }

    #[test]
    fn runtime_external_controller_handles_default_host_ipv6_and_unparsed_binds() {
        assert_eq!(
            parse_external_controller_bind(":19097"),
            Some(("127.0.0.1".into(), 19097))
        );

        let ipv6_local = runtime_external_controller(Some("[::1]:19097"));
        assert!(ipv6_local.enabled);
        assert_eq!(ipv6_local.host.as_deref(), Some("::1"));
        assert_eq!(ipv6_local.port, Some(19097));
        assert!(ipv6_local.safe_local_bind);

        let named = runtime_external_controller(Some("localhost:19097"));
        assert!(named.enabled);
        assert_eq!(named.host.as_deref(), Some("localhost:19097"));
        assert_eq!(named.port, None);
        assert!(!named.safe_local_bind);
    }

    #[test]
    fn sync_external_controller_patch_follows_safe_runtime_only() {
        let app_settings = AppSettings::default();
        let local = runtime_external_controller(Some("127.0.0.1:19097"));
        let patch = sync_external_controller_patch(&app_settings, &local).expect("local runtime patch");
        assert_eq!(patch.enable_external_controller, Some(true));
        assert_eq!(patch.external_controller_port, Some(19097));

        let remote = runtime_external_controller(Some("0.0.0.0:9097"));
        assert!(sync_external_controller_patch(&app_settings, &remote).is_none());
    }

    #[test]
    fn sync_external_controller_patch_covers_missing_matching_and_default_port_cases() {
        let saved_enabled = AppSettings {
            enable_external_controller: Some(true),
            external_controller_port: Some(19097),
            ..AppSettings::default()
        };
        let disabled_runtime = runtime_external_controller(None);
        assert!(sync_external_controller_patch(&saved_enabled, &disabled_runtime).is_none());

        let matching_runtime = runtime_external_controller(Some("127.0.0.1:19097"));
        assert!(sync_external_controller_patch(&saved_enabled, &matching_runtime).is_none());

        let default_port_config = AppSettings {
            enable_external_controller: Some(true),
            ..AppSettings::default()
        };
        let default_runtime = runtime_external_controller(Some("127.0.0.1:9097"));
        assert!(sync_external_controller_patch(&default_port_config, &default_runtime).is_none());
    }

    #[test]
    fn configured_external_controller_port_uses_default_for_missing_or_zero() {
        assert_eq!(
            configured_external_controller_port(&AppSettings::default()),
            network::DEFAULT_EXTERNAL_CONTROLLER_PORT
        );
        assert_eq!(
            configured_external_controller_port(&AppSettings {
                external_controller_port: Some(0),
                ..AppSettings::default()
            }),
            network::DEFAULT_EXTERNAL_CONTROLLER_PORT
        );
        assert_eq!(
            configured_external_controller_port(&AppSettings {
                external_controller_port: Some(19097),
                ..AppSettings::default()
            }),
            19097
        );
    }

    #[test]
    fn preserve_external_controller_enabled_uses_saved_config_before_runtime() {
        let saved_enabled = AppSettings {
            enable_external_controller: Some(true),
            ..AppSettings::default()
        };
        assert_eq!(preserve_external_controller_enabled(&saved_enabled, None), Some(true));

        let safe_runtime = runtime_external_controller(Some("127.0.0.1:19097"));
        assert_eq!(
            preserve_external_controller_enabled(&AppSettings::default(), Some(&safe_runtime)),
            Some(true)
        );

        let unsafe_runtime = runtime_external_controller(Some("0.0.0.0:19097"));
        assert_eq!(
            preserve_external_controller_enabled(&saved_enabled, Some(&unsafe_runtime)),
            Some(true)
        );
        assert_eq!(
            preserve_external_controller_enabled(&AppSettings::default(), Some(&unsafe_runtime)),
            None
        );

        let disabled_runtime = runtime_external_controller(None);
        assert_eq!(
            preserve_external_controller_enabled(&AppSettings::default(), Some(&disabled_runtime)),
            None
        );
    }

    #[test]
    fn external_controller_runtime_matches_target_requires_safe_bind_and_exact_port() {
        let enabled_target = AppSettings {
            enable_external_controller: Some(true),
            external_controller_port: Some(19097),
            ..AppSettings::default()
        };
        let matching = runtime_external_controller(Some("127.0.0.1:19097"));
        assert!(external_controller_runtime_matches_target(&enabled_target, &matching));

        let wrong_port = runtime_external_controller(Some("127.0.0.1:9097"));
        assert!(!external_controller_runtime_matches_target(
            &enabled_target,
            &wrong_port
        ));

        let unsafe_bind = runtime_external_controller(Some("0.0.0.0:19097"));
        assert!(!external_controller_runtime_matches_target(
            &enabled_target,
            &unsafe_bind
        ));

        let disabled_target = AppSettings {
            enable_external_controller: Some(false),
            external_controller_port: Some(19097),
            ..AppSettings::default()
        };
        let disabled_runtime = runtime_external_controller(None);
        assert!(external_controller_runtime_matches_target(
            &disabled_target,
            &disabled_runtime
        ));
        assert!(!external_controller_runtime_matches_target(&disabled_target, &matching));
    }

    #[test]
    fn configured_runtime_external_controller_uses_safe_local_bind() {
        let configured = AppSettings {
            enable_external_controller: Some(true),
            external_controller_port: Some(19097),
            ..AppSettings::default()
        };
        let runtime = configured_runtime_external_controller(&configured);

        assert!(runtime.enabled);
        assert_eq!(runtime.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(runtime.port, Some(19097));
        assert_eq!(runtime.bind.as_deref(), Some("127.0.0.1:19097"));
        assert!(runtime.safe_local_bind);
    }

    #[test]
    fn external_controller_summary_distinguishes_config_runtime_warning_and_error_sources() {
        let configured = AppSettings {
            enable_external_controller: Some(true),
            external_controller_port: Some(19097),
            ..AppSettings::default()
        };
        let config_summary = external_controller_summary(&configured, None, Some("未连接".into()));
        assert!(config_summary.configured_enabled);
        assert!(config_summary.enabled);
        assert_eq!(config_summary.port, 19097);
        assert_eq!(config_summary.source, "config");
        assert!(
            config_summary
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("未连接"))
        );

        let unsafe_runtime = runtime_external_controller(Some("0.0.0.0:9097"));
        let unsafe_summary = external_controller_summary(&configured, Some(&unsafe_runtime), None);
        assert!(unsafe_summary.enabled);
        assert_eq!(unsafe_summary.source, "mihomo");
        assert_eq!(unsafe_summary.host, "0.0.0.0");
        assert_eq!(unsafe_summary.port, 9097);
        assert!(unsafe_summary.unsafe_bind);
        assert!(
            unsafe_summary
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("不是本机安全绑定"))
        );

        assert_eq!(
            short_controller_error("failed to connect mihomo unix socket /tmp/demo.sock"),
            "未连接"
        );
        assert_eq!(short_controller_error("request timed out after 5s"), "超时");
        assert_eq!(short_controller_error("Permission denied"), "权限不足");
    }
}
