pub mod views;

mod app;

pub use app::run;

pub(crate) use app::{
    DashboardProxyPopup, ProviderDialogKind, ProviderSubscriptionInfoRow, ProxyPane, SettingRow, TuiApp, alive_label,
    bool_label, content_rows, job_status_label, kernel_state_label, mode_label, profile_update_message_label,
    sanitize_url_error, seconds_until_label, setting_label, setting_value, settings_rows, terminal_safe_log_text,
    terminal_safe_text, visible_indices, visible_indices_with_offset,
};
