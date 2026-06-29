use std::{collections::BTreeMap, fmt, str::FromStr, time::Duration};

use anyhow::{Context as _, Result, bail};
use clash_mihomo::{MihomoClient as _, MihomoHttpMethod, MihomoJsonStream, SimpleMihomoClient};
use serde::{
    Deserialize, Serialize,
    de::{DeserializeOwned, Error as _},
};
use serde_json::{Map, Value, json};
use url::form_urlencoded;

use crate::timeouts;

pub const DEFAULT_PROXY_DELAY_TEST_URL: &str = "http://www.gstatic.com/generate_204";
pub const DEFAULT_PROXY_DELAY_TEST_TIMEOUT_MILLIS: u64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Rule,
    Global,
    Direct,
}

impl Mode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Global => "global",
            Self::Direct => "direct",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Mode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "rule" => Ok(Self::Rule),
            "global" => Ok(Self::Global),
            "direct" => Ok(Self::Direct),
            _ => bail!("unsupported mode: {value}; expected rule, global, or direct"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerHealth {
    pub healthy: bool,
    pub version: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct RuntimeControllerConfig {
    #[serde(default)]
    pub external_controller: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyGroups {
    #[serde(default)]
    pub proxies: BTreeMap<String, ProxyEntry>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyEntry {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    #[serde(default)]
    pub now: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub all: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub history: Vec<ProxyHistoryEntry>,
    #[serde(default)]
    pub alive: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyHistoryEntry {
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub delay: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyProvidersResponse {
    #[serde(default, deserialize_with = "null_as_default")]
    pub providers: BTreeMap<String, ProxyProviderEntry>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyProviderEntry {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    #[serde(default)]
    pub vehicle_type: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub proxies: Vec<Value>,
    #[serde(default)]
    pub subscription_info: Option<ProviderSubscriptionInfo>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderSubscriptionInfo {
    #[serde(default, rename = "Upload", deserialize_with = "optional_u64_from_value")]
    pub upload: Option<u64>,
    #[serde(default, rename = "Download", deserialize_with = "optional_u64_from_value")]
    pub download: Option<u64>,
    #[serde(default, rename = "Total", deserialize_with = "optional_u64_from_value")]
    pub total: Option<u64>,
    #[serde(default, rename = "Expire", deserialize_with = "optional_u64_from_value")]
    pub expire: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuleProvidersResponse {
    #[serde(default, deserialize_with = "null_as_default")]
    pub providers: BTreeMap<String, RuleProviderEntry>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuleProviderEntry {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    #[serde(default)]
    pub behavior: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default, deserialize_with = "optional_u64_from_value")]
    pub rule_count: Option<u64>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub vehicle_type: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RulesResponse {
    #[serde(default)]
    pub rules: Vec<RuleEntry>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuleEntry {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub payload: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl RuleEntry {
    #[must_use]
    pub fn matches_query(&self, query: &str) -> bool {
        let query = query.to_ascii_lowercase();
        [self.r#type.as_deref(), self.payload.as_deref(), self.proxy.as_deref()]
            .into_iter()
            .flatten()
            .any(|value| value.to_ascii_lowercase().contains(&query))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsResponse {
    #[serde(default)]
    pub download_total: u64,
    #[serde(default)]
    pub upload_total: u64,
    #[serde(default, deserialize_with = "null_as_default")]
    pub connections: Vec<ConnectionRecord>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryResponse {
    #[serde(default, alias = "inuse", deserialize_with = "optional_u64_from_value")]
    pub in_use: Option<u64>,
    #[serde(default, alias = "oslimit", deserialize_with = "optional_u64_from_value")]
    pub os_limit: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TrafficResponse {
    #[serde(default, deserialize_with = "optional_u64_from_value")]
    pub up: Option<u64>,
    #[serde(default, deserialize_with = "optional_u64_from_value")]
    pub down: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl MemoryResponse {
    #[must_use]
    pub fn used_bytes(&self) -> Option<u64> {
        self.in_use.or_else(|| {
            ["memory", "mem", "inuse", "inUse"]
                .iter()
                .find_map(|key| self.extra.get(*key).and_then(value_as_u64))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub upload: u64,
    #[serde(default)]
    pub download: u64,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub chains: Vec<String>,
    #[serde(default)]
    pub rule: Option<String>,
    #[serde(default)]
    pub rule_payload: Option<String>,
    #[serde(default)]
    pub metadata: Option<ConnectionMetadata>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionMetadata {
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    #[serde(alias = "sourceIP")]
    pub source_ip: Option<String>,
    #[serde(default)]
    #[serde(alias = "destinationIP")]
    pub destination_ip: Option<String>,
    #[serde(default, deserialize_with = "optional_string_from_value")]
    pub source_port: Option<String>,
    #[serde(default, deserialize_with = "optional_string_from_value")]
    pub destination_port: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub process: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderOperation {
    Update,
    Healthcheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOperationResult {
    pub provider: String,
    pub operation: ProviderOperation,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProxyDelayTestResult {
    pub proxy: String,
    pub url: String,
    pub timeout_millis: u64,
    pub delay: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProxyDelayResponse {
    #[serde(default)]
    delay: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct MihomoController {
    client: SimpleMihomoClient,
}

impl MihomoController {
    #[must_use]
    pub const fn new(client: SimpleMihomoClient) -> Self {
        Self { client }
    }

    pub async fn health(&self) -> ControllerHealth {
        match self.client.version().await {
            Ok(version) => ControllerHealth {
                healthy: true,
                version: Some(version.version),
                message: None,
            },
            Err(err) => ControllerHealth {
                healthy: false,
                version: None,
                message: Some(err.to_string()),
            },
        }
    }

    pub async fn get_mode(&self) -> Result<Mode> {
        let config: Value = self.get_json("/configs").await?;
        let mode = config.get("mode").and_then(Value::as_str).unwrap_or("rule").parse()?;
        Ok(mode)
    }

    pub async fn runtime_controller_config(&self) -> Result<RuntimeControllerConfig> {
        self.get_json("/configs").await
    }

    pub async fn set_mode(&self, mode: Mode) -> Result<Mode> {
        self.request_json::<Value>(MihomoHttpMethod::Patch, "/configs", json!({ "mode": mode.as_str() }))
            .await?;
        Ok(mode)
    }

    pub async fn proxy_groups(&self) -> Result<ProxyGroups> {
        self.get_json("/proxies").await
    }

    pub async fn proxy_providers(&self) -> Result<ProxyProvidersResponse> {
        self.get_json("/providers/proxies").await
    }

    pub async fn rule_providers(&self) -> Result<RuleProvidersResponse> {
        self.get_json("/providers/rules").await
    }

    pub async fn select_proxy(&self, group: &str, proxy: &str) -> Result<()> {
        let path = format!("/proxies/{}", encode_path_segment(group));
        self.request_json::<Value>(MihomoHttpMethod::Put, &path, json!({ "name": proxy }))
            .await?;
        Ok(())
    }

    pub async fn reload_config(&self, path: &str, force: bool) -> Result<()> {
        self.request_json_status_with_timeout(
            MihomoHttpMethod::Put,
            "/configs",
            json!({ "path": path, "force": force }),
            timeouts::RUNTIME_RELOAD_TIMEOUT,
        )
        .await
    }

    pub async fn test_proxy_delay(&self, proxy: &str, url: &str, timeout_millis: u64) -> Result<ProxyDelayTestResult> {
        let path = proxy_delay_path(proxy, url, timeout_millis)?;
        let response: ProxyDelayResponse = self.request_empty_json(MihomoHttpMethod::Get, &path).await?;
        Ok(ProxyDelayTestResult {
            proxy: proxy.to_owned(),
            url: url.to_owned(),
            timeout_millis,
            delay: response.delay,
        })
    }

    pub async fn rules(&self) -> Result<RulesResponse> {
        self.get_json("/rules").await
    }

    pub async fn connections(&self) -> Result<ConnectionsResponse> {
        self.get_json("/connections").await
    }

    pub async fn traffic_stream(&self) -> Result<MihomoJsonStream<TrafficResponse>> {
        self.client.request_json_stream("/traffic").await
    }

    pub async fn memory(&self) -> Result<MemoryResponse> {
        self.client.request_json_stream_latest("/memory", 2).await
    }

    pub async fn close_connection(&self, id: &str) -> Result<()> {
        let path = format!("/connections/{}", encode_path_segment(id));
        let _ = self
            .client
            .request_rest(MihomoHttpMethod::Delete, &path, Vec::new(), None)
            .await?
            .success_json::<Value>(&path)?;
        Ok(())
    }

    pub async fn close_all_connections(&self) -> Result<()> {
        let _ = self
            .client
            .request_rest(MihomoHttpMethod::Delete, "/connections", Vec::new(), None)
            .await?
            .success_json::<Value>("/connections")?;
        Ok(())
    }

    pub async fn update_provider(&self, provider: &str) -> Result<ProviderOperationResult> {
        let path = format!("/providers/proxies/{}", encode_path_segment(provider));
        let raw = self.request_empty_json(MihomoHttpMethod::Put, &path).await?;
        Ok(ProviderOperationResult {
            provider: provider.to_owned(),
            operation: ProviderOperation::Update,
            raw,
        })
    }

    pub async fn update_rule_provider(&self, provider: &str) -> Result<ProviderOperationResult> {
        let path = format!("/providers/rules/{}", encode_path_segment(provider));
        let raw = self.request_empty_json(MihomoHttpMethod::Put, &path).await?;
        Ok(ProviderOperationResult {
            provider: provider.to_owned(),
            operation: ProviderOperation::Update,
            raw,
        })
    }

    pub async fn healthcheck_provider(&self, provider: &str) -> Result<ProviderOperationResult> {
        let path = format!("/providers/proxies/{}/healthcheck", encode_path_segment(provider));
        let raw = self.request_empty_json(MihomoHttpMethod::Get, &path).await?;
        Ok(ProviderOperationResult {
            provider: provider.to_owned(),
            operation: ProviderOperation::Healthcheck,
            raw,
        })
    }

    async fn get_json<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.client.request_json(path).await
    }

    async fn request_empty_json<T>(&self, method: MihomoHttpMethod, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.client
            .request_rest(method, path, Vec::new(), None)
            .await?
            .success_json(path)
    }

    async fn request_json<T>(&self, method: MihomoHttpMethod, path: &str, body: Value) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let body = serde_json::to_vec(&body).context("failed to encode mihomo request body")?;
        self.client
            .request_rest(method, path, body, Some("application/json"))
            .await?
            .success_json(path)
    }

    async fn request_json_status_with_timeout(
        &self,
        method: MihomoHttpMethod,
        path: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<()> {
        let body = serde_json::to_vec(&body).context("failed to encode mihomo request body")?;
        self.client
            .with_timeout(timeout)
            .request_rest(method, path, body, Some("application/json"))
            .await?
            .success_status(path)
    }
}

trait MihomoJsonResponseExt {
    fn success_json<T>(self, path: &str) -> Result<T>
    where
        T: DeserializeOwned;

    fn success_status(self, path: &str) -> Result<()>;
}

impl MihomoJsonResponseExt for clash_mihomo::MihomoResponse {
    fn success_json<T>(self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        if !(200..300).contains(&self.status) {
            bail!("{}", mihomo_status_error(path, self.status, &self.body));
        }
        if self.body.is_empty() {
            return serde_json::from_value(Value::Null)
                .with_context(|| format!("mihomo response from {path} was empty"));
        }
        serde_json::from_slice(&self.body).with_context(|| format!("failed to decode mihomo response from {path}"))
    }

    fn success_status(self, path: &str) -> Result<()> {
        if !(200..300).contains(&self.status) {
            bail!("{}", mihomo_status_error(path, self.status, &self.body));
        }
        Ok(())
    }
}

fn mihomo_status_error(path: &str, status: u16, body: &[u8]) -> String {
    let detail = String::from_utf8_lossy(body);
    let detail = detail.trim();
    if detail.is_empty() {
        format!("mihomo request {path} returned HTTP {status}")
    } else {
        format!("mihomo request {path} returned HTTP {status}: {detail}")
    }
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = fmt::Write::write_fmt(&mut encoded, format_args!("%{byte:02X}"));
        }
    }
    encoded
}

fn encode_query_value(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn proxy_delay_path(proxy: &str, url: &str, timeout_millis: u64) -> Result<String> {
    let proxy = proxy.trim();
    let url = url.trim();
    if proxy.is_empty() {
        bail!("代理节点不能为空");
    }
    if url.is_empty() {
        bail!("测速 URL 不能为空");
    }
    if timeout_millis == 0 {
        bail!("测速超时必须大于 0");
    }
    Ok(format!(
        "/proxies/{}/delay?timeout={timeout_millis}&url={}",
        encode_path_segment(proxy),
        encode_query_value(url)
    ))
}

fn null_as_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn optional_u64_from_value<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value_as_u64(&value)
        .map(Some)
        .ok_or_else(|| D::Error::custom(format!("expected u64-compatible value, got {value}")))
}

fn optional_string_from_value<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value)),
        Value::Number(value) => Ok(Some(value.to_string())),
        Value::Bool(value) => Ok(Some(value.to_string())),
        other => Err(D::Error::custom(format!(
            "expected string-compatible value, got {other}"
        ))),
    }
}

fn value_as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(value) => value.parse::<u64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionsResponse, MemoryResponse, MihomoController, Mode, ProxyGroups, ProxyProvidersResponse,
        RuleProvidersResponse, RulesResponse, encode_path_segment, proxy_delay_path,
    };
    use clash_mihomo::{MihomoClientConfig, SimpleMihomoClient};
    use std::{net::SocketAddr, time::Duration};
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::TcpListener,
        time::sleep,
    };

    #[test]
    fn mode_parses_supported_values() {
        assert_eq!("rule".parse::<Mode>().expect("rule"), Mode::Rule);
        assert_eq!("GLOBAL".parse::<Mode>().expect("global"), Mode::Global);
        assert!("invalid".parse::<Mode>().is_err());
    }

    #[test]
    fn path_segments_are_percent_encoded() {
        assert_eq!(
            encode_path_segment("Provider A/香港"),
            "Provider%20A%2F%E9%A6%99%E6%B8%AF"
        );
        assert_eq!(encode_path_segment("id:1?x=y"), "id%3A1%3Fx%3Dy");
    }

    #[test]
    fn proxy_delay_path_encodes_proxy_and_query_url() {
        assert_eq!(
            proxy_delay_path("香港节点/01", "http://www.gstatic.com/generate_204", 5_000).expect("path"),
            "/proxies/%E9%A6%99%E6%B8%AF%E8%8A%82%E7%82%B9%2F01/delay?timeout=5000&url=http%3A%2F%2Fwww.gstatic.com%2Fgenerate_204"
        );
    }

    #[tokio::test]
    async fn reload_config_sends_put_configs_and_accepts_empty_success() -> anyhow::Result<()> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut buffer = vec![0_u8; 4096];
            let size = stream.read(&mut buffer).await?;
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await?;
            Ok::<String, anyhow::Error>(String::from_utf8_lossy(&buffer[..size]).into_owned())
        });

        let client = SimpleMihomoClient::new(MihomoClientConfig::tcp(addr).with_timeout(Duration::from_secs(1)));
        MihomoController::new(client)
            .reload_config("/tmp/clash-tui-runtime.yaml", true)
            .await?;

        let request = server.await??;
        assert!(request.starts_with("PUT /configs HTTP/1.1"));
        let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
        let body: serde_json::Value = serde_json::from_str(body)?;
        assert_eq!(body["path"], "/tmp/clash-tui-runtime.yaml");
        assert_eq!(body["force"], true);
        Ok(())
    }

    #[tokio::test]
    async fn reload_config_uses_dedicated_timeout_instead_of_client_default() -> anyhow::Result<()> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut buffer = vec![0_u8; 4096];
            let _ = stream.read(&mut buffer).await?;
            sleep(Duration::from_millis(80)).await;
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await?;
            Ok::<(), anyhow::Error>(())
        });

        let client = SimpleMihomoClient::new(MihomoClientConfig::tcp(addr).with_timeout(Duration::from_millis(20)));
        MihomoController::new(client)
            .reload_config("/tmp/clash-tui-runtime.yaml", true)
            .await?;

        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn reload_config_error_includes_response_body() -> anyhow::Result<()> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut buffer = vec![0_u8; 4096];
            let _ = stream.read(&mut buffer).await?;
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 28\r\n\r\n{\"message\":\"bad config path\"}")
                .await?;
            Ok::<(), anyhow::Error>(())
        });

        let client = SimpleMihomoClient::new(MihomoClientConfig::tcp(addr).with_timeout(Duration::from_secs(1)));
        let err = MihomoController::new(client)
            .reload_config("/tmp/missing.yaml", true)
            .await
            .expect_err("reload should fail");

        let message = err.to_string();
        assert!(message.contains("HTTP 400"));
        assert!(message.contains("bad config path"));
        server.await??;
        Ok(())
    }

    #[test]
    fn rules_response_decodes_typed_entries_and_keeps_extra_fields() {
        let rules: RulesResponse = serde_json::from_value(serde_json::json!({
            "rules": [
                { "type": "DOMAIN-SUFFIX", "payload": "example.com", "proxy": "Proxy", "size": -1 }
            ],
            "provider": "rule-provider"
        }))
        .expect("rules");

        assert_eq!(rules.rules.len(), 1);
        assert_eq!(rules.rules[0].size, Some(-1));
        assert!(rules.rules[0].matches_query("example"));
        assert_eq!(
            rules.extra.get("provider").and_then(serde_json::Value::as_str),
            Some("rule-provider")
        );
    }

    #[test]
    fn connections_response_decodes_metadata() {
        let connections: ConnectionsResponse = serde_json::from_value(serde_json::json!({
            "downloadTotal": 20,
            "uploadTotal": 10,
            "connections": [
                {
                    "id": "abc",
                    "upload": 1,
                    "download": 2,
                    "metadata": {
                        "host": "example.com",
                        "destinationIP": "93.184.216.34",
                        "sourcePort": 51234,
                        "destinationPort": "443"
                    }
                }
            ]
        }))
        .expect("connections");

        assert_eq!(connections.upload_total, 10);
        assert_eq!(connections.connections[0].id, "abc");
        assert_eq!(
            connections.connections[0]
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.host.as_deref()),
            Some("example.com")
        );
        assert_eq!(
            connections.connections[0]
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.destination_ip.as_deref()),
            Some("93.184.216.34")
        );
        assert_eq!(
            connections.connections[0]
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.source_port.as_deref()),
            Some("51234")
        );
    }

    #[test]
    fn connections_response_accepts_null_connections() {
        let connections: ConnectionsResponse = serde_json::from_value(serde_json::json!({
            "downloadTotal": 0,
            "uploadTotal": 0,
            "connections": null,
            "memory": 0
        }))
        .expect("connections");

        assert!(connections.connections.is_empty());
        assert_eq!(
            connections.extra.get("memory").and_then(serde_json::Value::as_u64),
            Some(0)
        );
    }

    #[test]
    fn memory_response_decodes_mihomo_memory_shape() {
        let memory: MemoryResponse = serde_json::from_value(serde_json::json!({
            "inuse": 123456,
            "oslimit": "999999"
        }))
        .expect("memory");

        assert_eq!(memory.in_use, Some(123456));
        assert_eq!(memory.os_limit, Some(999999));
        assert_eq!(memory.used_bytes(), Some(123456));
    }

    #[test]
    fn proxy_groups_response_decodes_typed_entries() {
        let groups: ProxyGroups = serde_json::from_value(serde_json::json!({
            "proxies": {
                "Proxy": {
                    "name": "Proxy",
                    "type": "Selector",
                    "now": "HK-1",
                    "all": ["HK-1", "SG-1"],
                    "history": []
                },
                "HK-1": {
                    "name": "HK-1",
                    "type": "Shadowsocks",
                    "now": "",
                    "all": null,
                    "alive": true,
                    "history": [{ "time": "2026-06-18T00:00:00Z", "delay": 86 }]
                }
            },
            "provider": "proxy-provider"
        }))
        .expect("proxy groups");

        let group = groups.proxies.get("Proxy").expect("proxy group");
        assert_eq!(group.r#type.as_deref(), Some("Selector"));
        assert_eq!(group.now.as_deref(), Some("HK-1"));
        assert_eq!(group.all, vec!["HK-1".to_owned(), "SG-1".to_owned()]);
        assert_eq!(
            groups.extra.get("provider").and_then(serde_json::Value::as_str),
            Some("proxy-provider")
        );
        let leaf = groups.proxies.get("HK-1").expect("leaf proxy");
        assert!(leaf.all.is_empty());
        assert_eq!(leaf.alive, Some(true));
        assert_eq!(leaf.history.first().and_then(|history| history.delay), Some(86));
    }

    #[test]
    fn proxy_providers_response_decodes_typed_entries() {
        let providers: ProxyProvidersResponse = serde_json::from_value(serde_json::json!({
            "providers": {
                "Provider A/香港": {
                    "name": "Provider A/香港",
                    "type": "Proxy",
                    "vehicleType": "HTTP",
                    "updatedAt": "2026-06-18T00:00:00Z",
                    "proxies": [{ "name": "HK-1" }],
                    "subscriptionInfo": {
                        "Upload": "1024",
                        "Download": 2048,
                        "Total": 4096,
                        "Expire": 1893456000
                    }
                }
            }
        }))
        .expect("providers response");

        let provider = providers.providers.get("Provider A/香港").expect("provider");
        assert_eq!(provider.name.as_deref(), Some("Provider A/香港"));
        assert_eq!(provider.vehicle_type.as_deref(), Some("HTTP"));
        assert_eq!(provider.proxies.len(), 1);
        let subscription = provider.subscription_info.as_ref().expect("subscription");
        assert_eq!(subscription.upload, Some(1024));
        assert_eq!(subscription.download, Some(2048));
        assert_eq!(subscription.total, Some(4096));
        assert_eq!(subscription.expire, Some(1893456000));
    }

    #[test]
    fn rule_providers_response_decodes_typed_entries() {
        let providers: RuleProvidersResponse = serde_json::from_value(serde_json::json!({
            "providers": {
                "reject-rules": {
                    "name": "reject-rules",
                    "type": "Rule",
                    "behavior": "domain",
                    "format": "yaml",
                    "ruleCount": "128",
                    "updatedAt": "2026-06-18T00:00:00Z",
                    "vehicleType": "HTTP"
                }
            }
        }))
        .expect("rule providers response");

        let provider = providers.providers.get("reject-rules").expect("provider");
        assert_eq!(provider.name.as_deref(), Some("reject-rules"));
        assert_eq!(provider.behavior.as_deref(), Some("domain"));
        assert_eq!(provider.format.as_deref(), Some("yaml"));
        assert_eq!(provider.rule_count, Some(128));
    }
}
