use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

use crate::{
    config::Config,
    model::{QuerySpec, Signal, TailRecord},
};

#[derive(Debug, Error)]
pub enum ApiError {
    #[error(
        "Rush rejected the API key or session (401). Create an API key in Settings and set RUSH_API_KEY"
    )]
    Unauthorized,
    #[error("Rush denied access to this tenant or operation (403)")]
    Forbidden,
    #[error("Rush returned {status}: {message}")]
    Response { status: StatusCode, message: String },
    #[error(
        "Rush sent more than {max} bytes in one response; refusing to buffer it. \
This should not happen against a healthy server"
    )]
    ResponseTooLarge { max: usize },
    #[error("request failed: {0}")]
    Transport(#[from] reqwest::Error),
}

/// Largest response body we will buffer, in bytes.
///
/// A tail query asks for at most a few thousand rows, so a healthy server stays
/// far below this. The cap exists so a hostile or malfunctioning server cannot
/// make the client allocate without bound: the request timeout limits how long
/// a body may stream for, but says nothing about how large it may be.
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// Read a response body, refusing to buffer more than `max` bytes.
///
/// Chunks are counted as they arrive rather than trusting Content-Length,
/// which a hostile server can omit or understate; chunked encoding has no
/// length at all. Reading stops at the first chunk that would exceed the cap,
/// so the peak allocation is bounded by `max` plus one chunk.
async fn read_body_capped(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, ApiError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > max {
            return Err(ApiError::ResponseTooLarge { max });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Clone)]
pub struct RushClient {
    http: Client,
    base_url: String,
    tenant: String,
    api_key: Option<String>,
}

impl RushClient {
    pub fn new(config: &Config) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(20))
                .user_agent(concat!("rush-cli/", env!("CARGO_PKG_VERSION")))
                .build()?,
            base_url: config.url.clone(),
            tenant: config.tenant.clone(),
            api_key: config.api_key.clone(),
        })
    }

    pub async fn fetch(&self, spec: &QuerySpec) -> Result<Vec<TailRecord>, ApiError> {
        let now = Utc::now();
        let from = now
            - chrono::Duration::from_std(spec.window)
                .unwrap_or_else(|_| chrono::Duration::minutes(5));
        let time_range = json!({
            "from": from.to_rfc3339_opts(SecondsFormat::Millis, true),
            "to": now.to_rfc3339_opts(SecondsFormat::Millis, true),
        });
        let search = (!spec.search.trim().is_empty()).then(|| spec.search.trim());

        match spec.signal {
            Signal::Logs => {
                let body = json!({
                    "time_range": time_range,
                    "filters": spec.filters,
                    "limit": spec.limit,
                    "offset": 0,
                    "search": search,
                    "slim": true,
                });
                let response: LogResponse = self.post("/api/v1/logs", &body).await?;
                Ok(response.rows.into_iter().map(Into::into).collect())
            }
            Signal::Apm => {
                let body = json!({
                    "time_range": time_range,
                    "filters": spec.filters,
                    "group_by": [],
                    "aggregation": "count",
                    "limit": spec.limit,
                    "offset": 0,
                    "search": search,
                    "columns": "list",
                });
                let response: SpanResponse = self.post("/api/v1/query", &body).await?;
                Ok(response.rows.into_iter().map(Into::into).collect())
            }
        }
    }

    async fn post<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, ApiError> {
        let mut request = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("X-Rush-Tenant", &self.tenant)
            .json(body);
        if let Some(key) = self.api_key.as_deref() {
            request = request.bearer_auth(key);
        }
        let response = request.send().await?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized);
        }
        if status == StatusCode::FORBIDDEN {
            return Err(ApiError::Forbidden);
        }
        if !status.is_success() {
            // Error bodies are attacker-influenced too, so they get the same cap.
            let message = read_body_capped(response, MAX_RESPONSE_BYTES)
                .await
                .ok()
                .and_then(|body| String::from_utf8(body).ok())
                .unwrap_or_else(|| "request failed".to_string());
            let message = serde_json::from_str::<serde_json::Value>(&message)
                .ok()
                .and_then(|value| value.get("message")?.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("request failed ({status})"));
            return Err(ApiError::Response { status, message });
        }
        let body = read_body_capped(response, MAX_RESPONSE_BYTES).await?;
        serde_json::from_slice(&body).map_err(|error| ApiError::Response {
            status,
            message: format!("could not parse the response: {error}"),
        })
    }
}

#[derive(Debug, Deserialize)]
struct LogResponse {
    rows: Vec<LogRow>,
}

#[derive(Debug, Deserialize)]
struct LogRow {
    #[serde(rename = "Timestamp")]
    timestamp: i64,
    #[serde(rename = "TraceId", default)]
    trace_id: String,
    #[serde(rename = "SpanId", default)]
    span_id: String,
    #[serde(rename = "SeverityText", default)]
    severity_text: String,
    #[serde(rename = "ServiceName", default)]
    service_name: String,
    #[serde(rename = "Body", default)]
    body: String,
}

impl From<LogRow> for TailRecord {
    fn from(row: LogRow) -> Self {
        Self {
            signal: Signal::Logs,
            timestamp_ns: row.timestamp,
            service: row.service_name,
            level: row.severity_text.to_lowercase(),
            summary: row.body,
            trace_id: row.trace_id,
            span_id: row.span_id,
            duration_ns: None,
            http_method: None,
            http_path: None,
            http_status_code: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct SpanResponse {
    rows: Vec<SpanRow>,
}

#[derive(Debug, Deserialize)]
struct SpanRow {
    timestamp: i64,
    #[serde(default)]
    service_name: String,
    #[serde(default)]
    span_name: String,
    #[serde(default)]
    http_method: String,
    #[serde(default)]
    http_path: String,
    #[serde(default)]
    http_status_code: u16,
    #[serde(default)]
    duration_ns: u64,
    #[serde(default)]
    status: String,
    #[serde(default)]
    trace_id: String,
    #[serde(default)]
    span_id: String,
}

impl From<SpanRow> for TailRecord {
    fn from(row: SpanRow) -> Self {
        let summary = match (row.http_method.is_empty(), row.http_path.is_empty()) {
            (false, false) => format!("{} {}", row.http_method, row.http_path),
            _ => row.span_name,
        };
        let level = if row.http_status_code >= 500 || row.status.eq_ignore_ascii_case("error") {
            "error".to_string()
        } else if row.http_status_code >= 400 {
            "warn".to_string()
        } else {
            row.status.to_lowercase()
        };
        Self {
            signal: Signal::Apm,
            timestamp_ns: row.timestamp,
            service: row.service_name,
            level,
            summary,
            trace_id: row.trace_id,
            span_id: row.span_id,
            duration_ns: Some(row.duration_ns),
            http_method: (!row.http_method.is_empty()).then_some(row.http_method),
            http_path: (!row.http_path.is_empty()).then_some(row.http_path),
            http_status_code: (row.http_status_code > 0).then_some(row.http_status_code),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use httpmock::{Method::POST, MockServer};

    use crate::{
        config::Config,
        model::{Filter, QuerySpec, Signal},
    };

    use super::*;

    fn config(server: &MockServer) -> Config {
        Config {
            url: server.base_url(),
            web_url: "http://localhost:5173".into(),
            tenant: "default".into(),
            api_key: Some("test-key".into()),
            poll_interval_ms: 1000,
            window_seconds: 300,
            buffer_size: 5000,
        }
    }

    #[tokio::test]
    async fn body_within_the_cap_is_read_in_full() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/echo");
            then.status(200).body("x".repeat(512));
        });
        let response = reqwest::Client::new()
            .post(server.url("/echo"))
            .send()
            .await
            .expect("request should reach the mock server");
        let body = read_body_capped(response, 1024)
            .await
            .expect("a body under the cap must be accepted");
        assert_eq!(body.len(), 512);
    }

    #[tokio::test]
    async fn body_over_the_cap_is_refused() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/flood");
            then.status(200).body("x".repeat(4096));
        });
        let response = reqwest::Client::new()
            .post(server.url("/flood"))
            .send()
            .await
            .expect("request should reach the mock server");
        let error = read_body_capped(response, 256)
            .await
            .expect_err("a body over the cap must be refused");
        assert!(
            matches!(error, ApiError::ResponseTooLarge { max: 256 }),
            "expected ResponseTooLarge, got {error:?}"
        );
        // The message must be actionable without leaking the body back.
        let text = error.to_string();
        assert!(text.contains("256"), "error should state the cap: {text}");
        assert!(
            !text.contains("xxxx"),
            "error must not echo the body: {text}"
        );
    }

    #[tokio::test]
    async fn a_body_exactly_at_the_cap_is_accepted() {
        // Guards an off-by-one at the boundary.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/exact");
            then.status(200).body("x".repeat(100));
        });
        let response = reqwest::Client::new()
            .post(server.url("/exact"))
            .send()
            .await
            .expect("request should reach the mock server");
        let body = read_body_capped(response, 100)
            .await
            .expect("a body exactly at the cap must be accepted");
        assert_eq!(body.len(), 100);
    }

    #[tokio::test]
    async fn log_query_uses_auth_tenant_search_and_slim_rows() {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/logs")
                .header("authorization", "Bearer test-key")
                .header("x-rush-tenant", "default")
                .body_includes("\"search\":\"panic\"")
                .body_includes("\"slim\":true");
            then.status(200).json_body(json!({
                "rows": [{
                    "Timestamp": 1_700_000_000_000_000_000_i64,
                    "TraceId": "trace-1",
                    "SpanId": "span-1",
                    "SeverityText": "ERROR",
                    "SeverityNumber": 17,
                    "ServiceName": "gateway",
                    "Body": "panic in request",
                    "TimestampNs": "1700000000000000000",
                    "BlockNumber": "1",
                    "BlockOffset": "2",
                    "BodyHash": "3"
                }],
                "total": 1
            }));
        });
        let spec = QuerySpec {
            signal: Signal::Logs,
            search: "panic".into(),
            filters: vec![],
            window: Duration::from_secs(60),
            limit: 100,
        };

        let rows = RushClient::new(&config(&server))
            .unwrap()
            .fetch(&spec)
            .await
            .unwrap();

        request.assert();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].service, "gateway");
        assert_eq!(rows[0].level, "error");
    }

    #[tokio::test]
    async fn apm_query_sends_structured_filters_and_list_projection() {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/query")
                .body_includes("\"columns\":\"list\"")
                .body_includes("\"field\":\"service_name\"");
            then.status(200).json_body(json!({
                "rows": [{
                    "timestamp": 1_700_000_000_000_000_000_i64,
                    "service_name": "articles",
                    "span_name": "GET /articles",
                    "http_method": "GET",
                    "http_path": "/articles",
                    "http_status_code": 200,
                    "duration_ns": 25_000_000,
                    "status": "ok",
                    "trace_id": "trace-2",
                    "span_id": "span-2"
                }],
                "total": 1
            }));
        });
        let spec = QuerySpec {
            signal: Signal::Apm,
            search: String::new(),
            filters: vec!["service_name=articles".parse::<Filter>().unwrap()],
            window: Duration::from_secs(60),
            limit: 100,
        };

        let rows = RushClient::new(&config(&server))
            .unwrap()
            .fetch(&spec)
            .await
            .unwrap();

        request.assert();
        assert_eq!(rows[0].signal, Signal::Apm);
        assert_eq!(rows[0].summary, "GET /articles");
        assert_eq!(rows[0].duration_ns, Some(25_000_000));
    }

    #[tokio::test]
    async fn maps_unauthorized_responses_to_actionable_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/api/v1/logs");
            then.status(401);
        });
        let spec = QuerySpec {
            signal: Signal::Logs,
            search: String::new(),
            filters: vec![],
            window: Duration::from_secs(60),
            limit: 100,
        };

        let error = RushClient::new(&config(&server))
            .unwrap()
            .fetch(&spec)
            .await
            .unwrap_err();

        assert!(matches!(error, ApiError::Unauthorized));
        assert!(error.to_string().contains("RUSH_API_KEY"));
    }
}
