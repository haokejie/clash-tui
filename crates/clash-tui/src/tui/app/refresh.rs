use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};

use crate::{
    actions,
    mihomo_controller::{
        ConnectionRecord, ConnectionsResponse, ControllerHealth, ProxyGroups, ProxyProvidersResponse, RuleEntry,
        RuleProvidersResponse,
    },
    state::AppState,
};

use super::{
    labels::kernel_state_label,
    models::{
        CONTROLLER_TIMEOUT, IMPORT_PROVIDER_PROXY_READY_TIMEOUT, IMPORT_PROXY_READY_INTERVAL,
        IMPORT_PROXY_READY_TIMEOUT, MAX_AUTO_PROVIDER_REFRESHES, PROVIDER_REFRESH_SETTLE_INTERVAL,
        PROVIDER_REFRESH_TIMEOUT, ProviderAutoRefreshSummary, ProviderSubscriptionInfoRow, ProxyGroupLoadSummary,
        ProxyGroupRow, ProxyNodeMeta, ProxyProviderRow, ProxyReadyResult, RuleProviderRow, RuntimeProxySummary,
    },
    text::{sanitize_url_error, status_history_text},
};

pub(crate) async fn fetch_controller_status(state: &AppState) -> String {
    let controller = crate::mihomo_controller::MihomoController::new(state.mihomo.clone());
    match tokio::time::timeout(CONTROLLER_TIMEOUT, controller.health()).await {
        Ok(health) => controller_status_from_health(&health),
        Err(_) => "控制器：健康检查超时".into(),
    }
}

pub(crate) fn controller_status_from_health(health: &ControllerHealth) -> String {
    if health.healthy {
        format!("控制器：健康，版本 {}", health.version.as_deref().unwrap_or("未知"))
    } else {
        format!("控制器：不可用（{}）", health.message.as_deref().unwrap_or("未知"))
    }
}

pub(crate) fn diagnose_status_message(report: &actions::diagnose::DiagnoseReport) -> String {
    let profile = report
        .current_profile
        .as_ref()
        .and_then(|profile| profile.name.as_deref().or(profile.uid.as_deref()))
        .unwrap_or("未选择");
    let controller = if report.controller.health.healthy {
        "健康"
    } else {
        "不可用"
    };
    let suggestion = if !report.proxies.ready {
        report
            .logs
            .last_error
            .as_ref()
            .map(|last_error| format!("Core 最近错误：{last_error}"))
            .or_else(|| report.recommendations.first().cloned())
            .unwrap_or_else(|| "无额外建议".to_owned())
    } else {
        report
            .recommendations
            .first()
            .map_or_else(|| "无额外建议".to_owned(), Clone::clone)
    };
    let suggestion = status_history_text(&suggestion);
    let suggestion_label = if report.recommendations.len() > 1 {
        format!("建议（共{}条，按 n 查看）", report.recommendations.len())
    } else {
        "建议".to_owned()
    };
    format!(
        "诊断：{}，Profile={}，Core={}，controller={}，runtime 策略组={}，代理组={}/{}，日志错误={}；{}：{}",
        diagnose_status_label(report.status),
        profile,
        kernel_state_label(report.kernel.state),
        controller,
        report.runtime.groups,
        report.proxies.groups,
        report.proxies.nodes,
        report.logs.errors,
        suggestion_label,
        suggestion
    )
}

pub(crate) fn diagnose_recommendation_lines(report: &actions::diagnose::DiagnoseReport, limit: usize) -> Vec<String> {
    let recommendations = report
        .recommendations
        .iter()
        .filter(|item| !item.trim().is_empty())
        .collect::<Vec<_>>();
    let mut lines = recommendations
        .iter()
        .take(limit)
        .enumerate()
        .map(|(index, item)| format!("建议 {}：{}", index + 1, status_history_text(item)))
        .collect::<Vec<_>>();
    if recommendations.len() > limit {
        lines.push(format!(
            "还有 {} 条建议；按 E 导出诊断快照，或运行 clash-tui --json diagnose 查看完整报告",
            recommendations.len() - limit
        ));
    }
    lines
}

pub(crate) fn diagnose_runtime_detail_lines(report: &actions::diagnose::DiagnoseReport) -> Vec<String> {
    let runtime = &report.runtime;
    let mut lines = Vec::new();
    if let Some(error) = &runtime.error {
        lines.push(format!("runtime 读取失败：{error}"));
        return lines;
    }
    if runtime.readable {
        lines.push(format!(
            "runtime 配置：节点={}，Provider={}，策略组={}，规则={}",
            runtime.proxies, runtime.providers, runtime.groups, runtime.rules
        ));
    }
    if !runtime.proxy_types.is_empty() {
        lines.push(format!(
            "runtime 类型：{}",
            format_runtime_type_counts(&runtime.proxy_types, 6)
        ));
    }
    if !runtime.group_samples.is_empty() {
        lines.push(format!(
            "runtime 策略组样本：{}",
            format_sample_list(&runtime.group_samples, 4)
        ));
    }
    if runtime.group_proxy_refs > 0 || runtime.group_provider_refs > 0 {
        lines.push(format!(
            "策略组引用：节点={}，Provider={}",
            runtime.group_proxy_refs, runtime.group_provider_refs
        ));
    }
    if !runtime.provider_samples.is_empty() {
        lines.push(format!(
            "runtime Provider 样本：{}",
            format_sample_list(&runtime.provider_samples, 4)
        ));
    }
    if !runtime.proxy_samples.is_empty() {
        lines.push(format!(
            "runtime 节点样本：{}",
            format_sample_list(&runtime.proxy_samples, 4)
        ));
    }
    lines
}

pub(crate) fn format_runtime_type_counts(items: &[actions::diagnose::RuntimeTypeCount], limit: usize) -> String {
    let mut values = items
        .iter()
        .take(limit)
        .map(|item| format!("{}={}", item.proxy_type, item.count))
        .collect::<Vec<_>>();
    if items.len() > limit {
        values.push(format!("等{}类", items.len()));
    }
    values.join(", ")
}

pub(crate) fn format_sample_list(items: &[String], limit: usize) -> String {
    let mut values = items.iter().take(limit).cloned().collect::<Vec<_>>();
    if items.len() > limit {
        values.push(format!("等{}项", items.len()));
    }
    values.join("、")
}

pub(crate) const fn diagnose_status_label(status: actions::diagnose::DiagnoseStatus) -> &'static str {
    match status {
        actions::diagnose::DiagnoseStatus::Ready => "就绪",
        actions::diagnose::DiagnoseStatus::NeedsAttention => "需处理",
        actions::diagnose::DiagnoseStatus::Blocked => "阻塞",
    }
}

pub(crate) async fn fetch_proxy_groups_response(state: &AppState) -> Result<ProxyGroups> {
    tokio::time::timeout(CONTROLLER_TIMEOUT, actions::controller::proxy_groups(state)).await?
}

pub(crate) async fn fetch_proxy_providers(state: &AppState) -> Result<Vec<ProxyProviderRow>> {
    let providers = tokio::time::timeout(CONTROLLER_TIMEOUT, actions::controller::proxy_providers(state)).await??;
    Ok(proxy_providers_from_response(&providers))
}

pub(crate) async fn fetch_rule_providers(state: &AppState) -> Result<Vec<RuleProviderRow>> {
    let providers = tokio::time::timeout(CONTROLLER_TIMEOUT, actions::controller::rule_providers(state)).await??;
    Ok(rule_providers_from_response(&providers))
}

pub(crate) async fn fetch_rules(state: &AppState) -> Result<Vec<RuleEntry>> {
    Ok(
        tokio::time::timeout(CONTROLLER_TIMEOUT, actions::controller::rules(state))
            .await??
            .rules,
    )
}

pub(crate) async fn fetch_connections_response(state: &AppState) -> Result<ConnectionsResponse> {
    tokio::time::timeout(CONTROLLER_TIMEOUT, actions::controller::connections(state)).await?
}

pub(crate) async fn fetch_connections(state: &AppState) -> Result<Vec<ConnectionRecord>> {
    Ok(fetch_connections_response(state).await?.connections)
}

pub(crate) fn proxy_groups_from_response(response: &ProxyGroups) -> Vec<ProxyGroupRow> {
    let mut groups = response
        .proxies
        .iter()
        .filter_map(|(name, proxy)| {
            if proxy.all.is_empty() {
                return None;
            }
            Some(ProxyGroupRow {
                name: proxy.name.clone().unwrap_or_else(|| name.clone()),
                now: proxy.now.clone().unwrap_or_else(|| "-".into()),
                nodes: proxy.all.clone(),
                offline: false,
            })
        })
        .collect::<Vec<_>>();
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    groups
}

pub(crate) fn proxy_node_meta_from_response(response: &ProxyGroups) -> BTreeMap<String, ProxyNodeMeta> {
    response
        .proxies
        .iter()
        .map(|(name, proxy)| {
            let node_name = proxy.name.clone().unwrap_or_else(|| name.clone());
            (
                node_name,
                ProxyNodeMeta {
                    proxy_type: proxy.r#type.clone().unwrap_or_else(|| "-".into()),
                    delay_ms: latest_proxy_delay(proxy),
                    alive: proxy.alive,
                },
            )
        })
        .collect()
}

pub(crate) fn latest_proxy_delay(proxy: &crate::mihomo_controller::ProxyEntry) -> Option<i64> {
    proxy
        .history
        .iter()
        .rev()
        .find_map(|history| history.delay.filter(|delay| *delay >= 0))
}

pub(crate) async fn runtime_proxy_groups_preview(state: &AppState) -> Result<Vec<ProxyGroupRow>> {
    let groups = actions::controller::runtime_proxy_groups_preview(state).await?;
    Ok(proxy_group_rows_from_runtime_preview(groups))
}

#[cfg(test)]
pub(crate) fn runtime_proxy_groups_from_yaml(content: &str) -> Result<Vec<ProxyGroupRow>> {
    let groups = actions::controller::runtime_proxy_groups_from_yaml(content)?;
    Ok(proxy_group_rows_from_runtime_preview(groups))
}

pub(crate) fn proxy_group_rows_from_runtime_preview(
    groups: Vec<actions::controller::RuntimeProxyGroupPreview>,
) -> Vec<ProxyGroupRow> {
    groups
        .into_iter()
        .map(|group| ProxyGroupRow {
            name: group.name,
            now: group.now,
            nodes: group.nodes,
            offline: true,
        })
        .collect()
}

pub(crate) fn proxy_group_load_summary(response: &ProxyGroups) -> ProxyGroupLoadSummary {
    response
        .proxies
        .values()
        .fold(ProxyGroupLoadSummary::default(), |mut summary, proxy| {
            summary.entries += 1;
            if !proxy.all.is_empty() {
                summary.groups += 1;
                summary.nodes += proxy.all.len();
            }
            summary
        })
}

pub(crate) async fn wait_for_proxy_groups_ready(state: &AppState) -> Result<ProxyReadyResult> {
    let started = Instant::now();
    let runtime = runtime_proxy_summary_hint(state).await.ok();
    let timeout = if runtime.as_ref().is_some_and(RuntimeProxySummary::uses_providers) {
        IMPORT_PROVIDER_PROXY_READY_TIMEOUT
    } else {
        IMPORT_PROXY_READY_TIMEOUT
    };
    let mut provider_refresh = ProviderAutoRefreshSummary::default();
    let mut provider_refresh_attempted = false;

    loop {
        let attempt_error = match fetch_proxy_groups_response(state).await {
            Ok(response) => {
                let summary = proxy_group_load_summary(&response);
                let groups = proxy_groups_from_response(&response);
                if summary.is_ready() {
                    return Ok(ProxyReadyResult {
                        groups,
                        summary,
                        provider_refresh,
                    });
                }
                if !provider_refresh_attempted && runtime.as_ref().is_some_and(RuntimeProxySummary::uses_providers) {
                    provider_refresh_attempted = true;
                    provider_refresh = auto_refresh_runtime_providers(state, runtime.as_ref()).await;
                    if !provider_refresh.is_empty() {
                        tokio::time::sleep(PROVIDER_REFRESH_SETTLE_INTERVAL).await;
                    }
                }
                proxy_groups_empty_message(summary)
            }
            Err(err) => err.to_string(),
        };

        if !provider_refresh_attempted
            && runtime.as_ref().is_some_and(RuntimeProxySummary::uses_providers)
            && started.elapsed() >= Duration::from_secs(2)
        {
            provider_refresh_attempted = true;
            provider_refresh = auto_refresh_runtime_providers(state, runtime.as_ref()).await;
            if !provider_refresh.is_empty() {
                tokio::time::sleep(PROVIDER_REFRESH_SETTLE_INTERVAL).await;
            }
        }

        if started.elapsed() >= timeout {
            let provider_hint = provider_refresh
                .to_message()
                .map(|message| format!("；{message}"))
                .unwrap_or_default();
            bail!(
                "{}{}，等待 {} 秒后仍未就绪",
                attempt_error,
                provider_hint,
                timeout.as_secs()
            );
        }
        tokio::time::sleep(IMPORT_PROXY_READY_INTERVAL).await;
    }
}

pub(crate) fn proxy_groups_empty_message(summary: ProxyGroupLoadSummary) -> String {
    if summary.entries == 0 {
        "策略组为空：controller 未返回代理数据".into()
    } else if summary.groups == 0 {
        format!(
            "策略组为空：controller 返回 {} 个代理条目，但没有可选策略组",
            summary.entries
        )
    } else {
        format!("策略组为空：{} 个策略组没有可选节点", summary.groups)
    }
}

pub(crate) async fn runtime_proxy_summary_hint(state: &AppState) -> Result<RuntimeProxySummary> {
    let yaml = actions::runtime::read_yaml(state).await?;
    runtime_proxy_summary_from_yaml(&yaml)
}

pub(crate) fn runtime_proxy_summary_from_yaml(content: &str) -> Result<RuntimeProxySummary> {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(content).context("failed to parse runtime yaml")?;
    let mapping = value.as_mapping().context("runtime yaml root is not a mapping")?;
    Ok(RuntimeProxySummary {
        proxies: yaml_sequence_len(mapping, "proxies"),
        providers: yaml_mapping_len(mapping, "proxy-providers"),
        provider_names: yaml_mapping_string_keys(mapping, "proxy-providers"),
        group_provider_names: yaml_group_provider_names(mapping),
        groups: yaml_sequence_len(mapping, "proxy-groups"),
        rules: yaml_sequence_len(mapping, "rules"),
    })
}

impl RuntimeProxySummary {
    pub(crate) const fn uses_providers(&self) -> bool {
        self.providers > 0 || !self.provider_names.is_empty() || !self.group_provider_names.is_empty()
    }

    pub(crate) fn to_message(&self) -> String {
        format!(
            "runtime：节点 {}，Provider {}，策略组 {}，规则 {}",
            self.proxies, self.providers, self.groups, self.rules
        )
    }
}

pub(crate) fn yaml_sequence_len(mapping: &serde_yaml_ng::Mapping, key: &str) -> usize {
    mapping
        .get(key)
        .and_then(serde_yaml_ng::Value::as_sequence)
        .map_or(0, Vec::len)
}

pub(crate) fn yaml_mapping_len(mapping: &serde_yaml_ng::Mapping, key: &str) -> usize {
    mapping
        .get(key)
        .and_then(serde_yaml_ng::Value::as_mapping)
        .map_or(0, serde_yaml_ng::Mapping::len)
}

pub(crate) fn yaml_mapping_string_keys(mapping: &serde_yaml_ng::Mapping, key: &str) -> Vec<String> {
    mapping
        .get(key)
        .and_then(serde_yaml_ng::Value::as_mapping)
        .into_iter()
        .flat_map(serde_yaml_ng::Mapping::keys)
        .filter_map(serde_yaml_ng::Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn yaml_group_provider_names(mapping: &serde_yaml_ng::Mapping) -> Vec<String> {
    let mut names = BTreeSet::new();
    if let Some(groups) = mapping.get("proxy-groups").and_then(serde_yaml_ng::Value::as_sequence) {
        for group in groups {
            let Some(group) = group.as_mapping() else {
                continue;
            };
            if let Some(providers) = group.get("use").and_then(serde_yaml_ng::Value::as_sequence) {
                for provider in providers {
                    if let Some(name) = provider.as_str().filter(|name| !name.trim().is_empty()) {
                        names.insert(name.to_owned());
                    }
                }
            }
        }
    }
    names.into_iter().collect()
}

pub(crate) async fn auto_refresh_runtime_providers(
    state: &AppState,
    runtime: Option<&RuntimeProxySummary>,
) -> ProviderAutoRefreshSummary {
    let controller_providers =
        match tokio::time::timeout(CONTROLLER_TIMEOUT, actions::controller::proxy_providers(state)).await {
            Ok(Ok(providers)) => Some(providers),
            Ok(Err(_)) | Err(_) => None,
        };
    let provider_names = provider_names_for_auto_refresh(runtime, controller_providers.as_ref());
    let mut summary = ProviderAutoRefreshSummary {
        candidates: provider_names.len(),
        ..ProviderAutoRefreshSummary::default()
    };

    for provider in provider_names.into_iter().take(MAX_AUTO_PROVIDER_REFRESHES) {
        summary.attempted += 1;
        match tokio::time::timeout(
            PROVIDER_REFRESH_TIMEOUT,
            actions::controller::update_provider(state, &provider),
        )
        .await
        {
            Ok(Ok(_)) => summary.succeeded += 1,
            Ok(Err(err)) => {
                summary.failed += 1;
                if summary.errors.len() < 3 {
                    summary
                        .errors
                        .push(format!("{provider}: {}", sanitize_url_error(&err.to_string())));
                }
            }
            Err(_) => {
                summary.failed += 1;
                if summary.errors.len() < 3 {
                    summary.errors.push(format!(
                        "{provider}: 更新请求超过 {} 秒",
                        PROVIDER_REFRESH_TIMEOUT.as_secs()
                    ));
                }
            }
        }
    }

    summary
}

pub(crate) fn provider_names_for_auto_refresh(
    runtime: Option<&RuntimeProxySummary>,
    controller_providers: Option<&ProxyProvidersResponse>,
) -> Vec<String> {
    let runtime_provider_names = runtime
        .map(|runtime| {
            runtime
                .provider_names
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let group_provider_names = runtime
        .map(|runtime| {
            runtime
                .group_provider_names
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(controller_providers) = controller_providers {
        let mut controller_entries = controller_providers
            .providers
            .iter()
            .filter(|(_, provider)| is_visible_proxy_provider(provider.vehicle_type.as_deref()))
            .collect::<Vec<_>>();
        controller_entries.sort_by(|(left_name, left), (right_name, right)| {
            let left_priority =
                provider_auto_refresh_priority(left_name, left, &group_provider_names, &runtime_provider_names);
            let right_priority =
                provider_auto_refresh_priority(right_name, right, &group_provider_names, &runtime_provider_names);
            left_priority
                .cmp(&right_priority)
                .then_with(|| left.proxies.len().cmp(&right.proxies.len()))
                .then_with(|| left_name.cmp(right_name))
        });
        for provider_names in [&group_provider_names, &runtime_provider_names] {
            if provider_names.is_empty() {
                continue;
            }
            for (name, provider) in &controller_entries {
                if provider_matches_names(name, provider, provider_names) {
                    push_unique_provider_name(&mut names, &mut seen, name);
                }
            }
        }
        if names.is_empty() && group_provider_names.is_empty() && runtime_provider_names.is_empty() {
            for (name, _) in controller_entries {
                push_unique_provider_name(&mut names, &mut seen, name);
            }
        }
    }

    if names.is_empty()
        && let Some(runtime) = runtime
    {
        for name in &runtime.group_provider_names {
            push_unique_provider_name(&mut names, &mut seen, name);
        }
        for name in &runtime.provider_names {
            push_unique_provider_name(&mut names, &mut seen, name);
        }
    }

    names
}

pub(crate) fn provider_auto_refresh_priority(
    name: &str,
    provider: &crate::mihomo_controller::ProxyProviderEntry,
    group_provider_names: &BTreeSet<&str>,
    runtime_provider_names: &BTreeSet<&str>,
) -> usize {
    if provider_matches_names(name, provider, group_provider_names) {
        0
    } else if provider_matches_names(name, provider, runtime_provider_names) {
        1
    } else {
        2
    }
}

pub(crate) fn provider_matches_names(
    name: &str,
    provider: &crate::mihomo_controller::ProxyProviderEntry,
    provider_names: &BTreeSet<&str>,
) -> bool {
    provider_names.contains(name)
        || provider
            .name
            .as_deref()
            .is_some_and(|name| provider_names.contains(name))
}

pub(crate) fn push_unique_provider_name(names: &mut Vec<String>, seen: &mut BTreeSet<String>, name: &str) {
    if !name.trim().is_empty() && seen.insert(name.to_owned()) {
        names.push(name.to_owned());
    }
}

pub(crate) fn proxy_providers_from_response(response: &ProxyProvidersResponse) -> Vec<ProxyProviderRow> {
    response
        .providers
        .iter()
        .filter(|(_, provider)| is_visible_proxy_provider(provider.vehicle_type.as_deref()))
        .map(|(name, provider)| ProxyProviderRow {
            name: provider.name.clone().unwrap_or_else(|| name.clone()),
            provider_type: provider.r#type.clone().unwrap_or_else(|| "-".into()),
            vehicle_type: provider.vehicle_type.clone().unwrap_or_else(|| "-".into()),
            proxy_count: provider.proxies.len(),
            updated_at: provider.updated_at.clone(),
            subscription: provider
                .subscription_info
                .as_ref()
                .map(|subscription| ProviderSubscriptionInfoRow {
                    upload: subscription.upload,
                    download: subscription.download,
                    total: subscription.total,
                    expire: subscription.expire,
                }),
        })
        .collect()
}

fn is_visible_proxy_provider(vehicle_type: Option<&str>) -> bool {
    matches!(
        vehicle_type.map(str::trim),
        Some(vehicle_type) if vehicle_type.eq_ignore_ascii_case("HTTP") || vehicle_type.eq_ignore_ascii_case("File")
    )
}

pub(crate) fn rule_providers_from_response(response: &RuleProvidersResponse) -> Vec<RuleProviderRow> {
    response
        .providers
        .iter()
        .map(|(name, provider)| RuleProviderRow {
            name: provider.name.clone().unwrap_or_else(|| name.clone()),
            provider_type: provider.r#type.clone().unwrap_or_else(|| "-".into()),
            vehicle_type: provider.vehicle_type.clone().unwrap_or_else(|| "-".into()),
            behavior: provider.behavior.clone().unwrap_or_else(|| "-".into()),
            format: provider.format.clone().unwrap_or_else(|| "-".into()),
            rule_count: provider.rule_count.unwrap_or(0).try_into().unwrap_or(usize::MAX),
            updated_at: provider.updated_at.clone(),
        })
        .collect()
}
