use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use clash_core::{ProfileCatalog, ProfileEntry, config::ProxySelection};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};
use tokio::time::{Instant, sleep};

use crate::{
    mihomo_controller::{
        ConnectionsResponse, DEFAULT_PROXY_DELAY_TEST_TIMEOUT_MILLIS, DEFAULT_PROXY_DELAY_TEST_URL, MihomoController,
        ProviderOperationResult, ProxyDelayTestResult, ProxyGroups, ProxyProvidersResponse, RuleProvidersResponse,
        RulesResponse,
    },
    state::AppState,
    timeouts,
};

const OFFLINE_PRESELECT_EMPTY: &str = "未预选";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProxyGroupPreview {
    pub name: String,
    pub now: String,
    pub nodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySelectionSaveResult {
    pub profile_uid: String,
    pub group: String,
    pub proxy: String,
    pub profiles: ProfileCatalog,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySelectionResult {
    pub applied: bool,
    pub preselected: bool,
    pub profile_uid: String,
    pub group: String,
    pub proxy: String,
    pub message: String,
    pub profiles: ProfileCatalog,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProxySelectionApplyReport {
    pub attempted: usize,
    pub applied: usize,
    pub already_selected: usize,
    pub skipped: usize,
    pub failed: usize,
    pub controller_unavailable: bool,
    pub entries: Vec<ProxySelectionApplyEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProxySelectionApplyEntry {
    pub group: String,
    pub proxy: String,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleProviderUpdateSweep {
    pub total: usize,
    pub updated: usize,
    pub failed: usize,
    pub results: Vec<ProviderOperationResult>,
    pub errors: Vec<RuleProviderUpdateError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleProviderUpdateError {
    pub provider: String,
    pub error: String,
}

pub async fn proxy_groups(state: &AppState) -> Result<ProxyGroups> {
    MihomoController::new(state.mihomo.clone()).proxy_groups().await
}

pub async fn proxy_providers(state: &AppState) -> Result<ProxyProvidersResponse> {
    MihomoController::new(state.mihomo.clone()).proxy_providers().await
}

pub async fn rule_providers(state: &AppState) -> Result<RuleProvidersResponse> {
    MihomoController::new(state.mihomo.clone()).rule_providers().await
}

pub async fn select_proxy(state: &AppState, group: &str, proxy: &str) -> Result<()> {
    MihomoController::new(state.mihomo.clone())
        .select_proxy(group, proxy)
        .await
}

pub async fn reload_config(state: &AppState, path: &str, force: bool) -> Result<()> {
    MihomoController::new(state.mihomo.clone())
        .reload_config(path, force)
        .await
}

pub async fn test_proxy_delay(state: &AppState, proxy: &str) -> Result<ProxyDelayTestResult> {
    MihomoController::new(state.mihomo.clone())
        .test_proxy_delay(
            proxy,
            DEFAULT_PROXY_DELAY_TEST_URL,
            DEFAULT_PROXY_DELAY_TEST_TIMEOUT_MILLIS,
        )
        .await
}

pub async fn select_or_preselect_proxy(state: &AppState, group: &str, proxy: &str) -> Result<ProxySelectionResult> {
    let group = normalize_selection_part(group, "策略组")?;
    let proxy = normalize_selection_part(proxy, "节点")?;

    match select_proxy(state, &group, &proxy).await {
        Ok(()) => {
            let saved = save_proxy_selection(state, &group, &proxy).await?;
            Ok(ProxySelectionResult {
                applied: true,
                preselected: false,
                profile_uid: saved.profile_uid,
                group,
                proxy,
                message: "已切换节点，并保存为下次启动的默认选择".into(),
                profiles: saved.profiles,
            })
        }
        Err(err) => {
            if !controller_error_allows_offline_preselect(&err) {
                return Err(err);
            }
            ensure_runtime_preview_contains(state, &group, &proxy)
                .await
                .with_context(|| format!("controller 不可用，且无法确认 runtime 中存在 {group} -> {proxy}"))?;
            let saved = save_proxy_selection(state, &group, &proxy).await?;
            Ok(ProxySelectionResult {
                applied: false,
                preselected: true,
                profile_uid: saved.profile_uid,
                group,
                proxy,
                message: "已保存离线预选，Core 启动后自动应用".into(),
                profiles: saved.profiles,
            })
        }
    }
}

pub async fn save_proxy_selection(state: &AppState, group: &str, proxy: &str) -> Result<ProxySelectionSaveResult> {
    save_proxy_selection_if_current(state, group, proxy)
        .await?
        .context("当前未激活 Profile，无法保存代理选择")
}

pub async fn save_proxy_selection_if_current(
    state: &AppState,
    group: &str,
    proxy: &str,
) -> Result<Option<ProxySelectionSaveResult>> {
    let group = normalize_selection_part(group, "策略组")?;
    let proxy = normalize_selection_part(proxy, "节点")?;
    let profiles = state.store.load_profiles().await?;
    let Some(current_uid) = profiles
        .current
        .as_deref()
        .filter(|uid| !uid.trim().is_empty())
        .map(ToOwned::to_owned)
    else {
        return Ok(None);
    };
    let current = profiles
        .get_item(&current_uid)
        .with_context(|| format!("当前 Profile 不存在：{current_uid}"))?;
    let mut selected = current.selected.clone().unwrap_or_default();
    upsert_proxy_selection(&mut selected, &group, &proxy);

    let profiles = state
        .store
        .patch_profile(
            &current_uid,
            &ProfileEntry {
                selected: Some(selected),
                ..ProfileEntry::default()
            },
        )
        .await?;
    state.config.write().await.profiles = profiles.clone();

    Ok(Some(ProxySelectionSaveResult {
        profile_uid: current_uid,
        group,
        proxy,
        profiles,
    }))
}

pub async fn saved_proxy_selection_map(state: &AppState) -> Result<BTreeMap<String, String>> {
    let selections = saved_proxy_selections(state).await?;
    Ok(selections.into_iter().collect())
}

pub async fn runtime_proxy_groups_preview(state: &AppState) -> Result<Vec<RuntimeProxyGroupPreview>> {
    let yaml = super::runtime::read_yaml(state).await?;
    let provider_nodes = runtime_provider_nodes_from_yaml(state, &yaml).await.unwrap_or_default();
    let selected = saved_proxy_selection_map(state).await.unwrap_or_default();
    runtime_proxy_groups_from_yaml_with_selected_and_providers(&yaml, &selected, &provider_nodes)
}

#[cfg(test)]
pub fn runtime_proxy_groups_from_yaml(content: &str) -> Result<Vec<RuntimeProxyGroupPreview>> {
    runtime_proxy_groups_from_yaml_with_selected(content, &BTreeMap::new())
}

#[cfg(test)]
pub fn runtime_proxy_groups_from_yaml_with_selected(
    content: &str,
    selected: &BTreeMap<String, String>,
) -> Result<Vec<RuntimeProxyGroupPreview>> {
    runtime_proxy_groups_from_yaml_with_selected_and_providers(content, selected, &BTreeMap::new())
}

fn runtime_proxy_groups_from_yaml_with_selected_and_providers(
    content: &str,
    selected: &BTreeMap<String, String>,
    provider_nodes: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<RuntimeProxyGroupPreview>> {
    let value: Value = serde_yaml_ng::from_str(content).context("failed to parse runtime yaml")?;
    let mapping = value.as_mapping().context("runtime yaml root is not a mapping")?;
    let proxy_names = yaml_proxy_names(mapping);
    let provider_names = yaml_mapping_string_keys(mapping, "proxy-providers");
    let mut rows = Vec::new();

    if let Some(groups) = mapping.get("proxy-groups").and_then(Value::as_sequence) {
        for group in groups {
            let Some(group) = group.as_mapping() else {
                continue;
            };
            let Some(name) = group
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
            else {
                continue;
            };
            let mut nodes = Vec::new();
            let mut seen = BTreeSet::new();
            push_yaml_string_sequence(group, "proxies", &mut nodes, &mut seen);
            push_yaml_provider_refs(group, provider_nodes, &mut nodes, &mut seen);
            if bool_enabled(group, "include-all") || bool_enabled(group, "include-all-proxies") {
                push_names(&proxy_names, &mut nodes, &mut seen);
            }
            if bool_enabled(group, "include-all") || bool_enabled(group, "include-all-providers") {
                push_provider_names(&provider_names, provider_nodes, &mut nodes, &mut seen);
            }
            if nodes.is_empty() {
                continue;
            }
            let now = selected
                .get(name)
                .filter(|proxy| nodes.iter().any(|node| node == *proxy))
                .cloned()
                .unwrap_or_else(|| OFFLINE_PRESELECT_EMPTY.into());
            rows.push(RuntimeProxyGroupPreview {
                name: name.to_owned(),
                now,
                nodes,
            });
        }
    }
    if !proxy_names.is_empty() && !rows.iter().any(|row| row.name == "GLOBAL") {
        let now = selected
            .get("GLOBAL")
            .filter(|proxy| proxy_names.iter().any(|node| node == *proxy))
            .cloned()
            .unwrap_or_else(|| OFFLINE_PRESELECT_EMPTY.into());
        rows.push(RuntimeProxyGroupPreview {
            name: "GLOBAL".into(),
            now,
            nodes: proxy_names,
        });
    }
    Ok(rows)
}

async fn runtime_provider_nodes_from_yaml(state: &AppState, content: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let value: Value = serde_yaml_ng::from_str(content).context("failed to parse runtime yaml")?;
    let mapping = value.as_mapping().context("runtime yaml root is not a mapping")?;
    let Some(providers) = mapping.get("proxy-providers").and_then(Value::as_mapping) else {
        return Ok(BTreeMap::new());
    };
    let home_dir = state.config.read().await.paths.home_dir.clone();
    let mut output = BTreeMap::new();
    for (name, provider) in providers {
        let Some(name) = name.as_str().filter(|name| !name.trim().is_empty()) else {
            continue;
        };
        let Some(path) = provider
            .as_mapping()
            .and_then(|provider| provider.get("path"))
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
        else {
            continue;
        };
        let path = resolve_provider_path(&home_dir, path);
        let Ok(content) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        let Ok(nodes) = provider_proxy_names_from_yaml(&content) else {
            continue;
        };
        if !nodes.is_empty() {
            output.insert(name.to_owned(), nodes);
        }
    }
    Ok(output)
}

pub async fn apply_saved_proxy_selections_with_retry(
    state: &AppState,
    timeout_duration: Duration,
) -> ProxySelectionApplyReport {
    let deadline = Instant::now() + timeout_duration;
    let mut retry_attempt = 0;
    loop {
        match apply_saved_proxy_selections_once(state).await {
            Ok(report) => return report,
            Err(err) if controller_error_allows_offline_preselect(&err) => {
                if Instant::now() >= deadline {
                    return controller_unavailable_report(err);
                }
                let delay = timeouts::saved_proxy_selection_retry_delay(retry_attempt)
                    .min(deadline.saturating_duration_since(Instant::now()));
                retry_attempt += 1;
                sleep(delay).await;
            }
            Err(err) => {
                return controller_unavailable_report(err);
            }
        }
    }
}

fn controller_unavailable_report(err: anyhow::Error) -> ProxySelectionApplyReport {
    ProxySelectionApplyReport {
        controller_unavailable: true,
        entries: vec![ProxySelectionApplyEntry {
            group: "-".into(),
            proxy: "-".into(),
            status: "controller-unavailable".into(),
            message: Some(err.to_string()),
        }],
        ..ProxySelectionApplyReport::default()
    }
}

async fn apply_saved_proxy_selections_once(state: &AppState) -> Result<ProxySelectionApplyReport> {
    let selections = saved_proxy_selections(state).await?;
    let mut report = ProxySelectionApplyReport::default();
    if selections.is_empty() {
        return Ok(report);
    }

    let groups = proxy_groups(state).await?;
    for (group, proxy) in selections {
        report.attempted += 1;
        let Some((controller_group, entry)) = find_proxy_group(&groups, &group) else {
            report.skipped += 1;
            report.entries.push(ProxySelectionApplyEntry {
                group,
                proxy,
                status: "skipped".into(),
                message: Some("controller 未返回该策略组".into()),
            });
            continue;
        };
        if !selectable_proxy_group(entry) {
            report.skipped += 1;
            report.entries.push(ProxySelectionApplyEntry {
                group,
                proxy,
                status: "skipped".into(),
                message: Some("该条目不是可选择策略组".into()),
            });
            continue;
        }
        if !entry.all.iter().any(|node| node == &proxy) {
            report.skipped += 1;
            report.entries.push(ProxySelectionApplyEntry {
                group,
                proxy,
                status: "skipped".into(),
                message: Some("预选节点已不在该策略组中".into()),
            });
            continue;
        }
        if entry.now.as_deref() == Some(proxy.as_str()) {
            report.already_selected += 1;
            report.entries.push(ProxySelectionApplyEntry {
                group,
                proxy,
                status: "already-selected".into(),
                message: None,
            });
            continue;
        }
        match select_proxy(state, controller_group, &proxy).await {
            Ok(()) => {
                report.applied += 1;
                report.entries.push(ProxySelectionApplyEntry {
                    group,
                    proxy,
                    status: "applied".into(),
                    message: None,
                });
            }
            Err(err) => {
                report.failed += 1;
                report.entries.push(ProxySelectionApplyEntry {
                    group,
                    proxy,
                    status: "failed".into(),
                    message: Some(err.to_string()),
                });
            }
        }
    }
    Ok(report)
}

pub async fn rules(state: &AppState) -> Result<RulesResponse> {
    MihomoController::new(state.mihomo.clone()).rules().await
}

pub async fn connections(state: &AppState) -> Result<ConnectionsResponse> {
    MihomoController::new(state.mihomo.clone()).connections().await
}

pub async fn close_connection(state: &AppState, id: &str) -> Result<()> {
    MihomoController::new(state.mihomo.clone()).close_connection(id).await
}

pub async fn close_all_connections(state: &AppState) -> Result<()> {
    MihomoController::new(state.mihomo.clone())
        .close_all_connections()
        .await
}

pub async fn update_provider(state: &AppState, provider: &str) -> Result<ProviderOperationResult> {
    MihomoController::new(state.mihomo.clone())
        .update_provider(provider)
        .await
}

pub async fn update_rule_provider(state: &AppState, provider: &str) -> Result<ProviderOperationResult> {
    MihomoController::new(state.mihomo.clone())
        .update_rule_provider(provider)
        .await
}

pub async fn update_all_rule_providers(state: &AppState) -> Result<RuleProviderUpdateSweep> {
    let providers = rule_providers(state).await?;
    let total = providers.providers.len();
    let mut results = Vec::with_capacity(total);
    let mut errors = Vec::new();

    for provider in providers.providers.keys() {
        match update_rule_provider(state, provider).await {
            Ok(result) => results.push(result),
            Err(err) => errors.push(RuleProviderUpdateError {
                provider: provider.clone(),
                error: err.to_string(),
            }),
        }
    }

    Ok(RuleProviderUpdateSweep {
        total,
        updated: results.len(),
        failed: errors.len(),
        results,
        errors,
    })
}

pub async fn healthcheck_provider(state: &AppState, provider: &str) -> Result<ProviderOperationResult> {
    MihomoController::new(state.mihomo.clone())
        .healthcheck_provider(provider)
        .await
}

async fn ensure_runtime_preview_contains(state: &AppState, group: &str, proxy: &str) -> Result<()> {
    let previews = runtime_proxy_groups_preview(state).await?;
    let Some(preview) = previews.iter().find(|preview| preview.name == group) else {
        bail!("runtime 离线预览中没有策略组：{group}");
    };
    if !preview.nodes.iter().any(|node| node == proxy) {
        bail!("runtime 离线预览中策略组 {group} 没有节点：{proxy}");
    }
    Ok(())
}

async fn saved_proxy_selections(state: &AppState) -> Result<Vec<(String, String)>> {
    let profiles = state.store.load_profiles().await?;
    let Some(current_uid) = profiles.current.as_deref().filter(|uid| !uid.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let Some(current) = profiles
        .items
        .as_deref()
        .and_then(|items| items.iter().find(|item| item.uid.as_deref() == Some(current_uid)))
    else {
        return Ok(Vec::new());
    };
    Ok(current
        .selected
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            let group = item.name.as_deref()?.trim();
            let proxy = item.now.as_deref()?.trim();
            if group.is_empty() || proxy.is_empty() {
                None
            } else {
                Some((group.to_owned(), proxy.to_owned()))
            }
        })
        .collect())
}

fn upsert_proxy_selection(selected: &mut Vec<ProxySelection>, group: &str, proxy: &str) {
    if let Some(index) = selected.iter().position(|entry| entry.name.as_deref() == Some(group)) {
        selected.remove(index);
    }
    selected.push(ProxySelection {
        name: Some(group.to_owned()),
        now: Some(proxy.to_owned()),
    });
}

fn normalize_selection_part(value: &str, label: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{label}不能为空");
    }
    Ok(value.to_owned())
}

fn find_proxy_group<'a>(
    groups: &'a ProxyGroups,
    group: &str,
) -> Option<(&'a str, &'a crate::mihomo_controller::ProxyEntry)> {
    if let Some(entry) = groups.proxies.get_key_value(group) {
        return Some((entry.0.as_str(), entry.1));
    }
    groups.proxies.iter().find_map(|(key, entry)| {
        if entry.name.as_deref() == Some(group) {
            Some((key.as_str(), entry))
        } else {
            None
        }
    })
}

fn selectable_proxy_group(entry: &crate::mihomo_controller::ProxyEntry) -> bool {
    if entry.all.is_empty() {
        return false;
    }
    let Some(group_type) = entry.r#type.as_deref() else {
        return true;
    };
    matches!(
        group_type.to_ascii_lowercase().as_str(),
        "selector" | "urltest" | "fallback" | "loadbalance" | "load-balance"
    )
}

fn controller_error_allows_offline_preselect(err: &anyhow::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("failed to connect mihomo")
        || message.contains("timed out")
        || message.contains("named pipe controller is not implemented")
}

fn yaml_mapping_string_keys(mapping: &Mapping, key: &str) -> Vec<String> {
    mapping
        .get(key)
        .and_then(Value::as_mapping)
        .into_iter()
        .flat_map(Mapping::keys)
        .filter_map(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn yaml_proxy_names(mapping: &Mapping) -> Vec<String> {
    mapping
        .get("proxies")
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(|proxy| match proxy {
            Value::Mapping(proxy) => proxy.get("name").and_then(Value::as_str),
            Value::String(name) => Some(name.as_str()),
            _ => None,
        })
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn push_yaml_string_sequence(mapping: &Mapping, key: &str, output: &mut Vec<String>, seen: &mut BTreeSet<String>) {
    if let Some(items) = mapping.get(key).and_then(Value::as_sequence) {
        for item in items {
            if let Some(name) = item.as_str().filter(|name| !name.trim().is_empty()) {
                push_unique_name(output, seen, name);
            }
        }
    }
}

fn push_yaml_provider_refs(
    mapping: &Mapping,
    provider_nodes: &BTreeMap<String, Vec<String>>,
    output: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) {
    if let Some(items) = mapping.get("use").and_then(Value::as_sequence) {
        for item in items {
            if let Some(name) = item.as_str().filter(|name| !name.trim().is_empty()) {
                push_provider_node_names(name, provider_nodes, output, seen);
            }
        }
    }
}

fn push_names(names: &[String], output: &mut Vec<String>, seen: &mut BTreeSet<String>) {
    for name in names {
        push_unique_name(output, seen, name);
    }
}

fn push_provider_names(
    names: &[String],
    provider_nodes: &BTreeMap<String, Vec<String>>,
    output: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) {
    for name in names {
        push_provider_node_names(name, provider_nodes, output, seen);
    }
}

fn push_provider_node_names(
    name: &str,
    provider_nodes: &BTreeMap<String, Vec<String>>,
    output: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) {
    if let Some(nodes) = provider_nodes.get(name).filter(|nodes| !nodes.is_empty()) {
        push_names(nodes, output, seen);
    } else {
        push_unique_name(output, seen, &format!("Provider: {name}"));
    }
}

fn push_unique_name(output: &mut Vec<String>, seen: &mut BTreeSet<String>, name: &str) {
    if !name.trim().is_empty() && seen.insert(name.to_owned()) {
        output.push(name.to_owned());
    }
}

fn bool_enabled(mapping: &Mapping, key: &str) -> bool {
    mapping.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn resolve_provider_path(home_dir: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() { path } else { home_dir.join(path) }
}

fn provider_proxy_names_from_yaml(content: &str) -> Result<Vec<String>> {
    let value: Value = serde_yaml_ng::from_str(content).context("failed to parse provider yaml")?;
    let Some(mapping) = value.as_mapping() else {
        return Ok(Vec::new());
    };
    Ok(yaml_proxy_names(mapping))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use clash_core::LocalProfileImport;

    use super::*;
    use crate::{options::ClashTuiOptions, state::AppState};

    #[test]
    fn runtime_preview_defaults_to_unselected_and_overlays_saved_selection() {
        let yaml = r"
proxies:
  - name: HK-1
    type: ss
proxy-providers:
  remote-a:
    type: http
proxy-groups:
  - name: Proxy
    type: select
    proxies: [DIRECT, HK-1]
  - name: Auto
    type: url-test
    use: [remote-a]
    include-all: true
";
        let mut selected = BTreeMap::new();
        selected.insert("Proxy".to_owned(), "HK-1".to_owned());
        selected.insert("Auto".to_owned(), "missing".to_owned());

        let groups = runtime_proxy_groups_from_yaml_with_selected(yaml, &selected).expect("groups");

        assert_eq!(groups[0].name, "Proxy");
        assert_eq!(groups[0].now, "HK-1");
        assert_eq!(groups[0].nodes, vec!["DIRECT", "HK-1"]);
        assert_eq!(groups[1].name, "Auto");
        assert_eq!(groups[1].now, OFFLINE_PRESELECT_EMPTY);
        assert!(groups[1].nodes.iter().any(|node| node == "Provider: remote-a"));
        assert!(groups[1].nodes.iter().any(|node| node == "HK-1"));
    }

    #[test]
    fn runtime_preview_expands_provider_cache_nodes_for_offline_preselect() {
        let yaml = r"
proxy-providers:
  remote:
    type: http
    path: ./providers/remote.yaml
proxy-groups:
  - name: GLOBAL
    type: select
    use: [remote]
    proxies: [DIRECT]
";
        let mut providers = BTreeMap::new();
        providers.insert("remote".to_owned(), vec!["US-1".to_owned(), "HK-1".to_owned()]);
        let mut selected = BTreeMap::new();
        selected.insert("GLOBAL".to_owned(), "US-1".to_owned());

        let groups =
            runtime_proxy_groups_from_yaml_with_selected_and_providers(yaml, &selected, &providers).expect("groups");

        assert_eq!(groups[0].now, "US-1");
        assert_eq!(groups[0].nodes, vec!["DIRECT", "US-1", "HK-1"]);
        assert!(!groups[0].nodes.iter().any(|node| node == "Provider: remote"));
    }

    #[test]
    fn runtime_preview_adds_virtual_global_when_runtime_has_only_proxies() {
        let yaml = r"
proxies:
  - name: US-1
    type: ss
  - name: HK-1
    type: trojan
";
        let mut selected = BTreeMap::new();
        selected.insert("GLOBAL".to_owned(), "HK-1".to_owned());

        let groups = runtime_proxy_groups_from_yaml_with_selected(yaml, &selected).expect("groups");

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "GLOBAL");
        assert_eq!(groups[0].now, "HK-1");
        assert_eq!(groups[0].nodes, vec!["US-1", "HK-1"]);
    }

    #[test]
    fn provider_proxy_names_from_yaml_reads_proxy_names() {
        let names = provider_proxy_names_from_yaml(
            r"proxies:
  - name: US-1
    type: ss
  - name: HK-1
    type: trojan
",
        )
        .expect("provider names");

        assert_eq!(names, vec!["US-1", "HK-1"]);
    }

    #[test]
    fn upsert_proxy_selection_replaces_existing_group_without_losing_others() {
        let mut selected = vec![
            ProxySelection {
                name: Some("Proxy".into()),
                now: Some("HK-1".into()),
            },
            ProxySelection {
                name: Some("Auto".into()),
                now: Some("SG-1".into()),
            },
        ];

        upsert_proxy_selection(&mut selected, "Proxy", "US-1");

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].name.as_deref(), Some("Auto"));
        assert_eq!(selected[1].name.as_deref(), Some("Proxy"));
        assert_eq!(selected[1].now.as_deref(), Some("US-1"));
    }

    #[tokio::test]
    async fn save_proxy_selection_persists_current_profile_selected() {
        let root = temp_root("proxy-selection-save");
        let _ = fs::remove_dir_all(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");
        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("L001".into()),
                name: Some("Demo".into()),
                file_data: "proxies: []\nproxy-groups: []\nrules: []\n".into(),
            })
            .await
            .expect("profile");

        let result = save_proxy_selection(&state, "Proxy", "US-1").await.expect("save");
        let loaded = state.store.load_profiles().await.expect("profiles");
        let current = loaded.get_item("L001").expect("current");
        let saved = current.selected.as_ref().expect("selected");

        assert_eq!(result.profile_uid, "L001");
        assert_eq!(saved[0].name.as_deref(), Some("Proxy"));
        assert_eq!(saved[0].now.as_deref(), Some("US-1"));

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn select_or_preselect_proxy_saves_offline_selection_when_runtime_contains_node() {
        let root = temp_root("proxy-selection-offline");
        let _ = fs::remove_dir_all(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");
        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("L001".into()),
                name: Some("Demo".into()),
                file_data: "proxies: []\nproxy-groups: []\nrules: []\n".into(),
            })
            .await
            .expect("profile");
        let runtime_path = state.config.read().await.paths.runtime_config.clone();
        tokio::fs::write(
            &runtime_path,
            r"proxies:
  - name: HK
    type: direct
proxy-groups:
  - name: Proxy
    type: select
    proxies:
      - HK
rules:
  - MATCH,Proxy
",
        )
        .await
        .expect("runtime");

        let result = select_or_preselect_proxy(&state, "Proxy", "HK")
            .await
            .expect("preselect");
        let previews = runtime_proxy_groups_preview(&state).await.expect("preview");

        assert!(!result.applied);
        assert!(result.preselected);
        assert_eq!(result.profile_uid, "L001");
        assert_eq!(previews[0].now, "HK");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn selectable_proxy_group_accepts_selector_like_entries_only() {
        let selectable = crate::mihomo_controller::ProxyEntry {
            r#type: Some("Selector".into()),
            all: vec!["A".into()],
            ..crate::mihomo_controller::ProxyEntry::default()
        };
        let leaf = crate::mihomo_controller::ProxyEntry {
            r#type: Some("Shadowsocks".into()),
            all: Vec::new(),
            ..crate::mihomo_controller::ProxyEntry::default()
        };

        assert!(selectable_proxy_group(&selectable));
        assert!(!selectable_proxy_group(&leaf));
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("clash-tui-{name}-{}-{nanos}", std::process::id()))
    }
}
