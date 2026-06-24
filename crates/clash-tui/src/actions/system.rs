use anyhow::Result;
use clash_core::{IAppSettings, KernelState};
use serde::{Deserialize, Serialize};

use crate::{
    actions::{config, core},
    platform,
    state::AppState,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchStatus {
    pub enabled: bool,
    pub platform: String,
    pub config_saved: bool,
    pub runtime_generated: bool,
    pub runtime_applied: Option<bool>,
    pub platform_applied: Option<bool>,
    pub requires_core_restart: bool,
    pub core_restarted: bool,
    pub core_state: Option<KernelState>,
    pub runtime_path: Option<String>,
    pub manual_action: Option<String>,
    pub message: String,
}

pub async fn tun_status(state: &AppState) -> Result<SwitchStatus> {
    let app_settings = state.store.load_app_settings().await?;
    Ok(platform::tun_status(app_settings.enable_tun_mode.unwrap_or(false)))
}

pub async fn tun_diagnostics(state: &AppState) -> Result<platform::TunDiagnostics> {
    let app_settings = state.store.load_app_settings().await?;
    Ok(platform::tun_diagnostics(
        app_settings.enable_tun_mode.unwrap_or(false),
        &state.options.resolved_mihomo_bin(state.store.paths()),
    ))
}

pub async fn set_tun(state: &AppState, enabled: bool) -> Result<SwitchStatus> {
    let previous = state.store.load_app_settings().await?;
    let patch = IAppSettings {
        enable_tun_mode: Some(enabled),
        ..IAppSettings::default()
    };
    config::patch_app_settings(state, &patch).await?;
    let runtime = match state.runtime.generate().await {
        Ok(runtime) => runtime,
        Err(err) => {
            let rollback = IAppSettings {
                enable_tun_mode: Some(previous.enable_tun_mode.unwrap_or(false)),
                ..IAppSettings::default()
            };
            let _ = config::patch_app_settings(state, &rollback).await;
            return Err(err);
        }
    };

    let snapshot = state.kernel.external_snapshot().await;
    let mut status = platform::tun_status(enabled);
    status.config_saved = true;
    status.runtime_generated = true;
    status.runtime_path = Some(runtime.path);
    status.core_state = Some(snapshot.state);
    let diagnostics = platform::tun_diagnostics(enabled, &state.options.resolved_mihomo_bin(state.store.paths()));

    if matches!(
        snapshot.state,
        KernelState::Running | KernelState::Unhealthy | KernelState::Starting | KernelState::Restarting
    ) {
        let operation = core::restart(state).await?;
        status.core_restarted = operation.accepted
            && matches!(
                operation.state,
                KernelState::Running | KernelState::Unhealthy | KernelState::Starting | KernelState::Restarting
            );
        status.runtime_applied = Some(status.core_restarted);
        status.requires_core_restart = !status.core_restarted;
        status.core_state = Some(operation.state);
        status.message = if enabled && status.core_restarted {
            "TUN 配置已保存，runtime 已重新生成，正在运行的 Core 已重启并应用".into()
        } else if enabled {
            "TUN 配置已保存，runtime 已重新生成；Core 未接受重启，请用当前管理方重启后生效".into()
        } else if status.core_restarted {
            "TUN 配置已保存，runtime 已重新生成，正在运行的 Core 已按关闭状态重启".into()
        } else {
            "TUN 配置已保存，runtime 已重新生成；Core 未接受重启，请用当前管理方重启后生效".into()
        };
    } else {
        status.runtime_applied = Some(false);
        status.requires_core_restart = enabled;
        status.message = if enabled {
            "TUN 配置已保存，runtime 已重新生成；启动 Core 后生效".into()
        } else {
            "TUN 配置已保存，runtime 已重新生成；Core 当前未运行".into()
        };
    }
    enrich_tun_status_with_diagnostics(&mut status, diagnostics);

    Ok(status)
}

fn enrich_tun_status_with_diagnostics(status: &mut SwitchStatus, diagnostics: platform::TunDiagnostics) {
    if !status.enabled {
        status.manual_action = None;
        return;
    }

    if diagnostics.can_enable {
        status.manual_action = None;
        return;
    }

    let diagnostic_message = diagnostics.message.trim();
    if !diagnostic_message.is_empty() && !status.message.contains(diagnostic_message) {
        status.message = format!("{}；{}", status.message, diagnostic_message);
    }
    status.manual_action = diagnostics.manual_action.or_else(|| {
        diagnostics
            .checks
            .iter()
            .find(|check| !check.ok)
            .map(|check| format!("请处理 TUN 环境：{}", check.message))
    });
}

pub async fn system_proxy_status(state: &AppState) -> Result<SwitchStatus> {
    let app_settings = state.store.load_app_settings().await?;
    Ok(platform::system_proxy_status(
        app_settings.enable_system_proxy.unwrap_or(false),
    ))
}

pub async fn system_proxy_diagnostics(state: &AppState) -> Result<platform::SystemProxyDiagnostics> {
    let app_settings = state.store.load_app_settings().await?;
    Ok(platform::system_proxy_diagnostics(&app_settings))
}

pub async fn set_system_proxy(state: &AppState, enabled: bool) -> Result<SwitchStatus> {
    let previous = state.store.load_app_settings().await?;
    let patch = IAppSettings {
        enable_system_proxy: Some(enabled),
        ..IAppSettings::default()
    };
    let app_settings = config::patch_app_settings(state, &patch).await?;
    let mut status = platform::apply_system_proxy(&app_settings, enabled);
    if status.platform_applied == Some(false) {
        let previous_enabled = previous.enable_system_proxy.unwrap_or(false);
        let rollback = IAppSettings {
            enable_system_proxy: Some(previous_enabled),
            ..IAppSettings::default()
        };
        match config::patch_app_settings(state, &rollback).await {
            Ok(_) => {
                status.enabled = previous_enabled;
                status.config_saved = false;
                status.message = format!(
                    "系统代理{}失败，已回滚配置：{}",
                    if enabled { "开启" } else { "关闭" },
                    status.message
                );
            }
            Err(err) => {
                status.message = format!(
                    "系统代理{}失败，且配置回滚失败：{}；原始错误：{}",
                    if enabled { "开启" } else { "关闭" },
                    err,
                    status.message
                );
            }
        }
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use clash_core::KernelState;
    use serde_yaml_ng::Value;

    #[cfg(target_os = "linux")]
    use super::set_system_proxy;
    use super::{enrich_tun_status_with_diagnostics, set_tun};
    use crate::{options::ClashTuiOptions, state::AppState};

    #[tokio::test]
    async fn set_tun_generates_runtime_without_starting_stopped_core() {
        let root = temp_root("tun-runtime");
        let _ = std::fs::remove_dir_all(&root);
        let state = AppState::initialize(
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
        )
        .await
        .expect("state");

        let status = set_tun(&state, true).await.expect("set tun");

        assert!(status.enabled);
        assert!(status.config_saved);
        assert!(status.runtime_generated);
        assert_eq!(status.runtime_applied, Some(false));
        assert!(status.requires_core_restart);
        assert!(!status.core_restarted);
        assert_eq!(status.core_state, Some(KernelState::Stopped));
        let runtime_path = status.runtime_path.expect("runtime path");
        assert!(std::path::Path::new(&runtime_path).is_file());

        let runtime = clash_core::yaml::read_mapping(&runtime_path).await.expect("runtime");
        assert_eq!(
            runtime
                .get("tun")
                .and_then(Value::as_mapping)
                .and_then(|tun| tun.get("enable")),
            Some(&Value::from(true))
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn tun_switch_status_includes_doctor_manual_action_when_environment_is_not_ready() {
        let mut status = crate::platform::tun_status(true);
        status.config_saved = true;
        status.runtime_generated = true;
        status.runtime_applied = Some(false);
        status.requires_core_restart = true;
        status.message = "TUN 配置已保存，runtime 已重新生成；启动 Core 后生效".into();

        enrich_tun_status_with_diagnostics(
            &mut status,
            crate::platform::TunDiagnostics {
                platform: "linux".into(),
                enabled: true,
                can_enable: false,
                checks: vec![crate::platform::TunCheck {
                    name: "privilege".into(),
                    ok: false,
                    message: "当前进程不是 root，且未检测到 mihomo CAP_NET_ADMIN".into(),
                }],
                manual_action: Some(
                    "请确认 /dev/net/tun 存在，并以 root 运行或为 mihomo 授予 CAP_NET_ADMIN；开启失败时执行 tun off 和 core stop 恢复"
                        .into(),
                ),
                message: "当前 Linux 环境不满足 TUN 开启条件：当前进程不是 root，且未检测到 mihomo CAP_NET_ADMIN"
                    .into(),
            },
        );

        assert!(status.message.contains("当前 Linux 环境不满足 TUN 开启条件"));
        let manual_action = status.manual_action.as_deref().expect("manual action");
        assert!(manual_action.contains("/dev/net/tun"));
        assert!(manual_action.contains("CAP_NET_ADMIN"));
        assert!(manual_action.contains("tun off"));
        assert!(manual_action.contains("core stop"));
        assert!(!manual_action.contains("http://"));
        assert!(!manual_action.contains("https://"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn set_system_proxy_rolls_back_when_linux_platform_apply_fails() {
        let root = temp_root("system-proxy-rollback");
        let _ = std::fs::remove_dir_all(&root);
        let state = AppState::initialize(
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
        )
        .await
        .expect("state");

        let schemas = std::process::Command::new("gsettings")
            .arg("list-schemas")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default();
        if schemas.lines().any(|line| line == "org.gnome.system.proxy") {
            let _ = std::fs::remove_dir_all(root);
            return;
        }

        let status = set_system_proxy(&state, true).await.expect("set system proxy");

        assert!(!status.enabled);
        assert!(!status.config_saved);
        assert_eq!(status.platform_applied, Some(false));
        assert!(status.message.contains("已回滚配置"));
        assert!(status.message.contains("Linux 平台应用失败"));
        let manual_action = status.manual_action.as_deref().expect("manual action");
        assert!(manual_action.contains("HTTP/HTTPS/SOCKS"));
        assert!(manual_action.contains("主机 127.0.0.1"));
        assert!(manual_action.contains("端口 7897"));
        assert!(manual_action.contains("GNOME"));
        assert!(manual_action.contains("org.gnome.system.proxy"));
        assert!(manual_action.contains("gsettings set org.gnome.system.proxy mode manual"));
        assert!(!manual_action.contains("http://"));
        assert!(!manual_action.contains("https://"));
        let saved = state.store.load_app_settings().await.expect("app_settings");
        assert_eq!(saved.enable_system_proxy, Some(false));

        let _ = std::fs::remove_dir_all(root);
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("clash-tui-system-{name}-{}-{nanos}", std::process::id()))
    }
}
