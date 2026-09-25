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
use std::time::{Duration, Instant, SystemTime};

use std::error::Error as _;

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

use crate::error::{ApiError, ClientError, NetworkStage, network_stage_from_message};
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

fn unsupported_attribute_operation() -> ClientError {
    ClientError::Protocol("this API client does not support Attribute operations".to_string())
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

impl RateLimitSnapshot {
    /// The documented JSON shape shared by `api request --json`'s `meta`
    /// rate-limit field and the explicit rate-limit probe.
    #[must_use]
    pub fn to_json(self) -> Value {
        serde_json::json!({
            "limit": self.limit,
            "remaining": self.remaining,
            "resetIn": self.reset_in,
        })
    }
}

/// The result of an explicit Public API rate-limit probe.
///
/// A probe performs one cheap authenticated read and returns the rate-limit
/// snapshot the server reported on that response, without the transport's
/// proactive depleted-window wait. The snapshot is a point-in-time
/// observation, not a guarantee: limits can change between calls.
#[derive(Debug, Clone)]
pub struct RateLimitProbe {
    /// The server's snapshot, when the response carried every
    /// `RateLimit-*` header.
    pub snapshot: Option<RateLimitSnapshot>,
    /// Correlation id (`requestId` from the envelope, or `X-Request-Id`).
    pub request_id: Option<String>,
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

/// Request-shape facts about one failed request, reported to a
/// [`RequestObserver`].
///
/// The observation deliberately carries no header values, request/response
/// bodies, query strings, or credentials: only the method, the
/// percent-encoded path under `/api/v1`, the *names* of the headers the
/// request intended to send, the response status, the server request id, and
/// how long the attempt(s) took.
#[derive(Debug, Clone)]
pub struct RequestObservation {
    /// HTTP method (for example `GET`).
    pub method: String,
    /// Percent-encoded path under `/api/v1`, without the query string.
    pub path: String,
    /// Lowercase names of the headers the request intended to send.
    pub header_names: Vec<String>,
    /// HTTP status, when a response was received.
    pub status: Option<u16>,
    /// Server correlation id, when the response carried one.
    pub request_id: Option<String>,
    /// Elapsed wall-clock time for the attempt(s), including retries.
    pub duration: Duration,
    /// True when the failure was transport/protocol-level rather than HTTP.
    pub transport: bool,
}

/// Receives one [`RequestObservation`] for each failed request.
///
/// Observers run synchronously on the request path and must not block; the
/// client treats them as best-effort diagnostics and never depends on their
/// result for the request outcome.
pub trait RequestObserver: Send + Sync {
    /// Records one failed request.
    fn observe(&self, observation: &RequestObservation);
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
    /// Whether successful responses with an exhausted rate-limit window may
    /// proactively wait before the next request.
    pub wait_on_depleted_rate_limit: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            user_agent: String::from("hamstik-cli"),
            retry: RetryPolicy::default(),
            ca_pem: Vec::new(),
            wait_on_depleted_rate_limit: true,
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
    wait_on_depleted_rate_limit: bool,
    request_observer: Option<Arc<dyn RequestObserver>>,
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

    /// Attaches an observer invoked once for every failed request.
    ///
    /// The observer is best-effort diagnostics: it never changes the request
    /// outcome and is not consulted for successful responses.
    #[must_use]
    pub fn with_request_observer(mut self, observer: Arc<dyn RequestObserver>) -> Self {
        self.request_observer = Some(observer);
        self
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
            wait_on_depleted_rate_limit: config.wait_on_depleted_rate_limit,
            request_observer: None,
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
        let started = Instant::now();
        let (response, _) = match self
            .send_with_retry(spec.retryable, || self.build(&spec, authenticated))
            .await
        {
            Ok(value) => value,
            Err(error) => {
                self.observe_failure(&spec, authenticated, started.elapsed(), &error);
                return Err(error);
            }
        };
        let finalized = match self.finalize(response).await {
            Ok(value) => value,
            Err(error) => {
                self.observe_failure(&spec, authenticated, started.elapsed(), &error);
                return Err(error);
            }
        };
        if self.wait_on_depleted_rate_limit {
            self.absorb_depleted_window(finalized.rate_limit).await;
        }
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
                    return Err(ClientError::Network {
                        message: err.to_string(),
                        stage: classify_network_error(&err),
                    });
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

    /// Reports one failed request to the attached observer, if any.
    ///
    /// Best-effort by construction: it derives only request-shape facts
    /// (method, path, header names, status, request id, elapsed time) and
    /// never inspects header values, bodies, or query strings. A missing
    /// observer is a no-op.
    fn observe_failure(
        &self,
        spec: &RequestSpec<'_>,
        authenticated: bool,
        duration: Duration,
        error: &ClientError,
    ) {
        let Some(observer) = &self.request_observer else {
            return;
        };
        let (status, request_id, transport) = match error {
            ClientError::Api(api) => (Some(api.status), api.request_id.clone(), false),
            ClientError::Network { .. } | ClientError::Protocol(_) | ClientError::Host(_) => {
                (None, None, true)
            }
        };
        let segments: Vec<&str> = spec.segments.iter().map(String::as_str).collect();
        let path = self
            .host
            .resource_url(&segments)
            .map(|url| url.path().to_string())
            .unwrap_or_else(|_| format!("/api/v1/{}", spec.segments.join("/")));
        let mut header_names: Vec<String> = spec
            .headers
            .iter()
            .map(|(name, _)| name.as_str().to_ascii_lowercase())
            .collect();
        if authenticated {
            header_names.push("authorization".to_string());
        }
        if !spec.headers.iter().any(|(name, _)| name == ACCEPT) {
            header_names.push("accept".to_string());
        }
        header_names.sort();
        header_names.dedup();
        observer.observe(&RequestObservation {
            method: spec.method.as_str().to_string(),
            path,
            header_names,
            status,
            request_id,
            duration,
            transport,
        });
    }

    /// Sends a request expecting a structured error or `204 No Content`.
    async fn send_void(&self, spec: RequestSpec<'_>) -> Result<ApiResponse<()>, ClientError> {
        let started = Instant::now();
        let (response, retried) = match self
            .send_with_retry(spec.retryable, || self.build(&spec, true))
            .await
        {
            Ok(value) => value,
            Err(error) => {
                self.observe_failure(&spec, true, started.elapsed(), &error);
                return Err(error);
            }
        };
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            return match self.finalize(response).await {
                Ok(value) => Ok(value),
                Err(error) => {
                    self.observe_failure(&spec, true, started.elapsed(), &error);
                    Err(error)
                }
            };
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
        let error = self.to_api_error(response).await;
        self.observe_failure(&spec, true, started.elapsed(), &error);
        Err(error)
    }

    /// Sends a request expecting binary bytes (attachment download), capping
    /// the body at [`MAX_BODY_BYTES`].
    async fn send_bytes(&self, spec: RequestSpec<'_>) -> Result<DownloadedAttachment, ClientError> {
        let started = Instant::now();
        let (response, _) = match self
            .send_with_retry(spec.retryable, || self.build(&spec, true))
            .await
        {
            Ok(value) => value,
            Err(error) => {
                self.observe_failure(&spec, true, started.elapsed(), &error);
                return Err(error);
            }
        };
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let error = self.to_api_error(response).await;
            self.observe_failure(&spec, true, started.elapsed(), &error);
            return Err(error);
        }
        let headers = response.headers().clone();
        let bytes = match read_body_capped(response).await {
            Ok(bytes) => bytes,
            Err(error) => {
                self.observe_failure(&spec, true, started.elapsed(), &error);
                return Err(error);
            }
        };
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
        let started = Instant::now();
        let (response, _) = match self
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
            .await
        {
            Ok(value) => value,
            Err(error) => {
                self.observe_failure(spec, true, started.elapsed(), &error);
                return Err(error);
            }
        };
        match self.finalize(response).await {
            Ok(value) => Ok(value),
            Err(error) => {
                self.observe_failure(spec, true, started.elapsed(), &error);
                Err(error)
            }
        }
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

/// Classifies a transport failure into the earliest failing network stage.
///
/// The stage is computed here, where the concrete `reqwest::Error` is in
/// hand, instead of being re-derived from its rendered text:
///
/// 1. `reqwest` flags timeouts itself ([`reqwest::Error::is_timeout`]), and
///    timeouts can nest inside other errors, so they win unconditionally;
/// 2. the source chain is walked for concrete types: `std::io::Error` kinds
///    (stable `std` API) decide connection-class failures, `rustls::Error`
///    names TLS handshake failures, and DNS failures — whose `io`
///    kind is not stable across platforms — are matched on the *OS*
///    resolver message embedded in the `io::Error`, not on reqwest's prose;
/// 3. only when no source is recognizable does it fall back to marker
///    matching on the rendered text ([`network_stage_from_message`]).
#[must_use]
pub fn classify_network_error(err: &reqwest::Error) -> NetworkStage {
    if err.is_timeout() {
        return NetworkStage::Timeout;
    }
    if let Some(stage) = classify_from_source_chain(err.source()) {
        return stage;
    }
    network_stage_from_message(&err.to_string())
}

/// Walks a transport-error source chain for concrete types: connection-class
/// `io::Error` kinds, `rustls::Error` (TLS), and OS resolver messages (DNS).
fn classify_from_source_chain(
    source: Option<&(dyn std::error::Error + 'static)>,
) -> Option<NetworkStage> {
    let mut source = source;
    while let Some(src) = source {
        // `rustls::Error` in the chain is authoritative for TLS handshake
        // failures; matching the concrete type (not rendered prose) means a
        // reqwest/hyper rewording cannot degrade classification.
        if src.downcast_ref::<rustls::Error>().is_some() {
            return Some(NetworkStage::Tls);
        }
        if let Some(io) = src.downcast_ref::<std::io::Error>() {
            match io.kind() {
                std::io::ErrorKind::TimedOut => return Some(NetworkStage::Timeout),
                std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::AddrNotAvailable
                | std::io::ErrorKind::HostUnreachable
                | std::io::ErrorKind::NetworkUnreachable => return Some(NetworkStage::Connection),
                // DNS failures surface as an io::Error whose kind is not
                // stable (glibc: Uncategorized/InvalidInput; others vary);
                // match the OS resolver message, which is stable per platform.
                _ => {
                    let msg = io.to_string().to_ascii_lowercase();
                    if msg.contains("lookup address information")
                        || msg.contains("name or service not known")
                        || msg.contains("nodename nor servname provided")
                        || msg.contains("temporary failure in name resolution")
                        || msg.contains("no address associated")
                        // Windows: getaddrinfo reports WSAHOST_NOT_FOUND /
                        // WSATRY_AGAIN through `std`, with no kind to match.
                        || msg.contains("no such host is known")
                        || msg.contains("error occurred during a database lookup")
                    {
                        return Some(NetworkStage::Dns);
                    }
                }
            }
        }
        source = src.source();
    }
    None
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
    while let Some(chunk) = stream.chunk().await.map_err(|err| ClientError::Network {
        message: err.to_string(),
        stage: classify_network_error(&err),
    })? {
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

/// Appends an owned optional query parameter when present.
fn push_opt(query: &mut Vec<(String, String)>, name: &str, value: Option<String>) {
    if let Some(text) = value {
        query.push((name.to_string(), text));
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

fn advanced_query(opts: &ListAdvancedOptions) -> Vec<(String, String)> {
    let mut out = Vec::new();
    push_list(
        &mut out,
        &ListOptions {
            limit: opts.limit,
            cursor: opts.cursor.clone(),
        },
    );
    push_scalar(&mut out, "visibility", &opts.visibility);
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
    /// `GET /me`: one cheap authenticated read whose response carries the
    /// current `RateLimit-*` snapshot.
    ///
    /// Unlike typed reads, a probe never applies the proactive
    /// depleted-window wait, so callers observe present headroom immediately
    /// instead of sleeping toward a window reset.
    async fn probe_rate_limit(&self) -> Result<RateLimitProbe, ClientError>;
    /// Generic Public API v1 passthrough: sends one request to
    /// `GET|POST|PATCH|PUT|DELETE /api/v1/<segments...>` with an optional
    /// JSON body, query pairs, allowlisted header overrides, and an optional
    /// idempotency key. Transport-level behavior (auth, TLS, retries,
    /// error mapping, redaction, body cap) is identical to typed operations;
    /// the caller has already validated the path segments.
    async fn raw_request(
        &self,
        method: &str,
        segments: &[String],
        query: Vec<(String, String)>,
        headers: Vec<(String, String)>,
        body: Option<&Value>,
        idempotency_key: Option<&str>,
    ) -> Result<ApiResponse<Value>, ClientError>;
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
    /// `GET .../attributes`: Organization-governed Attribute catalog.
    async fn list_organization_attributes(
        &self,
        _org_slug: &str,
        _opts: ListAttributesOptions,
    ) -> Result<ApiResponse<AttributeCatalog>, ClientError> {
        Err(unsupported_attribute_operation())
    }
    /// `GET .../attributes/{key}`: one Attribute definition and its ETag.
    async fn get_organization_attribute(
        &self,
        _org_slug: &str,
        _key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        Err(unsupported_attribute_operation())
    }
    /// `POST .../attributes`: create an Organization Attribute.
    async fn create_organization_attribute(
        &self,
        _org_slug: &str,
        _body: &CreateAttributeDefinitionRequest,
        _idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        Err(unsupported_attribute_operation())
    }
    /// `PATCH .../attributes/{key}`: rename a definition using its ETag.
    async fn rename_organization_attribute(
        &self,
        _org_slug: &str,
        _key: &str,
        _body: &RenameAttributeDefinitionRequest,
        _if_match: &str,
        _idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        Err(unsupported_attribute_operation())
    }
    /// `POST .../attributes/{key}/transitions`: update definition lifecycle.
    async fn transition_organization_attribute(
        &self,
        _org_slug: &str,
        _key: &str,
        _body: &TransitionAttributeDefinitionRequest,
        _if_match: &str,
        _idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        Err(unsupported_attribute_operation())
    }
    /// `POST .../attributes/{key}/options`: add an option using the definition ETag.
    async fn create_organization_attribute_option(
        &self,
        _org_slug: &str,
        _key: &str,
        _body: &CreateAttributeOptionRequest,
        _if_match: &str,
        _idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        Err(unsupported_attribute_operation())
    }
    /// `PATCH .../attributes/{key}/options/{option}`: rename an option.
    async fn rename_organization_attribute_option(
        &self,
        _org_slug: &str,
        _key: &str,
        _option_key: &str,
        _body: &RenameAttributeOptionRequest,
        _if_match: &str,
        _idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        Err(unsupported_attribute_operation())
    }
    /// `POST .../attributes/{key}/options/reorder`: replace option ordering.
    async fn reorder_organization_attribute_options(
        &self,
        _org_slug: &str,
        _key: &str,
        _body: &ReorderAttributeOptionsRequest,
        _if_match: &str,
        _idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        Err(unsupported_attribute_operation())
    }
    /// `POST .../attributes/{key}/options/{option}/retire`: retire an option.
    async fn retire_organization_attribute_option(
        &self,
        _org_slug: &str,
        _key: &str,
        _option_key: &str,
        _if_match: &str,
        _idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        Err(unsupported_attribute_operation())
    }
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
    /// `GET .../projects/{key}/attributes`: only this Project's usable catalog.
    async fn list_project_attributes(
        &self,
        _org_slug: &str,
        _project_key: &str,
    ) -> Result<ApiResponse<ProjectAttributeCatalog>, ClientError> {
        Err(unsupported_attribute_operation())
    }
    /// `POST .../projects/{key}/attributes/{attribute}/enablement`.
    async fn set_project_attribute_enablement(
        &self,
        _org_slug: &str,
        _project_key: &str,
        _attribute_key: &str,
        _body: &SetProjectAttributeEnablementRequest,
        _if_match: &str,
        _idempotency_key: &str,
    ) -> Result<ApiResponse<ProjectAttributeEnablement>, ClientError> {
        Err(unsupported_attribute_operation())
    }
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
    /// `PATCH .../work-items/{key}`: conditional update via `If-Match`; an
    /// idempotency key is required by the API when the body changes Attributes.
    async fn update_work_item(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &UpdateWorkItemRequest,
        if_match: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError>;
    /// `PATCH .../work-items/{key}` using a required idempotency key. Use for
    /// Attribute-bearing updates, whose Public API contract requires the key.
    /// The default keeps existing third-party `HamstikApi` implementations
    /// source-compatible while failing closed until they implement retries.
    async fn update_work_item_with_idempotency(
        &self,
        _org_slug: &str,
        _project_key: &str,
        _key: &str,
        _body: &UpdateWorkItemRequest,
        _if_match: &str,
        _idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItem>, ClientError> {
        Err(ClientError::Protocol(
            "this API client does not support idempotent Work Item updates".to_string(),
        ))
    }
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
    /// `GET .../reports/{reportType}`: one project report (burndown-style
    /// series and rollups). The server owns the report shape; unknown types
    /// are rejected by the server, not by the client.
    async fn get_project_report(
        &self,
        org_slug: &str,
        project_key: &str,
        report_type: &str,
        opts: ProjectReportOptions,
    ) -> Result<ApiResponse<ProjectReport>, ClientError>;
    /// `GET .../sprints/{id}/report`: the Sprint delivery report (commitment,
    /// scope changes, carryover, burndown, and a paginated change feed).
    async fn get_sprint_report(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<SprintReport>, ClientError>;
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
    /// `POST .../sprints/{id}/archive`: archive a Sprint (idempotent,
    /// `If-Match`, empty `{}` body).
    async fn archive_sprint(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Sprint>, ClientError>;
    /// `POST .../sprints/{id}/unarchive`: unarchive a Sprint (idempotent,
    /// `If-Match`, empty `{}` body).
    async fn unarchive_sprint(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
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
    /// `GET .../advanced-reports`: list authorized Advanced Reports.
    async fn list_advanced_reports(
        &self,
        org_slug: &str,
        opts: ListAdvancedOptions,
    ) -> Result<ApiResponse<AdvancedReportList>, ClientError>;
    /// `POST .../advanced-reports`: create an Advanced Report (idempotent).
    async fn create_advanced_report(
        &self,
        org_slug: &str,
        body: &CreateAdvancedReportRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AdvancedReportDetail>, ClientError>;
    /// `GET .../advanced-reports/{reportId}`: one Advanced Report and ETag.
    async fn get_advanced_report(
        &self,
        org_slug: &str,
        report_id: &str,
    ) -> Result<ApiResponse<AdvancedReportDetail>, ClientError>;
    /// `PATCH .../advanced-reports/{reportId}`: replace an Advanced Report.
    async fn update_advanced_report(
        &self,
        org_slug: &str,
        report_id: &str,
        body: &UpdateAdvancedReportRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AdvancedReportDetail>, ClientError>;
    /// `DELETE .../advanced-reports/{reportId}`: delete an Advanced Report.
    async fn delete_advanced_report(
        &self,
        org_slug: &str,
        report_id: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<()>, ClientError>;
    /// Read-only `POST .../advanced-reports/{reportId}/runs`: evaluate a
    /// saved report at an expected revision (no idempotency key).
    async fn run_advanced_report(
        &self,
        org_slug: &str,
        report_id: &str,
        body: &AdvancedReportRunRequest,
    ) -> Result<ApiResponse<AdvancedReportRunResult>, ClientError>;
    /// `GET .../advanced-report-runs/{runId}/cells/{cellId}/items`: page
    /// through the captured selection behind one result cell.
    async fn list_advanced_selection_items(
        &self,
        org_slug: &str,
        run_id: &str,
        cell_id: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<AdvancedSelectionPage>, ClientError>;
    /// `GET .../advanced-dashboards`: list authorized Advanced Dashboards.
    async fn list_advanced_dashboards(
        &self,
        org_slug: &str,
        opts: ListAdvancedOptions,
    ) -> Result<ApiResponse<AdvancedDashboardList>, ClientError>;
    /// `GET .../advanced-dashboards/{dashboardId}`: one Advanced Dashboard.
    async fn get_advanced_dashboard(
        &self,
        org_slug: &str,
        dashboard_id: &str,
    ) -> Result<ApiResponse<AdvancedDashboardDetail>, ClientError>;
    /// Read-only `POST .../advanced-dashboards/{dashboardId}/runs`: evaluate
    /// a dashboard at an expected revision (no idempotency key).
    async fn run_advanced_dashboard(
        &self,
        org_slug: &str,
        dashboard_id: &str,
        body: &AdvancedDashboardRunRequest,
    ) -> Result<ApiResponse<AdvancedDashboardRunResult>, ClientError>;
    /// `GET .../releases`: list the Project's Release Versions.
    async fn list_release_versions(
        &self,
        org_slug: &str,
        project_key: &str,
        opts: ListReleaseVersionsOptions,
    ) -> Result<ApiResponse<ReleaseVersionList>, ClientError>;
    /// `POST .../releases`: create a Release Version (idempotent).
    async fn create_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &CreateReleaseVersionRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError>;
    /// `GET .../releases/{releaseId}`: one Release Version (captures the ETag).
    async fn get_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError>;
    /// `PATCH .../releases/{releaseId}`: update release metadata
    /// (`If-Match` + idempotency key).
    async fn update_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &UpdateReleaseVersionRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError>;
    /// `GET .../releases/{releaseId}/transitions`: permitted transitions.
    async fn list_release_version_transitions(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
    ) -> Result<ApiResponse<ReleaseVersionTransitionList>, ClientError>;
    /// `POST .../releases/{releaseId}/transitions`: move the release to a
    /// state (`If-Match` + idempotency key).
    async fn transition_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &TransitionReleaseVersionRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError>;
    /// `POST .../releases/{releaseId}/archive`: archive a release
    /// (`If-Match` + idempotency key).
    async fn archive_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &ReleaseVersionLifecycleRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError>;
    /// `POST .../releases/{releaseId}/restore`: restore an archived release
    /// (`If-Match` + idempotency key).
    async fn restore_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &ReleaseVersionLifecycleRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError>;
    /// `GET .../releases/{releaseId}/scope`: release progress and a page of
    /// its Work Items.
    async fn get_release_version_scope(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        query: ReleaseScopeQuery,
    ) -> Result<ApiResponse<ReleaseVersionScope>, ClientError>;
    /// `GET .../work-items/{key}/releases`: the item's Release Versions.
    async fn list_work_item_release_versions(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
    ) -> Result<ApiResponse<WorkItemReleaseVersions>, ClientError>;
    /// `POST .../work-items/{key}/releases`: add, remove, or replace the
    /// item's Release Version memberships (`If-Match` + idempotency key).
    async fn mutate_work_item_release_versions(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &MutateWorkItemReleaseVersionsRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItemReleaseVersions>, ClientError>;
    /// `POST .../release-memberships/bulk`: change memberships for up to 50
    /// Work Items (idempotent).
    async fn bulk_mutate_release_memberships(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &BulkReleaseMembershipRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<BulkReleaseMembershipResponse>, ClientError>;
    /// `GET .../releases/{releaseId}/announcements`: the latest published
    /// announcement.
    async fn get_current_release_announcement(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
    ) -> Result<ApiResponse<ReleaseAnnouncementRevision>, ClientError>;
    /// `GET .../announcements/{revision}`: one published revision.
    async fn get_release_announcement_revision(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        revision: i64,
    ) -> Result<ApiResponse<ReleaseAnnouncementRevision>, ClientError>;
    /// `GET .../announcements/draft`: the active announcement draft.
    async fn get_release_announcement_draft(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
    ) -> Result<ApiResponse<ReleaseAnnouncementDraft>, ClientError>;
    /// `POST .../announcements/draft`: generate or refresh the draft
    /// (idempotent).
    async fn generate_release_announcement_draft(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &GenerateReleaseAnnouncementDraftRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseAnnouncementDraft>, ClientError>;
    /// `PATCH .../announcements/draft`: replace the draft narrative
    /// (revision-checked body, no `If-Match`).
    async fn edit_release_announcement_draft(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &EditReleaseAnnouncementDraftRequest,
    ) -> Result<ApiResponse<ReleaseAnnouncementDraft>, ClientError>;
    /// `DELETE .../announcements/draft`: discard the active draft.
    async fn archive_release_announcement_draft(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &AnnouncementDraftRevisionRequest,
    ) -> Result<ApiResponse<()>, ClientError>;
    /// `POST .../announcements/publish`: publish an immutable revision
    /// (idempotent).
    async fn publish_release_announcement(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &PublishReleaseAnnouncementRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<PublishedReleaseAnnouncementReference>, ClientError>;
    /// `POST .../release-audit-reports`: generate a dossier or register
    /// (idempotent).
    async fn generate_release_audit_report(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &GenerateReleaseAuditReportRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseAuditReportSummary>, ClientError>;
    /// `GET .../release-audit-reports/{reportId}?format=json`: the exact
    /// saved JSON package.
    async fn get_release_audit_report(
        &self,
        org_slug: &str,
        project_key: &str,
        report_id: &str,
    ) -> Result<ApiResponse<Value>, ClientError>;
    /// `GET .../release-audit-reports/{reportId}?format=csv`: the CSV ZIP
    /// package as bytes (the operation's binary response form).
    async fn download_release_audit_report(
        &self,
        org_slug: &str,
        project_key: &str,
        report_id: &str,
    ) -> Result<DownloadedAttachment, ClientError>;
    /// `GET .../release-audit-reports`: list saved audit packages.
    async fn list_release_audit_reports(
        &self,
        org_slug: &str,
        project_key: &str,
        opts: ListReleaseAuditReportsOptions,
    ) -> Result<ApiResponse<ReleaseAuditReportList>, ClientError>;
    /// `GET .../milestones`: list Organization Milestones.
    async fn list_organization_milestones(
        &self,
        org_slug: &str,
        opts: ListMilestonesOptions,
    ) -> Result<ApiResponse<OrganizationMilestoneList>, ClientError>;
    /// `POST .../milestones`: create an Organization Milestone (idempotent).
    async fn create_organization_milestone(
        &self,
        org_slug: &str,
        body: &CreateOrganizationMilestoneRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<OrganizationMilestone>, ClientError>;
    /// `GET .../milestones/{milestoneId}`: the milestone plus a page of its
    /// Release Versions.
    async fn get_organization_milestone(
        &self,
        org_slug: &str,
        milestone_id: &str,
        query: MilestoneDetailQuery,
    ) -> Result<ApiResponse<OrganizationMilestoneDetail>, ClientError>;
    /// `PATCH .../milestones/{milestoneId}`: update milestone metadata
    /// (`If-Match` + idempotency key).
    async fn update_organization_milestone(
        &self,
        org_slug: &str,
        milestone_id: &str,
        body: &UpdateOrganizationMilestoneRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<OrganizationMilestone>, ClientError>;
    /// `GET .../milestones/{milestoneId}/transitions`: permitted transitions.
    async fn list_organization_milestone_transitions(
        &self,
        org_slug: &str,
        milestone_id: &str,
    ) -> Result<ApiResponse<OrganizationMilestoneTransitionList>, ClientError>;
    /// `POST .../milestones/{milestoneId}/transitions`: move the milestone to
    /// a state (`If-Match` + idempotency key).
    async fn transition_organization_milestone(
        &self,
        org_slug: &str,
        milestone_id: &str,
        body: &TransitionOrganizationMilestoneRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<OrganizationMilestone>, ClientError>;
    /// `GET .../milestones/{milestoneId}/releases`: the milestone's Release
    /// Versions.
    async fn list_organization_milestone_releases(
        &self,
        org_slug: &str,
        milestone_id: &str,
        query: MilestoneReleasesQuery,
    ) -> Result<ApiResponse<OrganizationMilestoneReleaseList>, ClientError>;
    /// `POST .../milestones/{milestoneId}/releases`: add or remove one
    /// Release Version (`If-Match` + idempotency key).
    async fn mutate_organization_milestone_release(
        &self,
        org_slug: &str,
        milestone_id: &str,
        body: &MutateOrganizationMilestoneReleaseRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<OrganizationMilestone>, ClientError>;
    /// `GET .../milestones/{milestoneId}/events`: the milestone history feed.
    async fn list_organization_milestone_events(
        &self,
        org_slug: &str,
        milestone_id: &str,
        query: MilestoneEventsQuery,
    ) -> Result<ApiResponse<OrganizationMilestoneEventList>, ClientError>;
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

    async fn probe_rate_limit(&self) -> Result<RateLimitProbe, ClientError> {
        // `GET /me` is the cheapest documented authenticated read; its
        // response carries the same `RateLimit-*` headers as any other
        // Public API call. This deliberately bypasses `send_json`, which
        // would absorb a depleted window: a probe reports the snapshot
        // instead of waiting out the reset.
        let spec = RequestSpec {
            method: Method::GET,
            segments: vec!["me".to_string()],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        };
        let started = Instant::now();
        let (response, _) = match self
            .send_with_retry(spec.retryable, || self.build(&spec, true))
            .await
        {
            Ok(value) => value,
            Err(error) => {
                self.observe_failure(&spec, true, started.elapsed(), &error);
                return Err(error);
            }
        };
        let finalized: ApiResponse<Value> = match self.finalize(response).await {
            Ok(value) => value,
            Err(error) => {
                self.observe_failure(&spec, true, started.elapsed(), &error);
                return Err(error);
            }
        };
        Ok(RateLimitProbe {
            snapshot: finalized.rate_limit,
            request_id: finalized.request_id,
        })
    }

    async fn raw_request(
        &self,
        method: &str,
        segments: &[String],
        query: Vec<(String, String)>,
        headers: Vec<(String, String)>,
        body: Option<&Value>,
        idempotency_key: Option<&str>,
    ) -> Result<ApiResponse<Value>, ClientError> {
        let http_method = Method::from_bytes(method.as_bytes()).map_err(|err| {
            ClientError::Protocol(format!("unsupported method {method:?}: {err}"))
        })?;
        let mut spec_headers: Vec<(HeaderName, String)> = Vec::new();
        for (name, value) in headers {
            let parsed = HeaderName::from_lowercase(name.to_ascii_lowercase().as_bytes()).map_err(
                |err| ClientError::Protocol(format!("invalid header name {name:?}: {err}")),
            )?;
            spec_headers.push((parsed, value));
        }
        if let Some(key) = idempotency_key {
            spec_headers.push((header_idempotency_key(), key.to_string()));
        }
        self.send_json(RequestSpec {
            method: http_method,
            segments: segments.to_vec(),
            query,
            headers: spec_headers,
            body,
            // POST/DELETE carry the idempotency key, making retries safe;
            // GET/PATCH/PUT are either read-only or revision-guarded. The
            // transport's retry policy applies to all of them.
            retryable: true,
        })
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

    async fn list_organization_attributes(
        &self,
        org_slug: &str,
        opts: ListAttributesOptions,
    ) -> Result<ApiResponse<AttributeCatalog>, ClientError> {
        let query = if opts.include_retired {
            vec![("includeRetired".to_string(), "true".to_string())]
        } else {
            Vec::new()
        };
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "attributes".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_organization_attribute(
        &self,
        org_slug: &str,
        key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "attributes".to_string(),
                key.to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn create_organization_attribute(
        &self,
        org_slug: &str,
        body: &CreateAttributeDefinitionRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "attributes".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn rename_organization_attribute(
        &self,
        org_slug: &str,
        key: &str,
        body: &RenameAttributeDefinitionRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::PATCH,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "attributes".to_string(),
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

    async fn transition_organization_attribute(
        &self,
        org_slug: &str,
        key: &str,
        body: &TransitionAttributeDefinitionRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "attributes".to_string(),
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

    async fn create_organization_attribute_option(
        &self,
        org_slug: &str,
        key: &str,
        body: &CreateAttributeOptionRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "attributes".to_string(),
                key.to_string(),
                "options".to_string(),
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

    async fn rename_organization_attribute_option(
        &self,
        org_slug: &str,
        key: &str,
        option_key: &str,
        body: &RenameAttributeOptionRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::PATCH,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "attributes".to_string(),
                key.to_string(),
                "options".to_string(),
                option_key.to_string(),
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

    async fn reorder_organization_attribute_options(
        &self,
        org_slug: &str,
        key: &str,
        body: &ReorderAttributeOptionsRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "attributes".to_string(),
                key.to_string(),
                "options".to_string(),
                "reorder".to_string(),
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

    async fn retire_organization_attribute_option(
        &self,
        org_slug: &str,
        key: &str,
        option_key: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AttributeDefinition>, ClientError> {
        let payload = Value::Object(Default::default());
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "attributes".to_string(),
                key.to_string(),
                "options".to_string(),
                option_key.to_string(),
                "retire".to_string(),
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

    async fn list_project_attributes(
        &self,
        org_slug: &str,
        project_key: &str,
    ) -> Result<ApiResponse<ProjectAttributeCatalog>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "attributes".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn set_project_attribute_enablement(
        &self,
        org_slug: &str,
        project_key: &str,
        attribute_key: &str,
        body: &SetProjectAttributeEnablementRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ProjectAttributeEnablement>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "attributes".to_string(),
                attribute_key.to_string(),
                "enablement".to_string(),
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
            retryable: false,
        })
        .await
    }

    async fn update_work_item_with_idempotency(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &UpdateWorkItemRequest,
        if_match: &str,
        idempotency_key: &str,
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
            headers: vec![
                (IF_MATCH.clone(), if_match.to_string()),
                (header_idempotency_key(), idempotency_key.to_string()),
            ],
            body: Some(&payload),
            retryable: true,
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

    async fn get_project_report(
        &self,
        org_slug: &str,
        project_key: &str,
        report_type: &str,
        opts: ProjectReportOptions,
    ) -> Result<ApiResponse<ProjectReport>, ClientError> {
        let mut query = Vec::new();
        push_list(
            &mut query,
            &ListOptions {
                limit: opts.limit,
                cursor: opts.cursor,
            },
        );
        push_opt(&mut query, "range", opts.range.map(|v| v.to_string()));
        push_opt(&mut query, "start", opts.start);
        push_opt(&mut query, "end", opts.end);
        push_opt(&mut query, "timeZone", opts.time_zone);
        push_opt(&mut query, "unit", opts.unit);
        push_opt(&mut query, "interval", opts.interval);
        push_opt(&mut query, "measure", opts.measure);
        push_opt(&mut query, "cycleStartStatus", opts.cycle_start_status);
        push_opt(&mut query, "window", opts.window.map(|v| v.to_string()));
        push_opt(&mut query, "groupBy", opts.group_by);
        push_opt(&mut query, "scope", opts.scope);
        push_opt(&mut query, "sprint", opts.sprint);
        push_opt(&mut query, "sort", opts.sort);
        push_opt(&mut query, "q", opts.query);
        push_opt(&mut query, "squeakql", opts.squeakql);
        push_opt(&mut query, "status", opts.status);
        push_opt(&mut query, "type", opts.work_type);
        push_opt(&mut query, "priority", opts.priority);
        push_opt(&mut query, "assignee", opts.assignee);
        push_opt(&mut query, "label", opts.label);
        push_opt(&mut query, "buckets", opts.buckets);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "reports".to_string(),
                report_type.to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_sprint_report(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<SprintReport>, ClientError> {
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
                sprint_id.to_string(),
                "report".to_string(),
            ],
            query,
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

    async fn archive_sprint(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Sprint>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "sprints".to_string(),
                sprint_id.to_string(),
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

    async fn unarchive_sprint(
        &self,
        org_slug: &str,
        project_key: &str,
        sprint_id: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<Sprint>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "sprints".to_string(),
                sprint_id.to_string(),
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

    async fn list_advanced_reports(
        &self,
        org_slug: &str,
        opts: ListAdvancedOptions,
    ) -> Result<ApiResponse<AdvancedReportList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "advanced-reports".to_string(),
            ],
            query: advanced_query(&opts),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn create_advanced_report(
        &self,
        org_slug: &str,
        body: &CreateAdvancedReportRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AdvancedReportDetail>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "advanced-reports".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn get_advanced_report(
        &self,
        org_slug: &str,
        report_id: &str,
    ) -> Result<ApiResponse<AdvancedReportDetail>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "advanced-reports".to_string(),
                report_id.to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn update_advanced_report(
        &self,
        org_slug: &str,
        report_id: &str,
        body: &UpdateAdvancedReportRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<AdvancedReportDetail>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::PATCH,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "advanced-reports".to_string(),
                report_id.to_string(),
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

    async fn delete_advanced_report(
        &self,
        org_slug: &str,
        report_id: &str,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<()>, ClientError> {
        self.send_void(RequestSpec {
            method: Method::DELETE,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "advanced-reports".to_string(),
                report_id.to_string(),
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

    async fn run_advanced_report(
        &self,
        org_slug: &str,
        report_id: &str,
        body: &AdvancedReportRunRequest,
    ) -> Result<ApiResponse<AdvancedReportRunResult>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "advanced-reports".to_string(),
                report_id.to_string(),
                "runs".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn list_advanced_selection_items(
        &self,
        org_slug: &str,
        run_id: &str,
        cell_id: &str,
        opts: ListOptions,
    ) -> Result<ApiResponse<AdvancedSelectionPage>, ClientError> {
        let mut query = Vec::new();
        push_list(&mut query, &opts);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "advanced-report-runs".to_string(),
                run_id.to_string(),
                "cells".to_string(),
                cell_id.to_string(),
                "items".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_advanced_dashboards(
        &self,
        org_slug: &str,
        opts: ListAdvancedOptions,
    ) -> Result<ApiResponse<AdvancedDashboardList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "advanced-dashboards".to_string(),
            ],
            query: advanced_query(&opts),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_advanced_dashboard(
        &self,
        org_slug: &str,
        dashboard_id: &str,
    ) -> Result<ApiResponse<AdvancedDashboardDetail>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "advanced-dashboards".to_string(),
                dashboard_id.to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn run_advanced_dashboard(
        &self,
        org_slug: &str,
        dashboard_id: &str,
        body: &AdvancedDashboardRunRequest,
    ) -> Result<ApiResponse<AdvancedDashboardRunResult>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "advanced-dashboards".to_string(),
                dashboard_id.to_string(),
                "runs".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn list_release_versions(
        &self,
        org_slug: &str,
        project_key: &str,
        opts: ListReleaseVersionsOptions,
    ) -> Result<ApiResponse<ReleaseVersionList>, ClientError> {
        let mut query = Vec::new();
        push_list(
            &mut query,
            &ListOptions {
                limit: opts.limit,
                cursor: opts.cursor,
            },
        );
        push_opt(&mut query, "state", opts.state);
        push_bool(&mut query, "includeArchived", opts.include_archived);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn create_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &CreateReleaseVersionRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn get_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn update_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &UpdateReleaseVersionRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::PATCH,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
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

    async fn list_release_version_transitions(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
    ) -> Result<ApiResponse<ReleaseVersionTransitionList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "transitions".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn transition_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &TransitionReleaseVersionRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
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

    async fn archive_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &ReleaseVersionLifecycleRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "archive".to_string(),
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

    async fn restore_release_version(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &ReleaseVersionLifecycleRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseVersion>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "restore".to_string(),
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

    async fn get_release_version_scope(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        query: ReleaseScopeQuery,
    ) -> Result<ApiResponse<ReleaseVersionScope>, ClientError> {
        let mut query_pairs = Vec::new();
        push_list(
            &mut query_pairs,
            &ListOptions {
                limit: query.limit,
                cursor: query.cursor,
            },
        );
        push_opt(&mut query_pairs, "q", query.query);
        push_opt(&mut query_pairs, "status", query.status);
        push_opt(&mut query_pairs, "type", query.item_type);
        push_opt(&mut query_pairs, "priority", query.priority);
        push_opt(&mut query_pairs, "assigneeId", query.assignee_id);
        push_opt(&mut query_pairs, "sort", query.sort);
        push_opt(&mut query_pairs, "direction", query.direction);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "scope".to_string(),
            ],
            query: query_pairs,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_work_item_release_versions(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
    ) -> Result<ApiResponse<WorkItemReleaseVersions>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "work-items".to_string(),
                key.to_string(),
                "releases".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn mutate_work_item_release_versions(
        &self,
        org_slug: &str,
        project_key: &str,
        key: &str,
        body: &MutateWorkItemReleaseVersionsRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<WorkItemReleaseVersions>, ClientError> {
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
                "releases".to_string(),
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

    async fn bulk_mutate_release_memberships(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &BulkReleaseMembershipRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<BulkReleaseMembershipResponse>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "release-memberships".to_string(),
                "bulk".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn get_current_release_announcement(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
    ) -> Result<ApiResponse<ReleaseAnnouncementRevision>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "announcements".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_release_announcement_revision(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        revision: i64,
    ) -> Result<ApiResponse<ReleaseAnnouncementRevision>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "announcements".to_string(),
                revision.to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn get_release_announcement_draft(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
    ) -> Result<ApiResponse<ReleaseAnnouncementDraft>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "announcements".to_string(),
                "draft".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn generate_release_announcement_draft(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &GenerateReleaseAnnouncementDraftRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseAnnouncementDraft>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "announcements".to_string(),
                "draft".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn edit_release_announcement_draft(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &EditReleaseAnnouncementDraftRequest,
    ) -> Result<ApiResponse<ReleaseAnnouncementDraft>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::PATCH,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "announcements".to_string(),
                "draft".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn archive_release_announcement_draft(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &AnnouncementDraftRevisionRequest,
    ) -> Result<ApiResponse<()>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_void(RequestSpec {
            method: Method::DELETE,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "announcements".to_string(),
                "draft".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn publish_release_announcement(
        &self,
        org_slug: &str,
        project_key: &str,
        release_id: &str,
        body: &PublishReleaseAnnouncementRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<PublishedReleaseAnnouncementReference>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "releases".to_string(),
                release_id.to_string(),
                "announcements".to_string(),
                "publish".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn generate_release_audit_report(
        &self,
        org_slug: &str,
        project_key: &str,
        body: &GenerateReleaseAuditReportRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<ReleaseAuditReportSummary>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "release-audit-reports".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn get_release_audit_report(
        &self,
        org_slug: &str,
        project_key: &str,
        report_id: &str,
    ) -> Result<ApiResponse<Value>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "release-audit-reports".to_string(),
                report_id.to_string(),
            ],
            query: vec![("format".to_string(), "json".to_string())],
            headers: vec![(ACCEPT.clone(), "application/json".to_string())],
            body: None,
            retryable: true,
        })
        .await
    }

    async fn download_release_audit_report(
        &self,
        org_slug: &str,
        project_key: &str,
        report_id: &str,
    ) -> Result<DownloadedAttachment, ClientError> {
        self.send_bytes(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "release-audit-reports".to_string(),
                report_id.to_string(),
            ],
            query: vec![("format".to_string(), "csv".to_string())],
            headers: vec![(ACCEPT.clone(), "application/zip".to_string())],
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_release_audit_reports(
        &self,
        org_slug: &str,
        project_key: &str,
        opts: ListReleaseAuditReportsOptions,
    ) -> Result<ApiResponse<ReleaseAuditReportList>, ClientError> {
        let mut query = Vec::new();
        push_list(
            &mut query,
            &ListOptions {
                limit: opts.limit,
                cursor: opts.cursor,
            },
        );
        push_opt(&mut query, "releaseVersionId", opts.release_version_id);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "projects".to_string(),
                project_key.to_string(),
                "release-audit-reports".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn list_organization_milestones(
        &self,
        org_slug: &str,
        opts: ListMilestonesOptions,
    ) -> Result<ApiResponse<OrganizationMilestoneList>, ClientError> {
        let mut query = Vec::new();
        push_list(
            &mut query,
            &ListOptions {
                limit: opts.limit,
                cursor: opts.cursor,
            },
        );
        push_opt(&mut query, "state", opts.state);
        push_bool(&mut query, "includeArchived", opts.include_archived);
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "milestones".to_string(),
            ],
            query,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn create_organization_milestone(
        &self,
        org_slug: &str,
        body: &CreateOrganizationMilestoneRequest,
        idempotency_key: &str,
    ) -> Result<ApiResponse<OrganizationMilestone>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "milestones".to_string(),
            ],
            query: Vec::new(),
            headers: vec![(header_idempotency_key(), idempotency_key.to_string())],
            body: Some(&payload),
            retryable: true,
        })
        .await
    }

    async fn get_organization_milestone(
        &self,
        org_slug: &str,
        milestone_id: &str,
        query: MilestoneDetailQuery,
    ) -> Result<ApiResponse<OrganizationMilestoneDetail>, ClientError> {
        let mut query_pairs = Vec::new();
        push_opt(&mut query_pairs, "releaseAfter", query.release_after);
        push_opt(
            &mut query_pairs,
            "limit",
            query.limit.map(|limit| limit.to_string()),
        );
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "milestones".to_string(),
                milestone_id.to_string(),
            ],
            query: query_pairs,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn update_organization_milestone(
        &self,
        org_slug: &str,
        milestone_id: &str,
        body: &UpdateOrganizationMilestoneRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<OrganizationMilestone>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::PATCH,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "milestones".to_string(),
                milestone_id.to_string(),
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

    async fn list_organization_milestone_transitions(
        &self,
        org_slug: &str,
        milestone_id: &str,
    ) -> Result<ApiResponse<OrganizationMilestoneTransitionList>, ClientError> {
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "milestones".to_string(),
                milestone_id.to_string(),
                "transitions".to_string(),
            ],
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn transition_organization_milestone(
        &self,
        org_slug: &str,
        milestone_id: &str,
        body: &TransitionOrganizationMilestoneRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<OrganizationMilestone>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "milestones".to_string(),
                milestone_id.to_string(),
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

    async fn list_organization_milestone_releases(
        &self,
        org_slug: &str,
        milestone_id: &str,
        query: MilestoneReleasesQuery,
    ) -> Result<ApiResponse<OrganizationMilestoneReleaseList>, ClientError> {
        let mut query_pairs = Vec::new();
        push_opt(&mut query_pairs, "cursor", query.cursor);
        push_opt(
            &mut query_pairs,
            "limit",
            query.limit.map(|limit| limit.to_string()),
        );
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "milestones".to_string(),
                milestone_id.to_string(),
                "releases".to_string(),
            ],
            query: query_pairs,
            headers: Vec::new(),
            body: None,
            retryable: true,
        })
        .await
    }

    async fn mutate_organization_milestone_release(
        &self,
        org_slug: &str,
        milestone_id: &str,
        body: &MutateOrganizationMilestoneReleaseRequest,
        if_match: &str,
        idempotency_key: &str,
    ) -> Result<ApiResponse<OrganizationMilestone>, ClientError> {
        let payload = serde_json::to_value(body)
            .map_err(|err| ClientError::Protocol(format!("invalid request body: {err}")))?;
        self.send_json(RequestSpec {
            method: Method::POST,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "milestones".to_string(),
                milestone_id.to_string(),
                "releases".to_string(),
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

    async fn list_organization_milestone_events(
        &self,
        org_slug: &str,
        milestone_id: &str,
        query: MilestoneEventsQuery,
    ) -> Result<ApiResponse<OrganizationMilestoneEventList>, ClientError> {
        let mut query_pairs = Vec::new();
        push_opt(&mut query_pairs, "beforeEventId", query.before_event_id);
        push_opt(
            &mut query_pairs,
            "limit",
            query.limit.map(|limit| limit.to_string()),
        );
        self.send_json(RequestSpec {
            method: Method::GET,
            segments: vec![
                "organizations".to_string(),
                org_slug.to_string(),
                "milestones".to_string(),
                milestone_id.to_string(),
                "events".to_string(),
            ],
            query: query_pairs,
            headers: Vec::new(),
            body: None,
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

    /// Sleeper that records every requested delay so tests can assert the
    /// transport never waited.
    #[derive(Default)]
    struct RecordingSleeper {
        sleeps: std::sync::Mutex<Vec<Duration>>,
    }

    #[async_trait::async_trait]
    impl crate::retry::Sleeper for RecordingSleeper {
        async fn sleep(&self, duration: Duration) {
            self.sleeps.lock().unwrap().push(duration);
        }
    }

    fn recording_client(uri: &str, sleeper: Arc<RecordingSleeper>) -> HamstikClient {
        let host = Host::parse(uri).unwrap();
        HamstikClient::with_sleeper(
            host,
            SecretString::from("tok".to_string()),
            ClientConfig {
                retry: RetryPolicy::none(),
                ..ClientConfig::default()
            },
            sleeper,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn failed_request_reports_observation_to_observer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(
                ResponseTemplate::new(500)
                    .insert_header("X-Request-Id", "req-obs")
                    .set_body_json(serde_json::json!({
                        "error": {"code": "INTERNAL_ERROR", "message": "boom"}
                    })),
            )
            .mount(&server)
            .await;

        #[derive(Default)]
        struct RecordingObserver {
            seen: std::sync::Mutex<Vec<RequestObservation>>,
        }
        impl RequestObserver for RecordingObserver {
            fn observe(&self, observation: &RequestObservation) {
                self.seen.lock().unwrap().push(observation.clone());
            }
        }

        let observer = Arc::new(RecordingObserver::default());
        let client = test_client(&server.uri()).with_request_observer(observer.clone());
        let err = client.whoami().await.unwrap_err();
        assert!(matches!(err, ClientError::Api(ref api) if api.status == 500));

        let seen = observer.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "exactly one observation per failed request");
        let observation = &seen[0];
        assert_eq!(observation.method, "GET");
        assert_eq!(observation.path, "/api/v1/me");
        assert_eq!(observation.status, Some(500));
        assert_eq!(observation.request_id.as_deref(), Some("req-obs"));
        assert!(!observation.transport);
        assert!(
            observation
                .header_names
                .iter()
                .any(|name| name == "authorization"),
            "header intent records auth: {:?}",
            observation.header_names
        );
        assert!(
            observation.header_names.iter().any(|name| name == "accept"),
            "header intent records accept: {:?}",
            observation.header_names
        );
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

    /// An explicit probe reports the snapshot even when the window is
    /// exhausted, and never applies the proactive depleted-window wait.
    #[tokio::test]
    async fn rate_limit_probe_reports_without_absorbing_the_window() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("RateLimit-Limit", "100")
                    .insert_header("RateLimit-Remaining", "0")
                    .insert_header("RateLimit-Reset", "30")
                    .insert_header("X-Request-Id", "req-probe")
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

        let sleeper = Arc::new(RecordingSleeper::default());
        let client = recording_client(&server.uri(), Arc::clone(&sleeper));
        let probe = client.probe_rate_limit().await.unwrap();
        let snapshot = probe.snapshot.expect("snapshot present");
        assert_eq!(snapshot.limit, 100);
        assert_eq!(snapshot.remaining, 0);
        assert_eq!(snapshot.reset_in, 30);
        assert_eq!(probe.request_id.as_deref(), Some("req-probe"));
        assert!(
            sleeper.sleeps.lock().unwrap().is_empty(),
            "a probe must not wait out a depleted window: {:?}",
            sleeper.sleeps.lock().unwrap()
        );
    }

    #[test]
    fn rate_limit_snapshot_json_shape_matches_the_meta_field() {
        let snapshot = RateLimitSnapshot {
            limit: 100,
            remaining: 37,
            reset_in: 12,
        };
        assert_eq!(
            snapshot.to_json(),
            serde_json::json!({"limit": 100, "remaining": 37, "resetIn": 12})
        );
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

    // --- Network-stage classification pinning -----------------------------
    //
    // The stage used to be re-derived from reqwest's rendered text by marker
    // matching; a reqwest/hyper rewording would silently degrade every
    // failure to `Unknown`. These tests run real transport failures and pin
    // the concrete-source classification so any reword fails loudly here
    // (and in CI) instead of degrading silently in the field.

    #[tokio::test]
    async fn unresolvable_host_classifies_as_dns() {
        // RFC 2606 reserves `.invalid`; resolution must fail with a DNS error
        // on every platform without touching a real server.
        let client = reqwest::Client::new();
        let err = client
            .get("https://no-such-host.invalid/api/v1/me")
            .send()
            .await
            .unwrap_err();
        assert_eq!(classify_network_error(&err), NetworkStage::Dns);
    }

    #[tokio::test]
    async fn refused_connection_classifies_as_connection() {
        // Port 1 on loopback is reserved and effectively never listening; the
        // connect attempt must fail with a connection-class io error.
        let client = reqwest::Client::new();
        let err = client.get("http://127.0.0.1:1/").send().await.unwrap_err();
        assert_eq!(classify_network_error(&err), NetworkStage::Connection);
    }

    #[tokio::test]
    async fn stalled_connect_classifies_as_timeout_or_connection() {
        // 10.255.255.1 is a non-routable black hole: the connect either times
        // out (expected) or the network stack fails it fast as unreachable
        // (rare, e.g. some sandboxes). Both are correct; `Unknown` is not.
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .build()
            .unwrap();
        let err = client
            .get("http://10.255.255.1:9/")
            .send()
            .await
            .unwrap_err();
        assert!(
            matches!(
                classify_network_error(&err),
                NetworkStage::Timeout | NetworkStage::Connection
            ),
            "expected Timeout or Connection, got {:?}: {}",
            classify_network_error(&err),
            err
        );
    }

    #[test]
    fn rendered_text_fallback_still_matches_known_markers() {
        use crate::error::network_stage_from_message;
        assert_eq!(
            network_stage_from_message("error sending request: certificate verify failed"),
            NetworkStage::Tls
        );
        assert_eq!(
            network_stage_from_message("error sending request: timed out"),
            NetworkStage::Timeout
        );
        assert_eq!(
            network_stage_from_message("dns error: failed to lookup address information"),
            NetworkStage::Dns
        );
        assert_eq!(
            network_stage_from_message("error sending request: connection refused"),
            NetworkStage::Connection
        );
        assert_eq!(
            network_stage_from_message("proxy connection failed"),
            NetworkStage::Proxy
        );
        assert_eq!(
            network_stage_from_message("something unrecognizable"),
            NetworkStage::Unknown
        );
    }

    #[test]
    fn windows_dns_message_classifies_as_dns() {
        // Windows `getaddrinfo` failures render through `io::Error` as
        // WSAHOST_NOT_FOUND / WSATRY_AGAIN prose with no stable ErrorKind;
        // the source-chain walk must recognize them instead of falling
        // through to `Unknown` (a real regression the Windows CI caught).
        for message in [
            "No such host is known. (os error 11001)",
            "A non-recoverable error occurred during a database lookup. (os error 11002)",
        ] {
            let io = std::io::Error::other(message);
            let stage = classify_from_source_chain(Some(&io));
            assert_eq!(stage, Some(NetworkStage::Dns), "{message}");
        }
        // Nothing recognizable stays `None` (falls back to rendered text).
        assert_eq!(
            classify_from_source_chain(Some(&std::io::Error::other("mysterious"))),
            None
        );
        assert_eq!(classify_from_source_chain(None), None);
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
