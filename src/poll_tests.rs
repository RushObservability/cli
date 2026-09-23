use super::*;
use httpmock::{Method::POST, MockServer};
use serde_json::json;
use test_support::{config, record, spec};

async fn next(rx: &mut mpsc::Receiver<PollUpdate>) -> PollUpdate {
    tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn query_switch_cancels_slow_fetch_for_search_and_signal() {
    for signal in [model::Signal::Logs, model::Signal::Apm] {
        let server = MockServer::start();
        let slow = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/logs")
                .body_includes("old-query");
            then.status(200)
                .delay(Duration::from_secs(10))
                .json_body(json!({"rows":[]}));
        });
        let fresh = server.mock(|when, then| {
            when.method(POST)
                .path(if signal == model::Signal::Logs {
                    "/api/v1/logs"
                } else {
                    "/api/v1/query"
                })
                .body_includes("new-query");
            then.status(200).json_body(json!({"rows":[]}));
        });
        let mut old = spec();
        old.search = "old-query".into();
        let (tx, rx) = watch::channel(old.clone());
        let (events, mut updates) = mpsc::channel(8);
        let task = tokio::spawn(poll(
            RushClient::new(&config(server.base_url())).unwrap(),
            rx,
            events,
            60_000,
        ));
        tokio::time::timeout(Duration::from_secs(3), async {
            while slow.calls_async().await == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let mut new = old;
        new.search = "new-query".into();
        new.signal = signal;
        tx.send(new.clone()).unwrap();
        let update = next(&mut updates).await;
        assert_eq!(update.spec, new);
        assert!(matches!(update.event, PollEvent::Records(_)));
        fresh.assert_calls(1);
        drop(tx);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
    }
}

#[test]
fn queued_old_records_and_errors_cannot_overwrite_new_query() {
    let old = spec();
    let mut new = old.clone();
    new.search = "new".into();
    let (tx, _) = watch::channel(new.clone());
    let mut app = App::new(new, "http://localhost".into(), 100, tx);
    for event in [
        PollEvent::Records(vec![record()]),
        PollEvent::Error("old error".into()),
    ] {
        PollUpdate {
            spec: old.clone(),
            event,
        }
        .apply(&mut app);
        assert!(app.records.is_empty());
        assert!(app.error.is_none());
    }
    PollUpdate {
        spec: app.spec.clone(),
        event: PollEvent::Records(vec![record()]),
    }
    .apply(&mut app);
    assert_eq!(app.records.len(), 1);
}

#[tokio::test]
async fn poll_recovers_after_error_and_stops_when_consumer_closes() {
    let server = MockServer::start();
    let mut failure = server.mock(|when, then| {
        when.method(POST);
        then.status(500);
    });
    let (_tx, rx) = watch::channel(spec());
    let (events, mut updates) = mpsc::channel(8);
    let task = tokio::spawn(poll(
        RushClient::new(&config(server.base_url())).unwrap(),
        rx,
        events,
        30,
    ));
    assert!(matches!(
        next(&mut updates).await.event,
        PollEvent::Error(_)
    ));
    failure.delete();
    server.mock(|when, then| {
        when.method(POST);
        then.status(200).json_body(json!({"rows":[]}));
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if matches!(next(&mut updates).await.event, PollEvent::Records(_)) {
                break;
            }
        }
    })
    .await
    .unwrap();
    drop(updates);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn closing_query_channel_interrupts_in_flight_request() {
    let server = MockServer::start();
    let slow = server.mock(|when, then| {
        when.method(POST);
        then.status(200).delay(Duration::from_secs(10));
    });
    let (tx, rx) = watch::channel(spec());
    let (events, _updates) = mpsc::channel(1);
    let task = tokio::spawn(poll(
        RushClient::new(&config(server.base_url())).unwrap(),
        rx,
        events,
        60_000,
    ));
    tokio::time::timeout(Duration::from_secs(3), async {
        while slow.calls_async().await == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(tx);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn query_switch_interrupts_a_backpressured_update() {
    let server = MockServer::start();
    let old = server.mock(|when, then| {
        when.method(POST).body_includes("old-query");
        then.status(200).json_body(json!({"rows":[]}));
    });
    let fresh = server.mock(|when, then| {
        when.method(POST).body_includes("new-query");
        then.status(200).json_body(json!({"rows":[]}));
    });
    let mut query = spec();
    query.search = "old-query".into();
    let (tx, rx) = watch::channel(query.clone());
    let (events, mut updates) = mpsc::channel(1);
    let task = tokio::spawn(poll(
        RushClient::new(&config(server.base_url())).unwrap(),
        rx,
        events,
        10,
    ));
    // The first response fills the channel; the second send must wait for the UI.
    tokio::time::timeout(Duration::from_secs(3), async {
        while old.calls_async().await < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    query.search = "new-query".into();
    tx.send(query.clone()).unwrap();
    // The new request must start even though the UI has not drained the channel.
    tokio::time::timeout(Duration::from_secs(3), async {
        while fresh.calls_async().await == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(next(&mut updates).await.spec.search, "old-query");
    assert_eq!(next(&mut updates).await.spec, query);
    drop(updates);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}
