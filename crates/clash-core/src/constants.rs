pub mod app {
    pub const APP_ID: &str = "io.github.clash-tui";
    pub const SERVICE_NAME: &str = "clash-tui";
}

pub mod env {
    pub const CLASH_TUI_HOME: &str = "CLASH_TUI_HOME";
}

pub mod files {
    pub const CLASH_CONFIG: &str = "config.yaml";
    pub const SETTINGS_CONFIG: &str = "settings.yaml";
    pub const PROFILE_YAML: &str = "profiles.yaml";
    pub const RUNTIME_CONFIG: &str = "mihomo-runtime.yaml";
    pub const CHECK_CONFIG: &str = "mihomo-check.yaml";
    pub const DNS_CONFIG: &str = "dns_config.yaml";
}

pub mod network {
    pub const DEFAULT_EXTERNAL_CONTROLLER_HOST: &str = "127.0.0.1";
    pub const DEFAULT_EXTERNAL_CONTROLLER_PORT: u16 = 9097;
    pub const DEFAULT_EXTERNAL_CONTROLLER: &str = "127.0.0.1:9097";

    pub mod ports {
        pub const DEFAULT_HTTP: u16 = 7899;
        pub const DEFAULT_MIXED: u16 = 7897;
        pub const DEFAULT_SOCKS: u16 = 7898;
        #[cfg(not(target_os = "windows"))]
        pub const DEFAULT_REDIR: u16 = 7895;
        #[cfg(target_os = "linux")]
        pub const DEFAULT_TPROXY: u16 = 7896;
    }
}

pub mod timeouts {
    use std::time::Duration;

    pub const REMOTE_PROFILE_DOWNLOAD: Duration = Duration::from_secs(30);
}

pub mod tun {
    pub const DEFAULT_STACK: &str = "gvisor";
    pub const DNS_HIJACK: &[&str] = &["any:53"];
}
