// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Typed, retrying client for the Hamstik Public API v1.
//!
//! [`HamstikApi`] is the single trait the CLI depends on (a mock lives beside
//! it in the CLI's dev-dependencies). [`HamstikClient`] is the production
//! implementation built on `reqwest`. All transport concerns — authentication,
//! idempotency, ETag capture, retry/backoff, error mapping, and request-id
//! propagation — are handled centrally in `send_with_retry`; the trait methods
//! only assemble request shapes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, HeaderMap, HeaderName, HeaderValue,
    IF_MATCH, LOCATION,
};
use reqwest::redirect::Policy as RedirectPolicy;
use reqwest::{Certificate, Client, Method, Response};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{ApiError, ClientError};
use crate::host::Host;
use crate::models::*;
use crate::retry::{
    MAX_RETRY_AFTER, RetryPolicy, SharedSleeper, TokioSleeper, backoff_delay, is_retryable_status,
    parse_retry_after,
};

/// The largest response body accepted from the server (10 MiB).
///
/// Anything larger is rejected instead of buffered: a compromised host (or any
/// intermediate that can spoof responses) must not be able to exhaust memory.
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

fn header_etag_name() -> HeaderName {
    HeaderName::from_static("etag")
}
fn header_idempotency_key() -> HeaderName {
    HeaderName::from_static("idempotency-key")
}
fn header_idempotency_replayed() -> HeaderName {
    HeaderName::from_static("idempotency-replayed")
}
fn header_request_id() -> HeaderName {
    HeaderName::from_static("x-request-id")
}
fn header_retry_after() -> HeaderName {
    HeaderName::from_static("retry-after")
}
fn header_rate_limit_limit() -> HeaderName {
    HeaderName::from_static("ratelimit-limit")
}
fn header_rate_limit_remaining() -> HeaderName {
    HeaderName::from_static("ratelimit-remaining")
}
fn header_rate_limit_reset() -> HeaderName {
    HeaderName::from_static("ratelimit-reset")
}
fn header_content_disposition() -> HeaderName {
    HeaderName::from_static("content-disposition")
}
fn header_content_type() -> HeaderName {
    HeaderName::from_static("content-type")
}

/// Extracts a UTF-8 file name from a `Content-Disposition` header value
/// (`filename=...` or RFC 5987 `filename*=UTF-8''...`).
fn file_name_from_disposition(value: Option<&str>) -> Option<String> {
    let value = value?;
    let star = value.split(';').map(str::trim).find_map(|part| {
        part.strip_prefix("filename*=")
            .and_then(|v| v.rsplit_once("''"))
            .map(|(_, encoded)| encoded.to_string())
    });
    if let Some(encoded) = star {
        return percent_decode(&encoded);
    }
    let plain = value
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("filename="))?
        .trim_matches('"')
        .to_string();
    if plain.is_empty() { None } else { Some(plain) }
}

/// Minimal percent-decoding for RFC 5987 file names.
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            let byte = u8::from_str_radix(hex, 16).ok()?;
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn header_str<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Removes control characters from a server-supplied string before it is
/// rendered or echoed.
///
/// The value is attacker-influenced (a hostile server, or any machine that can
/// intercept TLS-less traffic); terminal escape sequences must not survive into
/// CLI output. Horizontal tab is preserved (it is harmless and common in
/// messages); every other control character, including ESC, CR, and NUL, is
/// dropped, which neutralizes escape sequences by removing the introducer. The
/// `code` and `message` fields are also constrained by the API contract to
/// printable characters, so this can only ever be defense-in-depth.
#[must_use]
pub fn sanitize_server_text(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// A successful, decoded API response.
#[derive(Debug, Clone)]
pub struct ApiResponse<T> {
    /// The typed, deserialized body used for human rendering.
    pub value: T,
    /// The raw JSON body, echoed verbatim for `--json` fidelity.
    pub raw: Value,
    /// Correlation id (`requestId` from the envelope, or `X-Request-Id`).
    pub request_id: Option<String>,
    /// The `ETag` header, when the endpoint returns one.
    pub etag: Option<String>,
    /// True when the server signaled this was an idempotent replay.
    pub idempotency_replayed: bool,
    /// Resource location returned by a create operation, when present.
    pub location: Option<String>,
    /// The server's rate-limit snapshot, when the response carried the
    /// documented `RateLimit-*` headers.
    pub rate_limit: Option<RateLimitSnapshot>,
}

/// The server's per-origin rate-limit state from `RateLimit-*` headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitSnapshot {
    /// The request budget per window (`RateLimit-Limit`).
    pub limit: u64,
    /// Requests remaining in the current window (`RateLimit-Remaining`).
    pub remaining: u64,
    /// Seconds until the window resets (`RateLimit-Reset`).
    pub reset_in: u64,
}

/// A downloaded attachment's bytes plus the file name suggested by the server.
#[derive(Debug, Clone)]
pub struct DownloadedAttachment {
    /// The attachment's original bytes.
    pub bytes: Vec<u8>,
    /// The file name from `Content-Disposition`, when the server sent one.
    pub file_name: Option<String>,
    /// The declared `Content-Type`, when the server sent one.
    pub content_type: Option<String>,
    /// Raw `Content-Disposition` header, when the server sent one.
    pub content_disposition: Option<String>,
    /// Declared response length, when the header was present and valid.
    pub content_length: Option<u64>,
    /// Cache policy returned for cacheable binary resources such as avatars.
    pub cache_control: Option<String>,
    /// Correlation id (`X-Request-Id` header).
    pub request_id: Option<String>,
}

/// A file to upload as a `multipart/form-data` part.
#[derive(Debug, Clone)]
pub struct MultipartFile {
    /// The file name recorded with the upload.
    pub file_name: String,
    /// The MIME type; `None` uses `application/octet-stream`.
    pub content_type: Option<String>,
    /// The file bytes.
    pub bytes: Vec<u8>,
}

/// Construction options for [`HamstikClient`].
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Timeout for establishing a TCP/TLS connection.
    pub connect_timeout: Duration,
    /// Total timeout for one request, including body read.
    pub request_timeout: Duration,
    /// The `User-Agent` sent with every request.
    pub user_agent: String,
    /// Retry/backoff policy; [`RetryPolicy::none`] disables retries.
    pub retry: RetryPolicy,
    /// Additional PEM root certificates (from `--ca-bundle` / env).
    pub ca_pem: Vec<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            user_agent: String::from("hamstik-cli"),
            retry: RetryPolicy::default(),
            ca_pem: Vec::new(),
        }
    }
}

/// The production [`HamstikApi`] implementation.
#[derive(Clone)]
pub struct HamstikClient {
    http: Client,
    host: Host,
    token: Option<SecretString>,
    policy: RetryPolicy,
    sleeper: SharedSleeper,
}

impl HamstikClient {
    /// Builds a client using the real Tokio sleeper.
    pub fn new(host: Host, token: SecretString, config: ClientConfig) -> Result<Self, ClientError> {
        Self::with_sleeper(host, token, config, Arc::new(TokioSleeper))
    }

    /// Builds a client without a bearer credential for the unauthenticated
    /// `getOpenApi` operation.
    pub fn new_public(host: Host, config: ClientConfig) -> Result<Self, ClientError> {
        Self::build_client(host, None, config, Arc::new(TokioSleeper))
    }

    /// Builds a client with an injected sleeper (tests pass a no-op sleeper).
    pub fn with_sleeper(
        host: Host,
        token: SecretString,
        config: ClientConfig,
        sleeper: SharedSleeper,
    ) -> Result<Self, ClientError> {
        Self::build_client(host, Some(token), config, sleeper)
    }

    fn build_client(
        host: Host,
        token: Option<SecretString>,
        config: ClientConfig,
        sleeper: SharedSleeper,
    ) -> Result<Self, ClientError> {
        let mut builder = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .user_agent(config.user_agent.clone())
            .use_rustls_tls()
            // This client talks to one fixed origin with a bearer token. It
            // must never follow a redirect to another host: the response would
            // be attributed to the original host and the token-bearing
            // handshake would be replayed elsewhere.
            .redirect(RedirectPolicy::none());

        for pem in &config.ca_pem {
            let cert = Certificate::from_pem(pem.as_bytes())
                .map_err(|err| ClientError::Protocol(format!("invalid CA bundle: {err}")))?;
            builder = builder.add_root_certificate(cert);
        }

        let http = builder
            .build()
            .map_err(|err| ClientError::Protocol(format!("failed to build HTTP client: {err}")))?;

        Ok(Self {
            http,
            host,
            token,
            policy: config.retry,
            sleeper,
        })
    }

    async fn send_json<T>(&self, spec: RequestSpec<'_>) -> Result<ApiResponse<T>, ClientError>
    where
        T: DeserializeOwned,
    {
        self.send_json_with_auth(spec, true).await
    }

    async fn send_json_with_auth<T>(
        &self,
        spec: RequestSpec<'_>,
        authenticated: bool,
    ) -> Result<ApiResponse<T>, ClientError>
    where
        T: DeserializeOwned,
    {
        let (response, _) = self
            .send_with_retry(spec.retryable, || self.build(&spec, authenticated))
            .await?;
        let finalized = self.finalize(response).await?;
        self.absorb_depleted_window(finalized.rate_limit).await;
        Ok(finalized)
    }

    /// Sends one request, retrying transient failures under the policy.
    ///
    /// `build` is invoked per attempt: a retried request is always freshly
    /// constructed. Returns the final response together with whether at least
    /// one retry was performed, which lets callers distinguish "the server
    /// says 404" from "a retry found it already gone".
    async fn send_with_retry(
        &self,
        retryable: bool,
        mut build: impl FnMut() -> Result<reqwest::RequestBuilder, ClientError>,
    ) -> Result<(Response, bool), ClientError> {
        let mut attempt = 0u32;
        loop {
            let request = build()?;
            let response = match request.send().await {
                Ok(response) => response,
                Err(err) => {
                    let transient = err.is_connect() || err.is_timeout();
                    if retryable && transient && attempt + 1 < self.policy.attempts {
                        self.sleeper
                            .sleep(backoff_delay(&self.policy, attempt))
                            .await;
                        attempt += 1;
                        continue;
                    }
                    return Err(ClientError::Network(err.to_string()));
                }
            };
            if let Some(response) = self.retry_or_pass(response, retryable, attempt).await? {
                return Ok((response, attempt > 0));
            }
            attempt += 1;
        }
    }

    /// Decides whether a response should be retried.
    ///
    /// When a retry is warranted, sleeps (honoring `Retry-After`) and returns
    /// `None`, consuming the response. Otherwise returns the response for
    /// finalization. A `429` whose `Retry-After` exceeds [`MAX_RETRY_AFTER`]
    /// surfaces the rate-limit error immediately instead of blocking.
    ///
    /// When the server reports a zero `RateLimit-Remaining` on a successful
    /// response, a bounded proactive wait (capped at [`MAX_RETRY_AFTER`])
    /// absorbs the window reset instead of sending the next request straight
    /// into a guaranteed `429`.
    async fn retry_or_pass(
        &self,
        response: Response,
        retryable: bool,
        attempt: u32,
    ) -> Result<Option<Response>, ClientError> {
        let status = response.status().as_u16();
        let last_attempt = attempt + 1 >= self.policy.attempts;
        if !retryable || last_attempt || !is_retryable_status(status) {
            return Ok(Some(response));
        }
        let wait = match status {
            429 => match retry_after_from(&response) {
                Some(delay) if delay > MAX_RETRY_AFTER => {
                    // Server asks us to wait longer than we are willing:
                    // surface the rate-limit error immediately (exit 7).
                    return Err(self.to_api_error(response).await);
                }
                Some(delay) => delay,
                None => backoff_delay(&self.policy, attempt),
            },
            _ => backoff_delay(&self.policy, attempt),
        };
        self.sleeper.sleep(wait).await;
        Ok(None)
    }

    /// Waits out a depleted rate-limit window when the server reports one
    /// proactively on a success response.
    ///
    /// Only fires when `RateLimit-Remaining` is present and zero; the wait is
    /// the window reset capped at [`MAX_RETRY_AFTER`] so a pathological
    /// `RateLimit-Reset` never stalls the CLI.
    async fn absorb_depleted_window(&self, rate_limit: Option<RateLimitSnapshot>) {
        let Some(snapshot) = rate_limit else {
            return;
        };
        if snapshot.remaining > 0 || snapshot.reset_in == 0 {
            return;
        }
        let wait = Duration::from_secs(snapshot.reset_in).min(MAX_RETRY_AFTER);
        self.sleeper.sleep(wait).await;
    }

    fn build(
        &self,
        spec: &RequestSpec<'_>,
        authenticated: bool,
    ) -> Result<reqwest::RequestBuilder, ClientError> {
        let segments: Vec<&str> = spec.segments.iter().map(String::as_str).collect();
        let url = self.host.resource_url(&segments)?;

        let mut builder = self.http.request(spec.method.clone(), url);
        if authenticated {
            let token = self.token.as_ref().ok_or_else(|| {
                ClientError::Protocol("this operation requires an authenticated client".into())
            })?;
            let auth = HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
                .map_err(|_| ClientError::Protocol("token contains invalid characters".into()))?;
            builder = builder.header(AUTHORIZATION, auth);
        }
        if !spec.headers.iter().any(|(name, _)| name == ACCEPT) {
            builder = builder.header(ACCEPT, HeaderValue::from_static("application/json"));
        }

        for (name, value) in &spec.headers {
            let header = HeaderValue::from_str(value)
                .map_err(|_| ClientError::Protocol(format!("invalid value for header {name}")))?;
            builder = builder.header(name.clone(), header);
        }

        if !spec.query.is_empty() {
            builder = builder.query(&spec.query);
        }
        if let Some(body) = spec.body {
            builder = builder.json(body);
        }
        Ok(builder)
    }

    /// Sends a request expecting a structured error or `204 No Content`.
    async fn send_void(&self, spec: RequestSpec<'_>) -> Result<ApiResponse<()>, ClientError> {
        let (response, retried) = self
            .send_with_retry(spec.retryable, || self.build(&spec, true))
            .await?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            return self.finalize(response).await;
        }
        if retried && status == 404 {
            // Every `send_void` caller is a DELETE. A 404 observed on a retry
            // means the resource existed when the first attempt was sent and
            // is gone now: the delete has taken effect (either attempt one
            // applied it, or another actor removed it), so the desired end
            // state holds. Surfacing NOT_FOUND here would be a false failure.
            // A first-attempt 404 never retries and still reports NOT_FOUND.
            return Ok(ApiResponse {
                value: (),
                raw: Value::Null,
                request_id: None,
                etag: None,
                idempotency_replayed: false,
                location: None,
                rate_limit: None,
            });
        }
        Err(self.to_api_error(response).await)
    }

    /// Sends a request expecting binary bytes (attachment download), capping
    /// the body at [`MAX_BODY_BYTES`].
    async fn send_bytes(&self, spec: RequestSpec<'_>) -> Result<DownloadedAttachment, ClientError> {
        let (response, _) = self
            .send_with_retry(spec.retryable, || self.build(&spec, true))
            .await?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(self.to_api_error(response).await);
        }
        let headers = response.headers().clone();
        let bytes = read_body_capped(response).await?;
        let content_disposition =
            header_str(&headers, &header_content_disposition()).map(str::to_string);
        Ok(DownloadedAttachment {
            bytes: bytes.to_vec(),
            file_name: file_name_from_disposition(content_disposition.as_deref()),
            content_type: header_str(&headers, &header_content_type()).map(str::to_string),
            content_disposition,
            content_length: header_str(&headers, &CONTENT_LENGTH)
                .and_then(|value| value.parse().ok()),
            cache_control: header_str(&headers, &CACHE_CONTROL).map(str::to_string),
            request_id: header_str(&headers, &header_request_id())
                .map(sanitize_server_text)
                .filter(|id| !id.is_empty()),
        })
    }

    /// Uploads a file as `multipart/form-data` with a single `file` part.
    async fn send_multipart(
        &self,
        spec: &RequestSpec<'_>,
        file_name: &str,
        content_type: Option<&str>,
        bytes: Vec<u8>,
    ) -> Result<ApiResponse<Attachment>, ClientError> {
        let (response, _) = self
            .send_with_retry(spec.retryable, || {
                let form = reqwest::multipart::Form::new().part(
                    "file",
                    reqwest::multipart::Part::bytes(bytes.clone())
                        .file_name(file_name.to_string())
                        .mime_str(content_type.unwrap_or("application/octet-stream"))
                        .map_err(|err| {
                            ClientError::Protocol(format!("invalid content type: {err}"))
                        })?,
                );
                self.build_multipart(spec, form)
            })
            .await?;
        self.finalize(response).await
    }

    fn build_multipart(
        &self,
        spec: &RequestSpec<'_>,
        form: reqwest::multipart::Form,
    ) -> Result<reqwest::RequestBuilder, ClientError> {
        let segments: Vec<&str> = spec.segments.iter().map(String::as_str).collect();
        let url = self.host.resource_url(&segments)?;

        let token = self.token.as_ref().ok_or_else(|| {
            ClientError::Protocol("this operation requires an authenticated client".into())
        })?;
        let auth = HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
            .map_err(|_| ClientError::Protocol("token contains invalid characters".into()))?;

        let mut builder = self
            .http
            .request(spec.method.clone(), url)
            .header(AUTHORIZATION, auth);
        if !spec.headers.iter().any(|(name, _)| name == ACCEPT) {
            builder = builder.header(ACCEPT, HeaderValue::from_static("application/json"));
        }

        for (name, value) in &spec.headers {
            let header = HeaderValue::from_str(value)
                .map_err(|_| ClientError::Protocol(format!("invalid value for header {name}")))?;
            builder = builder.header(name.clone(), header);
        }

        if !spec.query.is_empty() {
            builder = builder.query(&spec.query);
        }
        Ok(builder.multipart(form))
    }

    async fn finalize<T>(&self, response: Response) -> Result<ApiResponse<T>, ClientError>
    where
        T: DeserializeOwned,
    {
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = read_body_capped(response).await?;

        let raw: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .map_err(|err| ClientError::Protocol(format!("invalid JSON response: {err}")))?
        };

        if (200..300).contains(&status) {
            // Deserialize directly from the raw value; `&Value` implements
            // `Deserializer`, so the whole body never needs to be cloned.
            let value: T = T::deserialize(&raw).map_err(|err| {
                ClientError::Protocol(format!("response does not match expected shape: {err}"))
            })?;
            let request_id = extract_request_id(&raw, &headers);
            let etag = header_str(&headers, &header_etag_name()).map(str::to_string);
            let idempotency_replayed = header_str(&headers, &header_idempotency_replayed())
                .is_some_and(|v| !v.eq_ignore_ascii_case("false"));
            Ok(ApiResponse {
                value,
                raw,
                request_id,
                etag,
                idempotency_replayed,
                location: header_str(&headers, &LOCATION).map(str::to_string),
                rate_limit: rate_limit_snapshot(&headers),
            })
        } else {
            Err(self.api_error_from(status, &raw, &headers))
        }
    }

    async fn to_api_error(&self, response: Response) -> ClientError {
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        // Best effort: a body that fails the cap or the parse still yields a
        // status-only API error.
        let bytes = read_body_capped(response).await.unwrap_or_default();
        let raw: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        self.api_error_from(status, &raw, &headers)
    }

    fn api_error_from(&self, status: u16, raw: &Value, headers: &HeaderMap) -> ClientError {
        let error = raw.get("error");
        let code = error
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            .map(sanitize_server_text)
            .filter(|code| !code.is_empty())
            .unwrap_or_else(|| default_code_for_status(status).to_string());
        let message = error
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .map(sanitize_server_text)
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| format!("request failed with status {status}"));
        let retry_after = header_str(headers, &header_retry_after())
            .and_then(|v| parse_retry_after(v, SystemTime::now()));

        ClientError::Api(ApiError {
            status,
            code,
            message,
            request_id: extract_request_id(raw, headers),
            field_errors: parse_field_errors(error.and_then(|e| e.get("fieldErrors"))),
            details: parse_details(error.and_then(|e| e.get("details"))),
            retry_after,
            rate_limit: rate_limit_snapshot(headers),
        })
    }
}

fn retry_after_from(response: &Response) -> Option<Duration> {
    header_str(response.headers(), &header_retry_after())
        .and_then(|v| parse_retry_after(v, SystemTime::now()))
}

/// Reads the documented `RateLimit-*` headers into a snapshot.
///
/// Returns `None` unless all three headers are present and parseable, so a
/// missing or partial set never misleads callers.
fn rate_limit_snapshot(headers: &HeaderMap) -> Option<RateLimitSnapshot> {
    let limit = header_str(headers, &header_rate_limit_limit())?
        .trim()
        .parse::<u64>()
        .ok()?;
    let remaining = header_str(headers, &header_rate_limit_remaining())?
        .trim()
        .parse::<u64>()
        .ok()?;
    let reset_in = header_str(headers, &header_rate_limit_reset())?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(RateLimitSnapshot {
        limit,
        remaining,
        reset_in,
    })
}

/// Reads a response body, enforcing [`MAX_BODY_BYTES`].
///
/// `Content-Length` above the cap is rejected before reading. When the length
/// is unknown, the body is streamed chunk-by-chunk and the read aborts as soon
/// as the running total would exceed the cap, so an oversized body never
/// buffers in memory.
async fn read_body_capped(response: Response) -> Result<bytes::Bytes, ClientError> {
    if response
        .content_length()
        .is_some_and(|length| usize::try_from(length).is_ok_and(|len| len > MAX_BODY_BYTES))
    {
        return Err(ClientError::Protocol(format!(
            "response body exceeds the {MAX_BODY_BYTES} byte limit"
        )));
    }

    let mut stream = response;
    let mut buffer = bytes::BytesMut::with_capacity(8 * 1024);
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|err| ClientError::Network(err.to_string()))?
    {
        if buffer.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(ClientError::Protocol(format!(
                "response body exceeds the {MAX_BODY_BYTES} byte limit"
            )));
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer.freeze())
}

fn extract_request_id(raw: &Value, headers: &HeaderMap) -> Option<String> {
    raw.get("requestId")
        .and_then(Value::as_str)
        .or_else(|| header_str(headers, &header_request_id()))
        .map(sanitize_server_text)
        .filter(|id| !id.is_empty())
}

fn parse_field_errors(value: Option<&Value>) -> BTreeMap<String, Vec<String>> {
    let mut map = BTreeMap::new();
    if let Some(object) = value.and_then(Value::as_object) {
        for (key, values) in object {
            let list = values
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(sanitize_server_text)
                        .collect()
                })
                .unwrap_or_default();
            map.insert(sanitize_server_text(key), list);
        }
    }
    map
}

fn parse_details(value: Option<&Value>) -> Option<BTreeMap<String, Value>> {
    value.and_then(Value::as_object).map(|object| {
        object
            .iter()
            .map(|(key, value)| (sanitize_server_text(key), value.clone()))
            .collect()
    })
}

fn default_code_for_status(status: u16) -> &'static str {
    match status {
        400 => "VALIDATION_ERROR",
        401 => "AUTH_REQUIRED",
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
        409 => "CONFLICT",
        412 => "REVISION_CONFLICT",
        413 => "PAYLOAD_TOO_LARGE",
        428 => "PRECONDITION_REQUIRED",
        429 => "RATE_LIMITED",
        500 => "INTERNAL_ERROR",
        _ => "INTERNAL_ERROR",
    }
}

/// Internal, transport-level description of a single logical request.
struct RequestSpec<'a> {
    method: Method,
    segments: Vec<String>,
    query: Vec<(String, String)>,
    headers: Vec<(HeaderName, String)>,
    body: Option<&'a Value>,
    retryable: bool,
}

fn push_list(query: &mut Vec<(String, String)>, opts: &ListOptions) {
    if let Some(limit) = opts.limit {
        query.push(("limit".to_string(), limit.to_string()));
    }
    if let Some(cursor) = &opts.cursor {
        query.push(("cursor".to_string(), cursor.clone()));
    }
}

fn push_bool(query: &mut Vec<(String, String)>, name: &str, value: Option<bool>) {
    if let Some(flag) = value {
        query.push((
            name.to_string(),
            if flag { "true" } else { "false" }.to_string(),
        ));
    }
}

fn push_scalar(query: &mut Vec<(String, String)>, name: &str, value: &Option<String>) {
    if let Some(text) = value {
        query.push((name.to_string(), text.clone()));
    }
}

fn push_repeated(query: &mut Vec<(String, String)>, name: &str, values: &[String]) {
    for value in values {
        query.push((name.to_string(), value.clone()));
    }
}

fn work_item_query(q: &ListWorkItemsQuery) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(limit) = q.limit {
        out.push(("limit".to_string(), limit.to_string()));
    }
    if let Some(cursor) = &q.cursor {
        out.push(("cursor".to_string(), cursor.clone()));
    }
    push_scalar(&mut out, "q", &q.q);
    for status in &q.status {
        out.push(("status".to_string(), status.clone()));
    }
    push_scalar(&mut out, "scope", &q.scope);
    for item_type in &q.item_type {
        out.push(("type".to_string(), item_type.clone()));
    }
    for priority in &q.priority {
        out.push(("priority".to_string(), priority.clone()));
    }
    push_scalar(&mut out, "assignee", &q.assignee);
    push_scalar(&mut out, "sprint", &q.sprint);
    push_repeated(&mut out, "label", &q.label);
    push_repeated(&mut out, "labelName", &q.label_name);
    push_scalar(&mut out, "parent", &q.parent);
    push_bool(&mut out, "topLevel", q.top_level);
    push_scalar(&mut out, "updatedAfter", &q.updated_after);
    push_repeated(&mut out, "project", &q.projects);
    push_repeated(&mut out, "organization", &q.organizations);
    push_repeated(&mut out, "involvement", &q.involvement);
    push_bool(&mut out, "overdue", q.overdue);
    push_scalar(&mut out, "dueBefore", &q.due_before);
    push_scalar(&mut out, "dueAfter", &q.due_after);
    push_scalar(&mut out, "sort", &q.sort);
    push_bool(&mut out, "archived", q.archived);
    // An empty `fields=` value is meaningful (selects the complete summary),
    // so `Some("")` must still be sent while `None` omits the parameter.
    if let Some(fields) = &q.fields {
        out.push(("fields".to_string(), fields.clone()));
    }
    out
}

fn organization_users_query(opts: &ListOrganizationUsersOptions) -> Vec<(String, String)> {
    let mut out = Vec::new();
    push_list(
        &mut out,
        &ListOptions {
            limit: opts.limit,
            cursor: opts.cursor.clone(),
        },
    );
    push_scalar(&mut out, "q", &opts.q);
    out
}

fn activity_query(opts: &ActivityOptions) -> Vec<(String, String)> {
    let mut out = Vec::new();
    push_list(
        &mut out,
        &ListOptions {
            limit: opts.limit,
            cursor: opts.cursor.clone(),
        },
    );
    push_scalar(&mut out, "since", &opts.since);
    out
}

/// The complete, typed surface of the Hamstik Public API used by the CLI.
///
/// Every method maps to exactly one endpoint; transport concerns (auth,
/// retries, idempotency, error mapping) are shared in [`HamstikClient`].
#[async_trait]
pub trait HamstikApi: Send + Sync {
    /// `GET /me`: the authenticated identity and credential context.
    async fn whoami(&self) -> Result<ApiResponse<Me>, ClientError>;
    /// `GET /openapi.json`: the unauthenticated Public API contract.
    async fn get_open_api(&self) -> Result<ApiResponse<Value>, ClientError>;
    /// `GET /organizations`: the organizations the user belongs to.
    async fn list_organizations(
        &self,
        opts: ListOptions,
    ) -> Result<ApiResponse<OrganizationList>, ClientError>;
    /// `GET /organizations/{slug}`: one organization.
    async fn get_organization(
        &self,
        org_slug: &str,
    ) -> Result<ApiResponse<Organization>, ClientError>;
    /// `GET /organizations/{slug}/projects`: the organization's projects.
    async fn list_projects(
        &self,
        org_slug: &str,
        opts: ListProjectsOptions,
    ) -> Result<ApiResponse<ProjectList>, ClientError>;
    /// `GET /organizations/{slug}/projects/{key}`: one project.
    async fn get_project(
        &self,
        org_slug: &str,
        project_key: &str,
    ) -> Result<ApiResponse<Project>, ClientError>;
    /// `GET .../work-items`: query work items with filters.
    async fn list_work_items(
        &self,
        org_slug: &str,
        project_key: &str,
        query: ListWorkItemsQuery,
    ) -> Result<ApiResponse<WorkItemList>, ClientError>;
    /// `POST .../work-items`: create a work item (idempotent).
    async fn create_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &CreateWorkItemRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError>;
    /// `GET .../work-items/{key}`: one work item (captures the `ETag`).
    async fn get_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError>;
    /// `PATCH .../work-items/{key}`: conditional update via `If-Match`.
    async fn update_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &UpdateWorkItemRequest,
        if_match: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError>;
    /// `GET .../work-items/{key}/transitions`: permitted status transitions.
    async fn list_transitions(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
    ) -> Result<ApiResponse<WorkItemTransitionList>, ClientError>;
    /// `GET .../work-items/{key}/watcher`: the authenticated user's watcher
    /// state (other watchers are never disclosed).
    async fn get_work_item_watcher(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
    ) -> Result<ApiResponse<WorkItemWatcher>, ClientError>;
    /// `POST .../work-items/{key}/watcher`: apply a watcher action to the
    /// authenticated user's watcher state (idempotent, no revision guard).
    async fn update_work_item_watcher(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        action: WatcherAction,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItemWatcher>, ClientError>;
    /// `POST .../work-items/{key}/transitions`: move the item to a status.
    async fn transition_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &TransitionRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError>;
    /// `GET .../work-items/{key}/comments`: list comments.
    async fn list_comments(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<CommentList>, ClientError>;
    /// `POST .../work-items/{key}/comments`: add a comment (idempotent).
    async fn create_comment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &CreateCommentRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Comment>, ClientError>;
    /// `DELETE .../work-items/{key}/comments/{commentId}`: remove an authored
    /// comment with no replies (204 on success).
    async fn delete_comment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        comment_id: &str,
    ) -> Result<ApiResponse<()>, ClientError>;
    /// `POST .../projects`: create a Project (Organization administrators,
    /// idempotent).
    async fn create_project(
        &self,
        org_slug: &str,
        body: &CreateProjectRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Project>, ClientError>;
    /// `GET .../sprints`: list the Project's Sprints (dated first).
    async fn list_sprints(
        &self,
        org_slug: &str,
        project_key: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<SprintList>, ClientError>;
    /// `POST .../sprints`: create a Sprint (idempotent).
    async fn create_sprint(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &CreateSprintRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Sprint>, ClientError>;
    /// `GET .../sprints/{id}`: one Sprint (captures the Sprint `ETag`).
    async fn get_sprint(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
    ) -> Result<ApiResponse<Sprint>, ClientError>;
    /// `GET .../sprints/{id}/transitions`: permitted Sprint state transitions.
    async fn list_sprint_transitions(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
    ) -> Result<ApiResponse<SprintTransitionList>, ClientError>;
    /// `POST .../sprints/{id}/transitions`: move the Sprint to a state
    /// (idempotent, `If-Match` required).
    async fn transition_sprint(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
        body: &TransitionSprintRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Sprint>, ClientError>;
    /// `GET .../labels`: list the Project's labels.
    async fn list_labels(
        &self,
        org_slug: &str,
        project_key: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<ProjectLabelList>, ClientError>;
    /// `POST .../labels`: create a Project label (idempotent).
    async fn create_label(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &CreateLabelRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ProjectLabel>, ClientError>;
    /// `POST .../work-items/{key}/labels`: attach a label (idempotent,
    /// `If-Match` required).
    async fn attach_label(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &AttachLabelRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError>;
    /// `DELETE .../work-items/{key}/labels/{labelId}`: detach a label
    /// (idempotent, `If-Match` required).
    async fn detach_label(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        label_id: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError>;
    /// `GET .../work-items/{key}/attachments`: list attachment metadata.
    async fn list_attachments(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<AttachmentList>, ClientError>;
    /// Uploads a file as `multipart/form-data` with a single `file` part.
    /// The bytes are bundled into a [`MultipartFile`] so the argument count
    /// stays reviewable. Idempotent.
    async fn upload_attachment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        file: &MultipartFile,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Attachment>, ClientError>;
    /// `GET .../work-items/{key}/attachments/{id}`: download the original
    /// bytes. Returns the bytes alongside the `Content-Disposition` file name.
    async fn download_attachment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        attachment_id: &str,
    ) -> Result<DownloadedAttachment, ClientError>;
    /// `DELETE .../work-items/{key}/attachments/{id}`: remove an attachment
    /// (creator or Organization administrator; 204 on success).
    async fn delete_attachment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        attachment_id: &str,
    ) -> Result<ApiResponse<()>, ClientError>;
    /// `GET /organizations/{slug}/users`: the member directory
    /// (`organization:members:read`).
    async fn list_organization_users(
        &self,
        org_slug: &str,
        opts: ListOrganizationUsersOptions,
    ) -> Result<ApiResponse<OrganizationUserList>, ClientError>;
    /// `GET /organizations/{slug}/work-items`: Work Items with Organization
    /// and Project context. Only the fields the endpoint accepts may be set.
    async fn list_organization_work_items(
        &self,
        org_slug: &str,
        query: ListWorkItemsQuery,
    ) -> Result<ApiResponse<WorkItemContextList>, ClientError>;
    /// Read-only `POST /organizations/{slug}/work-items/search`: execute a
    /// SqueakQL expression. This operation never sends an idempotency key.
    async fn search_organization_work_items_with_squeakql(
        &self,
        org_slug: &str,
        body: &SqueakQlSearchRequest,
    ) -> Result<ApiResponse<WorkItemContextList>, ClientError>;
    /// Read-only `POST /organizations/{slug}/squeakql/validate`: validate a
    /// SqueakQL expression without executing it or sending an idempotency key.
    async fn validate_squeakql(
        &self,
        org_slug: &str,
        body: &SqueakQlValidateRequest,
    ) -> Result<ApiResponse<SqueakQlValidationResponse>, ClientError>;
    /// `GET /my/work`: Work Items assigned to the PAT owner.
    async fn list_my_work(
        &self,
        query: ListWorkItemsQuery,
    ) -> Result<ApiResponse<WorkItemContextList>, ClientError>;
    /// `GET /users/{publicId}`: an authenticated profile summary.
    async fn get_user_profile(
        &self,
        public_id: &str,
    ) -> Result<ApiResponse<UserProfile>, ClientError>;
    /// `GET /users/{publicId}/work`: visible Work Items involving the user.
    async fn list_user_profile_work(
        &self,
        public_id: &str,
        query: ListWorkItemsQuery,
    ) -> Result<ApiResponse<ProfileWorkItemList>, ClientError>;
    /// `GET /users/{publicId}/activity`: meaningful activity by the user.
    async fn list_user_profile_activity(
        &self,
        public_id: &str,
        opts: ActivityOptions,
    ) -> Result<ApiResponse<ProfileActivityList>, ClientError>;
    /// `GET /users/{publicId}/avatar`: the profile's avatar bytes, when one
    /// exists (404 otherwise).
    async fn get_user_profile_avatar(
        &self,
        public_id: &str,
        opts: AvatarOptions,
    ) -> Result<DownloadedAttachment, ClientError>;
    /// `PATCH .../projects/{key}`: update a Project (Organization
    /// administrators, `If-Match`, idempotent).
    async fn update_project(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &UpdateProjectRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Project>, ClientError>;
    /// `POST .../projects/{key}/archive`: archive a Project (idempotent,
    /// `If-Match`, empty `{}` body).
    async fn archive_project(
        &self,
        org_slug: &str,
        project_key: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Project>, ClientError>;
    /// `POST .../projects/{key}/unarchive`: unarchive a Project (idempotent,
    /// `If-Match`, empty `{}` body).
    async fn unarchive_project(
        &self,
        org_slug: &str,
        project_key: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Project>, ClientError>;
    /// `GET .../work-items/{key}/links`: visible links from this Work Item.
    async fn list_work_item_links(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<WorkItemLinkList>, ClientError>;
    /// `POST .../work-items/{key}/links`: link two Work Items (idempotent).
    async fn create_work_item_link(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &CreateWorkItemLinkRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItemLink>, ClientError>;
    /// `DELETE .../work-items/{key}/links/{linkId}`: delete a link (204).
    async fn delete_work_item_link(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        link_id: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<()>, ClientError>;
    /// `GET .../work-items/{key}/activity`: chronological (oldest-first)
    /// Work Item activity.
    async fn list_work_item_activity(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        opts: ActivityOptions,
    ) -> Result<ApiResponse<ActivityList>, ClientError>;
    /// `GET .../projects/{key}/activity`: newest-first Project activity.
    async fn list_project_activity(
        &self,
        org_slug: &str,
        project_key: &str,
        opts: ActivityOptions,
    ) -> Result<ApiResponse<ProjectActivityList>, ClientError>;
    /// `DELETE .../work-items/{key}`: soft-delete a Work Item (Organization
    /// owner, `If-Match`, idempotent). `cascade` soft-deletes the descendant
    /// tree atomically.
    async fn delete_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        cascade: bool,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<()>, ClientError>;
    /// `POST .../work-items/{key}/archive`: archive a Work Item (idempotent,
    /// `If-Match`, empty `{}` body).
    async fn archive_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError>;
    /// `POST .../work-items/{key}/unarchive`: unarchive a Work Item
    /// (idempotent, `If-Match`, empty `{}` body).
    async fn unarchive_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError>;
    /// `PATCH .../comments/{commentId}`: edit an authored comment
    /// (idempotent).
    async fn update_comment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        comment_id: &str,
        body: &UpdateCommentRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Comment>, ClientError>;
    /// `POST .../bulk-work-items`: create up to 50 Work Items per request.
    async fn bulk_create_work_items(
        &self,
        org_slug: &str,
        body: &BulkCreateEnvelope,
        idempotency_key: &str,
    ) -> Result<ApiResponse<BulkResultList>, ClientError>;
    /// `PATCH .../bulk-work-items`: update up to 50 Work Items per request.
    async fn bulk_update_work_items(
        &self,
        org_slug: &str,
        body: &BulkUpdateEnvelope,
        idempotency_key: &str,
    ) -> Result<ApiResponse<BulkResultList>, ClientError>;
    /// `POST .../bulk-work-item-transitions`: transition up to 50 Work Items
    /// per request.
    async fn bulk_transition_work_items(
        &self,
        org_slug: &str,
        body: &BulkTransitionEnvelope,
        idempotency_key: &str,
    ) -> Result<ApiResponse<BulkResultList>, ClientError>;
}

#[async_trait]
impl HamstikApi for HamstikClient {
    async fn whoami(&self) -> Result<ApiResponse<Me>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec!["me".to_string()],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_open_api(&self) -> Result<ApiResponse<Value>, ClientError> {
        self.send_json_with_auth(
            RequestSpec {
                method: Method::GET,
                segments: vec!["openapi.json".to_string()],
                query: Vec::new(),
                headers: Vec::new(),
                body: None,
                retryable: true,
            },
            false,
        )
        .await
    }

    async fn list_organizations(
        &self,
        opts: ListOptions,
    ) -> Result<ApiResponse<OrganizationList>, ClientError> {
        let mut query = Vec::new();
        push_list(&mut query, &opts);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec!["organizations".to_string()],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_organization(
        &self,
        org_slug: &str,
    ) -> Result<ApiResponse<Organization>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec!["organizations".to_string(), org_slug.to_string()],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_projects(
        &self,
        org_slug: &str,
        opts: ListProjectsOptions,
    ) -> Result<ApiResponse<ProjectList>, ClientError> {
        let mut query = Vec::new();
        push_list(
            &mut query,
            &ListOptions {
                limit: opts.limit,
                cursor: opts.cursor.clone(),
            },
        );
        push_bool(&mut query, "archived", opts.archived);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_project(
        &self,
        org_slug: &str,
        project_key: &str,
    ) -> Result<ApiResponse<Project>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_work_items(
        &self,
        org_slug: &str,
        project_key: &str,
        query: ListWorkItemsQuery,
    ) -> Result<ApiResponse<WorkItemList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
            ],
            query: work_item_query(&query),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn create_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &CreateWorkItemRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn get_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_work_item_watcher(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
    ) -> Result<ApiResponse<WorkItemWatcher>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "watcher".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn update_work_item_watcher(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        action: WatcherAction,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItemWatcher>, ClientError> {
        let payload = serde_json::to_value(WorkItemWatcherActionRequest { action })
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "watcher".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn update_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &UpdateWorkItemRequest,
        if_match: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::PATCH,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
            ],
            query: Vec::new(),
            headers: vec![(IF_MATCH.clone(), if_match.to_string())],
            body: Some(&payload),
            // This revision-sensitive PATCH has no idempotency mechanism.
            retryable: false,
        })
        .await
    }

    async fn list_transitions(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
    ) -> Result<ApiResponse<WorkItemTransitionList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "transitions".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn transition_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &TransitionRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "transitions".to_string(),
            ],
            query: Vec::new(),
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn list_comments(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<CommentList>, ClientError> {
        let mut query = Vec::new();
        push_list(&mut query, &opts);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "comments".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn create_comment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &CreateCommentRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Comment>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "comments".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn delete_comment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        comment_id: &str,
    ) -> Result<ApiResponse<()>, ClientError> {
        self.send_void(RequestSpec {
            method: Method::DELETE,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "comments".to_string(),
                comment_id.to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn create_project(
        &self,
        org_slug: &str,
        body: &CreateProjectRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Project>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn list_sprints(
        &self,
        org_slug: &str,
        project_key: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<SprintList>, ClientError> {
        let mut query = Vec::new();
        push_list(&mut query, &opts);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "sprints".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn create_sprint(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &CreateSprintRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Sprint>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "sprints".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn get_sprint(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
    ) -> Result<ApiResponse<Sprint>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "sprints".to_string(),
                sprint_id.to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_sprint_transitions(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
    ) -> Result<ApiResponse<SprintTransitionList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "sprints".to_string(),
                sprint_id.to_string(),
                "transitions".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn transition_sprint(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
        body: &TransitionSprintRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Sprint>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "sprints".to_string(),
                sprint_id.to_string(),
                "transitions".to_string(),
            ],
            query: Vec::new(),
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn list_labels(
        &self,
        org_slug: &str,
        project_key: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<ProjectLabelList>, ClientError> {
        let mut query = Vec::new();
        push_list(&mut query, &opts);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "labels".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn create_label(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &CreateLabelRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ProjectLabel>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "labels".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn attach_label(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &AttachLabelRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "labels".to_string(),
            ],
            query: Vec::new(),
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn detach_label(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        label_id: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::DELETE,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "labels".to_string(),
                label_id.to_string(),
            ],
            query: Vec::new(),
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_attachments(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<AttachmentList>, ClientError> {
        let mut query = Vec::new();
        push_list(&mut query, &opts);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "attachments".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn upload_attachment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        file: &MultipartFile,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Attachment>, ClientError> {
        let spec = RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "attachments".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: None,
            retryable: true,
        };
        self.send_multipart(
            &spec,
            &file.file_name,
            file.content_type.as_deref(),
            file.bytes.clone(),
        )
        .await
    }

    async fn download_attachment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        attachment_id: &str,
    ) -> Result<DownloadedAttachment, ClientError> {
        self.send_bytes(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "attachments".to_string(),
                attachment_id.to_string(),
            ],
            query: Vec::new(),
            headers: vec![(
                ACCEPT,
                "application/octet-stream, application/json".to_string(),
            )],
            body: None,
            retryable: true,
        })
        .await
    }

    async fn delete_attachment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        attachment_id: &str,
    ) -> Result<ApiResponse<()>, ClientError> {
        self.send_void(RequestSpec {
            method: Method::DELETE,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "attachments".to_string(),
                attachment_id.to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_organization_users(
        &self,
        org_slug: &str,
        opts: ListOrganizationUsersOptions,
    ) -> Result<ApiResponse<OrganizationUserList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "users".to_string(),
            ],
            query: organization_users_query(&opts),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_organization_work_items(
        &self,
        org_slug: &str,
        query: ListWorkItemsQuery,
    ) -> Result<ApiResponse<WorkItemContextList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "work-items".to_string(),
            ],
            query: work_item_query(&query),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn search_organization_work_items_with_squeakql(
        &self,
        org_slug: &str,
        body: &SqueakQlSearchRequest,
    ) -> Result<ApiResponse<WorkItemContextList>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "work-items".to_string(),
                "search".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: Some(&payload),
            // This POST is a read-only operation and has no idempotency key.
            retryable: true,
        })
        .await
    }

    async fn validate_squeakql(
        &self,
        org_slug: &str,
        body: &SqueakQlValidateRequest,
    ) -> Result<ApiResponse<SqueakQlValidationResponse>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "squeakql".to_string(),
                "validate".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: Some(&payload),
            // Validation is read-only despite using POST.
            retryable: true,
        })
        .await
    }

    async fn list_my_work(
        &self,
        query: ListWorkItemsQuery,
    ) -> Result<ApiResponse<WorkItemContextList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec!["my".to_string(), "work".to_string()],
            query: work_item_query(&query),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_user_profile(
        &self,
        public_id: &str,
    ) -> Result<ApiResponse<UserProfile>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec!["users".to_string(), public_id.to_string()],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_user_profile_work(
        &self,
        public_id: &str,
        query: ListWorkItemsQuery,
    ) -> Result<ApiResponse<ProfileWorkItemList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "users".to_string(),
                public_id.to_string(),
                "work".to_string(),
            ],
            query: work_item_query(&query),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_user_profile_activity(
        &self,
        public_id: &str,
        opts: ActivityOptions,
    ) -> Result<ApiResponse<ProfileActivityList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "users".to_string(),
                public_id.to_string(),
                "activity".to_string(),
            ],
            query: activity_query(&opts),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_user_profile_avatar(
        &self,
        public_id: &str,
        opts: AvatarOptions,
    ) -> Result<DownloadedAttachment, ClientError> {
        let mut query = Vec::new();
        push_scalar(&mut query, "v", &opts.version);
        push_scalar(&mut query, "format", &opts.format);
        push_scalar(&mut query, "rev", &opts.revision);
        self.send_bytes(RequestSpec {
            method: Method::GET,
            segments: vec![
                "users".to_string(),
                public_id.to_string(),
                "avatar".to_string(),
            ],
            query,
            headers: vec![(
                ACCEPT,
                "image/jpeg, image/png, image/webp, application/json".to_string(),
            )],
            body: None,
            retryable: true,
        })
        .await
    }

    async fn update_project(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &UpdateProjectRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Project>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::PATCH,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
            ],
            query: Vec::new(),
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: Some(&payload),
            // The required idempotency key makes an ambiguous replay safe.
            retryable: true,
        })
        .await
    }

    async fn archive_project(
        &self,
        org_slug: &str,
        project_key: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Project>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "archive".to_string(),
            ],
            query: Vec::new(),
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: Some(&serde_json::json!({})),
            retryable: true,
        })
        .await
    }

    async fn unarchive_project(
        &self,
        org_slug: &str,
        project_key: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Project>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "unarchive".to_string(),
            ],
            query: Vec::new(),
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: Some(&serde_json::json!({})),
            retryable: true,
        })
        .await
    }

    async fn list_work_item_links(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<WorkItemLinkList>, ClientError> {
        let mut query = Vec::new();
        push_list(&mut query, &opts);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "links".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn create_work_item_link(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &CreateWorkItemLinkRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItemLink>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "links".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn delete_work_item_link(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        link_id: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<()>, ClientError> {
        self.send_void(RequestSpec {
            method: Method::DELETE,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "links".to_string(),
                link_id.to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_work_item_activity(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        opts: ActivityOptions,
    ) -> Result<ApiResponse<ActivityList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "activity".to_string(),
            ],
            query: activity_query(&opts),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_project_activity(
        &self,
        org_slug: &str,
        project_key: &str,
        opts: ActivityOptions,
    ) -> Result<ApiResponse<ProjectActivityList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "activity".to_string(),
            ],
            query: activity_query(&opts),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn delete_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        cascade: bool,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<()>, ClientError> {
        // The strict body is `{ "cascade": false }`; omitting the body has the
        // same meaning, so we always send the explicit object.
        let payload = serde_json::json!({ "cascade": cascade });
        self.send_void(RequestSpec {
            method: Method::DELETE,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
            ],
            query: Vec::new(),
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn archive_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "archive".to_string(),
            ],
            query: Vec::new(),
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: Some(&serde_json::json!({})),
            retryable: true,
        })
        .await
    }

    async fn unarchive_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "unarchive".to_string(),
            ],
            query: Vec::new(),
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: Some(&serde_json::json!({})),
            retryable: true,
        })
        .await
    }

    async fn update_comment(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        comment_id: &str,
        body: &UpdateCommentRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Comment>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::PATCH,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "comments".to_string(),
                comment_id.to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            // The required idempotency key is reused across retries.
            retryable: true,
        })
        .await
    }

    async fn bulk_create_work_items(
        &self,
        org_slug: &str,
        body: &BulkCreateEnvelope,
        idempotency_key: &str,
    ) -> Result<ApiResponse<BulkResultList>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "bulk-work-items".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn bulk_update_work_items(
        &self,
        org_slug: &str,
        body: &BulkUpdateEnvelope,
        idempotency_key: &str,
    ) -> Result<ApiResponse<BulkResultList>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::PATCH,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "bulk-work-items".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            // The required idempotency key is reused across retries.
            retryable: true,
        })
        .await
    }

    async fn bulk_transition_work_items(
        &self,
        org_slug: &str,
        body: &BulkTransitionEnvelope,
        idempotency_key: &str,
    ) -> Result<ApiResponse<BulkResultList>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "bulk-work-item-transitions".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::retry::NoopSleeper;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_client(uri: &str) -> HamstikClient {
        let host = Host::parse(uri).unwrap();
        HamstikClient::with_sleeper(
            host,
            SecretString::from("tok".to_string()),
            ClientConfig {
                retry: RetryPolicy::none(),
                ..ClientConfig::default()
            },
            Arc::new(NoopSleeper),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn oversized_body_is_rejected_not_buffered() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; MAX_BODY_BYTES + 1]))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let err = client.whoami().await.unwrap_err();
        assert!(
            matches!(err, ClientError::Protocol(ref m) if m.contains("exceeds")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/elsewhere"))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        // A redirect must surface as its own status (protocol error), never be
        // silently followed: the token must not travel to another origin.
        assert!(client.whoami().await.is_err());
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "redirect must not be followed");
    }

    #[tokio::test]
    async fn rate_limit_headers_are_captured_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("RateLimit-Limit", "100")
                    .insert_header("RateLimit-Remaining", "37")
                    .insert_header("RateLimit-Reset", "12")
                    .set_body_json(serde_json::json!({
                        "id": "u1", "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg",
                        "name": "N", "email": "n@x",
                        "authentication": {"type": "pat", "credentialId": "c",
                            "credentialName": "n", "scopes": [],
                            "expiresAt": "2027-01-01T00:00:00Z"},
                        "defaultOrganization": null,
                        "organizations": []
                    })),
            )
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let response = client.whoami().await.unwrap();
        let snapshot = response.rate_limit.expect("snapshot present");
        assert_eq!(snapshot.limit, 100);
        assert_eq!(snapshot.remaining, 37);
        assert_eq!(snapshot.reset_in, 12);
    }

    #[tokio::test]
    async fn partial_rate_limit_headers_yield_no_snapshot() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("RateLimit-Remaining", "37")
                    .set_body_json(serde_json::json!({
                        "id": "u1", "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg",
                        "name": "N", "email": "n@x",
                        "authentication": {"type": "pat", "credentialId": "c",
                            "credentialName": "n", "scopes": [],
                            "expiresAt": "2027-01-01T00:00:00Z"},
                        "defaultOrganization": null,
                        "organizations": []
                    })),
            )
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let response = client.whoami().await.unwrap();
        assert!(response.rate_limit.is_none());
    }

    #[tokio::test]
    async fn rate_limit_headers_are_captured_on_429() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "2")
                    .insert_header("RateLimit-Limit", "100")
                    .insert_header("RateLimit-Remaining", "0")
                    .insert_header("RateLimit-Reset", "30")
                    .set_body_json(serde_json::json!({
                        "error": {"code": "RATE_LIMITED", "message": "slow down"}
                    })),
            )
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let err = client.whoami().await.unwrap_err();
        let api = err.as_api().unwrap();
        assert_eq!(api.code, "RATE_LIMITED");
        assert_eq!(api.retry_after, Some(Duration::from_secs(2)));
        let snapshot = api.rate_limit.expect("snapshot on 429");
        assert_eq!(snapshot.remaining, 0);
        assert_eq!(snapshot.reset_in, 30);
    }

    #[tokio::test]
    async fn server_error_text_is_sanitized_of_control_characters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "requestId": "req-1\u{7}",
                "error": {
                    "code": "FORBIDDEN\u{1b}[2J",
                    "message": "no\u{1b}]0;evil\u{7}pe"
                }
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let err = client.whoami().await.unwrap_err();
        let api = err.as_api().unwrap();
        // The ESC introducer is removed, so `[2J` cannot reassemble into an
        // escape sequence even though its printable chars survive.
        assert_eq!(api.code, "FORBIDDEN[2J");
        assert_eq!(api.message, "no]0;evilpe");
        assert_eq!(api.request_id.as_deref(), Some("req-1"));
    }

    #[test]
    fn sanitize_removes_control_characters_only() {
        assert_eq!(sanitize_server_text("plain"), "plain");
        assert_eq!(sanitize_server_text("tab\tkept"), "tab\tkept");
        assert_eq!(sanitize_server_text("esc\u{1b}[31m"), "esc[31m");
        assert_eq!(sanitize_server_text("nul\u{0}"), "nul");
        assert_eq!(sanitize_server_text("cr\rlf\n"), "crlf");
        assert_eq!(sanitize_server_text("kept \t "), "kept");
        assert_eq!(sanitize_server_text("emoji 🐹 ok"), "emoji 🐹 ok");
    }
}
