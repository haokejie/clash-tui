use std::path::Path;

use anyhow::Result;
use serde_yaml_ng::{Mapping, Value};

use crate::yaml;

pub const DNS_CONFIG_HEADER: &str = "# Clash TUI DNS Config";

#[must_use]
pub fn default_dns_config() -> Mapping {
    let mut dns_config = Mapping::new();
    insert_basic_dns_defaults(&mut dns_config);
    insert_fake_ip_defaults(&mut dns_config);
    insert_resolver_defaults(&mut dns_config);
    insert_empty_policy_defaults(&mut dns_config);
    dns_config.insert("fallback-filter".into(), Value::Mapping(fallback_filter()));

    Mapping::from_iter([
        ("dns".into(), Value::Mapping(dns_config)),
        ("hosts".into(), Value::Mapping(Mapping::new())),
    ])
}

pub async fn ensure_dns_config(path: impl AsRef<Path>) -> Result<bool> {
    let path = path.as_ref();
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Ok(false);
    }

    yaml::save_yaml(path, &default_dns_config(), Some(DNS_CONFIG_HEADER)).await?;
    Ok(true)
}

pub fn apply_dns_config_to_runtime(config: &mut Mapping, dns_config: &Mapping) {
    if let Some(hosts_value) = dns_config.get("hosts").filter(|value| value.is_mapping()) {
        config.insert("hosts".into(), hosts_value.clone());
    }

    if let Some(dns_value) = dns_config.get("dns") {
        if let Some(dns_mapping) = dns_value.as_mapping() {
            config.insert("dns".into(), Value::Mapping(dns_mapping.clone()));
        }
    } else {
        config.insert("dns".into(), Value::Mapping(dns_config.clone()));
    }
}

fn string_sequence(items: &[&str]) -> Value {
    Value::Sequence(items.iter().map(|item| Value::String((*item).to_owned())).collect())
}

fn insert_basic_dns_defaults(config: &mut Mapping) {
    set_bool(config, "enable", true);
    set_text(config, "listen", ":53");
    set_text(config, "enhanced-mode", "fake-ip");
    set_bool(config, "prefer-h3", false);
    set_bool(config, "respect-rules", false);
    set_bool(config, "use-hosts", false);
    set_bool(config, "use-system-hosts", false);
}

fn insert_fake_ip_defaults(config: &mut Mapping) {
    set_text(config, "fake-ip-range", "198.18.0.1/16");
    set_text(config, "fake-ip-filter-mode", "blacklist");
    set_list(
        config,
        "fake-ip-filter",
        &[
            "*.lan",
            "*.local",
            "*.arpa",
            "time.*.com",
            "ntp.*.com",
            "time.*.com",
            "+.market.xiaomi.com",
            "localhost.ptlogin2.qq.com",
            "*.msftncsi.com",
            "www.msftconnecttest.com",
        ],
    );
}

fn insert_resolver_defaults(config: &mut Mapping) {
    set_list(
        config,
        "default-nameserver",
        &["system", "223.6.6.6", "8.8.8.8", "2400:3200::1", "2001:4860:4860::8888"],
    );
    set_list(
        config,
        "nameserver",
        &[
            "8.8.8.8",
            "https://doh.pub/dns-query",
            "https://dns.alidns.com/dns-query",
        ],
    );
    set_list(
        config,
        "proxy-server-nameserver",
        &[
            "https://doh.pub/dns-query",
            "https://dns.alidns.com/dns-query",
            "tls://223.5.5.5",
        ],
    );
}

fn insert_empty_policy_defaults(config: &mut Mapping) {
    config.insert("fallback".into(), Value::Sequence(Vec::new()));
    config.insert("direct-nameserver".into(), Value::Sequence(Vec::new()));
    config.insert("nameserver-policy".into(), Value::Mapping(Mapping::new()));
    set_bool(config, "direct-nameserver-follow-policy", false);
}

fn fallback_filter() -> Mapping {
    let mut filter = Mapping::new();
    set_bool(&mut filter, "geoip", true);
    set_text(&mut filter, "geoip-code", "CN");
    set_list(&mut filter, "ipcidr", &["240.0.0.0/4", "0.0.0.0/32"]);
    set_list(
        &mut filter,
        "domain",
        &["+.google.com", "+.facebook.com", "+.youtube.com"],
    );
    filter
}

fn set_bool(config: &mut Mapping, key: &str, value: bool) {
    config.insert(key.into(), Value::Bool(value));
}

fn set_text(config: &mut Mapping, key: &str, value: &str) {
    config.insert(key.into(), Value::String(value.to_owned()));
}

fn set_list(config: &mut Mapping, key: &str, value: &[&str]) {
    config.insert(key.into(), string_sequence(value));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use serde_yaml_ng::{Mapping, Value};

    use super::{apply_dns_config_to_runtime, default_dns_config};

    #[test]
    fn default_dns_config_matches_desktop_shape() {
        let config = default_dns_config();

        assert!(config.get("dns").and_then(Value::as_mapping).is_some());
        assert!(config.get("hosts").and_then(Value::as_mapping).is_some());
        assert_eq!(
            config
                .get("dns")
                .and_then(Value::as_mapping)
                .and_then(|dns| dns.get("listen")),
            Some(&Value::String(":53".into()))
        );
    }

    #[test]
    fn apply_dns_config_uses_wrapped_dns_section() {
        let mut runtime = Mapping::new();
        let mut dns = Mapping::new();
        dns.insert("enable".into(), Value::Bool(true));
        dns.insert(
            "nameserver".into(),
            Value::Sequence(vec![Value::String("1.1.1.1".into())]),
        );

        let mut hosts = Mapping::new();
        hosts.insert("example.test".into(), Value::String("127.0.0.1".into()));

        let mut dns_config = Mapping::new();
        dns_config.insert("dns".into(), Value::Mapping(dns.clone()));
        dns_config.insert("hosts".into(), Value::Mapping(hosts.clone()));

        apply_dns_config_to_runtime(&mut runtime, &dns_config);

        assert_eq!(runtime.get("dns"), Some(&Value::Mapping(dns)));
        assert_eq!(runtime.get("hosts"), Some(&Value::Mapping(hosts)));
    }

    #[test]
    fn apply_dns_config_accepts_unwrapped_dns_section() {
        let mut runtime = Mapping::new();
        let mut dns_config = Mapping::new();
        dns_config.insert("enable".into(), Value::Bool(true));
        dns_config.insert(
            "nameserver".into(),
            Value::Sequence(vec![Value::String("1.0.0.1".into())]),
        );

        apply_dns_config_to_runtime(&mut runtime, &dns_config);

        assert_eq!(runtime.get("dns"), Some(&Value::Mapping(dns_config)));
    }
}
