use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use clash_core::{
    KernelSnapshot, KernelState, LocalProfileImport, ProfileCatalog, ProfileEntry, RemoteProfileImport,
    config::profiles::generate_remote_uid,
};
use crossterm::event::KeyCode;

use crate::{
    actions,
    actions::config::SettingsSummary,
    jobs::JobRecord,
    mihomo_controller::{ConnectionRecord, Mode, ProviderOperation, RuleEntry},
    state::AppState,
    subscriptions::SubscriptionStatus,
    terminal_display,
    tui::views,
};

use super::{
    details::{connection_detail_lines, job_detail_lines, subscription_sweep_status_message},
    frame::{
        confirm_action_busy_message, input_display_value, input_target_busy_message, input_target_title,
        settings_row_busy_message,
    },
    labels::{
        accepted_label, alive_label, bool_action_label, bool_label, external_controller_status_message,
        job_status_label, kernel_state_label, mode_label, next_log_level, rule_provider_download_proxy_label,
        switch_status_message,
    },
    models::{
        BusyState, ConfirmAction, ConfirmState, DIAGNOSE_RECOMMENDATION_HISTORY_LIMIT,
        DIAGNOSE_RECOMMENDATION_VIEW_LIMIT, DashboardMetrics, DashboardProxyPopup, DetailState, IMPORTANT_STATUS_PIN,
        InputState, InputTarget, LogLevelFilter, ProviderDialogKind, ProxyGroupRow, ProxyNodeMeta, ProxyNodeSort,
        ProxyNodeSortKey, ProxyPane, ProxyProviderRow, RuleProviderRow, SETTINGS_ROWS, STATUS_HISTORY_LIMIT,
        SettingRow, View,
    },
    refresh::{
        controller_status_from_health, diagnose_recommendation_lines, diagnose_runtime_detail_lines,
        diagnose_status_message, fetch_connections, fetch_controller_status, fetch_proxy_groups_response,
        fetch_proxy_providers, fetch_rule_providers, fetch_rules, proxy_group_load_summary, proxy_groups_empty_message,
        proxy_groups_from_response, proxy_node_meta_from_response, runtime_proxy_groups_preview,
        runtime_proxy_summary_hint, wait_for_proxy_groups_ready,
    },
    selection::{
        clamp_index, clamp_with_indices, filter_indices, move_in_indices, move_index, proxy_selection_key_matches,
        text_matches,
    },
    text::{
        normalize_pasted_text, pasted_subscription_url, sanitize_url_error, status_history_text,
        terminal_safe_log_text, terminal_safe_text, validate_subscription_url,
    },
};

#[derive(Debug, Clone)]
pub struct TuiApp {
    pub(crate) view: View,
    pub(crate) status: String,
    pub(crate) controller_status: String,
    pub(crate) kernel_snapshot: Option<KernelSnapshot>,
    pub(crate) profiles: Vec<ProfileEntry>,
    pub(crate) profiles_current: Option<String>,
    pub(crate) profile_index: usize,
    pub(crate) proxy_groups: Vec<ProxyGroupRow>,
    pub(crate) proxy_group_index: usize,
    pub(crate) proxy_node_index: usize,
    pub(crate) proxy_node_meta: BTreeMap<String, ProxyNodeMeta>,
    pub(crate) proxy_providers: Vec<ProxyProviderRow>,
    pub(crate) proxy_provider_index: usize,
    pub(crate) rule_providers: Vec<RuleProviderRow>,
    pub(crate) rule_provider_index: usize,
    pub(crate) provider_dialog: Option<ProviderDialogKind>,
    pub(crate) provider_operation_feedback: BTreeMap<String, String>,
    pub(crate) proxy_group_selection_key: Option<String>,
    pub(crate) proxy_node_selection_key: Option<String>,
    pub(crate) proxy_provider_selection_key: Option<String>,
    pub(crate) rule_provider_selection_key: Option<String>,
    pub(crate) proxy_user_selection_at: Option<Instant>,
    pub(crate) proxy_pane: ProxyPane,
    pub(crate) proxy_node_sort: ProxyNodeSort,
    pub(crate) dashboard_proxy_popup: DashboardProxyPopup,
    pub(crate) dashboard_proxy_group_index: usize,
    pub(crate) dashboard_proxy_node_index: usize,
    pub(crate) dashboard_proxy_group_selection_key: Option<String>,
    pub(crate) dashboard_proxy_node_selection_key: Option<String>,
    pub(crate) dashboard_proxy_user_selection_at: Option<Instant>,
    pub(crate) dashboard_metrics: DashboardMetrics,
    pub(crate) mode: Option<Mode>,
    pub(crate) rules: Vec<RuleEntry>,
    pub(crate) rule_index: usize,
    pub(crate) connections: Vec<ConnectionRecord>,
    pub(crate) connection_index: usize,
    pub(crate) logs: Vec<String>,
    pub(crate) log_index: usize,
    pub(crate) log_follow: bool,
    pub(crate) log_level_filter: LogLevelFilter,
    pub(crate) settings: Option<SettingsSummary>,
    pub(crate) setting_index: usize,
    pub(crate) jobs: Vec<JobRecord>,
    pub(crate) job_index: usize,
    pub(crate) subscription_status: Option<SubscriptionStatus>,
    pub(crate) diagnose_report: Option<actions::diagnose::DiagnoseReport>,
    pub(crate) profile_query: String,
    pub(crate) proxy_query: String,
    pub(crate) rule_query: String,
    pub(crate) connection_query: String,
    pub(crate) log_query: String,
    pub(crate) job_query: String,
    pub(crate) input: Option<InputState>,
    pub(crate) confirm: Option<ConfirmState>,
    pub(crate) busy: Option<BusyState>,
    pub(crate) detail: Option<DetailState>,
    pub(crate) status_history: VecDeque<String>,
    pub(crate) show_help: bool,
    pub(crate) last_refresh: Option<Instant>,
    pub(crate) status_pinned_until: Option<Instant>,
}

impl Default for TuiApp {
    fn default() -> Self {
        Self {
            view: View::Dashboard,
            status: "就绪".into(),
            controller_status: "控制器：尚未刷新".into(),
            kernel_snapshot: None,
            profiles: Vec::new(),
            profiles_current: None,
            profile_index: 0,
            proxy_groups: Vec::new(),
            proxy_group_index: 0,
            proxy_node_index: 0,
            proxy_node_meta: BTreeMap::new(),
            proxy_providers: Vec::new(),
            proxy_provider_index: 0,
            rule_providers: Vec::new(),
            rule_provider_index: 0,
            provider_dialog: None,
            provider_operation_feedback: BTreeMap::new(),
            proxy_group_selection_key: None,
            proxy_node_selection_key: None,
            proxy_provider_selection_key: None,
            rule_provider_selection_key: None,
            proxy_user_selection_at: None,
            proxy_pane: ProxyPane::Groups,
            proxy_node_sort: ProxyNodeSort::Subscription,
            dashboard_proxy_popup: DashboardProxyPopup::None,
            dashboard_proxy_group_index: 0,
            dashboard_proxy_node_index: 0,
            dashboard_proxy_group_selection_key: None,
            dashboard_proxy_node_selection_key: None,
            dashboard_proxy_user_selection_at: None,
            dashboard_metrics: DashboardMetrics::default(),
            mode: None,
            rules: Vec::new(),
            rule_index: 0,
            connections: Vec::new(),
            connection_index: 0,
            logs: Vec::new(),
            log_index: 0,
            log_follow: true,
            log_level_filter: LogLevelFilter::All,
            settings: None,
            setting_index: 0,
            jobs: Vec::new(),
            job_index: 0,
            subscription_status: None,
            diagnose_report: None,
            profile_query: String::new(),
            proxy_query: String::new(),
            rule_query: String::new(),
            connection_query: String::new(),
            log_query: String::new(),
            job_query: String::new(),
            input: None,
            confirm: None,
            busy: None,
            detail: None,
            status_history: VecDeque::new(),
            show_help: false,
            last_refresh: None,
            status_pinned_until: None,
        }
    }
}

impl TuiApp {
    fn apply_settings(&mut self, settings: SettingsSummary) {
        terminal_display::set_current_display_mode(terminal_display::mode_from_summary(&settings.tui_display_mode));
        terminal_display::set_current_punctuation_mode(terminal_display::punctuation_mode_from_summary(
            &settings.tui_punctuation_mode,
        ));
        terminal_display::set_current_theme(terminal_display::theme_from_summary(&settings.tui_theme));
        self.settings = Some(settings);
    }

    pub(crate) fn restore_profile_proxy_group_selection(&mut self) {
        let Some(group) = self.profile_proxy_group_selection() else {
            return;
        };
        if !self.proxy_user_selection_is_sticky() {
            self.proxy_group_selection_key = Some(group.clone());
            self.restore_proxy_group_selection_from_key();
        }
        if !self.dashboard_proxy_user_selection_is_sticky() {
            self.dashboard_proxy_group_selection_key = Some(group);
            self.restore_dashboard_proxy_group_selection_from_key();
        }
    }

    pub(crate) fn set_view(&mut self, view: View) {
        self.clear_status_pin();
        if !matches!(view, View::Dashboard) {
            self.dashboard_proxy_popup = DashboardProxyPopup::None;
        }
        if !matches!(view, View::Proxies | View::Rules) {
            self.provider_dialog = None;
        }
        self.view = view;
        if matches!(view, View::Proxies) {
            self.enter_proxy_view();
        }
        self.last_refresh = None;
        self.set_status(format!("已切换到{}", view.title()));
    }

    pub(crate) fn next_view(&mut self) {
        let index = View::ALL.iter().position(|view| *view == self.view).unwrap_or(0);
        let next = View::ALL[(index + 1) % View::ALL.len()];
        self.set_view(next);
    }

    pub(crate) fn previous_view(&mut self) {
        let index = View::ALL.iter().position(|view| *view == self.view).unwrap_or(0);
        let previous = if index == 0 { View::ALL.len() - 1 } else { index - 1 };
        self.set_view(View::ALL[previous]);
    }

    pub(crate) fn start_busy(&mut self, message: impl Into<String>) {
        self.clear_status_pin();
        let message = message.into();
        self.set_status(message.clone());
        self.busy = Some(BusyState { message });
    }

    pub(crate) fn clear_busy(&mut self) {
        self.busy = None;
    }

    pub(crate) fn pin_status(&mut self, duration: Duration) {
        self.status_pinned_until = Some(Instant::now() + duration);
    }

    pub(crate) fn set_important_status(&mut self, message: impl Into<String>) {
        self.set_status(message);
        self.pin_status(IMPORTANT_STATUS_PIN);
    }

    pub(crate) fn set_refresh_status(&mut self, message: impl Into<String>) {
        if !self.status_is_pinned() {
            self.set_status(message);
        }
    }

    pub(crate) fn set_status(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.record_status(&message);
        self.status = message;
    }

    pub(crate) fn record_status(&mut self, message: &str) {
        let message = status_history_text(message);
        if message.trim().is_empty() {
            return;
        }
        if self.status_history.back() == Some(&message) {
            return;
        }
        self.status_history.push_back(message);
        while self.status_history.len() > STATUS_HISTORY_LIMIT {
            self.status_history.pop_front();
        }
    }

    pub(crate) fn open_status_history(&mut self) {
        let current = self.status.clone();
        self.record_status(&current);
        let mut lines = vec![format!("当前状态：{}", status_history_text(&current)), String::new()];
        if self.status_history.is_empty() {
            lines.push("暂无历史消息".into());
        } else {
            lines.push(format!("最近消息（最多 {STATUS_HISTORY_LIMIT} 条，最新在前）："));
            for (index, message) in self.status_history.iter().rev().enumerate() {
                lines.push(format!("{:02}. {}", index + 1, message));
            }
        }
        self.detail = Some(DetailState {
            title: "消息历史".into(),
            lines,
        });
        self.set_status("正在查看消息历史");
    }

    pub(crate) fn record_diagnose_recommendations(&mut self, report: &actions::diagnose::DiagnoseReport) {
        for line in diagnose_recommendation_lines(report, DIAGNOSE_RECOMMENDATION_HISTORY_LIMIT) {
            self.record_status(&format!("诊断{line}"));
        }
    }

    pub(crate) fn status_is_pinned(&self) -> bool {
        self.status_pinned_until
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    pub(crate) const fn clear_status_pin(&mut self) {
        self.status_pinned_until = None;
    }

    pub(crate) fn busy_message_for_key(&self, code: KeyCode) -> Option<&'static str> {
        if self.show_help {
            return None;
        }
        if let Some(confirm) = &self.confirm {
            return match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => Some(confirm_action_busy_message(&confirm.action)),
                _ => None,
            };
        }
        if let Some(input) = &self.input {
            return match code {
                KeyCode::Enter => input_target_busy_message(input.target),
                _ => None,
            };
        }
        if self.provider_dialog.is_some() {
            return match code {
                KeyCode::Enter | KeyCode::Char('u') | KeyCode::Char('U') => Some("正在更新选中的 Provider..."),
                KeyCode::Char('a') | KeyCode::Char('A') => Some("正在批量更新 Provider..."),
                KeyCode::Char('r') => Some("正在刷新 Provider 列表..."),
                _ => None,
            };
        }

        match code {
            KeyCode::Char('D') => Some("正在生成诊断报告..."),
            KeyCode::Char('E') => Some("正在导出诊断快照..."),
            KeyCode::Char('r') => Some("正在刷新当前页面..."),
            KeyCode::Char('s') => Some("正在启停 Core..."),
            KeyCode::Char('R') if matches!(self.view, View::Dashboard) => Some("正在重启 Core..."),
            KeyCode::Char('R') if matches!(self.view, View::Jobs) => Some("正在重试任务..."),
            KeyCode::Char('m') if matches!(self.view, View::Dashboard | View::Proxies) => Some("正在切换代理模式..."),
            KeyCode::Char('d') if matches!(self.view, View::Dashboard) => Some("正在切换 DNS 设置..."),
            KeyCode::Char('u') if matches!(self.view, View::Profiles) => Some("正在创建订阅更新任务..."),
            KeyCode::Char('a') if matches!(self.view, View::Profiles) => Some("正在批量检查订阅更新..."),
            KeyCode::Char('p') if matches!(self.view, View::Proxies | View::Rules) => Some("正在加载 Provider 列表..."),
            KeyCode::Char('t') if matches!(self.view, View::Proxies) => Some("正在测速当前光标节点..."),
            KeyCode::Char('c') if matches!(self.view, View::Jobs) => Some("正在取消任务..."),
            KeyCode::Enter => self.enter_busy_message(),
            _ => None,
        }
    }

    pub(crate) fn enter_busy_message(&self) -> Option<&'static str> {
        match self.view {
            View::Dashboard if matches!(self.dashboard_proxy_popup, DashboardProxyPopup::Nodes) => {
                if self.dashboard_proxy_group().is_some_and(|group| group.offline) {
                    None
                } else {
                    Some("正在切换代理节点...")
                }
            }
            View::Dashboard => None,
            View::Proxies => match self.proxy_pane {
                ProxyPane::Nodes => {
                    if self.selected_proxy_group().is_some_and(|group| group.offline) {
                        None
                    } else {
                        Some("正在切换代理节点...")
                    }
                }
                ProxyPane::Groups => None,
            },
            View::Settings => settings_row_busy_message(SETTINGS_ROWS[self.setting_index]),
            _ => None,
        }
    }

    #[allow(clippy::cognitive_complexity)]
    pub(crate) async fn handle_key(&mut self, code: KeyCode, state: &Arc<AppState>) -> Result<bool> {
        if self.show_help {
            match code {
                KeyCode::Char('?') | KeyCode::Esc => {
                    self.show_help = false;
                    self.set_status("已关闭帮助");
                    return Ok(false);
                }
                KeyCode::Char('q') => return Ok(true),
                _ => return Ok(false),
            }
        }
        if self.detail.is_some() {
            match code {
                KeyCode::Enter | KeyCode::Esc => {
                    self.detail = None;
                    self.set_status("已关闭详情");
                    return Ok(false);
                }
                KeyCode::Char('q') => return Ok(true),
                _ => return Ok(false),
            }
        }
        if self.confirm.is_some() {
            return self.handle_confirm_key(code, state).await;
        }
        if self.input.is_some() {
            return self.handle_input_key(code, state).await;
        }
        if self.provider_dialog.is_some() {
            return self.handle_provider_dialog_key(code, state).await;
        }
        if matches!(self.view, View::Dashboard) && self.dashboard_proxy_popup != DashboardProxyPopup::None {
            match code {
                KeyCode::Esc => {
                    self.dashboard_proxy_popup = DashboardProxyPopup::None;
                    self.set_status("已收起首页代理选择");
                    return Ok(false);
                }
                KeyCode::Char('q') => return Ok(true),
                _ => {}
            }
        }

        match code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
            KeyCode::Char('1') => self.set_view(View::Dashboard),
            KeyCode::Char('2') => self.set_view(View::Profiles),
            KeyCode::Char('3') => self.set_view(View::Proxies),
            KeyCode::Char('4') => self.set_view(View::Logs),
            KeyCode::Char('5') => self.set_view(View::Settings),
            KeyCode::Char('6') => self.set_view(View::Rules),
            KeyCode::Char('7') => self.set_view(View::Connections),
            KeyCode::Char('8') => self.set_view(View::Jobs),
            KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => self.next_view(),
            KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => self.previous_view(),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home => self.move_to_edge(false),
            KeyCode::End => self.move_to_edge(true),
            KeyCode::Char('?') => {
                self.show_help = true;
                self.set_status("帮助：Esc/? 关闭，q 退出");
            }
            KeyCode::Char('n') | KeyCode::Char('N') => self.open_status_history(),
            KeyCode::Char('D') => self.run_diagnose(state).await,
            KeyCode::Char('E') => self.export_diagnose(state).await,
            KeyCode::Char('/') => self.start_search(),
            KeyCode::Char('i') if matches!(self.view, View::Profiles) => self.start_subscription_import(),
            KeyCode::Char('o') if matches!(self.view, View::Profiles) => self.start_local_profile_import(),
            KeyCode::Enter => self.activate_selected(state).await,
            KeyCode::Char('r') => self.refresh_now(state).await,
            KeyCode::Char('s') => self.toggle_core(state).await,
            KeyCode::Char('R') if matches!(self.view, View::Dashboard) => self.restart_core(state).await,
            KeyCode::Char('R') if matches!(self.view, View::Jobs) => self.retry_selected_job(state).await,
            KeyCode::Char('m') if matches!(self.view, View::Dashboard | View::Proxies) => self.cycle_mode(state).await,
            KeyCode::Char('g') if matches!(self.view, View::Dashboard) => self.open_dashboard_proxy_groups(),
            KeyCode::Char('P') | KeyCode::Char('p') if matches!(self.view, View::Dashboard) => {
                self.confirm_dashboard_system_proxy()
            }
            KeyCode::Char('T') | KeyCode::Char('t') if matches!(self.view, View::Dashboard) => {
                self.confirm_dashboard_tun()
            }
            KeyCode::Char('d') if matches!(self.view, View::Dashboard) => self.toggle_dashboard_dns(state).await,
            KeyCode::Char('u') if matches!(self.view, View::Profiles) => self.update_selected_subscription(state).await,
            KeyCode::Char('a') if matches!(self.view, View::Profiles) => self.update_all_subscriptions(state).await,
            KeyCode::Char('d') | KeyCode::Delete if matches!(self.view, View::Profiles) => {
                self.confirm_delete_profile()
            }
            KeyCode::Char('f') if matches!(self.view, View::Proxies) => {
                self.focus_next_proxy_pane();
            }
            KeyCode::Char('p') if matches!(self.view, View::Proxies) => {
                self.open_provider_dialog(state, ProviderDialogKind::Proxy).await;
            }
            KeyCode::Char('t') if matches!(self.view, View::Proxies) => {
                self.test_selected_proxy_node_delay(state).await;
            }
            KeyCode::Char('p') if matches!(self.view, View::Rules) => {
                self.open_provider_dialog(state, ProviderDialogKind::Rule).await;
            }
            KeyCode::Char('S') if matches!(self.view, View::Proxies) => self.cycle_proxy_node_sort(),
            KeyCode::Char('f') if matches!(self.view, View::Logs) => {
                self.log_follow = !self.log_follow;
                self.set_status(format!("日志跟随已{}", bool_label(self.log_follow)));
            }
            KeyCode::Char('L') if matches!(self.view, View::Logs) => self.cycle_log_level_filter(),
            KeyCode::Char('x') if matches!(self.view, View::Logs) => {
                self.confirm = Some(ConfirmState {
                    prompt: "确认清空日志显示和本地日志文件？y 确认 / n 取消".into(),
                    action: ConfirmAction::ClearLogs,
                });
            }
            KeyCode::Char('c') if matches!(self.view, View::Connections) => {
                self.confirm = Some(ConfirmState {
                    prompt: "确认关闭全部连接？y 确认 / n 取消".into(),
                    action: ConfirmAction::CloseAllConnections,
                });
            }
            KeyCode::Char('d') | KeyCode::Delete if matches!(self.view, View::Connections) => {
                self.confirm_close_selected_connection();
            }
            KeyCode::Char('e') if matches!(self.view, View::Settings) => self.edit_selected_setting(),
            KeyCode::Char('c') if matches!(self.view, View::Jobs) => self.cancel_selected_job(state).await,
            _ => {}
        }
        Ok(false)
    }

    pub(crate) fn handle_paste(&mut self, value: String) -> bool {
        let pasted = normalize_pasted_text(&value);
        if let Some(input) = &mut self.input {
            input.value.push_str(&pasted);
            self.set_status("已粘贴输入内容，按 Enter 应用");
            return false;
        }

        if let Some(url) = pasted_subscription_url(&pasted) {
            self.view = View::Profiles;
            self.input = Some(InputState {
                target: InputTarget::ImportSubscriptionUrl,
                value: url,
            });
            self.set_status("已识别订阅链接，按 Enter 导入，Esc 取消");
        } else {
            self.set_status("已粘贴内容；如需导入订阅，请粘贴 http(s) 链接");
        }
        false
    }

    pub(crate) async fn handle_confirm_key(&mut self, code: KeyCode, state: &Arc<AppState>) -> Result<bool> {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(confirm) = self.confirm.take() {
                    self.execute_confirm(confirm.action, state).await;
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.confirm = None;
                self.set_status("已取消");
            }
            _ => {}
        }
        Ok(false)
    }

    pub(crate) async fn handle_input_key(&mut self, code: KeyCode, state: &Arc<AppState>) -> Result<bool> {
        let Some(input) = &mut self.input else {
            return Ok(false);
        };

        match code {
            KeyCode::Enter => {
                if let Some(input) = self.input.take() {
                    self.finish_input(input, state).await;
                }
            }
            KeyCode::Esc => {
                self.input = None;
                self.set_status("已取消输入");
            }
            KeyCode::Backspace => {
                input.value.pop();
            }
            KeyCode::Char(value) => {
                input.value.push(value);
            }
            _ => {}
        }
        Ok(false)
    }

    pub(crate) async fn handle_provider_dialog_key(&mut self, code: KeyCode, state: &Arc<AppState>) -> Result<bool> {
        match code {
            KeyCode::Esc => {
                self.provider_dialog = None;
                self.set_status("已关闭 Provider");
            }
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Up | KeyCode::Char('k') => self.move_provider_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_provider_selection(1),
            KeyCode::PageUp => self.move_provider_selection(-10),
            KeyCode::PageDown => self.move_provider_selection(10),
            KeyCode::Home => self.move_provider_to_edge(false),
            KeyCode::End => self.move_provider_to_edge(true),
            KeyCode::Enter | KeyCode::Char('u') | KeyCode::Char('U') => {
                self.update_selected_dialog_provider(state).await;
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.update_all_dialog_providers(state).await;
            }
            KeyCode::Char('r') => {
                self.refresh_provider_dialog(state).await;
            }
            _ => {}
        }
        Ok(false)
    }

    pub(crate) async fn finish_input(&mut self, input: InputState, state: &Arc<AppState>) {
        match input.target {
            InputTarget::Search(view) => {
                let value = input.value.trim().to_owned();
                match view {
                    View::Profiles => self.profile_query = value,
                    View::Proxies => self.proxy_query = value,
                    View::Logs => self.log_query = value,
                    View::Rules => self.rule_query = value,
                    View::Connections => self.connection_query = value,
                    View::Jobs => self.job_query = value,
                    View::Dashboard | View::Settings => {}
                }
                self.clamp_selections();
                self.set_status("过滤已更新");
            }
            InputTarget::ImportLocalProfilePath => self.import_local_profile_path(input.value, state).await,
            InputTarget::ImportSubscriptionUrl => self.import_subscription_url(input.value, state).await,
            InputTarget::MixedPort => {
                let result = input
                    .value
                    .trim()
                    .parse::<u16>()
                    .map_err(anyhow::Error::from)
                    .and_then(|port| {
                        if port == 0 {
                            bail!("混合端口必须大于 0");
                        }
                        Ok(port)
                    });
                match result {
                    Ok(port) => match actions::config::set_mixed_port(state, port).await {
                        Ok(settings) => {
                            self.apply_settings(settings);
                            self.set_status(format!("混合端口已设置为 {port}"));
                        }
                        Err(err) => self.set_status(format!("错误：{err}")),
                    },
                    Err(err) => self.set_status(format!("错误：{err}")),
                }
            }
            InputTarget::ExternalControllerPort => {
                let result = input
                    .value
                    .trim()
                    .parse::<u16>()
                    .map_err(anyhow::Error::from)
                    .and_then(|port| {
                        if port == 0 {
                            bail!("外部控制端口必须大于 0");
                        }
                        Ok(port)
                    });
                match result {
                    Ok(port) => {
                        self.confirm = Some(ConfirmState {
                            prompt: format!(
                                "确认将外部控制端口改为 {port}？当前开启时会重启 Core 并绑定本机，y 确认 / n 取消"
                            ),
                            action: ConfirmAction::SetExternalControllerPort { port },
                        });
                    }
                    Err(err) => self.set_status(format!("错误：{err}")),
                }
            }
        }
        self.last_refresh = None;
    }

    pub(crate) async fn import_subscription_url(&mut self, raw_url: String, state: &Arc<AppState>) {
        let url = raw_url.trim();
        if url.is_empty() {
            self.set_status("订阅链接不能为空");
            return;
        }
        if let Err(message) = validate_subscription_url(url) {
            self.set_status(message);
            return;
        }

        let requested_uid = generate_remote_uid();
        let input = RemoteProfileImport {
            url: url.to_owned(),
            uid: Some(requested_uid.clone()),
            name: None,
            desc: None,
            option: None,
        };
        match actions::profiles::import_remote_with_retry_and_activate(Arc::clone(state), &input, true).await {
            Ok(result) => {
                let import_attempt = result.import.attempt.clone();
                let activation = result.activation;
                self.apply_profiles(activation.profiles);
                self.focus_proxy_groups_after_import();
                self.last_refresh = None;
                let core_action = if activation.started_core {
                    "Core 已启动"
                } else if activation.runtime_reloaded {
                    "运行配置已热加载"
                } else {
                    "Core 未启动，启动后应用"
                };
                match wait_for_proxy_groups_ready(state).await {
                    Ok(ready) => {
                        self.apply_proxy_groups(ready.groups);
                        self.controller_status = fetch_controller_status(state).await;
                        self.clamp_selections();
                        let provider_hint = ready
                            .provider_refresh
                            .to_message()
                            .map(|message| format!("，{message}"))
                            .unwrap_or_default();
                        self.set_important_status(format!(
                            "订阅导入并激活成功（{}），{core_action}，策略组 {} 个，节点 {} 个",
                            import_attempt.label, ready.summary.groups, ready.summary.nodes
                        ));
                        self.status.push_str(&provider_hint);
                        let status = self.status.clone();
                        self.record_status(&status);
                    }
                    Err(err) => {
                        let runtime_hint = runtime_proxy_summary_hint(state)
                            .await
                            .map_or_else(|_| "runtime 摘要不可用".to_owned(), |summary| summary.to_message());
                        let preview_hint = if self.apply_runtime_proxy_preview(state, &runtime_hint).await {
                            format!("；已显示 runtime 离线预览 {} 个策略组", self.proxy_groups.len())
                        } else {
                            String::new()
                        };
                        let (report, saved_hint) = self.update_and_save_diagnose_report(state).await;
                        self.set_important_status(format!(
                                    "订阅已导入并激活，{core_action}，但代理组未加载：{}；{}{}；{}；{}；按 r 重试刷新或查看日志",
                                    sanitize_url_error(&err.to_string()),
                                    runtime_hint,
                                    preview_hint,
                                    diagnose_status_message(&report),
                                    saved_hint
                                ));
                    }
                }
            }
            Err(err) => {
                if let Ok(profiles) = actions::profiles::list(state).await {
                    self.apply_profiles(profiles);
                }
                let (report, saved_hint) = self.update_and_save_diagnose_report(state).await;
                self.set_important_status(format!(
                    "订阅导入或激活失败，本次导入未保留：{}；{}；{}",
                    sanitize_url_error(&err.to_string()),
                    diagnose_status_message(&report),
                    saved_hint
                ));
            }
        }
    }

    pub(crate) async fn import_local_profile_path(&mut self, raw_path: String, state: &AppState) {
        let path = raw_path.trim();
        if path.is_empty() {
            self.set_status("本地配置路径不能为空");
            return;
        }

        match tokio::fs::read_to_string(path).await {
            Ok(file_data) => {
                let input = LocalProfileImport {
                    uid: None,
                    name: None,
                    file_data,
                };
                match actions::profiles::import_local(state, &input).await {
                    Ok(profiles) => {
                        self.apply_profiles(profiles);
                        self.set_status("本地配置导入成功");
                    }
                    Err(err) => self.set_status(format!("本地配置导入失败：{err}")),
                }
            }
            Err(err) => self.set_status(format!("读取本地配置失败：{err}")),
        }
    }

    #[allow(clippy::cognitive_complexity)]
    pub(crate) async fn execute_confirm(&mut self, action: ConfirmAction, state: &Arc<AppState>) {
        match action {
            ConfirmAction::SwitchProfile { uid, name } => {
                match actions::profiles::switch(Arc::clone(state), uid.clone()).await {
                    Ok(result) => {
                        let runtime_hint = if result.runtime_reloaded {
                            "，运行配置已热加载"
                        } else {
                            "，Core 启动后应用"
                        };
                        self.apply_profiles(result.profiles);
                        self.set_status(format!("已切换订阅：{name}{runtime_hint}"));
                    }
                    Err(err) => self.set_status(format!("错误：{err}")),
                }
            }
            ConfirmAction::DeleteProfile { uid, name } => match actions::profiles::delete(state, &uid).await {
                Ok(result) => {
                    let current_changed = result.current_changed;
                    let runtime_reloaded = result.runtime_reloaded;
                    let warning = result.warning.clone();
                    self.apply_profiles(result.profiles);
                    let runtime_hint = if current_changed {
                        if runtime_reloaded {
                            "，已切换到下一个订阅并热加载运行配置"
                        } else {
                            "，已切换到下一个订阅并刷新 runtime"
                        }
                    } else {
                        ""
                    };
                    let warning = warning
                        .map(|message| format!("；{}", sanitize_url_error(&message)))
                        .unwrap_or_default();
                    self.set_status(format!("已删除订阅：{name}{runtime_hint}{warning}"));
                }
                Err(err) => self.set_status(format!("错误：{err}")),
            },
            ConfirmAction::CloseConnection { id } => match actions::controller::close_connection(state, &id).await {
                Ok(()) => {
                    self.connections.retain(|connection| connection.id != id);
                    self.clamp_selections();
                    self.set_status(format!("已关闭连接 {id}"));
                }
                Err(err) => self.set_status(format!("错误：{err}")),
            },
            ConfirmAction::CloseAllConnections => match actions::controller::close_all_connections(state).await {
                Ok(()) => {
                    self.connections.clear();
                    self.connection_index = 0;
                    self.set_status("已关闭全部连接");
                }
                Err(err) => self.set_status(format!("错误：{err}")),
            },
            ConfirmAction::ClearLogs => match actions::core::clear_logs(state).await {
                Ok(()) => {
                    self.logs.clear();
                    self.log_index = 0;
                    self.set_status("已清空日志");
                }
                Err(err) => self.set_status(format!("日志清空失败：{err}")),
            },
            ConfirmAction::ToggleTun { enabled } => {
                self.set_action_status(
                    actions::system::set_tun(state, enabled)
                        .await
                        .map(|status| switch_status_message("TUN", &status)),
                );
            }
            ConfirmAction::ToggleSystemProxy { enabled } => {
                self.set_action_status(
                    actions::system::set_system_proxy(state, enabled)
                        .await
                        .map(|status| switch_status_message("系统代理", &status)),
                );
            }
            ConfirmAction::ToggleExternalController { enabled } => {
                self.set_action_status(
                    actions::config::set_external_controller_enabled(state, enabled)
                        .await
                        .map(|status| external_controller_status_message("外部控制器", &status)),
                );
                if let Ok(settings) = actions::config::settings(state).await {
                    self.apply_settings(settings);
                }
            }
            ConfirmAction::SetExternalControllerPort { port } => {
                self.set_action_status(
                    actions::config::set_external_controller_port(state, port)
                        .await
                        .map(|status| external_controller_status_message("外部控制端口", &status)),
                );
                if let Ok(settings) = actions::config::settings(state).await {
                    self.apply_settings(settings);
                }
            }
            ConfirmAction::ToggleCoreLog { enabled } => {
                self.set_action_status(
                    actions::config::apply_core_log_enabled(state, enabled)
                        .await
                        .map(|status| status.message),
                );
                if let Ok(settings) = actions::config::settings(state).await {
                    self.apply_settings(settings);
                }
            }
        }
        self.last_refresh = None;
    }

    #[allow(clippy::cognitive_complexity)]
    pub(crate) async fn refresh(&mut self, state: &AppState) {
        if self
            .last_refresh
            .is_some_and(|last_refresh| last_refresh.elapsed() < Duration::from_secs(2))
        {
            return;
        }

        if matches!(self.view, View::Proxies) {
            self.remember_proxy_selection_for_current_pane();
        }
        self.last_refresh = Some(Instant::now());
        self.jobs = state.jobs.list().await;
        self.subscription_status = actions::subscriptions::status(state).await.ok();

        match self.view {
            View::Dashboard => {
                self.refresh_dashboard(state).await;
            }
            View::Profiles => {
                if let Ok(profiles) = actions::profiles::list(state).await {
                    self.apply_profiles(profiles);
                }
            }
            View::Proxies => {
                self.kernel_snapshot = Some(actions::core::status(state).await);
                self.controller_status = fetch_controller_status(state).await;
                match fetch_proxy_groups_response(state).await {
                    Ok(response) => {
                        let summary = proxy_group_load_summary(&response);
                        let groups = proxy_groups_from_response(&response);
                        let node_meta = proxy_node_meta_from_response(&response);
                        if summary.is_ready() {
                            self.proxy_node_meta = node_meta;
                            self.apply_proxy_groups(groups);
                        } else {
                            self.proxy_node_meta.clear();
                            let message = proxy_groups_empty_message(summary);
                            let runtime_message = match runtime_proxy_summary_hint(state).await {
                                Ok(runtime) => format!("{message}；{}", runtime.to_message()),
                                Err(_) => message,
                            };
                            if !self.apply_runtime_proxy_preview(state, &runtime_message).await {
                                let kept_current_proxy_rows = groups.is_empty()
                                    && !self.proxy_groups.is_empty()
                                    && self.proxy_user_selection_is_sticky();
                                if !kept_current_proxy_rows {
                                    self.apply_proxy_groups(groups);
                                }
                                let report = self.update_diagnose_report(state).await;
                                let keep_hint = if kept_current_proxy_rows {
                                    "；保留当前代理列表，避免刷新打断选择"
                                } else {
                                    ""
                                };
                                self.set_refresh_status(
                                    format!("{runtime_message}；{}", diagnose_status_message(&report)) + keep_hint,
                                );
                            }
                        }
                    }
                    Err(err) => {
                        self.proxy_node_meta.clear();
                        let message = format!("策略组不可用：{err}");
                        if !self.apply_runtime_proxy_preview(state, &message).await {
                            let report = self.update_diagnose_report(state).await;
                            self.set_refresh_status(format!("{message}；{}", diagnose_status_message(&report)));
                        }
                    }
                }
                if let Ok(providers) = fetch_proxy_providers(state).await {
                    self.apply_proxy_providers(providers);
                } else {
                    self.proxy_providers.clear();
                    self.proxy_provider_index = 0;
                    self.proxy_provider_selection_key = None;
                }
            }
            View::Logs => {
                self.logs = actions::core::logs(state).await;
                if self.log_follow && !self.logs.is_empty() {
                    self.log_index = self.logs.len() - 1;
                }
            }
            View::Settings => match actions::config::settings(state).await {
                Ok(settings) => self.apply_settings(settings),
                Err(_) => self.settings = None,
            },
            View::Rules => {
                match fetch_rules(state).await {
                    Ok(rules) => self.rules = rules,
                    Err(err) => self.set_refresh_status(format!("规则不可用：{err}")),
                }
                if let Ok(providers) = fetch_rule_providers(state).await {
                    self.apply_rule_providers(providers);
                } else {
                    self.rule_providers.clear();
                    self.rule_provider_index = 0;
                    self.rule_provider_selection_key = None;
                }
            }
            View::Connections => match fetch_connections(state).await {
                Ok(connections) => self.connections = connections,
                Err(err) => self.set_refresh_status(format!("连接不可用：{err}")),
            },
            View::Jobs => {}
        }
        self.clamp_selections();
    }

    pub(crate) async fn refresh_now(&mut self, state: &AppState) {
        self.last_refresh = None;
        self.refresh(state).await;
        if !(matches!(self.view, View::Proxies) && self.proxy_groups.is_empty() && self.diagnose_report.is_some()) {
            self.set_status(format!("已刷新{}", self.view.title()));
        }
    }

    pub(crate) async fn refresh_dashboard(&mut self, state: &AppState) {
        self.kernel_snapshot = Some(actions::core::status(state).await);
        self.controller_status = fetch_controller_status(state).await;
        match actions::config::settings(state).await {
            Ok(settings) => self.apply_settings(settings),
            Err(_) => self.settings = None,
        }
        self.mode = actions::config::get_mode(state).await.ok();
        if let Ok(profiles) = actions::profiles::list(state).await {
            self.apply_profiles(profiles);
        }
        self.sync_dashboard_metrics(state);
        match fetch_proxy_groups_response(state).await {
            Ok(response) => {
                self.proxy_node_meta = proxy_node_meta_from_response(&response);
                self.apply_proxy_groups(proxy_groups_from_response(&response));
                if self.proxy_groups.is_empty()
                    && let Ok(groups) = runtime_proxy_groups_preview(state).await
                {
                    self.proxy_node_meta.clear();
                    self.apply_proxy_groups(groups);
                }
            }
            Err(_) => {
                self.proxy_node_meta.clear();
                self.apply_proxy_groups(runtime_proxy_groups_preview(state).await.unwrap_or_default());
            }
        }
    }

    pub(crate) fn sync_dashboard_metrics(&mut self, state: &AppState) {
        let metrics = state.metrics.snapshot();
        self.dashboard_metrics = DashboardMetrics {
            upload_speed: metrics.upload_speed,
            download_speed: metrics.download_speed,
            memory: metrics.memory,
        };
    }

    pub(crate) async fn run_diagnose(&mut self, state: &AppState) {
        let report = self.update_diagnose_report(state).await;
        self.set_status(diagnose_status_message(&report));
        self.record_diagnose_recommendations(&report);
    }

    pub(crate) async fn export_diagnose(&mut self, state: &AppState) {
        let (_, saved_hint) = self.update_and_save_diagnose_report(state).await;
        self.set_status(saved_hint);
    }

    pub(crate) async fn update_diagnose_report(&mut self, state: &AppState) -> actions::diagnose::DiagnoseReport {
        let report = actions::diagnose::report(state).await;
        self.kernel_snapshot = Some(report.kernel.clone());
        self.subscription_status = report.subscription.clone();
        self.controller_status = controller_status_from_health(&report.controller.health);
        self.diagnose_report = Some(report.clone());
        report
    }

    pub(crate) async fn update_and_save_diagnose_report(
        &mut self,
        state: &AppState,
    ) -> (actions::diagnose::DiagnoseReport, String) {
        let report = self.update_diagnose_report(state).await;
        let saved_hint = match actions::diagnose::save_report(state, &report).await {
            Ok(saved) => format!("诊断快照已保存：{}", saved.path),
            Err(err) => format!("诊断快照保存失败：{err}"),
        };
        (report, saved_hint)
    }

    pub(crate) async fn apply_runtime_proxy_preview(&mut self, state: &AppState, reason: &str) -> bool {
        let Ok(groups) = runtime_proxy_groups_preview(state).await else {
            return false;
        };
        if groups.is_empty() {
            return false;
        }
        self.proxy_node_meta.clear();
        self.apply_proxy_groups(groups);
        self.set_refresh_status(format!(
            "{reason}；已显示 runtime 离线预览 {} 个策略组；可预选节点，启动 Core 后自动应用",
            self.proxy_groups.len()
        ));
        true
    }

    pub(crate) async fn toggle_core(&mut self, state: &AppState) {
        let snapshot = actions::core::status(state).await;
        let result = match snapshot.state {
            KernelState::Stopped | KernelState::Crashed => actions::core::start(state).await.map(|status| {
                format!(
                    "核心启动请求{}，状态：{}",
                    accepted_label(status.accepted),
                    kernel_state_label(status.state)
                )
            }),
            KernelState::Running | KernelState::Unhealthy => actions::core::stop(state).await.map(|status| {
                format!(
                    "核心停止请求{}，状态：{}",
                    accepted_label(status.accepted),
                    kernel_state_label(status.state)
                )
            }),
            KernelState::Starting | KernelState::Stopping | KernelState::Restarting | KernelState::Updating => {
                Ok(format!("核心正忙：{}", kernel_state_label(snapshot.state)))
            }
        };
        self.set_action_status(result);
        self.last_refresh = None;
    }

    pub(crate) async fn restart_core(&mut self, state: &AppState) {
        let result = actions::core::restart(state).await.map(|status| {
            format!(
                "核心重启请求{}，状态：{}",
                accepted_label(status.accepted),
                kernel_state_label(status.state)
            )
        });
        self.set_action_status(result);
        self.last_refresh = None;
    }

    pub(crate) async fn cycle_mode(&mut self, state: &AppState) {
        let result = async {
            let current = actions::config::get_mode(state).await?;
            let next = match current {
                Mode::Rule => Mode::Global,
                Mode::Global => Mode::Direct,
                Mode::Direct => Mode::Rule,
            };
            let mode = actions::config::set_mode(state, next).await?;
            Ok::<_, anyhow::Error>((mode, format!("代理模式已切换为 {}", mode_label(mode))))
        }
        .await;
        match result {
            Ok((mode, message)) => {
                self.mode = Some(mode);
                self.set_action_status(Ok(message));
            }
            Err(err) => self.set_action_status(Err(err)),
        }
        self.last_refresh = None;
    }

    pub(crate) async fn update_selected_subscription(&mut self, state: &Arc<AppState>) {
        let Some(profile) = self.selected_profile() else {
            self.set_status("未选择订阅");
            return;
        };
        if profile.itype.as_deref() != Some("remote") {
            self.set_status("当前订阅不是远程订阅，不能在线更新");
            return;
        }
        let Some(uid) = profile.uid.clone() else {
            self.set_status("当前订阅缺少 uid");
            return;
        };

        let started = actions::subscriptions::update_one(Arc::clone(state), uid).await;
        let status = if started.created {
            format!("已创建订阅更新任务：{}", started.job.id)
        } else {
            format!("订阅更新任务已在运行：{}", started.job.id)
        };
        self.set_status(status);
        self.last_refresh = None;
    }

    pub(crate) async fn update_all_subscriptions(&mut self, state: &Arc<AppState>) {
        let result = actions::subscriptions::update_all(Arc::clone(state))
            .await
            .map(|sweep| subscription_sweep_status_message(&sweep));
        self.set_action_status(result);
        self.last_refresh = None;
    }

    pub(crate) async fn open_provider_dialog(&mut self, state: &AppState, kind: ProviderDialogKind) {
        match self.refresh_provider_rows(state, kind).await {
            Ok(0) => {
                self.provider_dialog = None;
                self.set_important_status(format!("当前配置没有{}", kind.label()));
            }
            Ok(count) => {
                self.provider_dialog = Some(kind);
                self.set_status(format!("已打开{}，共 {count} 个", kind.label()));
            }
            Err(err) => {
                self.provider_dialog = None;
                self.set_important_status(format!(
                    "{}不可用：{}",
                    kind.label(),
                    sanitize_url_error(&err.to_string())
                ));
            }
        }
    }

    pub(crate) async fn refresh_provider_dialog(&mut self, state: &AppState) {
        let Some(kind) = self.provider_dialog else {
            return;
        };
        match self.refresh_provider_rows(state, kind).await {
            Ok(0) => {
                self.provider_dialog = None;
                self.set_important_status(format!("当前配置没有{}，已关闭弹窗", kind.label()));
            }
            Ok(count) => self.set_status(format!("已刷新{}，共 {count} 个", kind.label())),
            Err(err) => {
                self.set_important_status(format!(
                    "刷新{}失败：{}",
                    kind.label(),
                    sanitize_url_error(&err.to_string())
                ));
            }
        }
    }

    pub(crate) async fn update_selected_dialog_provider(&mut self, state: &AppState) {
        let Some(kind) = self.provider_dialog else {
            return;
        };
        let Some(provider) = self.selected_provider_name(kind) else {
            self.set_status("未选择 Provider");
            return;
        };
        let result = match kind {
            ProviderDialogKind::Proxy => actions::controller::update_provider(state, &provider).await,
            ProviderDialogKind::Rule => actions::controller::update_rule_provider(state, &provider).await,
        };
        self.set_provider_operation_status(kind, &provider, ProviderOperation::Update, result);
        self.refresh_after_provider_update(state, kind).await;
    }

    pub(crate) async fn update_all_dialog_providers(&mut self, state: &AppState) {
        let Some(kind) = self.provider_dialog else {
            return;
        };
        let providers = self.provider_names(kind);
        if providers.is_empty() {
            self.set_status("当前配置没有 Provider");
            return;
        }

        let mut succeeded = 0usize;
        let mut failed = 0usize;
        let mut errors = Vec::new();
        for provider in providers {
            let result = match kind {
                ProviderDialogKind::Proxy => actions::controller::update_provider(state, &provider).await,
                ProviderDialogKind::Rule => actions::controller::update_rule_provider(state, &provider).await,
            };
            match result {
                Ok(result) => {
                    succeeded += 1;
                    let provider_name = if result.provider.trim().is_empty() {
                        provider.as_str()
                    } else {
                        result.provider.as_str()
                    };
                    self.set_provider_feedback(kind, provider_name, "更新已触发");
                }
                Err(err) => {
                    failed += 1;
                    self.set_provider_feedback(kind, &provider, "更新失败");
                    if errors.len() < 3 {
                        errors.push(format!("{provider}: {}", sanitize_url_error(&err.to_string())));
                    }
                }
            }
        }

        self.refresh_after_provider_update(state, kind).await;
        let error_hint = if errors.is_empty() {
            String::new()
        } else {
            format!("；失败示例：{}", errors.join("；"))
        };
        self.set_important_status(format!(
            "{}批量更新完成：成功 {succeeded}，失败 {failed}{error_hint}",
            kind.label()
        ));
    }

    pub(crate) fn set_provider_operation_status(
        &mut self,
        kind: ProviderDialogKind,
        requested_provider: &str,
        operation: ProviderOperation,
        result: Result<crate::mihomo_controller::ProviderOperationResult>,
    ) {
        match result {
            Ok(result) => {
                let provider = if result.provider.trim().is_empty() {
                    requested_provider
                } else {
                    &result.provider
                };
                let row_message = format!("{}已触发", provider_operation_label(result.operation));
                self.set_provider_feedback(kind, provider, &row_message);
                self.set_important_status(format!("{} {} {}；已刷新相关数据", kind.label(), provider, row_message));
            }
            Err(err) => {
                let row_message = format!("{}失败", provider_operation_label(operation));
                self.set_provider_feedback(kind, requested_provider, &row_message);
                self.set_important_status(format!(
                    "{} {} {}：{}",
                    kind.label(),
                    requested_provider,
                    row_message,
                    sanitize_url_error(&err.to_string())
                ));
            }
        }
    }

    pub(crate) async fn refresh_after_provider_update(&mut self, state: &AppState, kind: ProviderDialogKind) {
        let _ = self.refresh_provider_rows(state, kind).await;
        match kind {
            ProviderDialogKind::Proxy => {
                if let Ok(response) = fetch_proxy_groups_response(state).await {
                    let summary = proxy_group_load_summary(&response);
                    if summary.is_ready() {
                        self.proxy_node_meta = proxy_node_meta_from_response(&response);
                        self.apply_proxy_groups(proxy_groups_from_response(&response));
                    }
                }
            }
            ProviderDialogKind::Rule => {
                if let Ok(rules) = fetch_rules(state).await {
                    self.rules = rules;
                    self.clamp_selections();
                }
            }
        }
        self.last_refresh = None;
    }

    pub(crate) async fn retry_selected_job(&mut self, state: &Arc<AppState>) {
        let Some(job) = self.selected_job().cloned() else {
            self.set_status("未选择任务");
            return;
        };
        if job.kind != "profile-update" {
            self.set_status("只有订阅更新任务支持重试");
            return;
        }
        let Some(target) = job.target.clone() else {
            self.set_status("当前任务缺少目标");
            return;
        };
        let started = actions::subscriptions::update_one(Arc::clone(state), target).await;
        let status = if started.created {
            format!("已创建重试任务：{}", started.job.id)
        } else {
            format!("已有同类任务正在运行：{}", started.job.id)
        };
        self.set_status(status);
        self.last_refresh = None;
    }

    pub(crate) async fn cancel_selected_job(&mut self, state: &AppState) {
        let Some(job) = self.selected_job() else {
            self.set_status("未选择任务");
            return;
        };
        let Some(report) = state.jobs.cancel_report(&job.id).await else {
            self.set_status("任务不存在");
            return;
        };
        self.set_status(format!("任务取消：{}", report.message));
        self.last_refresh = None;
    }

    pub(crate) async fn activate_selected(&mut self, state: &Arc<AppState>) {
        match self.view {
            View::Dashboard => self.activate_dashboard(state).await,
            View::Profiles => self.confirm_switch_profile(),
            View::Proxies => self.activate_proxy_selection(state).await,
            View::Logs => self.open_selected_log_detail(),
            View::Settings => self.activate_selected_setting(state).await,
            View::Rules => self.set_status(self.selected_rule_summary()),
            View::Connections => self.open_selected_connection_detail(),
            View::Jobs => self.open_selected_job_detail(),
        }
    }

    pub(crate) async fn activate_dashboard(&mut self, state: &AppState) {
        match self.dashboard_proxy_popup {
            DashboardProxyPopup::None => self.open_dashboard_proxy_nodes(),
            DashboardProxyPopup::Groups => {
                let status = self.dashboard_proxy_group().map_or_else(
                    || "未选择代理组".into(),
                    |group| format!("已定位代理组：{}，请选择节点", group.name),
                );
                self.mark_dashboard_proxy_user_selection();
                self.remember_dashboard_proxy_group_selection();
                self.focus_dashboard_proxy_nodes_on_current();
                self.dashboard_proxy_popup = DashboardProxyPopup::Nodes;
                self.set_status(status);
            }
            DashboardProxyPopup::Nodes => {
                self.activate_dashboard_proxy_node(state).await;
                self.dashboard_proxy_popup = DashboardProxyPopup::None;
            }
        }
    }

    pub(crate) fn open_dashboard_proxy_groups(&mut self) {
        self.restore_dashboard_proxy_group_selection_from_key();
        self.clamp_dashboard_proxy_group_selection();
        self.remember_dashboard_proxy_group_selection();
        self.dashboard_proxy_popup = DashboardProxyPopup::Groups;
        self.set_status("首页代理组选择：↑↓ 移动，Enter 定位节点，Esc 收起");
    }

    pub(crate) fn open_dashboard_proxy_nodes(&mut self) {
        if self.proxy_groups.is_empty() {
            self.set_status("尚未加载代理组；按 r 刷新，或按 3 进入代理页查看诊断");
            return;
        }
        self.focus_dashboard_proxy_nodes_on_current();
        self.dashboard_proxy_popup = DashboardProxyPopup::Nodes;
        let action = self
            .dashboard_proxy_group()
            .filter(|group| group.offline)
            .map_or("应用节点", |_| "预选节点");
        self.set_status(format!("首页节点选择：↑↓ 移动，Enter {action}，Esc 收起"));
    }

    pub(crate) fn confirm_dashboard_tun(&mut self) {
        let Some(settings) = self.settings.clone() else {
            self.set_status("设置尚未加载，按 r 刷新后再切换 TUN");
            return;
        };
        let diagnostic = if settings.tun_diagnostics.can_enable {
            "当前环境具备 TUN 基本条件"
        } else {
            "当前环境可能缺少 /dev/net/tun 或权限"
        };
        self.confirm = Some(ConfirmState {
            prompt: format!(
                "确认{} TUN？{diagnostic}；会影响本机网络路由，y 确认 / n 取消",
                bool_action_label(!settings.tun_enabled)
            ),
            action: ConfirmAction::ToggleTun {
                enabled: !settings.tun_enabled,
            },
        });
    }

    pub(crate) fn confirm_dashboard_system_proxy(&mut self) {
        let Some(settings) = self.settings.clone() else {
            self.set_status("设置尚未加载，按 r 刷新后再切换系统代理");
            return;
        };
        let diagnostic = if settings.system_proxy_diagnostics.can_auto_apply {
            format!(
                "当前环境自动应用可用，将设置 {}:{}",
                settings.system_proxy_diagnostics.endpoint.host, settings.system_proxy_diagnostics.endpoint.port
            )
        } else {
            format!(
                "当前环境需手动配置 HTTP/HTTPS/SOCKS {}:{}，失败会回滚",
                settings.system_proxy_diagnostics.endpoint.host, settings.system_proxy_diagnostics.endpoint.port
            )
        };
        self.confirm = Some(ConfirmState {
            prompt: format!(
                "确认{}系统代理？{diagnostic}；会修改本机代理设置，y 确认 / n 取消",
                bool_action_label(!settings.system_proxy_enabled),
            ),
            action: ConfirmAction::ToggleSystemProxy {
                enabled: !settings.system_proxy_enabled,
            },
        });
    }

    pub(crate) async fn toggle_dashboard_dns(&mut self, state: &Arc<AppState>) {
        let Some(settings) = self.settings.clone() else {
            self.set_status("设置尚未加载，按 r 刷新后再切换 DNS");
            return;
        };
        match actions::config::set_dns_enabled(Arc::clone(state), !settings.dns_enabled).await {
            Ok(settings) => {
                self.apply_settings(settings);
                self.set_status("DNS 覆写已切换");
            }
            Err(err) => self.set_status(format!("错误：{err}")),
        }
        self.last_refresh = None;
    }

    pub(crate) async fn activate_proxy_selection(&mut self, state: &AppState) {
        match self.proxy_pane {
            ProxyPane::Groups => {
                self.focus_proxy_nodes_on_current();
                let status = self.selected_proxy_group().map_or_else(
                    || "未选择策略组".into(),
                    |group| format!("已定位策略组：{}，请选择节点", group.name),
                );
                self.set_status(status);
            }
            ProxyPane::Nodes => {
                let Some(group) = self.selected_proxy_group() else {
                    self.set_status("未选择策略组");
                    return;
                };
                let Some(proxy) = self.selected_proxy_node_name() else {
                    self.set_status("未选择代理节点");
                    return;
                };
                let group_name = group.name.clone();
                let offline = group.offline;
                self.apply_proxy_node_selection(state, group_name, proxy, offline).await;
            }
        }
    }

    pub(crate) async fn activate_dashboard_proxy_node(&mut self, state: &AppState) {
        let Some(group) = self.dashboard_proxy_group() else {
            self.set_status("未选择代理组");
            return;
        };
        let Some(proxy) = self.selected_dashboard_proxy_node_name() else {
            self.set_status("未选择代理节点");
            return;
        };
        let group_name = group.name.clone();
        let offline = group.offline;
        self.apply_proxy_node_selection(state, group_name, proxy, offline).await;
    }

    pub(crate) async fn apply_proxy_node_selection(
        &mut self,
        state: &AppState,
        group_name: String,
        proxy: String,
        offline: bool,
    ) {
        if offline {
            match actions::controller::save_proxy_selection(state, &group_name, &proxy).await {
                Ok(result) => {
                    self.apply_profiles_without_runtime_reset(result.profiles);
                    self.set_selected_proxy_group_now(&group_name, &proxy);
                    self.set_status(format!("已预选 {group_name} -> {proxy}，启动 Core 后自动应用"));
                }
                Err(err) => self.set_status(sanitize_url_error(&err.to_string())),
            }
            self.last_refresh = None;
            return;
        }
        match actions::controller::select_or_preselect_proxy(state, &group_name, &proxy).await {
            Ok(result) => {
                self.apply_profiles_without_runtime_reset(result.profiles);
                self.set_selected_proxy_group_now(&group_name, &proxy);
                if result.preselected {
                    self.set_status(format!("已预选 {group_name} -> {proxy}，启动 Core 后自动应用"));
                } else {
                    self.set_status(format!("已为 {group_name} 选择 {proxy}"));
                }
            }
            Err(err) => self.set_status(sanitize_url_error(&err.to_string())),
        }
        self.last_refresh = None;
    }

    pub(crate) async fn test_selected_proxy_node_delay(&mut self, state: &AppState) {
        let Some(group) = self.selected_proxy_group().cloned() else {
            self.set_status("未选择策略组");
            return;
        };
        let Some(proxy) = self.selected_proxy_node_name() else {
            self.set_status("未选择代理节点");
            return;
        };
        if group.offline {
            self.set_status(format!("当前为离线预览，启动 Core 后才能测速节点：{proxy}"));
            return;
        }

        self.proxy_group_selection_key = Some(group.name.clone());
        self.proxy_node_selection_key = Some(proxy.clone());
        self.mark_proxy_user_selection();

        match actions::controller::test_proxy_delay(state, &proxy).await {
            Ok(result) => {
                let delay = result.delay;
                let delay_label = views::layout::format_proxy_delay(delay);
                self.proxy_node_meta
                    .entry(proxy.clone())
                    .or_insert_with(|| ProxyNodeMeta {
                        proxy_type: "-".into(),
                        delay_ms: None,
                        alive: None,
                    })
                    .delay_ms = delay;
                self.refresh_proxy_rows_after_delay_test(state).await;
                self.set_important_status(format!("已测速 {proxy}：{delay_label}"));
            }
            Err(err) => {
                self.set_important_status(format!("测速 {proxy} 失败：{}", sanitize_url_error(&err.to_string())));
            }
        }
        self.last_refresh = None;
    }

    pub(crate) async fn refresh_proxy_rows_after_delay_test(&mut self, state: &AppState) {
        let Ok(response) = fetch_proxy_groups_response(state).await else {
            return;
        };
        let summary = proxy_group_load_summary(&response);
        if summary.is_ready() {
            self.proxy_node_meta = proxy_node_meta_from_response(&response);
            self.apply_proxy_groups(proxy_groups_from_response(&response));
        }
        self.restore_proxy_selection_for_current_pane();
        self.remember_proxy_selection_for_current_pane();
    }

    #[allow(clippy::cognitive_complexity)]
    pub(crate) async fn activate_selected_setting(&mut self, state: &Arc<AppState>) {
        let Some(settings) = self.settings.clone() else {
            self.set_status("设置尚未加载");
            return;
        };
        match SETTINGS_ROWS[self.setting_index] {
            SettingRow::Dns => match actions::config::set_dns_enabled(Arc::clone(state), !settings.dns_enabled).await {
                Ok(settings) => {
                    self.apply_settings(settings);
                    self.set_status("DNS 覆写已切换");
                }
                Err(err) => self.set_status(format!("错误：{err}")),
            },
            SettingRow::Ipv6 => match actions::config::set_ipv6(state, !settings.ipv6).await {
                Ok(settings) => {
                    self.apply_settings(settings);
                    self.set_status("IPv6 已切换");
                }
                Err(err) => self.set_status(format!("错误：{err}")),
            },
            SettingRow::AllowLan => match actions::config::set_allow_lan(state, !settings.allow_lan).await {
                Ok(settings) => {
                    self.apply_settings(settings);
                    self.set_status("允许局域网已切换");
                }
                Err(err) => self.set_status(format!("错误：{err}")),
            },
            SettingRow::UnifiedDelay => {
                match actions::config::set_unified_delay(state, !settings.unified_delay).await {
                    Ok(settings) => {
                        self.apply_settings(settings);
                        self.set_status("统一延迟已切换");
                    }
                    Err(err) => self.set_status(format!("错误：{err}")),
                }
            }
            SettingRow::TuiTheme => {
                let current = terminal_display::parse_theme(Some(&settings.tui_theme.configured))
                    .unwrap_or(terminal_display::TuiTheme::Orange);
                let next = current.next();
                match actions::config::set_tui_theme(state, next).await {
                    Ok(settings) => {
                        let configured_label = settings.tui_theme.configured_label.clone();
                        let effective_label = settings.tui_theme.effective_label.clone();
                        let overridden = settings.tui_theme.overridden;
                        self.apply_settings(settings);
                        if overridden {
                            self.set_status(format!(
                                "主题已保存为 {configured_label}；当前受 CLASH_TUI_THEME 覆盖为 {effective_label}"
                            ));
                        } else {
                            self.set_status(format!("主题已切换为 {effective_label}"));
                        }
                    }
                    Err(err) => self.set_status(format!("错误：{err}")),
                }
            }
            SettingRow::TuiDisplayMode => {
                let current = terminal_display::parse_display_mode(Some(&settings.tui_display_mode.configured))
                    .unwrap_or(terminal_display::TuiDisplayMode::Standard);
                let next = current.next();
                match actions::config::set_tui_display_mode(state, next).await {
                    Ok(settings) => {
                        let configured_label = settings.tui_display_mode.configured_label.clone();
                        let effective_label = settings.tui_display_mode.effective_label.clone();
                        let overridden = settings.tui_display_mode.overridden;
                        self.apply_settings(settings);
                        if overridden {
                            self.set_status(format!(
                                "终端显示已保存为 {configured_label}；当前受 CLASH_TUI_DISPLAY_MODE 覆盖为 {effective_label}"
                            ));
                        } else {
                            self.set_status(format!("终端显示已切换为 {effective_label}"));
                        }
                    }
                    Err(err) => self.set_status(format!("错误：{err}")),
                }
            }
            SettingRow::TuiPunctuationMode => {
                let current = terminal_display::parse_punctuation_mode(Some(&settings.tui_punctuation_mode.configured))
                    .unwrap_or(terminal_display::TuiPunctuationMode::Preserve);
                let next = current.next();
                match actions::config::set_tui_punctuation_mode(state, next).await {
                    Ok(settings) => {
                        let configured_label = settings.tui_punctuation_mode.configured_label.clone();
                        let effective_label = settings.tui_punctuation_mode.effective_label.clone();
                        let overridden = settings.tui_punctuation_mode.overridden;
                        self.apply_settings(settings);
                        if overridden {
                            self.set_status(format!(
                                "中文标点已保存为 {configured_label}；当前受 CLASH_TUI_PUNCTUATION_MODE 覆盖为 {effective_label}"
                            ));
                        } else {
                            self.set_status(format!("中文标点已切换为 {effective_label}"));
                        }
                    }
                    Err(err) => self.set_status(format!("错误：{err}")),
                }
            }
            SettingRow::LogLevel => {
                let next = next_log_level(&settings.log_level);
                match actions::config::set_log_level(state, next).await {
                    Ok(settings) => {
                        self.apply_settings(settings);
                        self.set_status(format!("日志等级已设置为 {next}"));
                    }
                    Err(err) => self.set_status(format!("错误：{err}")),
                }
            }
            SettingRow::RuleProviderDownloadProxy => {
                let next = settings.rule_provider_download_proxy.next();
                match actions::config::set_rule_provider_download_proxy(state, next).await {
                    Ok(settings) => {
                        let label = rule_provider_download_proxy_label(settings.rule_provider_download_proxy);
                        self.apply_settings(settings);
                        self.set_status(format!("规则 Provider 下载已切换为{label}"));
                    }
                    Err(err) => self.set_status(format!("错误：{err}")),
                }
            }
            SettingRow::CoreLog => {
                let enabled = !settings.core_log_enabled;
                let snapshot = actions::core::status(state).await;
                if matches!(
                    snapshot.state,
                    KernelState::Running | KernelState::Starting | KernelState::Restarting | KernelState::Unhealthy
                ) {
                    self.confirm = Some(ConfirmState {
                        prompt: format!(
                            "确认{}核心日志？Core 运行中需要重启后生效，y 确认 / n 取消",
                            bool_action_label(enabled)
                        ),
                        action: ConfirmAction::ToggleCoreLog { enabled },
                    });
                } else {
                    match actions::config::set_core_log_enabled(state, enabled).await {
                        Ok(settings) => {
                            self.apply_settings(settings);
                            self.set_status(format!("核心日志已{}，下次启动 Core 生效", bool_label(enabled)));
                        }
                        Err(err) => self.set_status(format!("错误：{err}")),
                    }
                }
            }
            SettingRow::MixedPort => self.edit_selected_setting(),
            SettingRow::ExternalController => {
                self.confirm = Some(ConfirmState {
                    prompt: format!(
                        "确认{}外部控制器？开启后 mihomo 将监听本机 127.0.0.1:{}，y 确认 / n 取消",
                        bool_action_label(!settings.external_controller.enabled),
                        settings.external_controller.port
                    ),
                    action: ConfirmAction::ToggleExternalController {
                        enabled: !settings.external_controller.enabled,
                    },
                });
            }
            SettingRow::ExternalControllerPort => self.edit_selected_setting(),
            SettingRow::Tun => {
                let diagnostic = if settings.tun_diagnostics.can_enable {
                    "当前环境具备 TUN 基本条件"
                } else {
                    "当前环境可能缺少 /dev/net/tun 或权限"
                };
                self.confirm = Some(ConfirmState {
                    prompt: format!(
                        "确认{} TUN？{diagnostic}；会影响本机网络路由，y 确认 / n 取消",
                        bool_action_label(!settings.tun_enabled)
                    ),
                    action: ConfirmAction::ToggleTun {
                        enabled: !settings.tun_enabled,
                    },
                });
            }
            SettingRow::SystemProxy => {
                let diagnostic = if settings.system_proxy_diagnostics.can_auto_apply {
                    format!(
                        "当前环境自动应用可用，将设置 {}:{}",
                        settings.system_proxy_diagnostics.endpoint.host,
                        settings.system_proxy_diagnostics.endpoint.port
                    )
                } else {
                    format!(
                        "当前环境需手动配置 HTTP/HTTPS/SOCKS {}:{}，失败会回滚",
                        settings.system_proxy_diagnostics.endpoint.host,
                        settings.system_proxy_diagnostics.endpoint.port
                    )
                };
                self.confirm = Some(ConfirmState {
                    prompt: format!(
                        "确认{}系统代理？{diagnostic}；会修改本机代理设置，y 确认 / n 取消",
                        bool_action_label(!settings.system_proxy_enabled),
                    ),
                    action: ConfirmAction::ToggleSystemProxy {
                        enabled: !settings.system_proxy_enabled,
                    },
                });
            }
        }
        self.last_refresh = None;
    }

    pub(crate) fn edit_selected_setting(&mut self) {
        if !matches!(self.view, View::Settings) {
            return;
        }
        match SETTINGS_ROWS[self.setting_index] {
            SettingRow::MixedPort => {
                let value = self
                    .settings
                    .as_ref()
                    .map_or_else(String::new, |settings| settings.mixed_port.to_string());
                self.input = Some(InputState {
                    target: InputTarget::MixedPort,
                    value,
                });
                self.set_status("正在编辑混合端口");
            }
            SettingRow::ExternalControllerPort => {
                let value = self
                    .settings
                    .as_ref()
                    .map_or_else(String::new, |settings| settings.external_controller.port.to_string());
                self.input = Some(InputState {
                    target: InputTarget::ExternalControllerPort,
                    value,
                });
                self.set_status("正在编辑外部控制端口");
            }
            _ => {
                self.set_status("该设置请按 Enter 切换");
            }
        }
    }

    pub(crate) fn confirm_switch_profile(&mut self) {
        let Some(profile) = self.selected_profile() else {
            self.set_status("未选择订阅");
            return;
        };
        if !matches!(profile.itype.as_deref(), Some("local" | "remote")) {
            self.set_status("只有本地或远程订阅可以启用");
            return;
        }
        let Some(uid) = profile.uid.clone() else {
            self.set_status("当前订阅缺少 uid");
            return;
        };
        if self.profiles_current.as_deref() == Some(uid.as_str()) {
            self.set_status("该订阅已是当前启用项");
            return;
        }
        let name = profile.name.clone().unwrap_or_else(|| uid.clone());
        self.confirm = Some(ConfirmState {
            prompt: format!("确认切换到订阅 {name}？运行中的核心会热加载配置，y 确认 / n 取消"),
            action: ConfirmAction::SwitchProfile { uid, name },
        });
    }

    pub(crate) fn confirm_delete_profile(&mut self) {
        let Some(profile) = self.selected_profile() else {
            self.set_status("未选择订阅");
            return;
        };
        if !matches!(profile.itype.as_deref(), Some("local" | "remote")) {
            self.set_status("内置订阅不能删除");
            return;
        }
        let Some(uid) = profile.uid.clone() else {
            self.set_status("当前订阅缺少 uid");
            return;
        };
        let name = profile.name.clone().unwrap_or_else(|| uid.clone());
        self.confirm = Some(ConfirmState {
            prompt: format!("确认删除订阅 {name}？y 确认 / n 取消"),
            action: ConfirmAction::DeleteProfile { uid, name },
        });
    }

    pub(crate) fn confirm_close_selected_connection(&mut self) {
        let Some(connection) = self.selected_connection() else {
            self.set_status("未选择连接");
            return;
        };
        if connection.id.is_empty() {
            self.set_status("当前连接缺少 id");
            return;
        }
        self.confirm = Some(ConfirmState {
            prompt: format!("确认关闭连接 {}？y 确认 / n 取消", connection.id),
            action: ConfirmAction::CloseConnection {
                id: connection.id.clone(),
            },
        });
    }

    pub(crate) fn open_selected_connection_detail(&mut self) {
        let Some(connection) = self.selected_connection() else {
            self.set_status("未选择连接");
            return;
        };
        let connection_id = connection.id.clone();
        let lines = connection_detail_lines(connection);
        self.detail = Some(DetailState {
            title: "连接详情".into(),
            lines,
        });
        self.set_status(format!("正在查看连接：{connection_id}"));
    }

    pub(crate) fn open_selected_log_detail(&mut self) {
        let Some(log) = self.selected_log() else {
            self.set_status("未选择日志");
            return;
        };
        let safe = terminal_safe_log_text(log);
        let mut lines = safe.split(" <换行> ").map(str::to_owned).collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push("空日志".into());
        }
        self.detail = Some(DetailState {
            title: "日志详情".into(),
            lines,
        });
        self.set_status("正在查看日志详情");
    }

    pub(crate) fn start_search(&mut self) {
        if matches!(self.view, View::Dashboard | View::Settings) {
            self.set_status("当前页面不支持过滤");
            return;
        }
        let value = match self.view {
            View::Profiles => self.profile_query.clone(),
            View::Proxies => self.proxy_query.clone(),
            View::Logs => self.log_query.clone(),
            View::Rules => self.rule_query.clone(),
            View::Connections => self.connection_query.clone(),
            View::Jobs => self.job_query.clone(),
            View::Dashboard | View::Settings => String::new(),
        };
        self.input = Some(InputState {
            target: InputTarget::Search(self.view),
            value,
        });
    }

    pub(crate) fn start_subscription_import(&mut self) {
        self.input = Some(InputState {
            target: InputTarget::ImportSubscriptionUrl,
            value: String::new(),
        });
        self.set_status("请粘贴订阅链接，按 Enter 导入");
    }

    pub(crate) fn start_local_profile_import(&mut self) {
        self.input = Some(InputState {
            target: InputTarget::ImportLocalProfilePath,
            value: String::new(),
        });
        self.set_status("请输入本地配置文件路径，按 Enter 导入");
    }

    pub(crate) fn cycle_log_level_filter(&mut self) {
        self.log_level_filter = self.log_level_filter.next();
        self.clamp_selections();
        self.set_status(format!("日志等级过滤：{}", self.log_level_filter.title()));
    }

    pub(crate) fn cycle_proxy_node_sort(&mut self) {
        if !matches!(self.view, View::Proxies) {
            return;
        }
        self.remember_proxy_node_selection();
        let was_nodes = matches!(self.proxy_pane, ProxyPane::Nodes);
        self.proxy_node_sort = self.proxy_node_sort.next();
        if was_nodes {
            self.clamp_proxy_node_selection();
            self.remember_proxy_node_selection();
        } else {
            self.focus_proxy_nodes_on_current();
        }
        self.set_status(format!("节点排序：{}", self.proxy_node_sort.title()));
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        match self.view {
            View::Profiles => {
                let indices = self.filtered_profile_indices();
                move_in_indices(&mut self.profile_index, &indices, delta);
            }
            View::Proxies => match self.proxy_pane {
                ProxyPane::Groups => {
                    let indices = self.filtered_proxy_group_indices();
                    move_in_indices(&mut self.proxy_group_index, &indices, delta);
                    self.mark_proxy_user_selection();
                    self.remember_proxy_group_selection();
                    self.select_current_node_for_selected_proxy_group();
                }
                ProxyPane::Nodes => {
                    let indices = self.filtered_proxy_node_indices();
                    move_in_indices(&mut self.proxy_node_index, &indices, delta);
                    self.mark_proxy_user_selection();
                    self.remember_proxy_node_selection();
                }
            },
            View::Logs => {
                self.log_follow = false;
                let indices = self.filtered_log_indices();
                move_in_indices(&mut self.log_index, &indices, delta);
            }
            View::Settings => move_index(&mut self.setting_index, SETTINGS_ROWS.len(), delta),
            View::Rules => {
                let indices = self.filtered_rule_indices();
                move_in_indices(&mut self.rule_index, &indices, delta);
            }
            View::Connections => {
                let indices = self.filtered_connection_indices();
                move_in_indices(&mut self.connection_index, &indices, delta);
            }
            View::Jobs => {
                let indices = self.filtered_job_indices();
                move_in_indices(&mut self.job_index, &indices, delta);
            }
            View::Dashboard => match self.dashboard_proxy_popup {
                DashboardProxyPopup::Groups => {
                    let indices = self.filtered_dashboard_proxy_group_indices();
                    move_in_indices(&mut self.dashboard_proxy_group_index, &indices, delta);
                    self.mark_dashboard_proxy_user_selection();
                    self.remember_dashboard_proxy_group_selection();
                    self.select_current_node_for_dashboard_proxy_group();
                }
                DashboardProxyPopup::Nodes => {
                    let indices = self.filtered_dashboard_proxy_node_indices();
                    move_in_indices(&mut self.dashboard_proxy_node_index, &indices, delta);
                    self.mark_dashboard_proxy_user_selection();
                    self.remember_dashboard_proxy_node_selection();
                }
                DashboardProxyPopup::None => {}
            },
        }
    }

    pub(crate) fn move_to_edge(&mut self, end: bool) {
        let delta = if end { isize::MAX } else { isize::MIN };
        self.move_selection(delta);
    }

    pub(crate) fn apply_profiles(&mut self, profiles: ProfileCatalog) {
        let current_changed = self.profiles_current != profiles.current;
        self.profiles_current = profiles.current;
        self.profiles = profiles.items.unwrap_or_default();
        if current_changed {
            self.clear_profile_bound_runtime_state();
        }
        self.restore_profile_proxy_group_selection();
        self.clamp_selections();
    }

    pub(crate) fn apply_profiles_without_runtime_reset(&mut self, profiles: ProfileCatalog) {
        self.profiles_current = profiles.current;
        self.profiles = profiles.items.unwrap_or_default();
        self.restore_profile_proxy_group_selection();
        self.clamp_selections();
    }

    pub(crate) fn clear_profile_bound_runtime_state(&mut self) {
        self.proxy_groups.clear();
        self.proxy_group_index = 0;
        self.proxy_node_index = 0;
        self.proxy_node_meta.clear();
        self.proxy_providers.clear();
        self.proxy_provider_index = 0;
        self.rule_providers.clear();
        self.rule_provider_index = 0;
        self.provider_dialog = None;
        self.provider_operation_feedback.clear();
        self.proxy_group_selection_key = None;
        self.proxy_node_selection_key = None;
        self.proxy_provider_selection_key = None;
        self.rule_provider_selection_key = None;
        self.proxy_user_selection_at = None;
        self.proxy_pane = ProxyPane::Groups;
        self.dashboard_proxy_popup = DashboardProxyPopup::None;
        self.dashboard_proxy_group_index = 0;
        self.dashboard_proxy_node_index = 0;
        self.dashboard_proxy_group_selection_key = None;
        self.dashboard_proxy_node_selection_key = None;
        self.dashboard_proxy_user_selection_at = None;
        self.rules.clear();
        self.rule_index = 0;
        self.connections.clear();
        self.connection_index = 0;
        self.diagnose_report = None;
    }

    pub(crate) fn apply_proxy_groups(&mut self, groups: Vec<ProxyGroupRow>) {
        let current_group_name = self.selected_proxy_group().map(|group| group.name.clone());
        let remembered_group_name = self.proxy_group_selection_key.clone();
        let current_node_name = self
            .selected_proxy_group()
            .and_then(|group| group.nodes.get(self.proxy_node_index))
            .cloned();
        let remembered_node_name = self.proxy_node_selection_key.clone();
        let current_dashboard_group_name = self.dashboard_proxy_group().map(|group| group.name.clone());
        let remembered_dashboard_group_name = self.dashboard_proxy_group_selection_key.clone();
        let current_dashboard_node_name = self
            .dashboard_proxy_group()
            .and_then(|group| group.nodes.get(self.dashboard_proxy_node_index))
            .cloned();
        let remembered_dashboard_node_name = self.dashboard_proxy_node_selection_key.clone();

        self.proxy_groups = groups;

        if let Some(index) =
            self.preferred_proxy_group_index(current_group_name.as_deref(), remembered_group_name.as_deref())
        {
            self.proxy_group_index = index;
        }
        let indices = self.filtered_proxy_group_indices();
        clamp_with_indices(&mut self.proxy_group_index, &indices);
        if self.selected_proxy_group().is_some() {
            self.remember_proxy_group_selection();
        }

        if let Some(index) =
            self.preferred_proxy_node_index(current_node_name.as_deref(), remembered_node_name.as_deref())
        {
            self.proxy_node_index = index;
        }
        self.clamp_proxy_node_selection();
        if self.selected_proxy_group().is_some() {
            self.remember_proxy_node_selection();
        }

        if let Some(index) = current_dashboard_group_name
            .as_deref()
            .and_then(|name| self.find_proxy_group_index(name))
            .or_else(|| {
                remembered_dashboard_group_name
                    .as_deref()
                    .and_then(|name| self.find_proxy_group_index(name))
            })
        {
            self.dashboard_proxy_group_index = index;
        }
        self.clamp_dashboard_proxy_group_selection();
        if self.dashboard_proxy_group().is_some() {
            self.remember_dashboard_proxy_group_selection();
        }

        if let Some(index) = current_dashboard_node_name
            .as_deref()
            .and_then(|name| self.find_dashboard_proxy_node_index(name))
            .or_else(|| {
                remembered_dashboard_node_name
                    .as_deref()
                    .and_then(|name| self.find_dashboard_proxy_node_index(name))
            })
        {
            self.dashboard_proxy_node_index = index;
        }
        self.clamp_dashboard_proxy_node_selection();
        if self.dashboard_proxy_group().is_some() {
            self.remember_dashboard_proxy_node_selection();
        }
    }

    pub(crate) async fn refresh_provider_rows(&mut self, state: &AppState, kind: ProviderDialogKind) -> Result<usize> {
        match kind {
            ProviderDialogKind::Proxy => {
                let rows = fetch_proxy_providers(state).await?;
                let count = rows.len();
                self.apply_proxy_providers(rows);
                Ok(count)
            }
            ProviderDialogKind::Rule => {
                let rows = fetch_rule_providers(state).await?;
                let count = rows.len();
                self.apply_rule_providers(rows);
                Ok(count)
            }
        }
    }

    pub(crate) fn apply_proxy_providers(&mut self, providers: Vec<ProxyProviderRow>) {
        let current = self.selected_provider_name(ProviderDialogKind::Proxy);
        let remembered = self.proxy_provider_selection_key.clone();
        self.proxy_providers = providers;
        if let Some(index) = current
            .as_deref()
            .and_then(|name| self.find_proxy_provider_index(name))
            .or_else(|| {
                remembered
                    .as_deref()
                    .and_then(|name| self.find_proxy_provider_index(name))
            })
        {
            self.proxy_provider_index = index;
        }
        clamp_index(&mut self.proxy_provider_index, self.proxy_providers.len());
        self.remember_provider_selection(ProviderDialogKind::Proxy);
    }

    pub(crate) fn apply_rule_providers(&mut self, providers: Vec<RuleProviderRow>) {
        let current = self.selected_provider_name(ProviderDialogKind::Rule);
        let remembered = self.rule_provider_selection_key.clone();
        self.rule_providers = providers;
        if let Some(index) = current
            .as_deref()
            .and_then(|name| self.find_rule_provider_index(name))
            .or_else(|| {
                remembered
                    .as_deref()
                    .and_then(|name| self.find_rule_provider_index(name))
            })
        {
            self.rule_provider_index = index;
        }
        clamp_index(&mut self.rule_provider_index, self.rule_providers.len());
        self.remember_provider_selection(ProviderDialogKind::Rule);
    }

    pub(crate) fn move_provider_selection(&mut self, delta: isize) {
        let Some(kind) = self.provider_dialog else {
            return;
        };
        let len = self.provider_len(kind);
        match kind {
            ProviderDialogKind::Proxy => move_index(&mut self.proxy_provider_index, len, delta),
            ProviderDialogKind::Rule => move_index(&mut self.rule_provider_index, len, delta),
        }
        self.remember_provider_selection(kind);
    }

    pub(crate) fn move_provider_to_edge(&mut self, end: bool) {
        let Some(kind) = self.provider_dialog else {
            return;
        };
        let len = self.provider_len(kind);
        let index = if end { len.saturating_sub(1) } else { 0 };
        match kind {
            ProviderDialogKind::Proxy => self.proxy_provider_index = index,
            ProviderDialogKind::Rule => self.rule_provider_index = index,
        }
        self.remember_provider_selection(kind);
    }

    pub(crate) const fn provider_len(&self, kind: ProviderDialogKind) -> usize {
        match kind {
            ProviderDialogKind::Proxy => self.proxy_providers.len(),
            ProviderDialogKind::Rule => self.rule_providers.len(),
        }
    }

    pub(crate) fn provider_names(&self, kind: ProviderDialogKind) -> Vec<String> {
        match kind {
            ProviderDialogKind::Proxy => self
                .proxy_providers
                .iter()
                .map(|provider| provider.name.clone())
                .collect(),
            ProviderDialogKind::Rule => self
                .rule_providers
                .iter()
                .map(|provider| provider.name.clone())
                .collect(),
        }
    }

    pub(crate) fn selected_provider_name(&self, kind: ProviderDialogKind) -> Option<String> {
        match kind {
            ProviderDialogKind::Proxy => self
                .proxy_providers
                .get(self.proxy_provider_index)
                .map(|provider| provider.name.clone()),
            ProviderDialogKind::Rule => self
                .rule_providers
                .get(self.rule_provider_index)
                .map(|provider| provider.name.clone()),
        }
    }

    pub(crate) fn find_proxy_provider_index(&self, name: &str) -> Option<usize> {
        self.proxy_providers
            .iter()
            .position(|provider| proxy_selection_key_matches(&provider.name, name))
    }

    pub(crate) fn find_rule_provider_index(&self, name: &str) -> Option<usize> {
        self.rule_providers
            .iter()
            .position(|provider| proxy_selection_key_matches(&provider.name, name))
    }

    pub(crate) fn remember_provider_selection(&mut self, kind: ProviderDialogKind) {
        match kind {
            ProviderDialogKind::Proxy => {
                self.proxy_provider_selection_key = self
                    .proxy_providers
                    .get(self.proxy_provider_index)
                    .map(|provider| provider.name.clone());
            }
            ProviderDialogKind::Rule => {
                self.rule_provider_selection_key = self
                    .rule_providers
                    .get(self.rule_provider_index)
                    .map(|provider| provider.name.clone());
            }
        }
    }

    pub(crate) fn provider_feedback(&self, kind: ProviderDialogKind, provider: &str) -> Option<&str> {
        self.provider_operation_feedback
            .get(&provider_feedback_key(kind, provider))
            .map(String::as_str)
    }

    pub(crate) fn set_provider_feedback(&mut self, kind: ProviderDialogKind, provider: &str, message: &str) {
        self.provider_operation_feedback
            .insert(provider_feedback_key(kind, provider), message.to_owned());
    }

    pub(crate) fn focus_proxy_groups_after_import(&mut self) {
        self.view = View::Proxies;
        self.proxy_pane = ProxyPane::Nodes;
        self.proxy_query.clear();
        self.proxy_group_index = 0;
        self.proxy_node_index = 0;
        self.proxy_group_selection_key = None;
        self.proxy_node_selection_key = None;
        self.proxy_user_selection_at = None;
        self.dashboard_proxy_group_index = 0;
        self.dashboard_proxy_node_index = 0;
        self.dashboard_proxy_group_selection_key = None;
        self.dashboard_proxy_node_selection_key = None;
        self.dashboard_proxy_user_selection_at = None;
        self.focus_proxy_nodes_on_current();
        self.focus_dashboard_proxy_nodes_on_current();
    }

    pub(crate) fn enter_proxy_view(&mut self) {
        if self.selected_proxy_group().is_some() {
            self.focus_proxy_nodes_on_current();
        } else {
            self.proxy_pane = ProxyPane::Nodes;
            self.clamp_proxy_node_selection();
        }
    }

    pub(crate) fn focus_proxy_nodes_on_current(&mut self) {
        self.proxy_pane = ProxyPane::Nodes;
        self.select_current_node_for_selected_proxy_group();
        self.remember_proxy_group_selection();
        self.remember_proxy_node_selection();
    }

    pub(crate) fn select_current_node_for_selected_proxy_group(&mut self) {
        self.proxy_node_index = self
            .selected_proxy_group()
            .and_then(|group| group.nodes.iter().position(|node| node == &group.now))
            .unwrap_or(0);
        self.proxy_node_selection_key = self
            .selected_proxy_group()
            .and_then(|group| group.nodes.get(self.proxy_node_index))
            .cloned();
        if !self.proxy_query.trim().is_empty() && self.filtered_proxy_node_indices().is_empty() {
            self.proxy_query.clear();
        }
        self.clamp_proxy_node_selection();
        self.remember_proxy_node_selection();
    }

    pub(crate) fn focus_dashboard_proxy_nodes_on_current(&mut self) {
        self.select_current_node_for_dashboard_proxy_group();
        self.remember_dashboard_proxy_group_selection();
        self.remember_dashboard_proxy_node_selection();
    }

    pub(crate) fn select_current_node_for_dashboard_proxy_group(&mut self) {
        self.dashboard_proxy_node_index = self
            .dashboard_proxy_group()
            .and_then(|group| group.nodes.iter().position(|node| node == &group.now))
            .unwrap_or(0);
        self.dashboard_proxy_node_selection_key = self
            .dashboard_proxy_group()
            .and_then(|group| group.nodes.get(self.dashboard_proxy_node_index))
            .cloned();
        self.clamp_dashboard_proxy_node_selection();
        self.remember_dashboard_proxy_node_selection();
    }

    pub(crate) fn focus_next_proxy_pane(&mut self) {
        self.remember_proxy_selection_for_current_pane();
        self.proxy_pane = self.proxy_pane.next();
        match self.proxy_pane {
            ProxyPane::Groups => self.restore_proxy_group_selection_from_key(),
            ProxyPane::Nodes => self.clamp_proxy_node_selection(),
        }
        self.remember_proxy_selection_for_current_pane();
        self.set_status(format!("当前焦点：{}", self.proxy_pane.title()));
    }

    pub(crate) fn mark_proxy_user_selection(&mut self) {
        self.proxy_user_selection_at = Some(Instant::now());
    }

    pub(crate) const fn proxy_user_selection_is_sticky(&self) -> bool {
        self.proxy_user_selection_at.is_some()
    }

    pub(crate) fn mark_dashboard_proxy_user_selection(&mut self) {
        self.dashboard_proxy_user_selection_at = Some(Instant::now());
    }

    pub(crate) const fn dashboard_proxy_user_selection_is_sticky(&self) -> bool {
        self.dashboard_proxy_user_selection_at.is_some()
    }

    pub(crate) fn preferred_proxy_group_index(&self, current: Option<&str>, remembered: Option<&str>) -> Option<usize> {
        if self.proxy_user_selection_is_sticky()
            && let Some(index) = remembered.and_then(|name| self.find_proxy_group_index(name))
        {
            return Some(index);
        }
        current
            .and_then(|name| self.find_proxy_group_index(name))
            .or_else(|| remembered.and_then(|name| self.find_proxy_group_index(name)))
    }

    pub(crate) fn preferred_proxy_node_index(&self, current: Option<&str>, remembered: Option<&str>) -> Option<usize> {
        if self.proxy_user_selection_is_sticky()
            && let Some(index) = remembered.and_then(|name| self.find_proxy_node_index(name))
        {
            return Some(index);
        }
        current
            .and_then(|name| self.find_proxy_node_index(name))
            .or_else(|| remembered.and_then(|name| self.find_proxy_node_index(name)))
    }

    pub(crate) fn find_proxy_group_index(&self, name: &str) -> Option<usize> {
        self.proxy_groups
            .iter()
            .position(|group| proxy_selection_key_matches(&group.name, name))
    }

    pub(crate) fn find_proxy_node_index(&self, name: &str) -> Option<usize> {
        self.selected_proxy_group().and_then(|group| {
            group
                .nodes
                .iter()
                .position(|node| proxy_selection_key_matches(node, name))
        })
    }

    pub(crate) fn find_dashboard_proxy_node_index(&self, name: &str) -> Option<usize> {
        self.dashboard_proxy_group().and_then(|group| {
            group
                .nodes
                .iter()
                .position(|node| proxy_selection_key_matches(node, name))
        })
    }

    pub(crate) fn set_selected_proxy_group_now(&mut self, group_name: &str, proxy: &str) {
        let updated_index = self.find_proxy_group_index(group_name);
        if let Some(index) = updated_index
            && let Some(group) = self.proxy_groups.get_mut(index)
        {
            group.now = proxy.to_owned();
        }
        if self
            .selected_proxy_group()
            .is_some_and(|group| proxy_selection_key_matches(&group.name, group_name))
            && let Some(group) = self.selected_proxy_group()
        {
            if let Some(index) = group.nodes.iter().position(|node| node == proxy) {
                self.proxy_node_index = index;
            }
            self.remember_proxy_group_selection();
            self.remember_proxy_node_selection();
        }
        if self
            .dashboard_proxy_group()
            .is_some_and(|group| proxy_selection_key_matches(&group.name, group_name))
            && let Some(group) = self.dashboard_proxy_group()
        {
            if let Some(index) = group.nodes.iter().position(|node| node == proxy) {
                self.dashboard_proxy_node_index = index;
            }
            self.remember_dashboard_proxy_group_selection();
            self.remember_dashboard_proxy_node_selection();
        }
        self.clamp_proxy_node_selection();
        self.clamp_dashboard_proxy_node_selection();
    }

    pub(crate) fn clamp_proxy_node_selection(&mut self) {
        self.restore_proxy_node_selection_from_key();
        let indices = self.filtered_proxy_node_indices();
        clamp_with_indices(&mut self.proxy_node_index, &indices);
    }

    pub(crate) fn clamp_dashboard_proxy_group_selection(&mut self) {
        self.restore_dashboard_proxy_group_selection_from_key();
        let indices = self.filtered_dashboard_proxy_group_indices();
        clamp_with_indices(&mut self.dashboard_proxy_group_index, &indices);
    }

    pub(crate) fn clamp_dashboard_proxy_node_selection(&mut self) {
        self.restore_dashboard_proxy_node_selection_from_key();
        let indices = self.filtered_dashboard_proxy_node_indices();
        clamp_with_indices(&mut self.dashboard_proxy_node_index, &indices);
    }

    pub(crate) fn clamp_selections(&mut self) {
        let indices = self.filtered_profile_indices();
        clamp_with_indices(&mut self.profile_index, &indices);
        self.restore_proxy_group_selection_from_key();
        let indices = self.filtered_proxy_group_indices();
        clamp_with_indices(&mut self.proxy_group_index, &indices);
        self.clamp_proxy_node_selection();
        self.clamp_dashboard_proxy_group_selection();
        self.clamp_dashboard_proxy_node_selection();
        let indices = self.filtered_rule_indices();
        clamp_with_indices(&mut self.rule_index, &indices);
        let indices = self.filtered_connection_indices();
        clamp_with_indices(&mut self.connection_index, &indices);
        let indices = self.filtered_log_indices();
        clamp_with_indices(&mut self.log_index, &indices);
        clamp_index(&mut self.setting_index, SETTINGS_ROWS.len());
        let indices = self.filtered_job_indices();
        clamp_with_indices(&mut self.job_index, &indices);
    }

    pub(crate) fn restore_proxy_group_selection_from_key(&mut self) {
        let Some(name) = self.proxy_group_selection_key.as_deref() else {
            return;
        };
        if let Some(index) = self
            .proxy_groups
            .iter()
            .position(|group| proxy_selection_key_matches(&group.name, name))
        {
            self.proxy_group_index = index;
        }
    }

    pub(crate) fn restore_proxy_node_selection_from_key(&mut self) {
        let Some(node) = self.proxy_node_selection_key.as_deref() else {
            return;
        };
        let Some(group) = self.selected_proxy_group() else {
            return;
        };
        if let Some(index) = group
            .nodes
            .iter()
            .position(|item| proxy_selection_key_matches(item, node))
        {
            self.proxy_node_index = index;
        }
    }

    pub(crate) fn restore_dashboard_proxy_group_selection_from_key(&mut self) {
        let Some(name) = self.dashboard_proxy_group_selection_key.as_deref() else {
            return;
        };
        if let Some(index) = self
            .proxy_groups
            .iter()
            .position(|group| proxy_selection_key_matches(&group.name, name))
        {
            self.dashboard_proxy_group_index = index;
        }
    }

    pub(crate) fn restore_dashboard_proxy_node_selection_from_key(&mut self) {
        let Some(node) = self.dashboard_proxy_node_selection_key.as_deref() else {
            return;
        };
        let Some(group) = self.dashboard_proxy_group() else {
            return;
        };
        if let Some(index) = group
            .nodes
            .iter()
            .position(|item| proxy_selection_key_matches(item, node))
        {
            self.dashboard_proxy_node_index = index;
        }
    }

    pub(crate) fn restore_proxy_selection_for_current_pane(&mut self) {
        self.restore_proxy_group_selection_from_key();
        match self.proxy_pane {
            ProxyPane::Groups => {}
            ProxyPane::Nodes => self.clamp_proxy_node_selection(),
        }
    }

    pub(crate) fn remember_proxy_selection_for_current_pane(&mut self) {
        match self.proxy_pane {
            ProxyPane::Groups => self.remember_proxy_group_selection(),
            ProxyPane::Nodes => self.remember_proxy_node_selection(),
        }
    }

    pub(crate) fn remember_proxy_group_selection(&mut self) {
        if let Some(group) = self.selected_proxy_group() {
            self.proxy_group_selection_key = Some(group.name.clone());
        }
    }

    pub(crate) fn remember_dashboard_proxy_group_selection(&mut self) {
        if let Some(group) = self.dashboard_proxy_group() {
            self.dashboard_proxy_group_selection_key = Some(group.name.clone());
        }
    }

    pub(crate) fn remember_dashboard_proxy_node_selection(&mut self) {
        let Some(group) = self.dashboard_proxy_group() else {
            return;
        };
        if let Some(node) = self.selected_dashboard_proxy_node_name() {
            let group_name = group.name.clone();
            self.dashboard_proxy_group_selection_key = Some(group_name);
            self.dashboard_proxy_node_selection_key = Some(node);
        }
    }

    pub(crate) fn profile_proxy_group_selection(&self) -> Option<String> {
        let current = self.profiles_current.as_deref()?;
        let profile = self
            .profiles
            .iter()
            .find(|profile| profile.uid.as_deref() == Some(current))?;
        profile.selected.as_deref()?.iter().rev().find_map(|selected| {
            let name = selected.name.as_deref()?.trim();
            if name.is_empty() { None } else { Some(name.to_owned()) }
        })
    }

    pub(crate) fn remember_proxy_node_selection(&mut self) {
        let Some(group) = self.selected_proxy_group() else {
            return;
        };
        if let Some(node) = self.selected_proxy_node_name() {
            let group_name = group.name.clone();
            self.proxy_group_selection_key = Some(group_name);
            self.proxy_node_selection_key = Some(node);
        }
    }

    pub(crate) fn interaction_line(&self) -> String {
        if let Some(busy) = &self.busy {
            return format!("处理中：{}", terminal_safe_text(&busy.message));
        }
        if let Some(confirm) = &self.confirm {
            return format!("确认：{}", confirm.prompt);
        }
        if let Some(input) = &self.input {
            return format!(
                "{}: {}  Enter 应用  Esc 取消",
                input_target_title(input.target),
                input_display_value(input)
            );
        }
        if let Some(kind) = self.provider_dialog {
            return format!(
                "{}：↑↓ 选择，Enter/u 更新选中，a 更新全部，r 刷新，Esc 关闭",
                kind.label()
            );
        }
        match self.view {
            View::Dashboard if self.dashboard_proxy_popup == DashboardProxyPopup::Groups => {
                "首页代理组选择：↑↓ 移动，Enter 定位节点，Esc 收起".into()
            }
            View::Dashboard if self.dashboard_proxy_popup == DashboardProxyPopup::Nodes => {
                "首页节点选择：↑↓ 移动，Enter 应用节点，Esc 收起".into()
            }
            View::Profiles if !self.profile_query.is_empty() => format!("过滤：{}", self.profile_query),
            View::Proxies if !self.proxy_query.is_empty() => format!("过滤：{}", self.proxy_query),
            View::Logs if !self.log_query.is_empty() || self.log_level_filter != LogLevelFilter::All => {
                let mut filters = Vec::new();
                if self.log_level_filter != LogLevelFilter::All {
                    filters.push(format!("等级：{}", self.log_level_filter.title()));
                }
                if !self.log_query.is_empty() {
                    filters.push(format!("内容：{}", self.log_query));
                }
                format!("过滤：{}", filters.join(" | "))
            }
            View::Rules if !self.rule_query.is_empty() => format!("过滤：{}", self.rule_query),
            View::Connections if !self.connection_query.is_empty() => format!("过滤：{}", self.connection_query),
            View::Jobs if !self.job_query.is_empty() => format!("过滤：{}", self.job_query),
            _ => String::new(),
        }
    }

    pub(crate) fn selected_profile(&self) -> Option<&ProfileEntry> {
        self.profiles.get(self.profile_index)
    }

    pub(crate) fn selected_proxy_group(&self) -> Option<&ProxyGroupRow> {
        self.proxy_groups.get(self.proxy_group_index)
    }

    pub(crate) fn dashboard_proxy_group(&self) -> Option<&ProxyGroupRow> {
        self.proxy_groups.get(self.dashboard_proxy_group_index)
    }

    pub(crate) fn selected_proxy_node_name(&self) -> Option<String> {
        let group = self.selected_proxy_group()?;
        if let Some(node) = group.nodes.get(self.proxy_node_index)
            && self.filtered_proxy_node_indices().contains(&self.proxy_node_index)
        {
            return Some(node.clone());
        }
        let remembered = self.proxy_node_selection_key.as_deref()?;
        group
            .nodes
            .iter()
            .find(|node| proxy_selection_key_matches(node, remembered))
            .cloned()
    }

    pub(crate) fn selected_dashboard_proxy_node_name(&self) -> Option<String> {
        let group = self.dashboard_proxy_group()?;
        if let Some(node) = group.nodes.get(self.dashboard_proxy_node_index)
            && self
                .filtered_dashboard_proxy_node_indices()
                .contains(&self.dashboard_proxy_node_index)
        {
            return Some(node.clone());
        }
        let remembered = self.dashboard_proxy_node_selection_key.as_deref()?;
        group
            .nodes
            .iter()
            .find(|node| proxy_selection_key_matches(node, remembered))
            .cloned()
    }

    pub(crate) fn selected_connection(&self) -> Option<&ConnectionRecord> {
        self.connections.get(self.connection_index)
    }

    pub(crate) fn selected_job(&self) -> Option<&JobRecord> {
        self.jobs.get(self.job_index)
    }

    pub(crate) fn selected_log(&self) -> Option<&str> {
        self.logs.get(self.log_index).map(String::as_str)
    }

    pub(crate) fn filtered_profile_indices(&self) -> Vec<usize> {
        let query = self.profile_query.trim();
        filter_indices(self.profiles.len(), |index| {
            let profile = &self.profiles[index];
            query.is_empty()
                || text_matches(
                    query,
                    [
                        profile.uid.as_deref(),
                        profile.name.as_deref(),
                        profile.itype.as_deref(),
                        profile.desc.as_deref(),
                    ],
                )
        })
    }

    pub(crate) fn filtered_proxy_group_indices(&self) -> Vec<usize> {
        let query = if self.proxy_pane == ProxyPane::Groups {
            self.proxy_query.trim()
        } else {
            ""
        };
        filter_indices(self.proxy_groups.len(), |index| {
            let group = &self.proxy_groups[index];
            query.is_empty()
                || text_matches(query, [Some(group.name.as_str()), Some(group.now.as_str()), None, None])
                || group
                    .nodes
                    .iter()
                    .any(|node| text_matches(query, [Some(node.as_str()), None, None, None]))
        })
    }

    pub(crate) fn filtered_proxy_node_indices(&self) -> Vec<usize> {
        let Some(group) = self.selected_proxy_group() else {
            return Vec::new();
        };
        let query = if self.proxy_pane == ProxyPane::Nodes {
            self.proxy_query.trim()
        } else {
            ""
        };
        let mut indices = filter_indices(group.nodes.len(), |index| {
            let node = &group.nodes[index];
            let meta = self.proxy_node_meta.get(node);
            let delay = views::layout::format_proxy_delay(meta.and_then(|meta| meta.delay_ms));
            query.is_empty()
                || text_matches(
                    query,
                    [
                        Some(node.as_str()),
                        meta.map(|meta| meta.proxy_type.as_str()),
                        Some(delay.as_str()),
                        meta.and_then(|meta| meta.alive).map(alive_label),
                    ],
                )
        });
        self.sort_proxy_node_indices(group, &mut indices);
        indices
    }

    pub(crate) fn filtered_dashboard_proxy_group_indices(&self) -> Vec<usize> {
        filter_indices(self.proxy_groups.len(), |_| true)
    }

    pub(crate) fn filtered_dashboard_proxy_node_indices(&self) -> Vec<usize> {
        let Some(group) = self.dashboard_proxy_group() else {
            return Vec::new();
        };
        let mut indices = filter_indices(group.nodes.len(), |_| true);
        self.sort_proxy_node_indices(group, &mut indices);
        indices
    }

    pub(crate) fn sort_proxy_node_indices(&self, group: &ProxyGroupRow, indices: &mut [usize]) {
        match self.proxy_node_sort {
            ProxyNodeSort::Subscription => {}
            ProxyNodeSort::Latency => indices.sort_by(|left, right| {
                let left = self.proxy_node_sort_key(group, *left);
                let right = self.proxy_node_sort_key(group, *right);
                (left.delay, left.alive, left.index).cmp(&(right.delay, right.alive, right.index))
            }),
            ProxyNodeSort::Alive => indices.sort_by(|left, right| {
                let left = self.proxy_node_sort_key(group, *left);
                let right = self.proxy_node_sort_key(group, *right);
                (left.alive, left.delay, left.index).cmp(&(right.alive, right.delay, right.index))
            }),
        }
    }

    pub(crate) fn proxy_node_sort_key(&self, group: &ProxyGroupRow, index: usize) -> ProxyNodeSortKey {
        let meta = group.nodes.get(index).and_then(|node| self.proxy_node_meta.get(node));
        ProxyNodeSortKey {
            index,
            delay: meta
                .and_then(|meta| meta.delay_ms)
                .filter(|delay| *delay >= 0)
                .unwrap_or(i64::MAX),
            alive: match meta.and_then(|meta| meta.alive) {
                Some(true) => 0,
                None => 1,
                Some(false) => 2,
            },
        }
    }

    pub(crate) fn filtered_rule_indices(&self) -> Vec<usize> {
        let query = self.rule_query.trim();
        filter_indices(self.rules.len(), |index| {
            query.is_empty() || self.rules[index].matches_query(query)
        })
    }

    pub(crate) fn filtered_connection_indices(&self) -> Vec<usize> {
        let query = self.connection_query.trim();
        filter_indices(self.connections.len(), |index| {
            let connection = &self.connections[index];
            let metadata = connection.metadata.as_ref();
            query.is_empty()
                || text_matches(
                    query,
                    [
                        Some(connection.id.as_str()),
                        connection.rule.as_deref(),
                        connection.rule_payload.as_deref(),
                        metadata.and_then(|metadata| metadata.host.as_deref()),
                        metadata.and_then(|metadata| metadata.process.as_deref()),
                        metadata.and_then(|metadata| metadata.source_ip.as_deref()),
                        metadata.and_then(|metadata| metadata.destination_ip.as_deref()),
                    ],
                )
                || connection
                    .chains
                    .iter()
                    .any(|chain| text_matches(query, [Some(chain.as_str()), None, None, None]))
        })
    }

    pub(crate) fn filtered_log_indices(&self) -> Vec<usize> {
        let query = self.log_query.trim();
        filter_indices(self.logs.len(), |index| {
            self.log_level_filter.matches(&self.logs[index])
                && (query.is_empty()
                    || self.logs[index]
                        .to_ascii_lowercase()
                        .contains(&query.to_ascii_lowercase()))
        })
    }

    pub(crate) fn filtered_job_indices(&self) -> Vec<usize> {
        let query = self.job_query.trim();
        filter_indices(self.jobs.len(), |index| {
            let job = &self.jobs[index];
            query.is_empty()
                || text_matches(
                    query,
                    [
                        Some(job.id.as_str()),
                        Some(job.kind.as_str()),
                        Some(job.name.as_str()),
                        job.target.as_deref(),
                        Some(job_status_label(job.status)),
                        job.message.as_deref(),
                        job.error.as_deref(),
                    ],
                )
        })
    }

    pub(crate) fn selected_rule_summary(&self) -> String {
        let Some(rule) = self.rules.get(self.rule_index) else {
            return "未选择规则".into();
        };
        format!(
            "{} {} -> {}",
            rule.r#type.as_deref().unwrap_or("RULE"),
            rule.payload.as_deref().unwrap_or("-"),
            rule.proxy.as_deref().unwrap_or("-")
        )
    }

    pub(crate) fn open_selected_job_detail(&mut self) {
        let Some(job) = self.selected_job() else {
            self.set_status("未选择任务");
            return;
        };
        let job_id = job.id.clone();
        let lines = job_detail_lines(job);
        self.detail = Some(DetailState {
            title: "任务详情".into(),
            lines,
        });
        self.set_status(format!("正在查看任务：{job_id}"));
    }

    pub(crate) fn diagnose_summary_line(&self) -> Option<String> {
        self.diagnose_report
            .as_ref()
            .map(|report| format!("最近{}", diagnose_status_message(report)))
    }

    pub(crate) fn diagnose_runtime_detail_lines(&self) -> Vec<String> {
        self.diagnose_report
            .as_ref()
            .map(diagnose_runtime_detail_lines)
            .unwrap_or_default()
    }

    pub(crate) fn diagnose_recommendation_lines(&self) -> Vec<String> {
        self.diagnose_report
            .as_ref()
            .map(|report| diagnose_recommendation_lines(report, DIAGNOSE_RECOMMENDATION_VIEW_LIMIT))
            .unwrap_or_default()
    }

    pub(crate) fn set_action_status(&mut self, result: Result<String>) {
        match result {
            Ok(message) => self.set_important_status(message),
            Err(err) => self.set_important_status(format!("错误：{err}")),
        }
    }
}

const fn provider_operation_label(operation: ProviderOperation) -> &'static str {
    match operation {
        ProviderOperation::Update => "更新",
        ProviderOperation::Healthcheck => "测速",
    }
}

fn provider_feedback_key(kind: ProviderDialogKind, provider: &str) -> String {
    format!("{}:{provider}", kind.feedback_prefix())
}
