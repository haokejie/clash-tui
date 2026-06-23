use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::yaml;

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct IAppSettings {
    pub app_log_level: Option<String>,
    pub app_log_max_size: Option<u64>,
    pub app_log_max_count: Option<usize>,
    pub language: Option<String>,
    pub theme_mode: Option<String>,
    pub start_page: Option<String>,
    pub enable_auto_launch: Option<bool>,
    pub traffic_graph: Option<bool>,
    pub enable_memory_usage: Option<bool>,
    pub enable_tun_mode: Option<bool>,
    pub enable_system_proxy: Option<bool>,
    pub proxy_auto_config: Option<bool>,
    pub pac_file_content: Option<String>,
    pub proxy_host: Option<String>,
    pub enable_proxy_guard: Option<bool>,
    pub enable_bypass_check: Option<bool>,
    pub use_default_bypass: Option<bool>,
    pub proxy_guard_duration: Option<u64>,
    pub system_proxy_bypass: Option<String>,
    pub enable_builtin_enhanced: Option<bool>,
    pub enable_dns_settings: Option<bool>,
    pub enable_auto_backup_schedule: Option<bool>,
    pub auto_backup_interval_hours: Option<u64>,
    pub auto_backup_on_change: Option<bool>,
    pub clash_core: Option<String>,
    pub default_latency_test: Option<String>,
    pub default_latency_timeout: Option<i16>,
    pub mixed_port: Option<u16>,
    pub socks_port: Option<u16>,
    pub socks_enabled: Option<bool>,
    pub http_port: Option<u16>,
    pub http_enabled: Option<bool>,
    pub redir_port: Option<u16>,
    pub redir_enabled: Option<bool>,
    pub tproxy_port: Option<u16>,
    pub tproxy_enabled: Option<bool>,
    pub enable_external_controller: Option<bool>,
    pub external_controller_port: Option<u16>,
    pub enable_core_log: Option<bool>,
    pub tui_display_mode: Option<String>,
    pub tui_punctuation_mode: Option<String>,
    pub tui_theme: Option<String>,
    pub auto_close_connection: Option<bool>,
    pub home_cards: Option<serde_json::Value>,
}

impl IAppSettings {
    pub async fn load_or_default(path: impl AsRef<Path>) -> Self {
        yaml::read_yaml(path).await.unwrap_or_default()
    }

    pub async fn save_file(&self, path: impl AsRef<Path>) -> Result<()> {
        yaml::save_yaml(path, self, Some("# Clash TUI Settings")).await
    }

    #[allow(clippy::cognitive_complexity)]
    pub fn patch_config(&mut self, patch: &Self) {
        macro_rules! patch {
            ($key: tt) => {
                if patch.$key.is_some() {
                    self.$key = patch.$key.clone();
                }
            };
        }

        patch!(app_log_level);
        patch!(app_log_max_size);
        patch!(app_log_max_count);
        patch!(language);
        patch!(theme_mode);
        patch!(start_page);
        patch!(enable_auto_launch);
        patch!(traffic_graph);
        patch!(enable_memory_usage);
        patch!(enable_tun_mode);
        patch!(enable_system_proxy);
        patch!(proxy_auto_config);
        patch!(pac_file_content);
        patch!(proxy_host);
        patch!(enable_proxy_guard);
        patch!(enable_bypass_check);
        patch!(use_default_bypass);
        patch!(proxy_guard_duration);
        patch!(system_proxy_bypass);
        patch!(enable_builtin_enhanced);
        patch!(enable_dns_settings);
        patch!(enable_auto_backup_schedule);
        patch!(auto_backup_interval_hours);
        patch!(auto_backup_on_change);
        patch!(clash_core);
        patch!(default_latency_test);
        patch!(default_latency_timeout);
        patch!(mixed_port);
        patch!(socks_port);
        patch!(socks_enabled);
        patch!(http_port);
        patch!(http_enabled);
        patch!(redir_port);
        patch!(redir_enabled);
        patch!(tproxy_port);
        patch!(tproxy_enabled);
        patch!(enable_external_controller);
        patch!(external_controller_port);
        patch!(enable_core_log);
        patch!(tui_display_mode);
        patch!(tui_punctuation_mode);
        patch!(tui_theme);
        patch!(auto_close_connection);
        patch!(home_cards);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::IAppSettings;

    #[test]
    fn app_settings_serializes_current_field_names() {
        let value = IAppSettings {
            enable_tun_mode: Some(true),
            enable_auto_launch: Some(true),
            enable_system_proxy: Some(true),
            proxy_auto_config: Some(false),
            proxy_host: Some("127.0.0.1".into()),
            system_proxy_bypass: Some("localhost,127.0.0.1".into()),
            enable_dns_settings: Some(false),
            mixed_port: Some(7897),
            redir_port: Some(7895),
            redir_enabled: Some(true),
            tproxy_port: Some(7896),
            tproxy_enabled: Some(false),
            enable_external_controller: Some(false),
            external_controller_port: Some(9097),
            enable_core_log: Some(false),
            tui_display_mode: Some("punctuation".into()),
            tui_punctuation_mode: Some("colon-comma".into()),
            tui_theme: Some("orange".into()),
            enable_auto_backup_schedule: Some(true),
            auto_backup_interval_hours: Some(24),
            auto_backup_on_change: Some(false),
            ..IAppSettings::default()
        };
        let yaml = serde_yaml_ng::to_string(&value).expect("serialize app_settings");

        assert!(yaml.contains("enable_tun_mode: true"));
        assert!(yaml.contains("enable_auto_launch: true"));
        assert!(yaml.contains("enable_system_proxy: true"));
        assert!(yaml.contains("proxy_auto_config: false"));
        assert!(yaml.contains("proxy_host: 127.0.0.1"));
        assert!(yaml.contains("system_proxy_bypass: localhost,127.0.0.1"));
        assert!(yaml.contains("enable_dns_settings: false"));
        assert!(yaml.contains("mixed_port: 7897"));
        assert!(yaml.contains("redir_port: 7895"));
        assert!(yaml.contains("redir_enabled: true"));
        assert!(yaml.contains("tproxy_port: 7896"));
        assert!(yaml.contains("tproxy_enabled: false"));
        assert!(yaml.contains("enable_external_controller: false"));
        assert!(yaml.contains("external_controller_port: 9097"));
        assert!(yaml.contains("enable_core_log: false"));
        assert!(yaml.contains("tui_display_mode: punctuation"));
        assert!(yaml.contains("tui_punctuation_mode: colon-comma"));
        assert!(yaml.contains("tui_theme: orange"));
        assert!(yaml.contains("enable_auto_backup_schedule: true"));
        assert!(yaml.contains("auto_backup_interval_hours: 24"));
        assert!(yaml.contains("auto_backup_on_change: false"));
    }
}
