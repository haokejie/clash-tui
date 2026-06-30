use std::time::Duration;

use super::text::log_has_level;

pub(crate) const CONTROLLER_TIMEOUT: Duration = Duration::from_millis(800);
pub(crate) const IMPORT_PROXY_READY_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const IMPORT_PROVIDER_PROXY_READY_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const IMPORT_PROXY_READY_INTERVAL: Duration = Duration::from_millis(500);
pub(crate) const PROVIDER_REFRESH_TIMEOUT: Duration = Duration::from_secs(8);
pub(crate) const PROVIDER_REFRESH_SETTLE_INTERVAL: Duration = Duration::from_secs(1);
pub(crate) const MAX_AUTO_PROVIDER_REFRESHES: usize = 16;
pub(crate) const IMPORTANT_STATUS_PIN: Duration = Duration::from_secs(8);
pub(crate) const STATUS_HISTORY_LIMIT: usize = 20;
pub(crate) const DIAGNOSE_RECOMMENDATION_HISTORY_LIMIT: usize = 8;
pub(crate) const DIAGNOSE_RECOMMENDATION_VIEW_LIMIT: usize = 5;
pub(crate) const MIN_TUI_WIDTH: u16 = 80;
pub(crate) const MIN_TUI_HEIGHT: u16 = 20;
pub(crate) const SETTINGS_ROWS: [SettingRow; 15] = [
    SettingRow::TuiTheme,
    SettingRow::TuiDisplayMode,
    SettingRow::TuiPunctuationMode,
    SettingRow::MixedPort,
    SettingRow::LogLevel,
    SettingRow::CoreLog,
    SettingRow::RuleProviderDownloadProxy,
    SettingRow::Dns,
    SettingRow::Ipv6,
    SettingRow::AllowLan,
    SettingRow::UnifiedDelay,
    SettingRow::ExternalController,
    SettingRow::ExternalControllerPort,
    SettingRow::SystemProxy,
    SettingRow::Tun,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Dashboard,
    Profiles,
    Proxies,
    Logs,
    Settings,
    Rules,
    Connections,
    Jobs,
}

impl View {
    pub(crate) const ALL: [Self; 8] = [
        Self::Dashboard,
        Self::Profiles,
        Self::Proxies,
        Self::Logs,
        Self::Settings,
        Self::Rules,
        Self::Connections,
        Self::Jobs,
    ];

    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "总览",
            Self::Profiles => "订阅",
            Self::Proxies => "代理",
            Self::Logs => "日志",
            Self::Settings => "设置",
            Self::Rules => "规则",
            Self::Connections => "连接",
            Self::Jobs => "任务",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyPane {
    Groups,
    Nodes,
}

impl ProxyPane {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Groups => "策略组",
            Self::Nodes => "节点",
        }
    }

    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Groups => Self::Nodes,
            Self::Nodes => Self::Groups,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum DashboardProxyPopup {
    #[default]
    None,
    Groups,
    Nodes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyNodeSort {
    Subscription,
    Latency,
    Alive,
}

impl ProxyNodeSort {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Subscription => "订阅顺序",
            Self::Latency => "延迟优先",
            Self::Alive => "可用优先",
        }
    }

    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Subscription => Self::Latency,
            Self::Latency => Self::Alive,
            Self::Alive => Self::Subscription,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogLevelFilter {
    All,
    Error,
    Warn,
    Info,
    Debug,
}

impl LogLevelFilter {
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::All => Self::Error,
            Self::Error => Self::Warn,
            Self::Warn => Self::Info,
            Self::Info => Self::Debug,
            Self::Debug => Self::All,
        }
    }

    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Error => "错误",
            Self::Warn => "警告",
            Self::Info => "信息",
            Self::Debug => "调试",
        }
    }

    pub(crate) fn matches(self, log: &str) -> bool {
        match self {
            Self::All => true,
            Self::Error => log_has_level(log, &["error", "fatal"]),
            Self::Warn => log_has_level(log, &["warn", "warning"]),
            Self::Info => log_has_level(log, &["info"]),
            Self::Debug => log_has_level(log, &["debug", "trace"]),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingRow {
    Dns,
    Ipv6,
    AllowLan,
    UnifiedDelay,
    TuiTheme,
    TuiDisplayMode,
    TuiPunctuationMode,
    LogLevel,
    CoreLog,
    RuleProviderDownloadProxy,
    MixedPort,
    ExternalController,
    ExternalControllerPort,
    Tun,
    SystemProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputTarget {
    Search(View),
    MixedPort,
    ExternalControllerPort,
    ImportLocalProfilePath,
    ImportSubscriptionUrl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputState {
    pub(crate) target: InputTarget,
    pub(crate) value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfirmAction {
    SwitchProfile { uid: String, name: String },
    DeleteProfile { uid: String, name: String },
    CloseConnection { id: String },
    CloseAllConnections,
    ClearLogs,
    ToggleTun { enabled: bool },
    ToggleSystemProxy { enabled: bool },
    ToggleExternalController { enabled: bool },
    SetExternalControllerPort { port: u16 },
    ToggleCoreLog { enabled: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmState {
    pub(crate) prompt: String,
    pub(crate) action: ConfirmAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BusyState {
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailState {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderDialogKind {
    Proxy,
    Rule,
}

impl ProviderDialogKind {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Proxy => "Proxy Provider",
            Self::Rule => "Rule Provider",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Proxy => "代理 Provider",
            Self::Rule => "规则 Provider",
        }
    }

    pub(crate) const fn feedback_prefix(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Rule => "rule",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProxyGroupRow {
    pub(crate) name: String,
    pub(crate) now: String,
    pub(crate) nodes: Vec<String>,
    pub(crate) offline: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProviderSubscriptionInfoRow {
    pub(crate) upload: Option<u64>,
    pub(crate) download: Option<u64>,
    pub(crate) total: Option<u64>,
    pub(crate) expire: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProxyProviderRow {
    pub(crate) name: String,
    pub(crate) provider_type: String,
    pub(crate) vehicle_type: String,
    pub(crate) proxy_count: usize,
    pub(crate) updated_at: Option<String>,
    pub(crate) subscription: Option<ProviderSubscriptionInfoRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RuleProviderRow {
    pub(crate) name: String,
    pub(crate) provider_type: String,
    pub(crate) vehicle_type: String,
    pub(crate) behavior: String,
    pub(crate) format: String,
    pub(crate) rule_count: usize,
    pub(crate) updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProxyNodeMeta {
    pub(crate) proxy_type: String,
    pub(crate) delay_ms: Option<i64>,
    pub(crate) alive: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProxyNodeSortKey {
    pub(crate) index: usize,
    pub(crate) delay: i64,
    pub(crate) alive: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProxyGroupLoadSummary {
    pub(crate) entries: usize,
    pub(crate) groups: usize,
    pub(crate) nodes: usize,
}

impl ProxyGroupLoadSummary {
    pub(crate) const fn is_ready(self) -> bool {
        self.groups > 0 && self.nodes > 0
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProxyReadyResult {
    pub(crate) groups: Vec<ProxyGroupRow>,
    pub(crate) summary: ProxyGroupLoadSummary,
    pub(crate) provider_refresh: ProviderAutoRefreshSummary,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProviderAutoRefreshSummary {
    pub(crate) candidates: usize,
    pub(crate) attempted: usize,
    pub(crate) succeeded: usize,
    pub(crate) failed: usize,
    pub(crate) errors: Vec<String>,
}

impl ProviderAutoRefreshSummary {
    pub(crate) const fn is_empty(&self) -> bool {
        self.attempted == 0
    }

    pub(crate) fn to_message(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut message = format!(
            "Provider 自动更新：候选 {}，尝试 {}，成功 {}，失败 {}",
            self.candidates, self.attempted, self.succeeded, self.failed
        );
        if !self.errors.is_empty() {
            message.push_str("，错误：");
            message.push_str(&self.errors.join("；"));
        }
        Some(message)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DashboardMetrics {
    pub(crate) upload_speed: Option<u64>,
    pub(crate) download_speed: Option<u64>,
    pub(crate) memory: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RuntimeProxySummary {
    pub(crate) proxies: usize,
    pub(crate) providers: usize,
    pub(crate) provider_names: Vec<String>,
    pub(crate) group_provider_names: Vec<String>,
    pub(crate) groups: usize,
    pub(crate) rules: usize,
}
