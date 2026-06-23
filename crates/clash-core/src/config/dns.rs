use std::path::Path;

use anyhow::Result;
use serde_yaml_ng::{Mapping, Value};

use crate::yaml;

pub const DNS_CONFIG_HEADER: &str = "# Clash TUI DNS Config";

#[must_use]
pub fn default_dns_config() -> Mapping {
    let dns_config = Mapping::from_iter([
        ("enable".into(), Value::Bool(true)),
        ("listen".into(), Value::String(":53".into())),
        ("enhanced-mode".into(), Value::String("fake-ip".into())),
        ("fake-ip-range".into(), Value::String("198.18.0.1/16".into())),
        ("fake-ip-filter-mode".into(), Value::String("blacklist".into())),
        ("prefer-h3".into(), Value::Bool(false)),
        ("respect-rules".into(), Value::Bool(false)),
        ("use-hosts".into(), Value::Bool(false)),
        ("use-system-hosts".into(), Value::Bool(false)),
        (
            "fake-ip-filter".into(),
            string_sequence(&[
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
            ]),
        ),
        (
            "default-nameserver".into(),
            string_sequence(&["system", "223.6.6.6", "8.8.8.8", "2400:3200::1", "2001:4860:4860::8888"]),
        ),
        (
            "nameserver".into(),
            string_sequence(&[
                "8.8.8.8",
                "https://doh.pub/dns-query",
                "https://dns.alidns.com/dns-query",
            ]),
        ),
        ("fallback".into(), Value::Sequence(vec![])),
        ("nameserver-policy".into(), Value::Mapping(Mapping::new())),
        (
            "proxy-server-nameserver".into(),
            string_sequence(&[
                "https://doh.pub/dns-query",
                "https://dns.alidns.com/dns-query",
                "tls://223.5.5.5",
            ]),
        ),
        ("direct-nameserver".into(), Value::Sequence(vec![])),
        ("direct-nameserver-follow-policy".into(), Value::Bool(false)),
        (
            "fallback-filter".into(),
            Value::Mapping(Mapping::from_iter([
                ("geoip".into(), Value::Bool(true)),
                ("geoip-code".into(), Value::String("CN".into())),
                ("ipcidr".into(), string_sequence(&["240.0.0.0/4", "0.0.0.0/32"])),
                (
                    "domain".into(),
                    string_sequence(&["+.google.com", "+.facebook.com", "+.youtube.com"]),
                ),
            ])),
        ),
    ]);

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
