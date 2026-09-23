mod support;

use serde_json::{Value, json};
use support::{Process, Response, Server, command};

fn rows(timestamps: &[i64]) -> String {
    json!({"rows": timestamps.iter().map(|timestamp| json!({"Timestamp":timestamp,"ServiceName":"gateway","Body":format!("row-{timestamp}")})).collect::<Vec<_>>()}).to_string()
}

#[test]
fn json_tail_polls_deduplicates_recovers_and_sends_query_contract() {
    let server = Server::start(vec![
        Response::new(200, rows(&[2, 1])),
        Response::new(200, rows(&[2, 1])),
        Response::new(500, r#"{"message":"temporary failure"}"#),
        Response::new(200, rows(&[3, 2, 1])),
        Response::new(401, ""),
    ]);
    let (mut cmd, _directory) = command(&server.url);
    cmd.args([
        "--output",
        "json",
        "--tenant",
        "test-tenant",
        "--search",
        "service_name=gateway timeout",
        "--filter",
        "severity=ERROR",
        "--limit",
        "25",
        "--window-seconds",
        "90",
    ])
    .env("RUSH_API_KEY", "integration-test-key");
    let output = Process(cmd.spawn().unwrap()).output();
    assert!(!output.status.success()); // Authentication failure ends the stream.
    let values: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        values
            .iter()
            .map(|row| row["timestamp_ns"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("temporary failure"));
    assert!(error.contains("401"));
    assert!(!error.contains("integration-test-key"));
    let requests: Vec<_> = server.requests.try_iter().collect();
    assert_eq!(requests.len(), 5);
    for request in requests {
        assert!(request.starts_with("POST /api/v1/logs HTTP/1.1"));
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer integration-test-key")
        );
        assert!(
            request
                .to_lowercase()
                .contains("x-rush-tenant: test-tenant")
        );
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["limit"], 25);
        assert_eq!(body["slim"], true);
        assert_eq!(body["search"], "timeout");
        assert_eq!(body["filters"].as_array().unwrap().len(), 2);
        let from =
            chrono::DateTime::parse_from_rfc3339(body["time_range"]["from"].as_str().unwrap())
                .unwrap();
        let to = chrono::DateTime::parse_from_rfc3339(body["time_range"]["to"].as_str().unwrap())
            .unwrap();
        assert_eq!((to - from).num_seconds(), 90);
    }
}

#[test]
fn json_tail_exits_on_both_auth_failures_without_retry() {
    for status in [401, 403] {
        let server = Server::start(vec![Response::new(status, "")]);
        let (mut cmd, _directory) = command(&server.url);
        let output = Process(cmd.args(["--output", "json"]).spawn().unwrap()).output();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains(&status.to_string())
        );
        assert_eq!(server.requests.try_iter().count(), 1);
    }
}

#[test]
fn json_tail_apm_maps_http_fields_and_severity() {
    let server = Server::start(vec![Response::new(200,json!({"rows":[{"timestamp":42,"service_name":"payments","span_name":"request","http_method":"POST","http_path":"/pay","http_status_code":503,"duration_ns":1000,"trace_id":"trace","span_id":"span"}]}).to_string()),Response::new(403,"")]);
    let (mut cmd, _directory) = command(&server.url);
    let output = Process(cmd.args(["apm", "--output", "json"]).spawn().unwrap()).output();
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["signal"], "apm");
    assert_eq!(row["summary"], "POST /pay");
    assert_eq!(row["level"], "error");
    assert_eq!(row["duration_ns"], 1000);
    assert_eq!(row["trace_id"], "trace");
    assert_eq!(row["http_status_code"], 503);
    assert!(
        server
            .requests
            .try_iter()
            .all(|request| request.starts_with("POST /api/v1/query "))
    );
}

#[test]
fn closing_stdout_pipe_exits_successfully() {
    let server = Server::start(vec![Response::new(200, rows(&[1]))]);
    let (mut cmd, _directory) = command(&server.url);
    let mut process = Process(cmd.args(["--output", "json"]).spawn().unwrap());
    drop(process.0.stdout.take());
    let output = process.output();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn ctrl_c_interrupts_an_in_flight_json_request() {
    let server = Server::start(vec![Response {
        status: 200,
        body: String::new(),
        hang: true,
    }]);
    let (mut cmd, _directory) = command(&server.url);
    let process = Process(cmd.args(["--output", "json"]).spawn().unwrap());
    server
        .requests
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    // SAFETY: The owned child is still alive and has installed its SIGINT handler before fetching.
    assert_eq!(
        unsafe { libc::kill(process.0.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    assert!(process.output().status.success());
}
