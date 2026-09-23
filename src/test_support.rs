use std::time::Duration;

use crate::{
    config::Config,
    model::{QuerySpec, Signal, TailRecord},
};

pub fn config(url: String) -> Config {
    Config {
        url,
        web_url: "http://localhost:5173".into(),
        tenant: "default".into(),
        api_key: None,
        poll_interval_ms: 250,
        window_seconds: 60,
        buffer_size: 100,
    }
}

pub fn spec() -> QuerySpec {
    QuerySpec {
        signal: Signal::Logs,
        search: String::new(),
        filters: vec![],
        window: Duration::from_secs(60),
        limit: 100,
    }
}

pub fn record() -> TailRecord {
    TailRecord {
        signal: Signal::Logs,
        timestamp_ns: 1,
        service: "gateway".into(),
        level: "ERROR".into(),
        summary: "日本語 🚨 request failed".into(),
        trace_id: "trace-123".into(),
        span_id: "span-123".into(),
        duration_ns: None,
        http_method: None,
        http_path: None,
        http_status_code: None,
    }
}
