use super::*;

#[test]
fn every_filter_operator_and_scalar_type() {
    for op in ["=", "!=", ">", "<", ">=", "<=", " LIKE ", " NOT LIKE "] {
        let filter: Filter = format!("field{op}42").parse().unwrap();
        assert_eq!(filter.op, op.trim());
        assert_eq!(filter.value, Value::from(42));
    }
    for (raw, expected) in [
        ("true", Value::Bool(true)),
        ("FALSE", Value::Bool(false)),
        ("-42", Value::from(-42)),
        ("\"42\"", Value::from("42")),
        ("''", Value::from("")),
    ] {
        assert_eq!(
            format!("field={raw}").parse::<Filter>().unwrap().value,
            expected
        );
    }
    for input in ["field", "=value", "field=", " "] {
        assert!(input.parse::<Filter>().is_err());
    }
}

#[test]
fn operators_inside_values_do_not_change_the_field() {
    for value in ["a!=b", "a>=b", "a LIKE b", "a NOT LIKE b", "日本語 🚨"] {
        let filter: Filter = format!("message={value:?}").parse().unwrap();
        assert_eq!(filter.field, "message");
        assert_eq!(filter.op, "=");
        assert_eq!(filter.value, Value::from(value));
    }
}

#[test]
fn string_filters_round_trip_through_search_editor() {
    for value in [
        "",
        "42",
        "true",
        "api gateway",
        "say \"hello world\"",
        "C:\\logs\\error",
        "a=b",
        "it's slow",
        "日本語 🚨",
    ] {
        for op in ["=", "!=", "LIKE", "NOT LIKE"] {
            let filter = Filter {
                field: "service_name".into(),
                op: op.into(),
                value: Value::from(value),
            };
            let input = filter.to_string();
            assert_eq!(input.parse::<Filter>().unwrap(), filter, "{input}");
            let (filters, text) = parse_search_input(&input);
            assert_eq!(filters, vec![filter], "{input}");
            assert!(text.is_empty(), "{input}");
        }
    }
}

#[test]
fn quoted_phrases_and_boolean_search_stay_free_text() {
    for input in [
        "\"request failed\" OR timeout",
        "'request failed' AND timeout",
        "\"say \\\"hello world\\\"\" OR error",
    ] {
        let (filters, text) = parse_search_input(input);
        assert!(filters.is_empty());
        assert_eq!(text, input);
    }
}

fn record() -> TailRecord {
    TailRecord {
        signal: Signal::Logs,
        timestamp_ns: 0,
        service: "one".into(),
        level: "info".into(),
        summary: "same".into(),
        trace_id: String::new(),
        span_id: String::new(),
        duration_ns: None,
        http_method: None,
        http_path: None,
        http_status_code: None,
    }
}

#[test]
fn record_keys_distinguish_services_and_delimiters() {
    let first = record();
    let mut other = first.clone();
    other.service = "two".into();
    assert_ne!(first.key(), other.key());
    let mut first = first;
    first.trace_id = "a:b".into();
    first.span_id = "c".into();
    other = first.clone();
    other.trace_id = "a".into();
    other.span_id = "b:c".into();
    assert_ne!(first.key(), other.key());
    assert_eq!(first.key(), first.clone().key());
}

#[test]
fn timestamps_and_context_urls_preserve_boundaries() {
    let mut record = record();
    record.timestamp_ns = -1;
    assert_eq!(record.timestamp(), "23:59:59.999");
    for timestamp in [i64::MIN, -1, 0, i64::MAX] {
        record.timestamp_ns = timestamp;
        let url = record.web_url("https://rush.example").unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(pairs["log"], timestamp.to_string());
        let from = DateTime::parse_from_rfc3339(&pairs["from"]).unwrap();
        let to = DateTime::parse_from_rfc3339(&pairs["to"]).unwrap();
        assert_eq!((to - from).num_seconds(), 10);
    }
}
