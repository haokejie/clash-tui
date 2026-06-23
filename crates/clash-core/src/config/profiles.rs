use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, bail};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use percent_encoding::percent_decode_str;
use reqwest::header::{CONTENT_DISPOSITION, HeaderMap};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};

use crate::{constants::network, yaml};

const DEFAULT_REMOTE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrfItem {
    pub uid: Option<String>,
    #[serde(rename = "type")]
    pub itype: Option<String>,
    pub name: Option<String>,
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<Vec<PrfSelected>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<PrfExtra>,
    pub updated: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option: Option<PrfOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    #[serde(skip)]
    pub file_data: Option<String>,
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PrfSelected {
    pub name: Option<String>,
    pub now: Option<String>,
}

#[derive(Default, Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct PrfExtra {
    pub upload: u64,
    pub download: u64,
    pub total: u64,
    pub expire: u64,
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrfOption {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with_proxy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_proxy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_interval: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub danger_accept_invalid_certs: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_auto_update: Option<bool>,
    pub merge: Option<String>,
    pub script: Option<String>,
    pub rules: Option<String>,
    pub proxies: Option<String>,
    pub groups: Option<String>,
}

impl PrfOption {
    #[must_use]
    pub fn merge(one: Option<&Self>, other: Option<&Self>) -> Option<Self> {
        match (one, other) {
            (Some(base), Some(patch)) => {
                let mut result = base.clone();
                result.user_agent = patch.user_agent.clone().or(result.user_agent);
                result.with_proxy = patch.with_proxy.or(result.with_proxy);
                result.self_proxy = patch.self_proxy.or(result.self_proxy);
                result.danger_accept_invalid_certs =
                    patch.danger_accept_invalid_certs.or(result.danger_accept_invalid_certs);
                result.allow_auto_update = patch.allow_auto_update.or(result.allow_auto_update);
                result.update_interval = patch.update_interval.or(result.update_interval);
                result.timeout_seconds = patch.timeout_seconds.or(result.timeout_seconds);
                result.merge = patch.merge.clone().or(result.merge);
                result.script = patch.script.clone().or(result.script);
                result.rules = patch.rules.clone().or(result.rules);
                result.proxies = patch.proxies.clone().or(result.proxies);
                result.groups = patch.groups.clone().or(result.groups);
                Some(result)
            }
            (Some(base), None) => Some(base.clone()),
            (None, Some(patch)) => Some(patch.clone()),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalProfileDefault {
    pub uid: &'static str,
    pub itype: &'static str,
    pub name: &'static str,
    pub file: &'static str,
    pub file_data: &'static str,
}

pub const GLOBAL_PROFILE_DEFAULTS: [GlobalProfileDefault; 2] = [
    GlobalProfileDefault {
        uid: "Merge",
        itype: "merge",
        name: "Merge",
        file: "Merge.yaml",
        file_data: "{}\n",
    },
    GlobalProfileDefault {
        uid: "Script",
        itype: "script",
        name: "Script",
        file: "Script.js",
        file_data: "function main(config) {\n  return config\n}\n",
    },
];

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct IProfiles {
    pub current: Option<String>,
    pub items: Option<Vec<PrfItem>>,
}

impl Default for IProfiles {
    fn default() -> Self {
        Self {
            current: None,
            items: Some(
                GLOBAL_PROFILE_DEFAULTS
                    .iter()
                    .map(|item| PrfItem {
                        uid: Some(item.uid.to_owned()),
                        itype: Some(item.itype.to_owned()),
                        name: Some(item.name.to_owned()),
                        file: Some(item.file.to_owned()),
                        ..PrfItem::default()
                    })
                    .collect(),
            ),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalProfileImport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub file_data: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteProfileImport {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option: Option<PrfOption>,
}

#[derive(Debug, Clone)]
pub struct RemoteProfileDownload {
    pub url: String,
    pub file_data: String,
    pub name: String,
    pub extra: Option<PrfExtra>,
    pub update_interval: Option<u64>,
    pub home: Option<String>,
}

impl IProfiles {
    pub fn ensure_global_profile_items(&mut self) -> bool {
        let items = self.items.get_or_insert_with(Vec::new);
        let mut changed = false;

        for default in GLOBAL_PROFILE_DEFAULTS {
            let item = items.iter_mut().find(|item| item.uid.as_deref() == Some(default.uid));

            if let Some(item) = item {
                if item.itype.as_deref() != Some(default.itype) {
                    item.itype = Some(default.itype.to_owned());
                    changed = true;
                }
                if item.name.is_none() {
                    item.name = Some(default.name.to_owned());
                    changed = true;
                }
                if item.file.is_none() {
                    item.file = Some(default.file.to_owned());
                    changed = true;
                }
            } else {
                items.push(PrfItem {
                    uid: Some(default.uid.to_owned()),
                    itype: Some(default.itype.to_owned()),
                    name: Some(default.name.to_owned()),
                    file: Some(default.file.to_owned()),
                    ..PrfItem::default()
                });
                changed = true;
            }
        }

        changed
    }

    pub async fn load_or_default(path: impl AsRef<Path>) -> Self {
        yaml::read_yaml(path).await.unwrap_or_default()
    }

    pub async fn save_file(&self, path: impl AsRef<Path>) -> Result<()> {
        yaml::save_yaml(path, self, Some("# Profiles Config for Clash TUI")).await
    }

    pub fn patch_config(&mut self, patch: &Self) {
        if self.items.is_none() {
            self.items = Some(Vec::new());
        }

        if let Some(current) = &patch.current
            && self
                .items
                .as_ref()
                .is_some_and(|items| items.iter().any(|item| item.uid.as_ref() == Some(current)))
        {
            self.current = Some(current.clone());
        }
    }

    #[must_use]
    pub const fn get_current(&self) -> Option<&String> {
        self.current.as_ref()
    }

    #[must_use]
    pub const fn get_items(&self) -> Option<&Vec<PrfItem>> {
        self.items.as_ref()
    }

    pub fn get_item(&self, uid: impl AsRef<str>) -> Result<&PrfItem> {
        let uid = uid.as_ref();
        if let Some(item) = self
            .items
            .as_deref()
            .and_then(|items| items.iter().find(|item| item.uid.as_deref() == Some(uid)))
        {
            return Ok(item);
        }

        bail!("failed to get the profile item \"uid:{uid}\"")
    }

    pub fn switch_current(&mut self, uid: impl AsRef<str>) -> Result<()> {
        let uid = uid.as_ref();
        let _ = self.get_item(uid)?;
        self.current = Some(uid.to_owned());
        Ok(())
    }

    pub fn append_metadata(&mut self, item: PrfItem) -> Result<()> {
        let Some(uid) = item.uid.as_deref() else {
            bail!("the uid should not be null");
        };

        if self
            .items
            .as_deref()
            .is_some_and(|items| items.iter().any(|item| item.uid.as_deref() == Some(uid)))
        {
            bail!("the profile item \"uid:{uid}\" already exists");
        }

        let uid = item.uid.clone();
        if self.current.is_none() && matches!(item.itype.as_deref(), Some("remote" | "local")) {
            self.current = uid;
        }

        self.items.get_or_insert_with(Vec::new).push(item);
        Ok(())
    }

    pub fn patch_item(&mut self, uid: &str, patch: &PrfItem) -> Result<()> {
        let items = self.items.get_or_insert_with(Vec::new);
        let Some(item) = items.iter_mut().find(|item| item.uid.as_deref() == Some(uid)) else {
            bail!("failed to find the profile item \"uid:{uid}\"");
        };

        if patch.itype.is_some() {
            item.itype = patch.itype.clone();
        }
        if patch.name.is_some() {
            item.name = patch.name.clone();
        }
        if patch.file.is_some() {
            item.file = patch.file.clone();
        }
        if patch.desc.is_some() {
            item.desc = patch.desc.clone();
        }
        if patch.url.is_some() {
            item.url = patch.url.clone();
        }
        if patch.selected.is_some() {
            item.selected = patch.selected.clone();
        }
        if patch.extra.is_some() {
            item.extra = patch.extra;
        }
        if patch.updated.is_some() {
            item.updated = patch.updated;
        }
        if patch.option.is_some() {
            item.option = PrfOption::merge(item.option.as_ref(), patch.option.as_ref());
        }
        if patch.home.is_some() {
            item.home = patch.home.clone();
        }

        Ok(())
    }
}

#[must_use]
pub fn generate_local_uid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("L{nanos}")
}

#[must_use]
pub fn generate_remote_uid() -> String {
    generate_local_uid().replacen('L', "R", 1)
}

pub fn validate_profile_uid(uid: &str) -> Result<()> {
    if uid.is_empty() || uid.len() > 64 {
        bail!("profile uid must be 1..=64 characters");
    }

    if !uid
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        bail!("profile uid can only contain ascii letters, digits, '-' or '_'");
    }

    Ok(())
}

pub fn validate_profile_yaml(file_data: &str) -> Result<()> {
    let mut value: Value = serde_yaml_ng::from_str(file_data).context("failed to parse profile yaml")?;
    value.apply_merge().context("failed to apply profile yaml merge")?;
    if !value.is_mapping() {
        bail!("profile yaml root must be a mapping");
    }
    Ok(())
}

pub fn validate_remote_profile_yaml(file_data: &str) -> Result<()> {
    let data = file_data.trim_start_matches('\u{feff}');
    let mut value: Value = serde_yaml_ng::from_str(data).context("the remote profile data is invalid yaml")?;
    value
        .apply_merge()
        .context("failed to apply remote profile yaml merge")?;
    let mapping = value
        .as_mapping()
        .context("remote profile yaml root must be a mapping")?;
    if !mapping.contains_key("proxies") && !mapping.contains_key("proxy-providers") {
        bail!("profile does not contain `proxies` or `proxy-providers`");
    }
    if !remote_profile_has_proxy_sources(mapping) {
        bail!("profile does not contain any proxy or proxy-provider entries");
    }
    Ok(())
}

fn remote_profile_has_proxy_sources(mapping: &Mapping) -> bool {
    mapping
        .get("proxies")
        .and_then(Value::as_sequence)
        .is_some_and(|proxies| !proxies.is_empty())
        || mapping
            .get("proxy-providers")
            .and_then(Value::as_mapping)
            .is_some_and(|providers| !providers.is_empty())
}

fn normalize_remote_profile_data(file_data: &str) -> Result<String> {
    let data = file_data.trim_start_matches('\u{feff}');
    match validate_remote_profile_yaml(data) {
        Ok(()) => Ok(data.to_owned()),
        Err(yaml_error) => match convert_node_subscription_to_profile(data) {
            Ok(Some(converted)) => Ok(converted),
            Ok(None) => Err(yaml_error),
            Err(err) => {
                let message = format!("failed to convert node subscription to clash profile: {err}");
                Err(err.context(message))
            }
        },
    }
}

pub async fn download_remote_profile(input: &RemoteProfileImport) -> Result<RemoteProfileDownload> {
    let url = normalize_remote_url(&input.url)?;
    let option = input.option.as_ref();
    let timeout = option
        .and_then(|option| option.timeout_seconds)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_REMOTE_TIMEOUT);

    let mut builder = reqwest::Client::builder().timeout(timeout);
    if option.is_some_and(|option| option.danger_accept_invalid_certs.unwrap_or(false)) {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if option.is_some_and(|option| option.self_proxy.unwrap_or(false)) {
        let proxy = format!("http://127.0.0.1:{}", network::ports::DEFAULT_MIXED);
        builder = builder.proxy(
            reqwest::Proxy::all(&proxy)
                .with_context(|| format!("failed to configure local Clash proxy for remote profile: {proxy}"))?,
        );
    } else if !option.is_some_and(|option| option.with_proxy.unwrap_or(false)) {
        builder = builder.no_proxy();
    }
    let client = builder.build().context("failed to build remote profile HTTP client")?;

    let user_agent = option
        .and_then(|option| option.user_agent.as_deref())
        .filter(|user_agent| !user_agent.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(default_subscription_user_agent);
    let request = client.get(url.clone()).header(reqwest::header::USER_AGENT, user_agent);

    let response = request
        .send()
        .await
        .with_context(|| format!("failed to fetch remote profile: {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("failed to fetch remote profile with status {status}");
    }

    let headers = response.headers().clone();
    let file_data = response
        .text()
        .await
        .context("failed to read remote profile response")?;
    let file_data = normalize_remote_profile_data(&file_data)?;

    Ok(RemoteProfileDownload {
        name: input
            .name
            .clone()
            .or_else(|| content_disposition_filename(&headers))
            .or_else(|| url_last_segment(&url))
            .unwrap_or_else(|| "Remote File".into()),
        url,
        file_data,
        extra: parse_subscription_userinfo(&headers),
        update_interval: input
            .option
            .as_ref()
            .and_then(|option| option.update_interval)
            .or_else(|| parse_update_interval(&headers)),
        home: header_to_string(&headers, "profile-web-page-url"),
    })
}

#[must_use]
pub fn remote_profile_name_override_for_update(current_name: Option<&str>, url: &str) -> Option<String> {
    let current_name = current_name?.trim();
    if current_name.is_empty() || current_name == "Remote File" {
        return None;
    }

    if url_last_segment(url)
        .as_deref()
        .is_some_and(|fallback_name| fallback_name == current_name)
    {
        return None;
    }

    Some(current_name.to_owned())
}

#[derive(Debug)]
struct NodeSubscriptionLines {
    supported: Vec<String>,
    unsupported_schemes: BTreeSet<String>,
    had_uri: bool,
}

fn convert_node_subscription_to_profile(file_data: &str) -> Result<Option<String>> {
    let collected = collect_node_subscription_lines(file_data);
    let collected = if collected.had_uri {
        collected
    } else {
        let Some(decoded) = decode_base64_text(file_data) else {
            return Ok(None);
        };
        collect_node_subscription_lines(&decoded)
    };

    if collected.supported.is_empty() && !collected.unsupported_schemes.is_empty() {
        bail!(
            "unsupported node subscription schemes: {}",
            collected
                .unsupported_schemes
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    if collected.supported.is_empty() {
        return Ok(None);
    }

    let mut proxies = Vec::with_capacity(collected.supported.len());
    let mut names = Vec::with_capacity(collected.supported.len());
    let mut used_names = HashMap::new();
    let mut parse_errors = Vec::new();

    for line in collected.supported {
        let mut proxy = match parse_node_uri(&line) {
            Ok(proxy) => proxy,
            Err(err) => {
                parse_errors.push(format!(
                    "{}: {err}",
                    node_uri_scheme(&line).unwrap_or_else(|| "unknown".to_owned())
                ));
                continue;
            }
        };
        let original_name = mapping_string(&proxy, "name").context("converted proxy is missing name")?;
        let name = unique_proxy_name(original_name, &mut used_names);
        proxy.insert(yaml_key("name"), Value::String(name.clone()));
        names.push(name);
        proxies.push(Value::Mapping(proxy));
    }

    if proxies.is_empty() && !parse_errors.is_empty() {
        bail!(
            "node subscription contains URI lines but none could be converted: {}",
            parse_errors.join("; ")
        );
    }

    let yaml = build_node_subscription_profile(proxies, &names)?;
    validate_remote_profile_yaml(&yaml)?;
    Ok(Some(yaml))
}

fn collect_node_subscription_lines(file_data: &str) -> NodeSubscriptionLines {
    let mut supported = Vec::new();
    let mut unsupported_schemes = BTreeSet::new();
    let mut had_uri = false;

    for line in file_data.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some(scheme) = node_uri_scheme(line) else {
            continue;
        };
        had_uri = true;
        match scheme.as_str() {
            "vmess" | "ss" | "ssr" | "trojan" | "vless" | "hysteria2" | "hy2" | "hysteria" | "hy" | "tuic"
            | "anytls" | "http" | "https" | "socks" | "socks5" | "wireguard" | "wg" => supported.push(line.to_owned()),
            _ => {
                unsupported_schemes.insert(scheme);
            }
        }
    }

    NodeSubscriptionLines {
        supported,
        unsupported_schemes,
        had_uri,
    }
}

fn decode_base64_text(file_data: &str) -> Option<String> {
    let normalized = file_data.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    if normalized.is_empty() {
        return None;
    }

    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(bytes) = engine.decode(&normalized)
            && let Ok(decoded) = String::from_utf8(bytes)
        {
            return Some(decoded);
        }
    }

    None
}

fn decode_base64_fragment(value: &str) -> Option<String> {
    let decoded = percent_decode_str(value.trim()).decode_utf8_lossy();
    let normalized = decoded.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    if normalized.is_empty() {
        return None;
    }

    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(bytes) = engine.decode(&normalized)
            && let Ok(decoded) = String::from_utf8(bytes)
        {
            return Some(decoded);
        }
    }

    None
}

fn node_uri_scheme(line: &str) -> Option<String> {
    let (scheme, _) = line.split_once("://")?;
    let scheme = scheme.trim().to_ascii_lowercase();
    (!scheme.is_empty()
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.')))
    .then_some(scheme)
}

fn parse_node_uri(line: &str) -> Result<Mapping> {
    match node_uri_scheme(line).as_deref() {
        Some("vmess") => parse_vmess_uri(line),
        Some("ss") => parse_shadowsocks_uri(line),
        Some("ssr") => parse_shadowsocksr_uri(line),
        Some("trojan") => parse_trojan_uri(line),
        Some("vless") => parse_vless_uri(line),
        Some("hysteria2" | "hy2") => parse_hysteria2_uri(line),
        Some("hysteria" | "hy") => parse_hysteria_uri(line),
        Some("tuic") => parse_tuic_uri(line),
        Some("anytls") => parse_anytls_uri(line),
        Some("http" | "https") => parse_http_proxy_uri(line),
        Some("socks" | "socks5") => parse_socks_proxy_uri(line),
        Some("wireguard" | "wg") => parse_wireguard_uri(line),
        Some(scheme) => bail!("unsupported node uri scheme: {scheme}"),
        None => bail!("invalid node uri"),
    }
}

fn parse_vmess_uri(line: &str) -> Result<Mapping> {
    let payload = line
        .strip_prefix("vmess://")
        .context("invalid vmess uri: missing payload")?;
    let decoded = decode_base64_fragment(payload).context("invalid vmess uri: payload is not valid base64")?;
    let value: serde_json::Value = serde_json::from_str(&decoded).context("invalid vmess uri: payload is not json")?;
    let object = value
        .as_object()
        .context("invalid vmess uri: payload root is not object")?;
    let server = json_string(object, &["add", "server"]).context("invalid vmess uri: missing server")?;
    let port = json_u16(object, &["port"]).context("invalid vmess uri: missing port")?;
    let uuid = json_string(object, &["id", "uuid"]).context("invalid vmess uri: missing uuid")?;
    let name = json_string(object, &["ps", "name", "remarks"]).unwrap_or_else(|| format!("VMess {server}:{port}"));
    let network = if json_string(object, &["type"]).is_some_and(|header_type| header_type == "http") {
        "http".into()
    } else {
        json_string(object, &["net", "network"]).unwrap_or_else(|| "tcp".into())
    };
    let tls = json_string(object, &["tls"]).is_some_and(|tls| !tls.is_empty() && !tls.eq_ignore_ascii_case("none"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "vmess");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_string(&mut proxy, "uuid", uuid);
    insert_number(
        &mut proxy,
        "alterId",
        json_u64(object, &["aid", "alterId"]).unwrap_or(0),
    );
    insert_string(
        &mut proxy,
        "cipher",
        json_string(object, &["scy", "cipher"]).unwrap_or_else(|| "auto".into()),
    );
    if tls {
        insert_bool(&mut proxy, "tls", true);
    }
    insert_optional_string(
        &mut proxy,
        "servername",
        json_string(object, &["sni", "servername", "host"]).as_deref(),
    );
    insert_optional_string(
        &mut proxy,
        "client-fingerprint",
        json_string(object, &["fp", "fingerprint"]).as_deref(),
    );
    insert_string_array(
        &mut proxy,
        "alpn",
        split_param(json_string(object, &["alpn"]).as_deref()),
    );
    if let Some(skip) = json_bool_like(object, &["allowInsecure", "skip-cert-verify"]) {
        insert_bool(&mut proxy, "skip-cert-verify", skip);
    }

    let params = vmess_transport_params(object);
    let (network, httpupgrade) = normalize_transport_network(&network);
    insert_string(&mut proxy, "network", &network);
    insert_vless_transport_opts(&mut proxy, &network, &params, httpupgrade);
    fill_vless_servername_from_transport(&mut proxy, tls);

    Ok(proxy)
}

fn parse_shadowsocks_uri(line: &str) -> Result<Mapping> {
    let raw = line
        .strip_prefix("ss://")
        .context("invalid shadowsocks uri: missing payload")?;
    let (body, fragment) = raw
        .split_once('#')
        .map_or((raw, None), |(body, fragment)| (body, Some(fragment)));
    let (body, query) = body
        .split_once('?')
        .map_or((body, None), |(body, query)| (body, Some(query)));
    let body = if body.contains('@') {
        body.to_owned()
    } else {
        decode_base64_fragment(body).context("invalid shadowsocks uri: payload is not valid base64")?
    };
    let (userinfo, endpoint) = body
        .rsplit_once('@')
        .context("invalid shadowsocks uri: missing user info or endpoint")?;
    let (cipher, password) = if let Some((cipher, password)) = userinfo.split_once(':') {
        (cipher.to_owned(), password.to_owned())
    } else {
        let decoded =
            decode_base64_fragment(userinfo).context("invalid shadowsocks uri: user info is not valid base64")?;
        decoded
            .split_once(':')
            .map(|(cipher, password)| (cipher.to_owned(), password.to_owned()))
            .context("invalid shadowsocks uri: user info must be method:password")?
    };
    let (server, port) = split_host_port(endpoint).context("invalid shadowsocks uri: missing server or port")?;
    let name = fragment
        .and_then(decode_component)
        .unwrap_or_else(|| format!("SS {server}:{port}"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "ss");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_string(
        &mut proxy,
        "cipher",
        decode_component(&cipher).unwrap_or_else(|| cipher.trim().to_owned()),
    );
    insert_string(
        &mut proxy,
        "password",
        decode_component(&password).unwrap_or_else(|| password.trim().to_owned()),
    );
    if let Some(plugin) = query.and_then(|query| {
        parse_query_params(Some(query))
            .get("plugin")
            .filter(|plugin| !plugin.trim().is_empty())
            .cloned()
    }) {
        insert_shadowsocks_plugin(&mut proxy, &plugin);
    }

    Ok(proxy)
}

fn parse_shadowsocksr_uri(line: &str) -> Result<Mapping> {
    let payload = line
        .strip_prefix("ssr://")
        .context("invalid ssr uri: missing payload")?;
    let decoded = decode_base64_fragment(payload).context("invalid ssr uri: payload is not valid base64")?;
    let (main, query) = decoded
        .split_once("/?")
        .map_or((decoded.as_str(), None), |(main, query)| (main, Some(query)));
    let mut parts = main.rsplitn(5, ':');
    let password = parts.next().context("invalid ssr uri: missing password")?;
    let obfs = parts.next().context("invalid ssr uri: missing obfs")?;
    let cipher = parts.next().context("invalid ssr uri: missing cipher")?;
    let protocol = parts.next().context("invalid ssr uri: missing protocol")?;
    let server_port = parts.next().context("invalid ssr uri: missing server or port")?;
    let (server, port) = server_port
        .rsplit_once(':')
        .context("invalid ssr uri: missing server or port")?;
    let server = clean_ssr_server(server).context("invalid ssr uri: missing server")?;
    let port = port.trim().parse::<u16>().context("invalid ssr uri: invalid port")?;
    let params = parse_query_params(query);
    let password = decode_base64_or_original_fragment(password);
    let name = first_param(&params, &["remarks"])
        .map(decode_base64_or_original_fragment)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| server.clone());

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "ssr");
    insert_string(&mut proxy, "name", name.trim());
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_string(&mut proxy, "protocol", protocol.trim());
    insert_string(&mut proxy, "cipher", normalize_cipher_name(cipher));
    insert_string(&mut proxy, "obfs", obfs.trim());
    insert_string(&mut proxy, "password", password);
    if let Some(value) = first_param(&params, &["protoparam", "protocol-param", "protocolparam"])
        .map(decode_base64_or_original_fragment)
        .map(|value| remove_whitespace(&value))
        .filter(|value| !value.is_empty())
    {
        insert_string(&mut proxy, "protocol-param", value);
    }
    if let Some(value) = first_param(&params, &["obfsparam", "obfs-param"])
        .map(decode_base64_or_original_fragment)
        .map(|value| remove_whitespace(&value))
        .filter(|value| !value.is_empty())
    {
        insert_string(&mut proxy, "obfs-param", value);
    }

    Ok(proxy)
}

fn parse_trojan_uri(line: &str) -> Result<Mapping> {
    let url = reqwest::Url::parse(line).context("invalid trojan uri")?;
    let password = decode_component(url.username()).context("invalid trojan uri: missing password")?;
    let server = url
        .host_str()
        .filter(|server| !server.is_empty())
        .context("invalid trojan uri: missing server")?;
    let port = url.port().unwrap_or(443);
    let params = parse_query_params(url.query());
    let name =
        decode_component(url.fragment().unwrap_or_default()).unwrap_or_else(|| format!("Trojan {server}:{port}"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "trojan");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_string(&mut proxy, "password", password);
    insert_optional_string(&mut proxy, "sni", first_param(&params, &["sni", "peer"]));
    if params.contains_key("allowInsecure") || params.contains_key("insecure") {
        insert_bool(
            &mut proxy,
            "skip-cert-verify",
            parse_bool_or_presence(
                params
                    .get("allowInsecure")
                    .or_else(|| params.get("insecure"))
                    .map(String::as_str),
            ),
        );
    }
    insert_optional_string(&mut proxy, "client-fingerprint", param(&params, "fp"));
    insert_string_array(&mut proxy, "alpn", split_param(param(&params, "alpn")));
    let (network, httpupgrade) = normalize_transport_network(param(&params, "type").unwrap_or("tcp"));
    insert_string(&mut proxy, "network", &network);
    insert_vless_transport_opts(&mut proxy, &network, &params, httpupgrade);

    Ok(proxy)
}

fn json_string(object: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        let Some(value) = object.get(*key) else {
            continue;
        };
        match value {
            serde_json::Value::String(value) => {
                if let Some(value) = decode_component(value) {
                    return Some(value);
                }
            }
            serde_json::Value::Number(value) => return Some(value.to_string()),
            _ => {}
        }
    }
    None
}

fn json_u64(object: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        let value = object.get(*key)?;
        match value {
            serde_json::Value::Number(value) => value.as_u64(),
            serde_json::Value::String(value) => value.trim().parse().ok(),
            _ => None,
        }
    })
}

fn json_u16(object: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<u16> {
    json_u64(object, keys).and_then(|value| u16::try_from(value).ok())
}

fn json_bool_like(object: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        let value = object.get(*key)?;
        match value {
            serde_json::Value::Bool(value) => Some(*value),
            serde_json::Value::Number(value) => Some(value.as_u64().is_some_and(|value| value > 0)),
            serde_json::Value::String(value) => Some(parse_bool_or_presence(Some(value.as_str()))),
            _ => None,
        }
    })
}

fn vmess_transport_params(object: &serde_json::Map<String, serde_json::Value>) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for (source, target) in [
        ("host", "host"),
        ("path", "path"),
        ("serviceName", "path"),
        ("sni", "sni"),
        ("type", "headerType"),
    ] {
        if let Some(value) = json_string(object, &[source]) {
            params.insert(target.to_owned(), value);
        }
    }
    params
}

fn split_host_port(endpoint: &str) -> Option<(String, u16)> {
    let url = reqwest::Url::parse(&format!("ss://{endpoint}")).ok()?;
    let server = url.host_str()?.trim();
    let port = url.port()?;
    (!server.is_empty()).then(|| (server.to_owned(), port))
}

fn normalize_transport_network(value: &str) -> (String, bool) {
    match value.trim().to_ascii_lowercase().as_str() {
        "websocket" | "ws" => ("ws".into(), false),
        "httpupgrade" => ("ws".into(), true),
        "grpc" => ("grpc".into(), false),
        "h2" => ("h2".into(), false),
        "http" => ("http".into(), false),
        _ => ("tcp".into(), false),
    }
}

fn insert_shadowsocks_plugin(proxy: &mut Mapping, plugin: &str) {
    let mut parts = plugin.split(';').map(str::trim).filter(|part| !part.is_empty());
    let Some(name) = parts.next() else {
        return;
    };
    insert_string(proxy, "plugin", name);
    let mut opts = Mapping::new();
    for part in parts {
        if let Some((key, value)) = part.split_once('=') {
            insert_string(
                &mut opts,
                &key.replace('_', "-"),
                decode_component(value).unwrap_or_else(|| value.to_owned()),
            );
        } else {
            insert_bool(&mut opts, &part.replace('_', "-"), true);
        }
    }
    if !opts.is_empty() {
        proxy.insert(yaml_key("plugin-opts"), Value::Mapping(opts));
    }
}

fn parse_vless_uri(line: &str) -> Result<Mapping> {
    let url = reqwest::Url::parse(line).context("invalid vless uri")?;
    let uuid = decode_component(url.username()).context("invalid vless uri: missing uuid")?;
    let server = url
        .host_str()
        .filter(|server| !server.is_empty())
        .context("invalid vless uri: missing server")?;
    let port = url.port().context("invalid vless uri: missing port")?;
    let params = parse_query_params(url.query());
    let name = decode_component(url.fragment().unwrap_or_default())
        .or_else(|| param(&params, "remarks").map(ToOwned::to_owned))
        .or_else(|| param(&params, "remark").map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("VLESS {server}:{port}"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "vless");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_string(&mut proxy, "uuid", uuid);

    let security = param(&params, "security");
    let tls = security.is_some_and(|security| !security.eq_ignore_ascii_case("none"));
    if tls {
        insert_bool(&mut proxy, "tls", true);
    }
    insert_optional_string(&mut proxy, "servername", first_param(&params, &["sni", "peer"]));
    insert_optional_string(&mut proxy, "flow", vless_flow(param(&params, "flow")));
    insert_optional_string(&mut proxy, "client-fingerprint", param(&params, "fp"));
    insert_string_array(&mut proxy, "alpn", split_param(param(&params, "alpn")));
    if params.contains_key("allowInsecure") {
        insert_bool(
            &mut proxy,
            "skip-cert-verify",
            parse_bool_or_presence(params.get("allowInsecure").map(String::as_str)),
        );
    }

    if security.is_some_and(|security| security.eq_ignore_ascii_case("reality")) {
        insert_reality_opts(&mut proxy, &params);
    }

    let (network, httpupgrade) = vless_network(&params);
    insert_string(&mut proxy, "network", &network);
    insert_vless_transport_opts(&mut proxy, &network, &params, httpupgrade);
    fill_vless_servername_from_transport(&mut proxy, tls);

    Ok(proxy)
}

fn parse_hysteria2_uri(line: &str) -> Result<Mapping> {
    let url = reqwest::Url::parse(line).context("invalid hysteria2 uri")?;
    let password = decode_component(url.username()).context("invalid hysteria2 uri: missing password")?;
    let server = url
        .host_str()
        .filter(|server| !server.is_empty())
        .context("invalid hysteria2 uri: missing server")?;
    let port = url.port().unwrap_or(443);
    let params = parse_query_params(url.query());
    let name =
        decode_component(url.fragment().unwrap_or_default()).unwrap_or_else(|| format!("Hysteria2 {server}:{port}"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "hysteria2");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_string(&mut proxy, "password", password);
    insert_optional_string(&mut proxy, "sni", first_param(&params, &["sni", "peer"]));
    if let Some(obfs) = param(&params, "obfs").filter(|obfs| !obfs.eq_ignore_ascii_case("none")) {
        insert_string(&mut proxy, "obfs", obfs);
    }
    insert_optional_string(&mut proxy, "ports", param(&params, "mport"));
    insert_optional_string(&mut proxy, "obfs-password", param(&params, "obfs-password"));
    if params.contains_key("insecure") {
        insert_bool(
            &mut proxy,
            "skip-cert-verify",
            parse_bool_or_presence(params.get("insecure").map(String::as_str)),
        );
    }
    if params.contains_key("fastopen") {
        insert_bool(
            &mut proxy,
            "tfo",
            parse_bool_or_presence(params.get("fastopen").map(String::as_str)),
        );
    }
    insert_optional_string(&mut proxy, "fingerprint", param(&params, "pinSHA256"));

    Ok(proxy)
}

fn parse_hysteria_uri(line: &str) -> Result<Mapping> {
    let parts = parse_url_like_uri(line, &["hysteria", "hy"], "invalid hysteria uri")?;
    let server = parts.host.context("invalid hysteria uri: missing server")?;
    let port = parse_port_or_default(parts.port, 443);
    let params = parse_query_params(parts.query);
    let name = parts
        .fragment
        .and_then(decode_component)
        .unwrap_or_else(|| format!("Hysteria {server}:{port}"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "hysteria");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_string_array(&mut proxy, "alpn", split_param(param(&params, "alpn")));
    insert_optional_string(&mut proxy, "auth-str", param(&params, "auth"));
    insert_optional_string(&mut proxy, "ports", param(&params, "mport"));
    insert_optional_string(
        &mut proxy,
        "obfs",
        first_param(&params, &["obfsParam", "obfs-param", "obfs"]),
    );
    insert_optional_string(&mut proxy, "up", first_param(&params, &["upmbps", "up"]));
    insert_optional_string(&mut proxy, "down", first_param(&params, &["downmbps", "down"]));
    insert_optional_string(&mut proxy, "sni", first_param(&params, &["sni", "peer"]));
    insert_optional_string(&mut proxy, "ca", param(&params, "ca"));
    insert_optional_string(&mut proxy, "ca-str", param(&params, "ca-str"));
    insert_optional_string(&mut proxy, "fingerprint", param(&params, "fingerprint"));
    insert_string(&mut proxy, "protocol", param(&params, "protocol").unwrap_or("udp"));
    insert_bool_param(
        &mut proxy,
        "skip-cert-verify",
        &params,
        &["insecure", "skip-cert-verify"],
    );
    insert_bool_param(&mut proxy, "fast-open", &params, &["fast-open"]);
    insert_bool_param(&mut proxy, "disable-mtu-discovery", &params, &["disable-mtu-discovery"]);
    insert_optional_number_param(&mut proxy, "recv-window-conn", &params, &["recv-window-conn"]);
    insert_optional_number_param(&mut proxy, "recv-window", &params, &["recv-window"]);

    Ok(proxy)
}

fn parse_tuic_uri(line: &str) -> Result<Mapping> {
    let url = reqwest::Url::parse(line).context("invalid tuic uri")?;
    let server = url
        .host_str()
        .filter(|server| !server.is_empty())
        .context("invalid tuic uri: missing server")?;
    let port = url.port().context("invalid tuic uri: missing port")?;
    let params = parse_query_params(url.query());
    let username = decode_component(url.username());
    let password = url.password().and_then(decode_component);
    let name = decode_component(url.fragment().unwrap_or_default()).unwrap_or_else(|| format!("TUIC {server}:{port}"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "tuic");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);

    if let Some(token) = first_param(&params, &["token"]) {
        insert_string(&mut proxy, "token", token);
    } else if let (Some(uuid), Some(password)) = (
        first_param(&params, &["uuid"])
            .map(ToOwned::to_owned)
            .or_else(|| username.clone()),
        first_param(&params, &["password"])
            .map(ToOwned::to_owned)
            .or_else(|| password.clone()),
    ) {
        insert_string(&mut proxy, "uuid", uuid);
        insert_string(&mut proxy, "password", password);
    } else if let Some(token) = username {
        insert_string(&mut proxy, "token", token);
    } else {
        bail!("invalid tuic uri: missing token or uuid/password");
    }

    insert_optional_string(&mut proxy, "sni", first_param(&params, &["sni", "peer"]));
    insert_string_array(&mut proxy, "alpn", split_param(param(&params, "alpn")));
    insert_optional_string(&mut proxy, "udp-relay-mode", param(&params, "udp-relay-mode"));
    insert_optional_string(
        &mut proxy,
        "congestion-controller",
        param(&params, "congestion-controller"),
    );
    insert_optional_string(&mut proxy, "ip", param(&params, "ip"));
    insert_optional_number_param(&mut proxy, "heartbeat-interval", &params, &["heartbeat-interval"]);
    insert_optional_number_param(&mut proxy, "request-timeout", &params, &["request-timeout"]);
    insert_bool_param(&mut proxy, "disable-sni", &params, &["disable-sni"]);
    insert_bool_param(&mut proxy, "reduce-rtt", &params, &["reduce-rtt"]);
    insert_skip_cert_verify(&mut proxy, &params, &["allowInsecure", "insecure", "skip-cert-verify"]);

    Ok(proxy)
}

fn parse_http_proxy_uri(line: &str) -> Result<Mapping> {
    let scheme = node_uri_scheme(line).context("invalid http uri: missing scheme")?;
    let parts = parse_url_like_uri(line, &["http", "https"], "invalid http uri")?;
    let server = parts.host.context("invalid http uri: missing server")?;
    let port = parse_port_or_default(parts.port, 443);
    let params = parse_query_params(parts.query);
    let name = parts
        .fragment
        .and_then(decode_component)
        .unwrap_or_else(|| format!("HTTP {server}:{port}"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "http");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_auth_fields(&mut proxy, parts.auth);
    if scheme == "https" || params.contains_key("tls") {
        insert_bool(
            &mut proxy,
            "tls",
            scheme == "https" || parse_bool_or_presence(params.get("tls").map(String::as_str)),
        );
    }
    insert_optional_string(&mut proxy, "fingerprint", param(&params, "fingerprint"));
    insert_optional_string(&mut proxy, "ip-version", parse_ip_version(param(&params, "ip-version")));
    insert_skip_cert_verify(&mut proxy, &params, &["skip-cert-verify"]);

    Ok(proxy)
}

fn parse_socks_proxy_uri(line: &str) -> Result<Mapping> {
    let parts = parse_url_like_uri(line, &["socks", "socks5"], "invalid socks uri")?;
    let server = parts.host.context("invalid socks uri: missing server")?;
    let port = parse_port_or_default(parts.port, 443);
    let params = parse_query_params(parts.query);
    let name = parts
        .fragment
        .and_then(decode_component)
        .unwrap_or_else(|| format!("SOCKS5 {server}:{port}"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "socks5");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_auth_fields(&mut proxy, parts.auth);
    insert_bool_param(&mut proxy, "tls", &params, &["tls"]);
    insert_optional_string(&mut proxy, "fingerprint", param(&params, "fingerprint"));
    insert_optional_string(&mut proxy, "ip-version", parse_ip_version(param(&params, "ip-version")));
    insert_skip_cert_verify(&mut proxy, &params, &["skip-cert-verify"]);
    insert_bool_param(&mut proxy, "udp", &params, &["udp"]);

    Ok(proxy)
}

fn parse_wireguard_uri(line: &str) -> Result<Mapping> {
    let parts = parse_url_like_uri(line, &["wireguard", "wg"], "invalid wireguard uri")?;
    let server = parts.host.context("invalid wireguard uri: missing server")?;
    let port = parse_port_or_default(parts.port, 443);
    let private_key = parts
        .auth
        .and_then(decode_component)
        .context("invalid wireguard uri: missing private key")?;
    let params = parse_query_params(parts.query);
    let name = parts
        .fragment
        .and_then(decode_component)
        .unwrap_or_else(|| format!("WireGuard {server}:{port}"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "wireguard");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_string(&mut proxy, "private-key", private_key);
    insert_bool(&mut proxy, "udp", true);
    insert_optional_string(
        &mut proxy,
        "public-key",
        first_param(&params, &["publickey", "public-key"]),
    );
    insert_string_array(&mut proxy, "allowed-ips", split_param(param(&params, "allowed-ips")));
    insert_optional_string(&mut proxy, "pre-shared-key", param(&params, "pre-shared-key"));
    insert_wireguard_addresses(&mut proxy, first_param(&params, &["address", "ip"]));
    insert_wireguard_reserved(&mut proxy, param(&params, "reserved"));
    insert_bool_param(&mut proxy, "udp", &params, &["udp"]);
    insert_optional_number_param(&mut proxy, "mtu", &params, &["mtu"]);
    insert_optional_string(&mut proxy, "dialer-proxy", param(&params, "dialer-proxy"));
    insert_bool_param(&mut proxy, "remote-dns-resolve", &params, &["remote-dns-resolve"]);
    insert_string_array(&mut proxy, "dns", split_param(param(&params, "dns")));

    Ok(proxy)
}

fn parse_anytls_uri(line: &str) -> Result<Mapping> {
    let url = reqwest::Url::parse(line).context("invalid anytls uri")?;
    let server = url
        .host_str()
        .filter(|server| !server.is_empty())
        .context("invalid anytls uri: missing server")?;
    let port = url.port().context("invalid anytls uri: missing port")?;
    let params = parse_query_params(url.query());
    let password = first_param(&params, &["password"])
        .map(ToOwned::to_owned)
        .or_else(|| decode_component(url.username()))
        .context("invalid anytls uri: missing password")?;
    let name =
        decode_component(url.fragment().unwrap_or_default()).unwrap_or_else(|| format!("AnyTLS {server}:{port}"));

    let mut proxy = Mapping::new();
    insert_string(&mut proxy, "type", "anytls");
    insert_string(&mut proxy, "name", name);
    insert_string(&mut proxy, "server", server);
    insert_number(&mut proxy, "port", port);
    insert_string(&mut proxy, "password", password);
    insert_optional_string(&mut proxy, "sni", first_param(&params, &["sni", "peer"]));
    insert_optional_string(
        &mut proxy,
        "client-fingerprint",
        first_param(&params, &["fp", "client-fingerprint"]),
    );
    insert_string_array(&mut proxy, "alpn", split_param(param(&params, "alpn")));
    insert_bool_param(&mut proxy, "udp", &params, &["udp"]);
    insert_optional_number_param(
        &mut proxy,
        "idle-session-check-interval",
        &params,
        &["idle-session-check-interval"],
    );
    insert_optional_number_param(&mut proxy, "idle-session-timeout", &params, &["idle-session-timeout"]);
    insert_optional_number_param(&mut proxy, "min-idle-session", &params, &["min-idle-session"]);
    insert_skip_cert_verify(&mut proxy, &params, &["allowInsecure", "insecure", "skip-cert-verify"]);

    Ok(proxy)
}

fn parse_query_params(query: Option<&str>) -> HashMap<String, String> {
    let mut params = HashMap::new();
    let Some(query) = query else {
        return params;
    };

    for part in query.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').map_or((part, ""), |(key, value)| (key, value));
        let key = decode_component(key)
            .unwrap_or_else(|| key.to_owned())
            .replace('_', "-");
        if key.is_empty() {
            continue;
        }
        params.insert(key, decode_component(value).unwrap_or_else(|| value.to_owned()));
    }

    params
}

#[derive(Debug)]
struct UrlLikeParts<'a> {
    auth: Option<&'a str>,
    host: Option<&'a str>,
    port: Option<&'a str>,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

fn parse_url_like_uri<'a>(line: &'a str, schemes: &[&str], error: &str) -> Result<UrlLikeParts<'a>> {
    let (scheme, rest) = line.split_once("://").with_context(|| error.to_owned())?;
    let scheme = scheme.to_ascii_lowercase();
    if !schemes.iter().any(|expected| *expected == scheme) {
        bail!("{error}");
    }

    let (body, fragment) = rest
        .split_once('#')
        .map_or((rest, None), |(body, fragment)| (body, Some(fragment)));
    let (body, query) = body
        .split_once('?')
        .map_or((body, None), |(body, query)| (body, Some(query)));
    let body = body.strip_suffix('/').unwrap_or(body);
    let (auth, endpoint) = body
        .rsplit_once('@')
        .map_or((None, body), |(auth, endpoint)| (Some(auth), endpoint));
    let (host, port) = split_url_like_host_port(endpoint);

    Ok(UrlLikeParts {
        auth: auth.filter(|auth| !auth.trim().is_empty()),
        host: host.filter(|host| !host.trim().is_empty()),
        port,
        query,
        fragment,
    })
}

fn split_url_like_host_port(endpoint: &str) -> (Option<&str>, Option<&str>) {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return (None, None);
    }

    if let Some(rest) = endpoint.strip_prefix('[')
        && let Some((host, suffix)) = rest.split_once(']')
    {
        let port = suffix.strip_prefix(':').filter(|port| !port.is_empty());
        return (Some(host), port);
    }

    if let Some((host, port)) = endpoint.rsplit_once(':')
        && !host.is_empty()
        && port.chars().all(|ch| ch.is_ascii_digit())
    {
        return (Some(host), Some(port));
    }

    (Some(endpoint), None)
}

fn build_node_subscription_profile(proxies: Vec<Value>, names: &[String]) -> Result<String> {
    let group_name = select_group_name(names);
    let mut group = Mapping::new();
    insert_string(&mut group, "name", &group_name);
    insert_string(&mut group, "type", "select");
    insert_string_array(&mut group, "proxies", names.iter().map(String::as_str).collect());

    let mut root = Mapping::new();
    root.insert(yaml_key("proxies"), Value::Sequence(proxies));
    root.insert(yaml_key("proxy-groups"), Value::Sequence(vec![Value::Mapping(group)]));
    root.insert(
        yaml_key("rules"),
        Value::Sequence(vec![Value::String(format!("MATCH,{group_name}"))]),
    );

    serde_yaml_ng::to_string(&Value::Mapping(root)).context("failed to serialize converted node subscription")
}

fn select_group_name(names: &[String]) -> String {
    ["Proxy", "Remote", "Subscription"]
        .into_iter()
        .find(|candidate| !names.iter().any(|name| name == candidate))
        .unwrap_or("Subscription Select")
        .to_owned()
}

fn unique_proxy_name(name: &str, used_names: &mut HashMap<String, usize>) -> String {
    let base = if name.trim().is_empty() { "Proxy" } else { name.trim() };
    let mut candidate = base.to_owned();
    let mut suffix = 2;
    while used_names.contains_key(&candidate) {
        candidate = format!("{base} {suffix}");
        suffix += 1;
    }
    used_names.insert(candidate.clone(), 1);
    candidate
}

fn vless_network(params: &HashMap<String, String>) -> (String, bool) {
    if param(params, "headerType").is_some_and(|header_type| header_type == "http") {
        return ("http".into(), false);
    }

    let r#type = param(params, "type")
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "tcp".into());
    match r#type.as_str() {
        "websocket" => ("ws".into(), false),
        "httpupgrade" => ("ws".into(), true),
        "ws" => ("ws".into(), true),
        "tcp" | "http" | "grpc" | "h2" => (r#type, false),
        _ => ("tcp".into(), false),
    }
}

fn insert_reality_opts(proxy: &mut Mapping, params: &HashMap<String, String>) {
    let mut opts = Mapping::new();
    insert_optional_string(&mut opts, "public-key", param(params, "pbk"));
    insert_optional_string(&mut opts, "short-id", param(params, "sid"));
    if !opts.is_empty() {
        proxy.insert(yaml_key("reality-opts"), Value::Mapping(opts));
    }
}

fn insert_vless_transport_opts(
    proxy: &mut Mapping,
    network: &str,
    params: &HashMap<String, String>,
    httpupgrade: bool,
) {
    let host = first_param(params, &["host", "obfsParam"]);
    let path = param(params, "path");
    match network {
        "grpc" => insert_grpc_opts(proxy, path),
        "h2" => insert_h2_opts(proxy, host, path),
        "http" => insert_http_opts(proxy, host, path),
        "ws" => insert_ws_opts(proxy, host, path, httpupgrade),
        _ => {}
    }
}

fn insert_grpc_opts(proxy: &mut Mapping, path: Option<&str>) {
    let Some(service_name) = path else {
        return;
    };
    let mut opts = Mapping::new();
    insert_string(&mut opts, "grpc-service-name", service_name);
    proxy.insert(yaml_key("grpc-opts"), Value::Mapping(opts));
}

fn insert_h2_opts(proxy: &mut Mapping, host: Option<&str>, path: Option<&str>) {
    let mut opts = Mapping::new();
    insert_optional_string(&mut opts, "host", host);
    insert_optional_string(&mut opts, "path", path);
    if !opts.is_empty() {
        proxy.insert(yaml_key("h2-opts"), Value::Mapping(opts));
    }
}

fn insert_http_opts(proxy: &mut Mapping, host: Option<&str>, path: Option<&str>) {
    let mut opts = Mapping::new();
    if let Some(path) = path {
        opts.insert(yaml_key("path"), Value::Sequence(vec![Value::String(path.to_owned())]));
    }
    if let Some(host) = host {
        let mut headers = Mapping::new();
        headers.insert(yaml_key("Host"), Value::Sequence(vec![Value::String(host.to_owned())]));
        opts.insert(yaml_key("headers"), Value::Mapping(headers));
    }
    if !opts.is_empty() {
        proxy.insert(yaml_key("http-opts"), Value::Mapping(opts));
    }
}

fn insert_ws_opts(proxy: &mut Mapping, host: Option<&str>, path: Option<&str>, httpupgrade: bool) {
    let mut opts = Mapping::new();
    if let Some(host) = host {
        let mut headers = Mapping::new();
        insert_string(&mut headers, "Host", host);
        opts.insert(yaml_key("headers"), Value::Mapping(headers));
    }
    insert_optional_string(&mut opts, "path", path);
    if httpupgrade {
        insert_bool(&mut opts, "v2ray-http-upgrade", true);
        insert_bool(&mut opts, "v2ray-http-upgrade-fast-open", true);
    }
    if !opts.is_empty() {
        proxy.insert(yaml_key("ws-opts"), Value::Mapping(opts));
    }
}

fn fill_vless_servername_from_transport(proxy: &mut Mapping, tls: bool) {
    if !tls || mapping_string(proxy, "servername").is_some() {
        return;
    }

    let servername = mapping_mapping(proxy, "ws-opts")
        .and_then(|opts| mapping_mapping(opts, "headers"))
        .and_then(|headers| mapping_string(headers, "Host"))
        .or_else(|| {
            mapping_mapping(proxy, "http-opts")
                .and_then(|opts| mapping_mapping(opts, "headers"))
                .and_then(|headers| mapping_sequence_string(headers, "Host"))
        })
        .or_else(|| mapping_mapping(proxy, "h2-opts").and_then(|opts| mapping_string(opts, "host")))
        .map(ToOwned::to_owned);

    if let Some(servername) = servername {
        insert_string(proxy, "servername", servername);
    }
}

fn param<'a>(params: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn first_param<'a>(params: &'a HashMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| param(params, key))
}

fn split_param(value: Option<&str>) -> Vec<&str> {
    value
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_bool_or_presence(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none_or(|value| matches!(value.to_ascii_lowercase().as_str(), "true" | "1"))
}

fn parse_port_or_default(value: Option<&str>, default: u16) -> u16 {
    value
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(default)
}

fn insert_bool_param(map: &mut Mapping, key: &str, params: &HashMap<String, String>, keys: &[&str]) {
    if let Some(raw) = first_param(params, keys) {
        insert_bool(map, key, parse_bool_or_presence(Some(raw)));
    }
}

fn insert_skip_cert_verify(map: &mut Mapping, params: &HashMap<String, String>, keys: &[&str]) {
    insert_bool_param(map, "skip-cert-verify", params, keys);
}

fn insert_optional_number_param(map: &mut Mapping, key: &str, params: &HashMap<String, String>, keys: &[&str]) {
    if let Some(value) = first_param(params, keys).and_then(|value| value.trim().parse::<u64>().ok()) {
        insert_number(map, key, value);
    }
}

fn insert_auth_fields(proxy: &mut Mapping, auth: Option<&str>) {
    let Some(auth) = auth.and_then(decode_component) else {
        return;
    };
    let (username, password) = auth
        .split_once(':')
        .map_or((auth.as_str(), None), |(username, password)| (username, Some(password)));
    if !username.trim().is_empty() {
        insert_string(proxy, "username", username.trim());
    }
    if let Some(password) = password.filter(|password| !password.trim().is_empty()) {
        insert_string(proxy, "password", password.trim());
    }
}

fn parse_ip_version(value: Option<&str>) -> Option<&str> {
    value.filter(|value| matches!(value.trim(), "dual" | "ipv4" | "ipv6" | "ipv4-prefer" | "ipv6-prefer"))
}

fn insert_wireguard_addresses(proxy: &mut Mapping, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    for address in value.split(',') {
        let address = address.trim();
        let address_without_cidr = address
            .split_once('/')
            .map_or_else(|| address, |(address, _)| address.trim());
        let address = address_without_cidr
            .strip_prefix('[')
            .and_then(|address| address.strip_suffix(']'))
            .unwrap_or(address_without_cidr);
        if address.parse::<std::net::Ipv4Addr>().is_ok() {
            insert_string(proxy, "ip", address);
        } else if address.parse::<std::net::Ipv6Addr>().is_ok() {
            insert_string(proxy, "ipv6", address);
        }
    }
}

fn insert_wireguard_reserved(proxy: &mut Mapping, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    let parsed = value
        .split(',')
        .filter_map(|part| part.trim().parse::<u8>().ok())
        .collect::<Vec<_>>();
    if parsed.len() == 3 {
        insert_number_array(proxy, "reserved", parsed);
    }
}

fn vless_flow(value: Option<&str>) -> Option<&str> {
    let flow = value?.trim();
    if flow.is_empty() || flow.eq_ignore_ascii_case("none") {
        return None;
    }
    let mut chars = flow.chars();
    let first = chars.next()?;
    (first.is_ascii_alphanumeric() && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '-')).then_some(flow)
}

fn decode_component(value: &str) -> Option<String> {
    let decoded = percent_decode_str(value).decode_utf8_lossy();
    let trimmed = decoded.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn decode_base64_or_original_fragment(value: &str) -> String {
    decode_base64_fragment(value)
        .or_else(|| decode_component(value))
        .unwrap_or_else(|| value.trim().to_owned())
}

fn clean_ssr_server(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let trimmed = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed)
        .trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn normalize_cipher_name(value: &str) -> String {
    match value.trim() {
        "chacha20-poly1305" => "chacha20-ietf-poly1305".into(),
        "" => "auto".into(),
        value => value.to_owned(),
    }
}

fn remove_whitespace(value: &str) -> String {
    value.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn yaml_key(key: &str) -> Value {
    Value::String(key.to_owned())
}

fn yaml_value<T: Serialize>(value: T) -> Value {
    serde_yaml_ng::to_value(value).unwrap_or_else(|_| Value::Null)
}

fn insert_string(map: &mut Mapping, key: &str, value: impl Into<String>) {
    map.insert(yaml_key(key), Value::String(value.into()));
}

fn insert_optional_string(map: &mut Mapping, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        insert_string(map, key, value);
    }
}

fn insert_number(map: &mut Mapping, key: &str, value: impl Serialize) {
    map.insert(yaml_key(key), yaml_value(value));
}

fn insert_bool(map: &mut Mapping, key: &str, value: bool) {
    map.insert(yaml_key(key), Value::Bool(value));
}

fn insert_string_array(map: &mut Mapping, key: &str, values: Vec<&str>) {
    if values.is_empty() {
        return;
    }
    map.insert(
        yaml_key(key),
        Value::Sequence(
            values
                .into_iter()
                .map(|value| Value::String(value.to_owned()))
                .collect(),
        ),
    );
}

fn insert_number_array<T>(map: &mut Mapping, key: &str, values: Vec<T>)
where
    T: Serialize,
{
    if values.is_empty() {
        return;
    }
    map.insert(yaml_key(key), yaml_value(values));
}

fn mapping_string<'a>(map: &'a Mapping, key: &str) -> Option<&'a str> {
    map.get(yaml_key(key))?.as_str()
}

fn mapping_mapping<'a>(map: &'a Mapping, key: &str) -> Option<&'a Mapping> {
    map.get(yaml_key(key))?.as_mapping()
}

fn mapping_sequence_string<'a>(map: &'a Mapping, key: &str) -> Option<&'a str> {
    map.get(yaml_key(key))?.as_sequence()?.first()?.as_str()
}

fn normalize_remote_url(raw: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(raw).with_context(|| format!("failed to parse subscription URL: {raw}"))?;
    match url.scheme() {
        "http" | "https" => {}
        scheme => bail!("unsupported subscription URL scheme: {scheme}"),
    }

    if url.query().is_none() && url.path().contains('&') {
        let path = url.path().to_owned();
        if let Some((clean_path, dirty_query)) = path.split_once('&') {
            url.set_path(clean_path);
            url.set_query(Some(dirty_query));
        }
    }

    Ok(url.into())
}

fn parse_subscription_userinfo(headers: &HeaderMap) -> Option<PrfExtra> {
    for (key, value) in headers {
        let key = key.as_str().to_ascii_lowercase();
        if !key
            .strip_suffix("subscription-userinfo")
            .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('-'))
        {
            continue;
        }

        let value = value.to_str().ok()?;
        return Some(PrfExtra {
            upload: parse_header_param(value, "upload").unwrap_or(0),
            download: parse_header_param(value, "download").unwrap_or(0),
            total: parse_header_param(value, "total").unwrap_or(0),
            expire: parse_header_param(value, "expire").unwrap_or(0),
        });
    }
    None
}

fn parse_header_param<T>(value: &str, key: &str) -> Option<T>
where
    T: std::str::FromStr,
{
    value.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name.trim() == key).then(|| value.trim().parse::<T>().ok()).flatten()
    })
}

fn parse_update_interval(headers: &HeaderMap) -> Option<u64> {
    header_to_string(headers, "profile-update-interval")?
        .parse::<u64>()
        .ok()
        .map(|hours| hours * 60)
}

fn content_disposition_filename(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(CONTENT_DISPOSITION)?.to_str().ok()?;
    parse_header_param::<String>(value, "filename*")
        .and_then(|name| decode_rfc5987_filename(&name))
        .or_else(|| parse_header_param::<String>(value, "filename").and_then(|name| clean_header_filename(&name)))
}

fn default_subscription_user_agent() -> String {
    "clash-verge/v2.5.1".to_owned()
}

fn decode_rfc5987_filename(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"');
    let encoded = value.split_once("''").map_or(value, |(_, encoded)| encoded);
    clean_header_filename(percent_decode_str(encoded).decode_utf8_lossy().as_ref())
}

fn clean_header_filename(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"').trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn header_to_string(headers: &HeaderMap, key: &str) -> Option<String> {
    headers
        .get(key)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn url_last_segment(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    url.path_segments()?
        .next_back()
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        IProfiles, PrfItem, PrfOption, RemoteProfileImport, content_disposition_filename, download_remote_profile,
        normalize_remote_profile_data, parse_subscription_userinfo, remote_profile_name_override_for_update,
        validate_profile_uid, validate_profile_yaml, validate_remote_profile_yaml,
    };
    use anyhow::Result;
    use base64::{
        Engine as _,
        engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    };
    use reqwest::header::{CONTENT_DISPOSITION, HeaderMap};
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::TcpListener,
    };

    #[test]
    fn append_first_usable_profile_sets_current() {
        let mut profiles = IProfiles::default();
        profiles
            .append_metadata(PrfItem {
                uid: Some("R123".into()),
                itype: Some("remote".into()),
                name: Some("Remote".into()),
                ..PrfItem::default()
            })
            .expect("append profile");

        assert_eq!(profiles.current.as_deref(), Some("R123"));
    }

    #[test]
    fn profile_uid_validation_rejects_path_like_values() {
        assert!(validate_profile_uid("L001").is_ok());
        assert!(validate_profile_uid("../bad").is_err());
        assert!(validate_profile_uid("").is_err());
    }

    #[test]
    fn profile_yaml_validation_requires_mapping_root() {
        assert!(validate_profile_yaml("proxies: []\nrules: []\n").is_ok());
        assert!(validate_profile_yaml("- a\n- b\n").is_err());
    }

    #[test]
    fn remote_profile_yaml_requires_proxies_or_providers() {
        assert!(validate_remote_profile_yaml("proxies:\n  - name: direct\n    type: direct\nrules: []\n").is_ok());
        assert!(
            validate_remote_profile_yaml(
                "proxy-providers:\n  remote:\n    type: http\n    url: https://example.invalid/provider.yaml\nrules: []\n",
            )
            .is_ok()
        );
        assert!(validate_remote_profile_yaml("proxies: []\nrules: []\n").is_err());
        assert!(validate_remote_profile_yaml("proxy-providers: {}\nrules: []\n").is_err());
        assert!(validate_remote_profile_yaml("rules: []\n").is_err());
    }

    #[test]
    fn base64_node_subscription_is_converted_to_profile_yaml() -> Result<()> {
        let vmess = STANDARD.encode(
            serde_json::json!({
                "v": "2",
                "ps": "VMess Node",
                "add": "vmess.example.com",
                "port": "443",
                "id": "11111111-1111-1111-1111-111111111111",
                "aid": "0",
                "scy": "auto",
                "net": "ws",
                "type": "none",
                "host": "cdn.vmess.example.com",
                "path": "/ws",
                "tls": "tls",
                "sni": "sni.vmess.example.com",
                "fp": "chrome"
            })
            .to_string(),
        );
        let ss_userinfo = STANDARD.encode("aes-128-gcm:secret");
        let ssr_password = URL_SAFE_NO_PAD.encode("ssr-secret");
        let ssr = URL_SAFE_NO_PAD.encode(format!(
            "ssr.example.com:8388:auth_sha1_v4:aes-256-cfb:http_simple:{ssr_password}/?remarks={}&obfsparam={}&protoparam={}",
            URL_SAFE_NO_PAD.encode("SSR Node"),
            URL_SAFE_NO_PAD.encode("cdn.ssr.example.com"),
            URL_SAFE_NO_PAD.encode("user-param"),
        ));
        let raw = [
            format!("vmess://{vmess}"),
            format!("ss://{ss_userinfo}@ss.example.com:8388#SS%20Node"),
            format!("ssr://{ssr}"),
            "trojan://secret@trojan.example.com:443?sni=trojan.example.com&type=ws&host=cdn.trojan.example.com&path=%2Fedge#Trojan%20Node".to_owned(),
            "vless://00000000-0000-0000-0000-000000000000@example.com:443?security=tls&type=ws&host=cdn.example.com&path=%2Fedge&sni=sni.example.com&fp=chrome#VLESS%20Node".to_owned(),
            "hysteria2://secret@example.org:443?obfs=salamander&obfs-password=mask&insecure=1&sni=hy.example.org#HY2%20Node".to_owned(),
            "hysteria://hysteria.example.org:443?auth=hy-secret&insecure=1&peer=hysteria.example.org&upmbps=100&downmbps=100#Hysteria%20Node".to_owned(),
            "tuic://11111111-1111-1111-1111-111111111111:tuic-secret@tuic.example.org:10443?sni=tuic.example.org&alpn=h3&udp_relay_mode=native&congestion_controller=bbr&reduce_rtt=1#TUIC%20Node".to_owned(),
            "anytls://any-secret@anytls.example.org:443?sni=anytls.example.org&alpn=h2,http%2F1.1&insecure=1&fp=chrome#AnyTLS%20Node".to_owned(),
            "https://http-user:http-pass@http.example.org:8443?skip-cert-verify=1#HTTP%20Node".to_owned(),
            "socks5://socks-user:socks-pass@socks.example.org:1080?udp=1#SOCKS%20Node".to_owned(),
            "wireguard://private%2Bkey@wg.example.org:51820?public-key=pubkey&address=10.0.0.2%2F32,%5Bfd00%3A%3A2%5D%2F128&reserved=1,2,3&dns=1.1.1.1,8.8.8.8#WG%20Node".to_owned(),
        ]
        .join("\n");
        let encoded = STANDARD.encode(raw.as_bytes());
        let converted = normalize_remote_profile_data(&encoded)?;

        assert!(converted.contains("type: vmess"));
        assert!(converted.contains("type: ss"));
        assert!(converted.contains("type: ssr"));
        assert!(converted.contains("type: trojan"));
        assert!(converted.contains("type: vless"));
        assert!(converted.contains("type: hysteria2"));
        assert!(converted.contains("type: hysteria"));
        assert!(converted.contains("type: tuic"));
        assert!(converted.contains("type: anytls"));
        assert!(converted.contains("type: http"));
        assert!(converted.contains("type: socks5"));
        assert!(converted.contains("type: wireguard"));
        assert!(converted.contains("protocol: auth_sha1_v4"));
        assert!(converted.contains("obfs: http_simple"));
        assert!(converted.contains("protocol-param: user-param"));
        assert!(converted.contains("obfs-param: cdn.ssr.example.com"));
        assert!(converted.contains("auth-str: hy-secret"));
        assert!(converted.contains("udp-relay-mode: native"));
        assert!(converted.contains("private-key: private+key"));
        assert!(converted.contains("public-key: pubkey"));
        assert!(converted.contains("client-fingerprint: chrome"));
        assert!(converted.contains("MATCH,Proxy"));
        validate_remote_profile_yaml(&converted)?;
        Ok(())
    }

    #[test]
    fn mixed_node_subscription_skips_unsupported_and_invalid_lines() -> Result<()> {
        let ss_userinfo = STANDARD.encode("aes-128-gcm:secret");
        let raw = [
            "snell://unsupported.example.com:443#Unsupported",
            "vless://@bad-host:443#Broken",
            &format!("ss://{ss_userinfo}@ss.example.com:8388#Usable%20SS"),
        ]
        .join("\n");
        let encoded = STANDARD.encode(raw.as_bytes());
        let converted = normalize_remote_profile_data(&encoded)?;

        assert!(converted.contains("type: ss"));
        assert!(converted.contains("Usable SS"));
        assert!(!converted.contains("snell"));
        assert!(!converted.contains("Broken"));
        validate_remote_profile_yaml(&converted)?;
        Ok(())
    }

    #[test]
    fn unsupported_only_node_subscription_is_rejected() {
        let raw = [
            "snell://unsupported.example.com:443#Unsupported",
            "naive://example.com:443#Unsupported",
        ]
        .join("\n");
        let encoded = STANDARD.encode(raw.as_bytes());
        let err = normalize_remote_profile_data(&encoded).expect_err("unsupported-only subscription should fail");

        assert!(err.to_string().contains("unsupported node subscription schemes"));
    }

    #[test]
    fn subscription_userinfo_header_is_parsed() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "subscription-userinfo",
            "upload=1; download=2; total=3; expire=4".parse().expect("header value"),
        );
        let extra = parse_subscription_userinfo(&headers).expect("extra");

        assert_eq!(extra.upload, 1);
        assert_eq!(extra.download, 2);
        assert_eq!(extra.total, 3);
        assert_eq!(extra.expire, 4);
    }

    #[test]
    fn content_disposition_filename_star_is_decoded() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_DISPOSITION,
            "attachment;filename*=UTF-8''%E8%B5%94%E9%92%B1%E6%9C%BA%E5%9C%BA"
                .parse()
                .expect("header value"),
        );

        assert_eq!(content_disposition_filename(&headers).as_deref(), Some("赔钱机场"));
    }

    #[test]
    fn update_name_override_refreshes_automatic_url_names_only() {
        let url = "https://example.test/api/v1/pq/2ac0b5057a222bc5bf08ed2b9703ca2a";

        assert_eq!(
            remote_profile_name_override_for_update(Some("2ac0b5057a222bc5bf08ed2b9703ca2a"), url),
            None
        );
        assert_eq!(remote_profile_name_override_for_update(Some("Remote File"), url), None);
        assert_eq!(
            remote_profile_name_override_for_update(Some("用户自定义名称"), url).as_deref(),
            Some("用户自定义名称"),
        );
    }

    #[tokio::test]
    async fn remote_profile_download_reads_yaml_and_headers() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut buffer = vec![0; 2048];
            let read = stream.read(&mut buffer).await?;
            let request = String::from_utf8_lossy(&buffer[..read]);
            assert!(request.starts_with("GET /sub.yaml HTTP/1.1"));
            assert!(request.contains("user-agent: clash-verge/v2.5.1"));
            let body = "proxies:\n  - name: direct\n    type: direct\nrules: []\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nsubscription-userinfo: upload=1; download=2; total=3; expire=4\r\nprofile-update-interval: 6\r\nprofile-web-page-url: https://example.test\r\ncontent-disposition: attachment; filename*=UTF-8''remote%20profile.yaml\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await?;
            Ok::<(), anyhow::Error>(())
        });

        let download = download_remote_profile(&RemoteProfileImport {
            url: format!("http://{addr}/sub.yaml"),
            uid: None,
            name: None,
            desc: None,
            option: None,
        })
        .await?;

        assert_eq!(download.name, "remote profile.yaml");
        assert_eq!(download.update_interval, Some(360));
        assert_eq!(download.home.as_deref(), Some("https://example.test"));
        assert_eq!(download.extra.expect("extra").total, 3);
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn remote_profile_download_prefers_custom_user_agent() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut buffer = vec![0; 2048];
            let read = stream.read(&mut buffer).await?;
            let request = String::from_utf8_lossy(&buffer[..read]);
            assert!(request.contains("user-agent: test-agent/1"));
            assert!(!request.contains("user-agent: clash-verge/v2.5.1"));
            let body = "proxies:\n  - name: direct\n    type: direct\nrules: []\n";
            let response = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{}", body.len(), body);
            stream.write_all(response.as_bytes()).await?;
            Ok::<(), anyhow::Error>(())
        });

        download_remote_profile(&RemoteProfileImport {
            url: format!("http://{addr}/sub.yaml"),
            uid: None,
            name: None,
            desc: None,
            option: Some(PrfOption {
                user_agent: Some("test-agent/1".into()),
                ..PrfOption::default()
            }),
        })
        .await?;

        server.await??;
        Ok(())
    }
}
