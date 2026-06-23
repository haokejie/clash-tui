use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    net::TcpStream,
    sync::mpsc,
    time::timeout,
};

#[cfg(unix)]
use tokio::net::UnixStream;

use crate::models::{MihomoHealth, MihomoVersion};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ControllerEndpoint {
    Tcp {
        addr: SocketAddr,
    },
    #[cfg(unix)]
    Unix {
        path: PathBuf,
    },
    #[cfg(windows)]
    NamedPipe {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MihomoClientConfig {
    pub endpoint: ControllerEndpoint,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    pub timeout_millis: u64,
}

impl MihomoClientConfig {
    #[must_use]
    pub const fn tcp(addr: SocketAddr) -> Self {
        Self {
            endpoint: ControllerEndpoint::Tcp { addr },
            secret: None,
            timeout_millis: DEFAULT_TIMEOUT.as_millis() as u64,
        }
    }

    #[cfg(unix)]
    #[must_use]
    pub const fn unix(path: PathBuf) -> Self {
        Self {
            endpoint: ControllerEndpoint::Unix { path },
            secret: None,
            timeout_millis: DEFAULT_TIMEOUT.as_millis() as u64,
        }
    }

    #[must_use]
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        self.secret = Some(secret.into());
        self
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_millis = timeout.as_millis() as u64;
        self
    }

    #[must_use]
    pub const fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_millis)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MihomoHttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl MihomoHttpMethod {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MihomoResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

pub struct MihomoJsonStream<T> {
    receiver: mpsc::Receiver<Result<T>>,
}

impl<T> MihomoJsonStream<T> {
    pub async fn next(&mut self) -> Option<Result<T>> {
        self.receiver.recv().await
    }
}

#[allow(async_fn_in_trait)]
pub trait MihomoClient: Send + Sync {
    async fn version(&self) -> Result<MihomoVersion>;

    async fn health(&self) -> Result<MihomoHealth>;
}

#[derive(Debug, Clone)]
pub struct SimpleMihomoClient {
    config: MihomoClientConfig,
}

impl SimpleMihomoClient {
    #[must_use]
    pub const fn new(config: MihomoClientConfig) -> Self {
        Self { config }
    }

    pub async fn request_json<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let body = self
            .request_rest(MihomoHttpMethod::Get, path, Vec::new(), None)
            .await?
            .success_body(path)?;
        serde_json::from_slice(&body).with_context(|| format!("failed to decode mihomo response from {path}"))
    }

    pub async fn request_rest(
        &self,
        method: MihomoHttpMethod,
        path: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<MihomoResponse> {
        let timeout_duration = self.config.timeout();
        match timeout(timeout_duration, self.request_inner(method, path, body, content_type)).await {
            Ok(result) => result,
            Err(_) => bail!(
                "mihomo {} request {path} timed out after {}ms",
                method.as_str(),
                timeout_duration.as_millis()
            ),
        }
    }

    pub async fn request_json_snapshot<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let body = self
            .request_stream_snapshot(MihomoHttpMethod::Get, path, Vec::new(), None)
            .await?
            .success_body(path)?;
        serde_json::from_slice(&body).with_context(|| format!("failed to decode mihomo response from {path}"))
    }

    pub async fn request_json_stream_latest<T>(&self, path: &str, max_frames: usize) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let body = self
            .request_stream_latest(MihomoHttpMethod::Get, path, Vec::new(), None, max_frames)
            .await?
            .success_body(path)?;
        serde_json::from_slice(&body).with_context(|| format!("failed to decode mihomo response from {path}"))
    }

    pub async fn request_json_stream<T>(&self, path: &str) -> Result<MihomoJsonStream<T>>
    where
        T: DeserializeOwned + Send + 'static,
    {
        let timeout_duration = self.config.timeout();
        match timeout(timeout_duration, self.request_json_stream_inner(path)).await {
            Ok(result) => result,
            Err(_) => bail!(
                "mihomo GET json stream {path} timed out after {}ms",
                timeout_duration.as_millis()
            ),
        }
    }

    pub async fn request_stream_snapshot(
        &self,
        method: MihomoHttpMethod,
        path: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<MihomoResponse> {
        let timeout_duration = self.config.timeout();
        match timeout(
            timeout_duration,
            self.request_stream_snapshot_inner(method, path, body, content_type),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => bail!(
                "mihomo {} stream snapshot {path} timed out after {}ms",
                method.as_str(),
                timeout_duration.as_millis()
            ),
        }
    }

    pub async fn request_stream_latest(
        &self,
        method: MihomoHttpMethod,
        path: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
        max_frames: usize,
    ) -> Result<MihomoResponse> {
        let timeout_duration = self.config.timeout();
        match timeout(
            timeout_duration,
            self.request_stream_latest_inner(method, path, body, content_type, max_frames.max(1)),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => bail!(
                "mihomo {} stream latest {path} timed out after {}ms",
                method.as_str(),
                timeout_duration.as_millis()
            ),
        }
    }

    async fn request_inner(
        &self,
        method: MihomoHttpMethod,
        path: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<MihomoResponse> {
        match &self.config.endpoint {
            ControllerEndpoint::Tcp { addr } => {
                let stream = TcpStream::connect(addr)
                    .await
                    .with_context(|| format!("failed to connect mihomo controller {addr}"))?;
                request_over_stream(stream, method, path, self.config.secret.as_deref(), &body, content_type).await
            }
            #[cfg(unix)]
            ControllerEndpoint::Unix { path: socket_path } => {
                let stream = UnixStream::connect(socket_path)
                    .await
                    .with_context(|| format!("failed to connect mihomo unix socket {}", socket_path.display()))?;
                request_over_stream(stream, method, path, self.config.secret.as_deref(), &body, content_type).await
            }
            #[cfg(windows)]
            ControllerEndpoint::NamedPipe { path } => {
                bail!("mihomo named pipe controller is not implemented yet: {path}")
            }
        }
    }

    async fn request_stream_snapshot_inner(
        &self,
        method: MihomoHttpMethod,
        path: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<MihomoResponse> {
        match &self.config.endpoint {
            ControllerEndpoint::Tcp { addr } => {
                let stream = TcpStream::connect(addr)
                    .await
                    .with_context(|| format!("failed to connect mihomo controller {addr}"))?;
                request_snapshot_over_stream(stream, method, path, self.config.secret.as_deref(), &body, content_type)
                    .await
            }
            #[cfg(unix)]
            ControllerEndpoint::Unix { path: socket_path } => {
                let stream = UnixStream::connect(socket_path)
                    .await
                    .with_context(|| format!("failed to connect mihomo unix socket {}", socket_path.display()))?;
                request_snapshot_over_stream(stream, method, path, self.config.secret.as_deref(), &body, content_type)
                    .await
            }
            #[cfg(windows)]
            ControllerEndpoint::NamedPipe { path } => {
                bail!("mihomo named pipe controller is not implemented yet: {path}")
            }
        }
    }

    async fn request_stream_latest_inner(
        &self,
        method: MihomoHttpMethod,
        path: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
        max_frames: usize,
    ) -> Result<MihomoResponse> {
        match &self.config.endpoint {
            ControllerEndpoint::Tcp { addr } => {
                let stream = TcpStream::connect(addr)
                    .await
                    .with_context(|| format!("failed to connect mihomo controller {addr}"))?;
                request_latest_over_stream(
                    stream,
                    method,
                    path,
                    self.config.secret.as_deref(),
                    &body,
                    content_type,
                    max_frames,
                )
                .await
            }
            #[cfg(unix)]
            ControllerEndpoint::Unix { path: socket_path } => {
                let stream = UnixStream::connect(socket_path)
                    .await
                    .with_context(|| format!("failed to connect mihomo unix socket {}", socket_path.display()))?;
                request_latest_over_stream(
                    stream,
                    method,
                    path,
                    self.config.secret.as_deref(),
                    &body,
                    content_type,
                    max_frames,
                )
                .await
            }
            #[cfg(windows)]
            ControllerEndpoint::NamedPipe { path } => {
                bail!("mihomo named pipe controller is not implemented yet: {path}")
            }
        }
    }

    async fn request_json_stream_inner<T>(&self, path: &str) -> Result<MihomoJsonStream<T>>
    where
        T: DeserializeOwned + Send + 'static,
    {
        match &self.config.endpoint {
            ControllerEndpoint::Tcp { addr } => {
                let stream = TcpStream::connect(addr)
                    .await
                    .with_context(|| format!("failed to connect mihomo controller {addr}"))?;
                request_json_stream_over_stream(
                    stream,
                    MihomoHttpMethod::Get,
                    path,
                    self.config.secret.as_deref(),
                    &[],
                    None,
                )
                .await
            }
            #[cfg(unix)]
            ControllerEndpoint::Unix { path: socket_path } => {
                let stream = UnixStream::connect(socket_path)
                    .await
                    .with_context(|| format!("failed to connect mihomo unix socket {}", socket_path.display()))?;
                request_json_stream_over_stream(
                    stream,
                    MihomoHttpMethod::Get,
                    path,
                    self.config.secret.as_deref(),
                    &[],
                    None,
                )
                .await
            }
            #[cfg(windows)]
            ControllerEndpoint::NamedPipe { path } => {
                bail!("mihomo named pipe controller is not implemented yet: {path}")
            }
        }
    }
}

impl MihomoResponse {
    fn success_body(self, path: &str) -> Result<Vec<u8>> {
        if !(200..300).contains(&self.status) {
            bail!("mihomo request {path} returned HTTP {}", self.status);
        }
        Ok(self.body)
    }
}

impl MihomoClient for SimpleMihomoClient {
    async fn version(&self) -> Result<MihomoVersion> {
        self.request_json("/version").await
    }

    async fn health(&self) -> Result<MihomoHealth> {
        let version = self.version().await?;
        Ok(MihomoHealth {
            healthy: true,
            version: Some(version.version),
            message: None,
        })
    }
}

async fn request_over_stream<S>(
    mut stream: S,
    method: MihomoHttpMethod,
    path: &str,
    secret: Option<&str>,
    body: &[u8],
    content_type: Option<&str>,
) -> Result<MihomoResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_http_request(&mut stream, method, path, secret, body, content_type).await?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .with_context(|| format!("failed to read mihomo response {path}"))?;

    parse_http_response(&response)
}

async fn request_snapshot_over_stream<S>(
    mut stream: S,
    method: MihomoHttpMethod,
    path: &str,
    secret: Option<&str>,
    body: &[u8],
    content_type: Option<&str>,
) -> Result<MihomoResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_http_request(&mut stream, method, path, secret, body, content_type).await?;

    let (head, mut body_buffer) = read_response_head_and_body_start(&mut stream, path).await?;
    let body = if head.is_chunked {
        read_first_chunked_frame(&mut stream, &mut body_buffer, path).await?
    } else if let Some(content_length) = head.content_length {
        while body_buffer.len() < content_length {
            let mut buffer = [0_u8; 4096];
            let read = stream
                .read(&mut buffer)
                .await
                .with_context(|| format!("failed to read mihomo response {path}"))?;
            if read == 0 {
                break;
            }
            body_buffer.extend_from_slice(&buffer[..read]);
        }
        body_buffer.truncate(content_length);
        body_buffer
    } else {
        read_first_json_frame(&mut stream, &mut body_buffer, path).await?
    };

    Ok(MihomoResponse {
        status: head.status,
        content_type: head.content_type,
        body,
    })
}

async fn request_latest_over_stream<S>(
    mut stream: S,
    method: MihomoHttpMethod,
    path: &str,
    secret: Option<&str>,
    body: &[u8],
    content_type: Option<&str>,
    max_frames: usize,
) -> Result<MihomoResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_http_request(&mut stream, method, path, secret, body, content_type).await?;

    let (head, mut body_buffer) = read_response_head_and_body_start(&mut stream, path).await?;
    let body = if head.is_chunked {
        read_latest_chunked_frame(&mut stream, &mut body_buffer, path, max_frames).await?
    } else if let Some(content_length) = head.content_length {
        while body_buffer.len() < content_length {
            let mut buffer = [0_u8; 4096];
            let read = stream
                .read(&mut buffer)
                .await
                .with_context(|| format!("failed to read mihomo response {path}"))?;
            if read == 0 {
                break;
            }
            body_buffer.extend_from_slice(&buffer[..read]);
        }
        body_buffer.truncate(content_length);
        body_buffer
    } else {
        read_first_json_frame(&mut stream, &mut body_buffer, path).await?
    };

    Ok(MihomoResponse {
        status: head.status,
        content_type: head.content_type,
        body,
    })
}

async fn request_json_stream_over_stream<S, T>(
    mut stream: S,
    method: MihomoHttpMethod,
    path: &str,
    secret: Option<&str>,
    body: &[u8],
    content_type: Option<&str>,
) -> Result<MihomoJsonStream<T>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    T: DeserializeOwned + Send + 'static,
{
    write_http_request(&mut stream, method, path, secret, body, content_type).await?;
    let (head, body_buffer) = read_response_head_and_body_start(&mut stream, path).await?;
    if !(200..300).contains(&head.status) {
        bail!("mihomo request {path} returned HTTP {}", head.status);
    }

    let (sender, receiver) = mpsc::channel(8);
    let path = path.to_owned();
    tokio::spawn(async move {
        stream_json_response_frames(stream, body_buffer, head, path, sender).await;
    });
    Ok(MihomoJsonStream { receiver })
}

async fn write_http_request<S>(
    stream: &mut S,
    method: MihomoHttpMethod,
    path: &str,
    secret: Option<&str>,
    body: &[u8],
    content_type: Option<&str>,
) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let mut request = format!(
        "{} {path} HTTP/1.1\r\nHost: clash-tui\r\nConnection: close\r\nContent-Length: {}\r\n",
        method.as_str(),
        body.len()
    );
    if let Some(secret) = secret {
        request.push_str("Authorization: Bearer ");
        request.push_str(secret);
        request.push_str("\r\n");
    }
    if let Some(content_type) = content_type.filter(|content_type| !content_type.trim().is_empty()) {
        request.push_str("Content-Type: ");
        request.push_str(content_type);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");

    stream
        .write_all(request.as_bytes())
        .await
        .with_context(|| format!("failed to write mihomo request {path}"))?;
    if !body.is_empty() {
        stream
            .write_all(body)
            .await
            .with_context(|| format!("failed to write mihomo request body {path}"))?;
    }
    stream
        .shutdown()
        .await
        .with_context(|| format!("failed to shutdown mihomo request {path}"))?;
    Ok(())
}

async fn read_response_head_and_body_start<S>(stream: &mut S, path: &str) -> Result<(HttpResponseHead, Vec<u8>)>
where
    S: AsyncRead + Unpin,
{
    let mut response = Vec::new();
    let header_end = loop {
        if let Some(header_end) = find_subsequence(&response, b"\r\n\r\n") {
            break header_end;
        }
        let mut buffer = [0_u8; 4096];
        let read = stream
            .read(&mut buffer)
            .await
            .with_context(|| format!("failed to read mihomo response {path}"))?;
        if read == 0 {
            bail!("invalid mihomo HTTP response: missing header terminator");
        }
        response.extend_from_slice(&buffer[..read]);
    };

    let head = parse_http_response_head(&response[..header_end])?;
    Ok((head, response[header_end + 4..].to_vec()))
}

fn parse_http_response(response: &[u8]) -> Result<MihomoResponse> {
    let Some(header_end) = find_subsequence(response, b"\r\n\r\n") else {
        bail!("invalid mihomo HTTP response: missing header terminator");
    };

    let head = parse_http_response_head(&response[..header_end])?;
    let body = &response[header_end + 4..];
    let body = if head.is_chunked {
        decode_chunked_body(body).context("failed to decode mihomo chunked response body")?
    } else {
        body.to_vec()
    };

    Ok(MihomoResponse {
        status: head.status,
        content_type: head.content_type,
        body,
    })
}

async fn stream_json_response_frames<S, T>(
    mut stream: S,
    mut body_buffer: Vec<u8>,
    head: HttpResponseHead,
    path: String,
    sender: mpsc::Sender<Result<T>>,
) where
    S: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    if head.is_chunked {
        loop {
            match read_next_chunked_frame(&mut stream, &mut body_buffer, &path).await {
                Ok(Some(body)) => {
                    let frame = serde_json::from_slice(&body)
                        .with_context(|| format!("failed to decode mihomo response from {path}"));
                    if sender.send(frame).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    let _ = sender.send(Err(err)).await;
                    break;
                }
            }
        }
        return;
    }

    let body = if let Some(content_length) = head.content_length {
        while body_buffer.len() < content_length {
            match read_more_response_body(&mut stream, &mut body_buffer, &path).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) => {
                    let _ = sender.send(Err(err)).await;
                    return;
                }
            }
        }
        body_buffer.truncate(content_length);
        body_buffer
    } else {
        match read_first_json_frame(&mut stream, &mut body_buffer, &path).await {
            Ok(body) => body,
            Err(err) => {
                let _ = sender.send(Err(err)).await;
                return;
            }
        }
    };
    let frame = serde_json::from_slice(&body).with_context(|| format!("failed to decode mihomo response from {path}"));
    let _ = sender.send(frame).await;
}

#[derive(Debug, Clone)]
struct HttpResponseHead {
    status: u16,
    content_type: Option<String>,
    content_length: Option<usize>,
    is_chunked: bool,
}

fn parse_http_response_head(headers: &[u8]) -> Result<HttpResponseHead> {
    let headers = std::str::from_utf8(headers).context("invalid mihomo HTTP response headers")?;
    let status_line = headers.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or_default();

    let content_type = headers.lines().find_map(|line| header_value(line, "content-type"));
    let content_length = headers
        .lines()
        .find_map(|line| header_value(line, "content-length"))
        .and_then(|value| value.parse::<usize>().ok());
    let is_chunked = headers
        .lines()
        .filter_map(|line| header_value(line, "transfer-encoding"))
        .any(|value| {
            value
                .split(',')
                .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
        });

    Ok(HttpResponseHead {
        status,
        content_type,
        content_length,
        is_chunked,
    })
}

async fn read_next_chunked_frame<S>(stream: &mut S, body: &mut Vec<u8>, path: &str) -> Result<Option<Vec<u8>>>
where
    S: AsyncRead + Unpin,
{
    loop {
        let Some(line_end) = find_subsequence(body, b"\r\n") else {
            let read = read_more_response_body(stream, body, path).await?;
            if read == 0 {
                if body.is_empty() {
                    return Ok(None);
                }
                bail!("invalid chunked mihomo response: missing chunk size terminator");
            }
            continue;
        };
        let size_line = std::str::from_utf8(&body[..line_end]).context("invalid chunked mihomo response size line")?;
        let size_token = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_token, 16)
            .with_context(|| format!("invalid chunked mihomo response size: {size_token}"))?;
        let chunk_start = line_end + 2;
        if size == 0 {
            body.drain(..chunk_start);
            return Ok(None);
        }
        while body.len() < chunk_start + size + 2 {
            let read = read_more_response_body(stream, body, path).await?;
            if read == 0 {
                bail!("invalid chunked mihomo response: chunk exceeds body length");
            }
        }
        let chunk_end = chunk_start + size;
        if body.get(chunk_end..chunk_end + 2) != Some(b"\r\n") {
            bail!("invalid chunked mihomo response: missing chunk terminator");
        }
        let chunk = body[chunk_start..chunk_end].to_vec();
        body.drain(..chunk_end + 2);
        return Ok(Some(
            first_json_frame(&chunk).unwrap_or_else(|| trim_ascii(&chunk).to_vec()),
        ));
    }
}

async fn read_first_chunked_frame<S>(stream: &mut S, body: &mut Vec<u8>, path: &str) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut offset = 0;
    loop {
        let Some(line_end) = find_subsequence(&body[offset..], b"\r\n").map(|index| offset + index) else {
            read_more_response_body(stream, body, path).await?;
            continue;
        };
        let size_line =
            std::str::from_utf8(&body[offset..line_end]).context("invalid chunked mihomo response size line")?;
        let size_token = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_token, 16)
            .with_context(|| format!("invalid chunked mihomo response size: {size_token}"))?;
        offset = line_end + 2;
        if size == 0 {
            return Ok(Vec::new());
        }
        while body.len() < offset + size + 2 {
            read_more_response_body(stream, body, path).await?;
        }
        let chunk = body[offset..offset + size].to_vec();
        return Ok(first_json_frame(&chunk).unwrap_or_else(|| trim_ascii(&chunk).to_vec()));
    }
}

async fn read_latest_chunked_frame<S>(
    stream: &mut S,
    body: &mut Vec<u8>,
    path: &str,
    max_frames: usize,
) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut offset = 0;
    let mut latest = None;
    let mut frames = 0;
    while frames < max_frames {
        let Some(line_end) = find_subsequence(&body[offset..], b"\r\n").map(|index| offset + index) else {
            read_more_response_body(stream, body, path).await?;
            continue;
        };
        let size_line =
            std::str::from_utf8(&body[offset..line_end]).context("invalid chunked mihomo response size line")?;
        let size_token = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_token, 16)
            .with_context(|| format!("invalid chunked mihomo response size: {size_token}"))?;
        offset = line_end + 2;
        if size == 0 {
            break;
        }
        while body.len() < offset + size + 2 {
            read_more_response_body(stream, body, path).await?;
        }
        let chunk = body[offset..offset + size].to_vec();
        latest = Some(first_json_frame(&chunk).unwrap_or_else(|| trim_ascii(&chunk).to_vec()));
        offset += size + 2;
        frames += 1;
    }
    latest.context("mihomo stream did not yield a response frame")
}

async fn read_first_json_frame<S>(stream: &mut S, body: &mut Vec<u8>, path: &str) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    loop {
        if let Some(frame) = first_json_frame(body) {
            return Ok(frame);
        }
        let read = read_more_response_body(stream, body, path).await?;
        if read == 0 {
            return Ok(trim_ascii(body).to_vec());
        }
    }
}

async fn read_more_response_body<S>(stream: &mut S, body: &mut Vec<u8>, path: &str) -> Result<usize>
where
    S: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 4096];
    let read = stream
        .read(&mut buffer)
        .await
        .with_context(|| format!("failed to read mihomo response {path}"))?;
    body.extend_from_slice(&buffer[..read]);
    Ok(read)
}

fn first_json_frame(body: &[u8]) -> Option<Vec<u8>> {
    let body = trim_ascii(body);
    let first = *body.first()?;
    if first != b'{' && first != b'[' {
        let line_end = body.iter().position(|byte| *byte == b'\n')?;
        return Some(trim_ascii(&body[..line_end]).to_vec());
    }

    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in body.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(body[..=index].to_vec());
                }
            }
            _ => {}
        }
    }
    None
}

fn trim_ascii(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &value[start..end]
}

fn header_value(line: &str, expected_name: &str) -> Option<String> {
    let (name, value) = line.split_once(':')?;
    name.eq_ignore_ascii_case(expected_name)
        .then(|| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn decode_chunked_body(body: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    let mut offset = 0;

    loop {
        let Some(line_end) = find_subsequence(&body[offset..], b"\r\n").map(|index| offset + index) else {
            bail!("invalid chunked mihomo response: missing chunk size terminator");
        };
        let size_line =
            std::str::from_utf8(&body[offset..line_end]).context("invalid chunked mihomo response size line")?;
        let size_token = size_line.split(';').next().unwrap_or_default().trim();
        if size_token.is_empty() {
            bail!("invalid chunked mihomo response: empty chunk size");
        }
        let size = usize::from_str_radix(size_token, 16)
            .with_context(|| format!("invalid chunked mihomo response size: {size_token}"))?;
        offset = line_end + 2;

        if size == 0 {
            return Ok(decoded);
        }

        let chunk_end = offset
            .checked_add(size)
            .context("invalid chunked mihomo response: chunk size overflow")?;
        if body.len() < chunk_end + 2 {
            bail!("invalid chunked mihomo response: chunk exceeds body length");
        }
        decoded.extend_from_slice(&body[offset..chunk_end]);
        if body.get(chunk_end..chunk_end + 2) != Some(b"\r\n") {
            bail!("invalid chunked mihomo response: missing chunk terminator");
        }
        offset = chunk_end + 2;
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use anyhow::Result;
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::TcpListener,
    };

    use super::{MihomoClient as _, MihomoClientConfig, MihomoHttpMethod, SimpleMihomoClient, parse_http_response};

    #[test]
    fn tcp_config_keeps_timeout_and_secret() {
        let addr = SocketAddr::from(([127, 0, 0, 1], 9097));
        let config = MihomoClientConfig::tcp(addr)
            .with_secret("secret")
            .with_timeout(Duration::from_secs(2));

        assert_eq!(config.secret.as_deref(), Some("secret"));
        assert_eq!(config.timeout(), Duration::from_secs(2));
    }

    #[test]
    fn http_response_parser_keeps_controller_status() -> Result<()> {
        let response = parse_http_response(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")?;

        assert_eq!(response.status, 401);
        Ok(())
    }

    #[test]
    fn http_response_parser_decodes_chunked_body() -> Result<()> {
        let response = parse_http_response(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: gzip, Chunked\r\n\r\n4;ext=value\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n",
        )?;

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type.as_deref(), Some("application/json"));
        assert_eq!(response.body, b"Wikipedia");
        Ok(())
    }

    #[tokio::test]
    async fn tcp_client_reads_version() -> Result<()> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .map_err(anyhow::Error::from)?;
        let addr = listener.local_addr()?;

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut request = vec![0; 1024];
            let read = stream.read(&mut request).await?;
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /version HTTP/1.1"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 22\r\n\r\n{\"version\":\"1.19.25\"}")
                .await?;

            Ok::<(), anyhow::Error>(())
        });

        let client = SimpleMihomoClient::new(MihomoClientConfig::tcp(addr).with_timeout(Duration::from_secs(1)));
        let version = client.version().await?;

        assert_eq!(version.version, "1.19.25");
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn tcp_client_times_out_when_controller_does_not_respond() -> Result<()> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .map_err(anyhow::Error::from)?;
        let addr = listener.local_addr()?;

        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await?;
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok::<(), anyhow::Error>(())
        });

        let client = SimpleMihomoClient::new(MihomoClientConfig::tcp(addr).with_timeout(Duration::from_millis(30)));
        let error = match client.version().await {
            Ok(_) => anyhow::bail!("controller request unexpectedly succeeded"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("timed out after 30ms"));
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn tcp_client_forwards_method_headers_and_body() -> Result<()> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .map_err(anyhow::Error::from)?;
        let addr = listener.local_addr()?;

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let request = read_http_request(&mut stream).await?;
            let request_text = String::from_utf8_lossy(&request);
            assert!(request_text.starts_with("PATCH /proxies/Proxy%20Name?force=true HTTP/1.1"));
            assert!(request_text.contains("Authorization: Bearer secret"));
            assert!(request_text.contains("Content-Type: application/json"));
            assert!(request_text.ends_with(r#"{"name":"DIRECT"}"#));
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await?;

            Ok::<(), anyhow::Error>(())
        });

        let client = SimpleMihomoClient::new(
            MihomoClientConfig::tcp(addr)
                .with_secret("secret")
                .with_timeout(Duration::from_secs(1)),
        );
        let response = client
            .request_rest(
                MihomoHttpMethod::Patch,
                "/proxies/Proxy%20Name?force=true",
                br#"{"name":"DIRECT"}"#.to_vec(),
                Some("application/json"),
            )
            .await?;

        assert_eq!(response.status, 204);
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn tcp_client_reads_first_stream_snapshot_chunk() -> Result<()> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .map_err(anyhow::Error::from)?;
        let addr = listener.local_addr()?;

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let request = read_http_request(&mut stream).await?;
            let request_text = String::from_utf8_lossy(&request);
            assert!(request_text.starts_with("GET /memory HTTP/1.1"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n10\r\n{\"inuse\":12345}\n\r\n",
                )
                .await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok::<(), anyhow::Error>(())
        });

        let client = SimpleMihomoClient::new(MihomoClientConfig::tcp(addr).with_timeout(Duration::from_secs(1)));
        let response: serde_json::Value = client.request_json_snapshot("/memory").await?;

        assert_eq!(response.get("inuse").and_then(serde_json::Value::as_u64), Some(12345));
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn tcp_client_reads_latest_stream_sample_chunk() -> Result<()> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .map_err(anyhow::Error::from)?;
        let addr = listener.local_addr()?;

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let request = read_http_request(&mut stream).await?;
            let request_text = String::from_utf8_lossy(&request);
            assert!(request_text.starts_with("GET /memory HTTP/1.1"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\nB\r\n{\"inuse\":0}\r\n11\r\n{\"inuse\":1048576}\r\n",
                )
                .await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok::<(), anyhow::Error>(())
        });

        let client = SimpleMihomoClient::new(MihomoClientConfig::tcp(addr).with_timeout(Duration::from_secs(1)));
        let response: serde_json::Value = client.request_json_stream_latest("/memory", 2).await?;

        assert_eq!(response.get("inuse").and_then(serde_json::Value::as_u64), Some(1048576));
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn tcp_client_streams_chunked_json_frames() -> Result<()> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .map_err(anyhow::Error::from)?;
        let addr = listener.local_addr()?;

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let request = read_http_request(&mut stream).await?;
            let request_text = String::from_utf8_lossy(&request);
            assert!(request_text.starts_with("GET /traffic HTTP/1.1"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await?;
            write_chunk(&mut stream, br#"{"up":1,"down":2}"#).await?;
            write_chunk(&mut stream, br#"{"up":30,"down":40}"#).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok::<(), anyhow::Error>(())
        });

        let client = SimpleMihomoClient::new(MihomoClientConfig::tcp(addr).with_timeout(Duration::from_secs(1)));
        let mut stream = client.request_json_stream::<serde_json::Value>("/traffic").await?;
        let first = stream.next().await.transpose()?.expect("first frame");
        let second = stream.next().await.transpose()?.expect("second frame");

        assert_eq!(first.get("up").and_then(serde_json::Value::as_u64), Some(1));
        assert_eq!(first.get("down").and_then(serde_json::Value::as_u64), Some(2));
        assert_eq!(second.get("up").and_then(serde_json::Value::as_u64), Some(30));
        assert_eq!(second.get("down").and_then(serde_json::Value::as_u64), Some(40));
        server.abort();
        Ok(())
    }

    async fn write_chunk(stream: &mut tokio::net::TcpStream, body: &[u8]) -> Result<()> {
        stream.write_all(format!("{:X}\r\n", body.len()).as_bytes()).await?;
        stream.write_all(body).await?;
        stream.write_all(b"\r\n").await?;
        Ok(())
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Result<Vec<u8>> {
        let mut request = Vec::new();
        loop {
            let mut buffer = [0; 1024];
            let read = stream.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = super::find_subsequence(&request, b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or_default();
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
        Ok(request)
    }
}
