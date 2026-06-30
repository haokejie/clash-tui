use std::path::Path;

use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::yaml;

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct AppSettings {
    pub enable_tun_mode: Option<bool>,
    pub enable_system_proxy: Option<bool>,
    pub proxy_host: Option<String>,
    pub system_proxy_bypass: Option<String>,
    pub enable_dns_settings: Option<bool>,
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
    pub rule_provider_download_proxy: Option<RuleProviderDownloadProxy>,
    pub tui_display_mode: Option<String>,
    pub tui_punctuation_mode: Option<String>,
    pub tui_theme: Option<String>,
}

#[derive(Default, Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RuleProviderDownloadProxy {
    #[default]
    Inherit,
    Direct,
}

impl RuleProviderDownloadProxy {
    #[must_use]
    pub const fn config_value(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Direct => "direct",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Inherit => Self::Direct,
            Self::Direct => Self::Inherit,
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        value.parse()
    }
}

impl FromStr for RuleProviderDownloadProxy {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "inherit" | "default" | "auto" | "follow" | "runtime" => Ok(Self::Inherit),
            "direct" | "directly" => Ok(Self::Direct),
            _ => bail!("unsupported rule provider download proxy: {value}; expected inherit or direct"),
        }
    }
}

impl AppSettings {
    pub async fn load_or_default(path: impl AsRef<Path>) -> Self {
        yaml::read_yaml(path).await.unwrap_or_default()
    }

    pub async fn save_file(&self, path: impl AsRef<Path>) -> Result<()> {
        yaml::save_yaml(path, self, Some("# Clash TUI Settings")).await
    }

    pub fn patch_config(&mut self, patch: &Self) {
        let (Ok(mut current), Ok(patch)) = (serde_json::to_value(&*self), serde_json::to_value(patch)) else {
            return;
        };

        copy_present_json_fields(&mut current, &patch);
        if let Ok(next) = serde_json::from_value(current) {
            *self = next;
        }
    }
}

fn copy_present_json_fields(current: &mut JsonValue, patch: &JsonValue) {
    let (Some(current), Some(patch)) = (current.as_object_mut(), patch.as_object()) else {
        return;
    };

    for (key, value) in patch {
        if !value.is_null() {
            current.insert(key.clone(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{AppSettings, RuleProviderDownloadProxy};

    #[test]
    fn app_settings_serializes_current_field_names() {
        let value = AppSettings {
            enable_tun_mode: Some(true),
            enable_system_proxy: Some(true),
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
            rule_provider_download_proxy: Some(RuleProviderDownloadProxy::Direct),
            tui_display_mode: Some("punctuation".into()),
            tui_punctuation_mode: Some("colon-comma".into()),
            tui_theme: Some("orange".into()),
            ..AppSettings::default()
        };
        let yaml = serde_yaml_ng::to_string(&value).expect("serialize app_settings");

        assert!(yaml.contains("enable_tun_mode: true"));
        assert!(yaml.contains("enable_system_proxy: true"));
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
        assert!(yaml.contains("rule_provider_download_proxy: direct"));
        assert!(yaml.contains("tui_display_mode: punctuation"));
        assert!(yaml.contains("tui_punctuation_mode: colon-comma"));
        assert!(yaml.contains("tui_theme: orange"));
    }

    #[test]
    fn patch_config_updates_only_present_values() {
        let mut value = AppSettings {
            tui_theme: Some("orange".into()),
            mixed_port: Some(7897),
            enable_tun_mode: Some(false),
            ..AppSettings::default()
        };
        let patch = AppSettings {
            mixed_port: Some(19090),
            enable_tun_mode: Some(true),
            ..AppSettings::default()
        };

        value.patch_config(&patch);

        assert_eq!(value.tui_theme.as_deref(), Some("orange"));
        assert_eq!(value.mixed_port, Some(19090));
        assert_eq!(value.enable_tun_mode, Some(true));
    }

    #[test]
    fn app_settings_ignores_removed_desktop_only_fields() {
        let value: AppSettings = serde_yaml_ng::from_str(
            "enable_tun_mode: true\n\
             enable_auto_launch: true\n\
             home_cards: []\n\
             theme_mode: dark\n\
             subscription_user_agent: clash-verge/v0.2.6\n",
        )
        .expect("deserialize app_settings");
        let yaml = serde_yaml_ng::to_string(&value).expect("serialize app_settings");

        assert_eq!(value.enable_tun_mode, Some(true));
        assert!(!yaml.contains("enable_auto_launch"));
        assert!(!yaml.contains("home_cards"));
        assert!(!yaml.contains("theme_mode"));
        assert!(!yaml.contains("subscription_user_agent"));
    }
}
