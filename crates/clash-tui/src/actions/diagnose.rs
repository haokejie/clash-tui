use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use clash_core::{KernelSnapshot, KernelState, PrfItem};
use serde::Serialize;
use serde_yaml_ng::{Mapping, Value};

use crate::{
    actions::{config, controller, runtime, subscriptions},
    mihomo_controller::{ControllerHealth, MihomoController},
    platform::{self, SystemProxyDiagnostics, TunDiagnostics},
    state::AppState,
    subscriptions::SubscriptionStatus,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnoseReport {
    pub status: DiagnoseStatus,
    pub current_profile: Option<ProfileBrief>,
    pub kernel: KernelSnapshot,
    pub controller: ControllerProbe,
    pub runtime: RuntimeProbe,
    pub proxies: ProxyProbe,
    pub network: NetworkProbe,
    pub logs: LogProbe,
    pub subscription: Option<SubscriptionStatus>,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnoseStatus {
    Ready,
    NeedsAttention,
    Blocked,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileBrief {
    pub uid: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub profile_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerProbe {
    pub health: ControllerHealth,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProbe {
    pub readable: bool,
    pub path: String,
    pub proxies: usize,
    pub providers: usize,
    pub groups: usize,
    pub rules: usize,
    pub proxy_types: Vec<RuntimeTypeCount>,
    pub proxy_samples: Vec<String>,
    pub provider_samples: Vec<String>,
    pub group_samples: Vec<String>,
    pub group_proxy_refs: usize,
    pub group_provider_refs: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTypeCount {
    #[serde(rename = "type")]
    pub proxy_type: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProxyProbe {
    pub ready: bool,
    pub entries: usize,
    pub groups: usize,
    pub nodes: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkProbe {
    pub tun: TunDiagnostics,
    pub system_proxy_enabled: bool,
    pub system_proxy: SystemProxyDiagnostics,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogProbe {
    pub recent: Vec<String>,
    pub warnings: usize,
    pub errors: usize,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedDiagnoseReport {
    pub path: String,
}

pub async fn report(state: &AppState) -> DiagnoseReport {
    let current_profile = super::profiles::current(state)
        .await
        .ok()
        .flatten()
        .map(ProfileBrief::from);
    let kernel = super::core::status(state).await;
    let controller = ControllerProbe {
        health: MihomoController::new(state.mihomo.clone()).health().await,
    };
    let runtime = runtime_probe(state).await;
    let proxies = proxy_probe(state).await;
    let network = network_probe(state).await;
    let logs = log_probe(super::core::logs(state).await);
    let subscription = subscriptions::status(state).await.ok();
    let recommendations = recommendations(
        &current_profile,
        &kernel,
        &controller,
        &runtime,
        &proxies,
        &network,
        &logs,
    );
    let status = diagnose_status(&current_profile, &kernel, &controller, &runtime, &proxies);

    DiagnoseReport {
        status,
        current_profile,
        kernel,
        controller,
        runtime,
        proxies,
        network,
        logs,
        subscription,
        recommendations,
    }
}

pub async fn save_report(state: &AppState, report: &DiagnoseReport) -> Result<SavedDiagnoseReport> {
    let paths = state.options.app_paths();
    let diagnostics_dir = paths.home_dir.join("diagnostics");
    tokio::fs::create_dir_all(&diagnostics_dir)
        .await
        .with_context(|| format!("failed to create diagnostics dir {}", diagnostics_dir.display()))?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let path = diagnostics_dir.join(format!(
        "diagnose-{}-{:03}.json",
        timestamp.as_secs(),
        timestamp.subsec_millis()
    ));
    let content = serde_json::to_vec_pretty(report).context("failed to serialize diagnose report")?;
    tokio::fs::write(&path, content)
        .await
        .with_context(|| format!("failed to write diagnose report {}", path.display()))?;
    Ok(SavedDiagnoseReport {
        path: path.as_os_str().to_string_lossy().into_owned(),
    })
}

async fn runtime_probe(state: &AppState) -> RuntimeProbe {
    let paths = config::paths(state);
    match runtime::read_yaml(state).await {
        Ok(yaml) => match runtime_summary_from_yaml(&yaml) {
            Ok(summary) => RuntimeProbe {
                readable: true,
                path: paths.runtime_config,
                proxies: summary.proxies,
                providers: summary.providers,
                groups: summary.groups,
                rules: summary.rules,
                proxy_types: summary.proxy_types,
                proxy_samples: summary.proxy_samples,
                provider_samples: summary.provider_samples,
                group_samples: summary.group_samples,
                group_proxy_refs: summary.group_proxy_refs,
                group_provider_refs: summary.group_provider_refs,
                error: None,
            },
            Err(err) => RuntimeProbe {
                readable: false,
                path: paths.runtime_config,
                proxies: 0,
                providers: 0,
                groups: 0,
                rules: 0,
                proxy_types: Vec::new(),
                proxy_samples: Vec::new(),
                provider_samples: Vec::new(),
                group_samples: Vec::new(),
                group_proxy_refs: 0,
                group_provider_refs: 0,
                error: Some(err.to_string()),
            },
        },
        Err(err) => RuntimeProbe {
            readable: false,
            path: paths.runtime_config,
            proxies: 0,
            providers: 0,
            groups: 0,
            rules: 0,
            proxy_types: Vec::new(),
            proxy_samples: Vec::new(),
            provider_samples: Vec::new(),
            group_samples: Vec::new(),
            group_proxy_refs: 0,
            group_provider_refs: 0,
            error: Some(err.to_string()),
        },
    }
}

async fn network_probe(state: &AppState) -> NetworkProbe {
    let app_settings = state.store.load_app_settings().await.unwrap_or_default();
    let tun_enabled = app_settings.enable_tun_mode.unwrap_or(false);
    let system_proxy_enabled = app_settings.enable_system_proxy.unwrap_or(false);
    NetworkProbe {
        tun: platform::tun_diagnostics(tun_enabled, &state.options.resolved_mihomo_bin(state.store.paths())),
        system_proxy_enabled,
        system_proxy: platform::system_proxy_diagnostics(&app_settings),
    }
}

async fn proxy_probe(state: &AppState) -> ProxyProbe {
    match controller::proxy_groups(state).await {
        Ok(groups) => {
            let summary = proxy_summary(&groups);
            ProxyProbe {
                ready: summary.groups > 0 && summary.nodes > 0,
                entries: summary.entries,
                groups: summary.groups,
                nodes: summary.nodes,
                error: None,
            }
        }
        Err(err) => ProxyProbe {
            ready: false,
            entries: 0,
            groups: 0,
            nodes: 0,
            error: Some(err.to_string()),
        },
    }
}

fn recommendations(
    current_profile: &Option<ProfileBrief>,
    kernel: &KernelSnapshot,
    controller: &ControllerProbe,
    runtime: &RuntimeProbe,
    proxies: &ProxyProbe,
    network: &NetworkProbe,
    logs: &LogProbe,
) -> Vec<String> {
    let mut recommendations = Vec::new();
    if current_profile.is_none() {
        recommendations.push("先导入或切换一个订阅/Profile".into());
    }
    if !runtime.readable {
        recommendations.push("runtime 不可读，切换 Profile 或执行 runtime generate 后再刷新".into());
    } else if runtime.proxies + runtime.providers > 0 && runtime.groups == 0 {
        recommendations
            .push("runtime 有节点或 Provider，但没有可选策略组，需要检查订阅 proxy-groups 或兜底生成".into());
    }
    if !matches!(kernel.state, KernelState::Running | KernelState::Unhealthy) {
        recommendations.push("Core 未运行，导入订阅时使用 --start-core，或在 TUI 中启动 Core".into());
    }
    if !controller.health.healthy {
        recommendations.push("controller 未就绪，先看 core status/logs，等待 socket/版本健康检查通过".into());
    }
    if runtime.groups > 0 && !proxies.ready {
        recommendations.push("runtime 已有策略组但 controller 未加载，查看 Core 日志中的配置加载错误".into());
    }
    if proxies.entries > 0 && proxies.groups == 0 {
        recommendations.push("controller 只返回叶子代理，没有可选策略组；TUI Proxies 需要策略组才能选择节点".into());
    }
    if let Some(recommendation) = tun_recommendation(&network.tun) {
        recommendations.push(recommendation);
    }
    if let Some(recommendation) = system_proxy_recommendation(network.system_proxy_enabled, &network.system_proxy) {
        recommendations.push(recommendation);
    }
    if let Some(last_error) = &logs.last_error
        && !proxies.ready
    {
        recommendations.push(format!("Core 最近错误：{last_error}"));
    }
    if recommendations.is_empty() {
        recommendations.push("代理组已加载，可以在 TUI Proxies 页选择策略组和节点".into());
    }
    recommendations
}

fn tun_recommendation(tun: &TunDiagnostics) -> Option<String> {
    if !tun.enabled || tun.can_enable {
        return None;
    }
    let reason = tun
        .checks
        .iter()
        .find(|check| !check.ok)
        .map(|check| check.message.as_str())
        .unwrap_or(tun.message.as_str());
    let mut recommendation = format!("TUN 已开启但当前环境不满足基本条件：{reason}");
    if let Some(manual_action) = tun.manual_action.as_deref().filter(|value| !value.trim().is_empty()) {
        recommendation.push_str("；处理建议：");
        recommendation.push_str(manual_action.trim());
    }
    Some(recommendation)
}

fn system_proxy_recommendation(enabled: bool, diagnostics: &SystemProxyDiagnostics) -> Option<String> {
    if !enabled || diagnostics.can_auto_apply {
        return None;
    }
    let reason = diagnostics
        .checks
        .iter()
        .find(|check| !check.ok)
        .map(|check| check.message.as_str())
        .unwrap_or(diagnostics.message.as_str());
    let mut recommendation = format!("系统代理已开启但当前环境无法自动应用：{reason}");
    if let Some(manual_action) = diagnostics
        .manual_action
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        recommendation.push_str("；处理建议：");
        recommendation.push_str(manual_action.trim());
    }
    Some(recommendation)
}

const fn diagnose_status(
    current_profile: &Option<ProfileBrief>,
    kernel: &KernelSnapshot,
    controller: &ControllerProbe,
    runtime: &RuntimeProbe,
    proxies: &ProxyProbe,
) -> DiagnoseStatus {
    if current_profile.is_some()
        && matches!(kernel.state, KernelState::Running | KernelState::Unhealthy)
        && controller.health.healthy
        && runtime.readable
        && proxies.ready
    {
        DiagnoseStatus::Ready
    } else if current_profile.is_none() || !runtime.readable {
        DiagnoseStatus::Blocked
    } else {
        DiagnoseStatus::NeedsAttention
    }
}

impl From<PrfItem> for ProfileBrief {
    fn from(item: PrfItem) -> Self {
        Self {
            uid: item.uid,
            name: item.name,
            profile_type: item.itype,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct RuntimeSummary {
    proxies: usize,
    providers: usize,
    groups: usize,
    rules: usize,
    proxy_types: Vec<RuntimeTypeCount>,
    proxy_samples: Vec<String>,
    provider_samples: Vec<String>,
    group_samples: Vec<String>,
    group_proxy_refs: usize,
    group_provider_refs: usize,
}

fn runtime_summary_from_yaml(content: &str) -> Result<RuntimeSummary> {
    let value: Value = serde_yaml_ng::from_str(content).context("failed to parse runtime yaml")?;
    let mapping = value.as_mapping().context("runtime yaml root is not a mapping")?;
    let proxies = mapping.get("proxies").and_then(Value::as_sequence);
    let providers = mapping.get("proxy-providers").and_then(Value::as_mapping);
    let groups = mapping.get("proxy-groups").and_then(Value::as_sequence);
    let (group_proxy_refs, group_provider_refs) = group_ref_counts(groups);

    Ok(RuntimeSummary {
        proxies: proxies.map_or(0, Vec::len),
        providers: providers.map_or(0, Mapping::len),
        groups: groups.map_or(0, Vec::len),
        rules: sequence_len(mapping, "rules"),
        proxy_types: proxy_type_counts(proxies),
        proxy_samples: sequence_name_samples(proxies),
        provider_samples: mapping_key_samples(providers),
        group_samples: sequence_name_samples(groups),
        group_proxy_refs,
        group_provider_refs,
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct ProxySummary {
    entries: usize,
    groups: usize,
    nodes: usize,
}

fn proxy_summary(response: &crate::mihomo_controller::ProxyGroups) -> ProxySummary {
    response
        .proxies
        .values()
        .fold(ProxySummary::default(), |mut summary, proxy| {
            summary.entries += 1;
            if !proxy.all.is_empty() {
                summary.groups += 1;
                summary.nodes += proxy.all.len();
            }
            summary
        })
}

fn log_probe(lines: Vec<String>) -> LogProbe {
    const RECENT_LOG_LIMIT: usize = 20;
    let sanitized = lines
        .into_iter()
        .map(|line| truncate_log_line(&redact_urls(&line)))
        .collect::<Vec<_>>();
    log_probe_from_sanitized(sanitized, RECENT_LOG_LIMIT)
}

fn log_probe_from_sanitized(lines: Vec<String>, limit: usize) -> LogProbe {
    let mut warnings = 0;
    let mut errors = 0;
    let mut last_error = None;
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        if lower.contains("warn") {
            warnings += 1;
        }
        if lower.contains("error")
            || lower.contains("failed")
            || lower.contains("failure")
            || lower.contains("fatal")
            || lower.contains("panic")
            || lower.contains("invalid")
        {
            errors += 1;
            last_error = Some(line.clone());
        }
    }
    let recent = lines
        .into_iter()
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    LogProbe {
        recent,
        warnings,
        errors,
        last_error,
    }
}

fn truncate_log_line(line: &str) -> String {
    const LOG_LINE_LIMIT: usize = 500;
    let mut truncated = line.chars().take(LOG_LINE_LIMIT).collect::<String>();
    if line.chars().count() > LOG_LINE_LIMIT {
        truncated.push_str("...");
    }
    truncated
}

fn redact_urls(message: &str) -> String {
    let mut output = String::with_capacity(message.len());
    let mut rest = message;
    loop {
        let http = rest.find("http://");
        let https = rest.find("https://");
        let Some(start) = [http, https].into_iter().flatten().min() else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..start]);
        output.push_str("[redacted-url]");
        let after_url = &rest[start..];
        let end = after_url
            .char_indices()
            .find_map(|(idx, ch)| is_url_boundary(ch).then_some(idx))
            .unwrap_or(after_url.len());
        rest = &after_url[end..];
    }
    output
}

const fn is_url_boundary(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | ')' | ']' | '}')
}

fn sequence_len(mapping: &Mapping, key: &str) -> usize {
    mapping.get(key).and_then(Value::as_sequence).map_or(0, Vec::len)
}

fn proxy_type_counts(items: Option<&Vec<Value>>) -> Vec<RuntimeTypeCount> {
    let mut counts = BTreeMap::<String, usize>::new();
    for item in items.into_iter().flatten() {
        if let Some(proxy_type) = item
            .as_mapping()
            .and_then(|mapping| mapping.get("type"))
            .and_then(Value::as_str)
            .map(sanitize_runtime_sample)
            .filter(|value| !value.is_empty())
        {
            *counts.entry(proxy_type).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .map(|(proxy_type, count)| RuntimeTypeCount { proxy_type, count })
        .collect()
}

fn sequence_name_samples(items: Option<&Vec<Value>>) -> Vec<String> {
    const RUNTIME_SAMPLE_LIMIT: usize = 20;
    items
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.as_mapping()
                .and_then(|mapping| mapping.get("name"))
                .and_then(Value::as_str)
                .map(sanitize_runtime_sample)
                .filter(|value| !value.is_empty())
        })
        .take(RUNTIME_SAMPLE_LIMIT)
        .collect()
}

fn mapping_key_samples(items: Option<&Mapping>) -> Vec<String> {
    const RUNTIME_SAMPLE_LIMIT: usize = 20;
    items
        .into_iter()
        .flat_map(Mapping::keys)
        .filter_map(Value::as_str)
        .map(sanitize_runtime_sample)
        .filter(|value| !value.is_empty())
        .take(RUNTIME_SAMPLE_LIMIT)
        .collect()
}

fn group_ref_counts(items: Option<&Vec<Value>>) -> (usize, usize) {
    let mut proxy_refs = 0;
    let mut provider_refs = 0;
    for group in items.into_iter().flatten().filter_map(Value::as_mapping) {
        proxy_refs += group.get("proxies").and_then(Value::as_sequence).map_or(0, Vec::len);
        provider_refs += group.get("use").and_then(Value::as_sequence).map_or(0, Vec::len);
    }
    (proxy_refs, provider_refs)
}

fn sanitize_runtime_sample(value: &str) -> String {
    const RUNTIME_SAMPLE_CHAR_LIMIT: usize = 120;
    let redacted = redact_urls(value);
    let mut sample = redacted.chars().take(RUNTIME_SAMPLE_CHAR_LIMIT).collect::<String>();
    if redacted.chars().count() > RUNTIME_SAMPLE_CHAR_LIMIT {
        sample.push_str("...");
    }
    sample
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        DiagnoseStatus, log_probe, report, runtime_summary_from_yaml, save_report, system_proxy_recommendation,
        tun_recommendation,
    };
    use crate::{options::ClashTuiOptions, state::AppState};

    #[tokio::test]
    async fn diagnose_report_guides_empty_fresh_state() {
        let root = temp_root("diagnose-empty");
        let _ = std::fs::remove_dir_all(&root);
        let state = AppState::initialize(
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
        )
        .await
        .expect("state");

        let report = report(&state).await;

        assert_eq!(report.status, DiagnoseStatus::Blocked);
        assert!(report.current_profile.is_none());
        assert!(!report.proxies.ready);
        assert!(!report.network.tun.enabled);
        assert!(!report.network.system_proxy_enabled);
        assert!(report.recommendations.iter().any(|item| item.contains("导入或切换")));

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn save_report_writes_redacted_snapshot_under_home() {
        let root = temp_root("diagnose-save");
        let _ = std::fs::remove_dir_all(&root);
        let state = AppState::initialize(
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
        )
        .await
        .expect("state");
        let report = report(&state).await;

        let saved = save_report(&state, &report).await.expect("save");

        let path = std::path::PathBuf::from(&saved.path);
        assert!(path.starts_with(root.join("diagnostics")));
        let content = std::fs::read_to_string(path).expect("diagnose file");
        assert!(content.contains("\"runtime\""));
        assert!(content.contains("\"network\""));
        assert!(content.contains("\"tun\""));
        assert!(content.contains("\"systemProxy\""));
        assert!(!content.contains("https://"));
        assert!(!content.contains("http://"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_summary_counts_sections() {
        let summary = runtime_summary_from_yaml(
            r"proxies:
  - name: HK
    type: ss
  - name: https://example.test/node?token=secret
    type: vmess
proxy-providers:
  remote:
    type: http
proxy-groups:
  - name: Proxy
    type: select
    use:
      - remote
    proxies:
      - HK
      - DIRECT
rules:
  - MATCH,Proxy
",
        )
        .expect("summary");

        assert_eq!(summary.proxies, 2);
        assert_eq!(summary.providers, 1);
        assert_eq!(summary.groups, 1);
        assert_eq!(summary.rules, 1);
        assert_eq!(summary.provider_samples, vec!["remote"]);
        assert_eq!(summary.group_samples, vec!["Proxy"]);
        assert_eq!(summary.group_proxy_refs, 2);
        assert_eq!(summary.group_provider_refs, 1);
        assert_eq!(summary.proxy_types[0].proxy_type, "ss");
        assert_eq!(summary.proxy_types[0].count, 1);
        assert_eq!(summary.proxy_types[1].proxy_type, "vmess");
        assert_eq!(summary.proxy_types[1].count, 1);
        assert_eq!(summary.proxy_samples[0], "HK");
        assert_eq!(summary.proxy_samples[1], "[redacted-url]");
    }

    #[test]
    fn log_probe_redacts_urls_and_reports_recent_errors() {
        let probe = log_probe(vec![
            "info boot".into(),
            "warn slow provider".into(),
            "error failed to load https://example.test/sub?token=secret".into(),
            "error health check Head \"http://cp.cloudflare.com/generate_204\": failed".into(),
        ]);

        assert_eq!(probe.warnings, 1);
        assert_eq!(probe.errors, 2);
        assert_eq!(
            probe.last_error.as_deref(),
            Some("error health check Head \"[redacted-url]\": failed")
        );
        assert!(!probe.recent.join("\n").contains("https://"));
        assert!(!probe.recent.join("\n").contains("http://"));
        assert!(
            probe
                .recent
                .iter()
                .any(|line| line.contains("Head \"[redacted-url]\": failed"))
        );
    }

    #[test]
    fn network_recommendations_include_tun_manual_action_without_urls() {
        let recommendation = tun_recommendation(&crate::platform::TunDiagnostics {
            platform: "linux".into(),
            enabled: true,
            can_enable: false,
            checks: vec![
                crate::platform::TunCheck {
                    name: "privilege".into(),
                    ok: false,
                    message: "当前进程不是 root，且未找到 getcap，无法检测 mihomo CAP_NET_ADMIN".into(),
                },
                crate::platform::TunCheck {
                    name: "capability-tools".into(),
                    ok: false,
                    message: "未找到 getcap/setcap".into(),
                },
            ],
            manual_action: Some(
                "请安装提供 getcap/setcap 的 libcap 工具后重试 tun doctor；执行 tun off 和 core stop 恢复".into(),
            ),
            message: "当前 Linux 环境不满足 TUN 开启条件".into(),
        })
        .expect("tun recommendation");

        assert!(recommendation.contains("TUN 已开启"));
        assert!(recommendation.contains("未找到 getcap"));
        assert!(recommendation.contains("处理建议"));
        assert!(recommendation.contains("libcap"));
        assert!(recommendation.contains("tun doctor"));
        assert!(recommendation.contains("tun off"));
        assert!(recommendation.contains("core stop"));
        assert!(!recommendation.contains("http://"));
        assert!(!recommendation.contains("https://"));

        assert!(
            tun_recommendation(&crate::platform::TunDiagnostics {
                platform: "linux".into(),
                enabled: false,
                can_enable: false,
                checks: Vec::new(),
                manual_action: Some("should not show".into()),
                message: "disabled".into(),
            })
            .is_none()
        );
    }

    #[test]
    fn network_recommendations_include_system_proxy_manual_action_without_urls() {
        let recommendation = system_proxy_recommendation(
            true,
            &crate::platform::SystemProxyDiagnostics {
                platform: "linux".into(),
                endpoint: crate::platform::SystemProxyEndpoint {
                    host: "127.0.0.1".into(),
                    port: 7897,
                    bypass: "localhost,127.0.0.1".into(),
                },
                auto_apply_supported: true,
                can_auto_apply: false,
                checks: vec![crate::platform::SystemProxyCheck {
                    name: "desktop-session".into(),
                    ok: false,
                    message: "未检测到 DBUS_SESSION_BUS_ADDRESS".into(),
                }],
                manual_action: Some("可手动在桌面系统代理中设置 HTTP/HTTPS/SOCKS 主机 127.0.0.1、端口 7897".into()),
                message: "当前 Linux 环境无法自动应用系统代理".into(),
            },
        )
        .expect("system proxy recommendation");

        assert!(recommendation.contains("系统代理已开启"));
        assert!(recommendation.contains("未检测到 DBUS_SESSION_BUS_ADDRESS"));
        assert!(recommendation.contains("处理建议"));
        assert!(recommendation.contains("HTTP/HTTPS/SOCKS"));
        assert!(recommendation.contains("127.0.0.1"));
        assert!(recommendation.contains("7897"));
        assert!(!recommendation.contains("http://"));
        assert!(!recommendation.contains("https://"));
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("clash-tui-{name}-{}-{nanos}", std::process::id()))
    }
}
