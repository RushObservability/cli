use std::{fmt, str::FromStr, time::Duration};

use anyhow::{Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

#[derive(Debug, Clone, Copy, Default, ValueEnum, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Signal {
    #[default]
    Logs,
    Apm,
}

impl Signal {
    pub fn toggled(self) -> Self {
        match self {
            Self::Logs => Self::Apm,
            Self::Apm => Self::Logs,
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Logs => "logs",
            Self::Apm => "apm",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Filter {
    pub field: String,
    pub op: String,
    pub value: Value,
}

impl fmt::Display for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self
            .value
            .as_str()
            .map(|value| {
                if value.chars().any(char::is_whitespace) {
                    format!("\"{value}\"")
                } else {
                    value.to_string()
                }
            })
            .unwrap_or_else(|| self.value.to_string());
        write!(f, "{}{}{}", self.field, self.op, value)
    }
}

impl FromStr for Filter {
    type Err = anyhow::Error;

    fn from_str(input: &str) -> Result<Self> {
        for op in [" NOT LIKE ", " LIKE ", "!=", ">=", "<=", "=", ">", "<", "~"] {
            if let Some((field, raw)) = input.split_once(op) {
                let field = field.trim();
                let raw = raw.trim();
                if field.is_empty() || raw.is_empty() {
                    bail!("filter must include a field and value: {input}")
                }
                let (op, value) = if op == "~" {
                    ("LIKE".to_string(), Value::String(format!("%{raw}%")))
                } else {
                    (op.trim().to_string(), parse_value(raw))
                };
                return Ok(Self {
                    field: field.to_string(),
                    op,
                    value,
                });
            }
        }
        bail!("invalid filter `{input}`; try service_name=gateway or duration_ns>=100000000")
    }
}

fn parse_value(raw: &str) -> Value {
    let raw = if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };

    if raw.eq_ignore_ascii_case("true") {
        Value::Bool(true)
    } else if raw.eq_ignore_ascii_case("false") {
        Value::Bool(false)
    } else if let Ok(number) = raw.parse::<i64>() {
        Value::Number(number.into())
    } else {
        Value::String(raw.to_string())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuerySpec {
    pub signal: Signal,
    pub search: String,
    pub filters: Vec<Filter>,
    pub window: Duration,
    pub limit: u16,
}

impl QuerySpec {
    /// Render the combined filter + free-text syntax used by the web Explore bar.
    pub fn search_input(&self) -> String {
        self.filters
            .iter()
            .map(ToString::to_string)
            .chain((!self.search.is_empty()).then(|| self.search.clone()))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Split the web Explore search syntax into structured filters and free text.
///
/// Unquoted `field=value` tokens become filters. Quoted phrases, boolean
/// keywords, and all other tokens remain in the free-text search expression.
pub fn parse_search_input(input: &str) -> (Vec<Filter>, String) {
    let mut filters = Vec::new();
    let mut text = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some((start, character)) = chars.next() {
        if character.is_whitespace() {
            continue;
        }

        if character == '"' {
            let mut end = input.len();
            for (index, next) in chars.by_ref() {
                end = index + next.len_utf8();
                if next == '"' {
                    break;
                }
            }
            text.push(input[start..end].to_string());
            continue;
        }

        let mut end = start + character.len_utf8();
        let mut quote = None;
        while let Some(&(index, next)) = chars.peek() {
            if quote.is_none() && next.is_whitespace() {
                break;
            }
            chars.next();
            end = index + next.len_utf8();
            match quote {
                Some(open) if next == open => quote = None,
                None if next == '"' || next == '\'' => quote = Some(next),
                _ => {}
            }
        }

        let token = &input[start..end];
        if token.eq_ignore_ascii_case("AND") || token.eq_ignore_ascii_case("OR") {
            text.push(token.to_string());
        } else if let Ok(filter) = token.parse::<Filter>() {
            filters.push(filter);
        } else {
            text.push(token.to_string());
        }
    }

    (filters, text.join(" "))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TailRecord {
    pub signal: Signal,
    pub timestamp_ns: i64,
    pub service: String,
    pub level: String,
    pub summary: String,
    pub trace_id: String,
    pub span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status_code: Option<u16>,
}

impl TailRecord {
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.signal, self.timestamp_ns, self.trace_id, self.span_id, self.summary
        )
    }

    pub fn timestamp(&self) -> String {
        let seconds = self.timestamp_ns.div_euclid(1_000_000_000);
        let nanos = self.timestamp_ns.rem_euclid(1_000_000_000) as u32;
        DateTime::<Utc>::from_timestamp(seconds, nanos)
            .map(|value| value.format("%H:%M:%S%.3f").to_string())
            .unwrap_or_else(|| "--:--:--.---".to_string())
    }

    pub fn web_url(&self, base: &str) -> Result<Url> {
        let mut url = Url::parse(base)?;
        if self.signal == Signal::Apm && !self.trace_id.is_empty() {
            url.set_path(&format!("/trace/{}", self.trace_id));
            url.set_query(None);
            return Ok(url);
        }

        url.set_path("/");
        let seconds = self.timestamp_ns.div_euclid(1_000_000_000);
        let nanos = self.timestamp_ns.rem_euclid(1_000_000_000) as u32;
        let target = DateTime::<Utc>::from_timestamp(seconds, nanos).unwrap_or_else(Utc::now);
        let from = target - chrono::Duration::seconds(5);
        let to = target + chrono::Duration::seconds(5);
        url.query_pairs_mut()
            .clear()
            .append_pair("mode", "logs")
            .append_pair("from", &from.to_rfc3339_opts(SecondsFormat::Millis, true))
            .append_pair("to", &to.to_rfc3339_opts(SecondsFormat::Millis, true))
            .append_pair("log", &self.timestamp_ns.to_string());
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_structured_and_contains_filters() {
        assert_eq!(
            "duration_ns>=100".parse::<Filter>().unwrap().value,
            Value::from(100)
        );
        let contains = "service_name~gate".parse::<Filter>().unwrap();
        assert_eq!(contains.op, "LIKE");
        assert_eq!(contains.value, Value::from("%gate%"));
    }

    #[test]
    fn parses_web_style_filters_and_free_text() {
        let (filters, search) = parse_search_input("service_name=gateway POST");

        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].to_string(), "service_name=gateway");
        assert_eq!(search, "POST");
    }

    #[test]
    fn mixed_search_preserves_quotes_and_boolean_keywords() {
        let (filters, search) = parse_search_input(
            "service_name='api gateway' http_status_code>=500 \"request failed\" OR timeout",
        );

        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].value, Value::String("api gateway".into()));
        assert_eq!(filters[1].value, Value::Number(500.into()));
        assert_eq!(search, "\"request failed\" OR timeout");
    }

    #[test]
    fn combined_search_round_trips_filter_values_with_spaces() {
        let spec = QuerySpec {
            signal: Signal::Logs,
            search: "POST".into(),
            filters: vec!["service_name='api gateway'".parse().unwrap()],
            window: Duration::from_secs(60),
            limit: 100,
        };

        let input = spec.search_input();
        let (filters, search) = parse_search_input(&input);

        assert_eq!(input, "service_name=\"api gateway\" POST");
        assert_eq!(filters, spec.filters);
        assert_eq!(search, "POST");
    }

    #[test]
    fn builds_trace_and_log_context_urls() {
        let mut record = TailRecord {
            signal: Signal::Apm,
            timestamp_ns: 1_700_000_000_000_000_000,
            service: "gateway".into(),
            level: "ok".into(),
            summary: "GET /articles".into(),
            trace_id: "abc123".into(),
            span_id: "def456".into(),
            duration_ns: Some(1),
            http_method: Some("GET".into()),
            http_path: Some("/articles".into()),
            http_status_code: Some(200),
        };
        assert_eq!(
            record.web_url("http://localhost:5173").unwrap().path(),
            "/trace/abc123"
        );
        record.signal = Signal::Logs;
        let url = record.web_url("http://localhost:5173").unwrap();
        assert!(url.query().unwrap().contains("mode=logs"));
        assert!(url.query().unwrap().contains("log=1700000000000000000"));
    }
}
