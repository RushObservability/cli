use super::*;
use crate::test_support::{config, spec};
use httpmock::{Method::POST, MockServer};

#[tokio::test]
async fn forbidden_and_rate_limit_preserve_status_and_safe_reason() {
    for status in [403, 429] {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(POST);
            then.status(status)
                .json_body(json!({"message":"slow down\n\u{001b}[2J"}));
        });
        let error = RushClient::new(&config(server.base_url()))
            .unwrap()
            .fetch(&spec())
            .await
            .unwrap_err();
        match error {
            ApiError::Forbidden => assert_eq!(status, 403),
            ApiError::Response {
                status: actual,
                message,
            } => {
                assert_eq!(actual.as_u16(), 429);
                assert_eq!(message, "slow down[2J");
            }
            error => panic!("unexpected error: {error}"),
        }
        request.assert_calls(1);
    }
}

#[tokio::test]
async fn malformed_and_missing_response_fields_fail_for_both_signals() {
    for signal in [Signal::Logs, Signal::Apm] {
        for body in [
            "not JSON",
            "{}",
            r#"{"rows":null}"#,
            r#"{"rows":[{}]}"#,
            r#"{"rows":[{"Timestamp":"yesterday"}]}"#,
        ] {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(POST);
                then.status(200).body(body);
            });
            let mut query = spec();
            query.signal = signal;
            let error = RushClient::new(&config(server.base_url()))
                .unwrap()
                .fetch(&query)
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                ApiError::Response {
                    status: StatusCode::OK,
                    ..
                }
            ));
            assert!(error.to_string().contains("could not parse"));
        }
    }
}

#[tokio::test]
async fn minimal_rows_and_empty_results_are_valid() {
    for signal in [Signal::Logs, Signal::Apm] {
        let row = if signal == Signal::Logs {
            json!({"Timestamp":1})
        } else {
            json!({"timestamp":1})
        };
        for rows in [json!([]), json!([row])] {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(POST);
                then.status(200).json_body(json!({"rows":rows}));
            });
            let mut query = spec();
            query.signal = signal;
            let result = RushClient::new(&config(server.base_url()))
                .unwrap()
                .fetch(&query)
                .await
                .unwrap();
            assert_eq!(result.len(), rows.as_array().unwrap().len());
            if let Some(row) = result.first() {
                assert_eq!(row.timestamp_ns, 1);
                assert_eq!(row.signal, signal);
            }
        }
    }
}

#[tokio::test]
async fn request_timeout_is_a_transport_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST);
        then.status(200).delay(Duration::from_secs(2));
    });
    let mut client = RushClient::new(&config(server.base_url())).unwrap();
    client.http = Client::builder()
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    let error = client.fetch(&spec()).await.unwrap_err();
    assert!(matches!(error, ApiError::Transport(error) if error.is_timeout()));
}

// Raw HTTP lets these tests control framing that HTTP mocking libraries normalize.
fn raw_server(response: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "no HTTP request arrived"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        socket.write_all(response).unwrap();
    });
    (url, task)
}

#[tokio::test]
async fn chunked_body_is_capped_without_content_length() {
    let (url, task) = raw_server(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\nabcd\r\n4\r\nefgh\r\n0\r\n\r\n");
    let response = Client::new().get(url).send().await.unwrap();
    assert_eq!(response.content_length(), None);
    assert!(matches!(
        read_body_capped(response, 7).await,
        Err(ApiError::ResponseTooLarge { max: 7 })
    ));
    task.join().unwrap();
}

#[tokio::test]
async fn disconnect_during_response_is_not_empty_success() {
    let (url, task) = raw_server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"rows\":",
    );
    let response = Client::new().get(url).send().await.unwrap();
    assert!(matches!(
        read_body_capped(response, 100).await,
        Err(ApiError::Transport(_))
    ));
    task.join().unwrap();
}
