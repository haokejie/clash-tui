mod details;
mod events;
mod frame;
mod input;
mod labels;
mod models;
mod refresh;
mod runner;
mod selection;
mod state;
mod text;

pub use runner::run;

pub(crate) use labels::{
    alive_label, bool_label, job_status_label, kernel_state_label, mode_label, seconds_until_label, setting_label,
    setting_value, settings_rows,
};
pub(crate) use models::{DashboardProxyPopup, ProviderDialogKind, ProviderSubscriptionInfoRow, ProxyPane, SettingRow};
pub(crate) use selection::{content_rows, visible_indices, visible_indices_with_offset};
pub(crate) use state::TuiApp;
pub(crate) use text::{profile_update_message_label, sanitize_url_error, terminal_safe_log_text, terminal_safe_text};

#[cfg(test)]
pub(crate) use details::subscription_sweep_status_message;
#[cfg(test)]
pub(crate) use events::drain_job_events;
#[cfg(test)]
pub(crate) use frame::{footer_line_strings, render, render_transient_modal};
#[cfg(test)]
pub(crate) use input::{TuiInputEvent, tui_input_event_trace_line, tui_input_events_from_bytes};
#[cfg(test)]
pub(crate) use labels::switch_status_message;
#[cfg(test)]
pub(crate) use models::{
    BusyState, ConfirmAction, ConfirmState, DashboardMetrics, DetailState, InputState, InputTarget, LogLevelFilter,
    MIN_TUI_HEIGHT, MIN_TUI_WIDTH, ProxyGroupLoadSummary, ProxyGroupRow, ProxyNodeMeta, ProxyNodeSort,
    ProxyProviderRow, SETTINGS_ROWS, STATUS_HISTORY_LIMIT, View,
};
#[cfg(test)]
pub(crate) use refresh::{
    diagnose_status_message, provider_names_for_auto_refresh, proxy_group_load_summary, proxy_groups_empty_message,
    proxy_node_meta_from_response, proxy_providers_from_response, runtime_proxy_groups_from_yaml,
    runtime_proxy_summary_from_yaml,
};
#[cfg(test)]
pub(crate) use text::{normalize_pasted_text, pasted_subscription_url, validate_subscription_url};

#[cfg(test)]
mod tests;
