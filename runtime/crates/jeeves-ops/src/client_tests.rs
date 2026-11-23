//! Transport-layer tests. The fake server is just two pipes in memory — no real process,
//! no real disk: the only thing to verify here is "how frames go in and out", so test that.

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::io::{BufReader, DuplexStream, ReadHalf, WriteHalf};

use super::*;

pub(crate) struct Fake {
    requests: BufReader<ReadHalf<DuplexStream>>,
    to_client: WriteHalf<DuplexStream>,
}

pub(crate) fn fake() -> (AppServer, Fake) {
    fake_at(std::env::temp_dir())
}

pub(crate) fn fake_at(home: std::path::PathBuf) -> (AppServer, Fake) {
    let (ours, theirs) = tokio::io::duplex(8192);
    let (reader, writer) = tokio::io::split(ours);
    let (theirs_read, theirs_write) = tokio::io::split(theirs);
    let app = AppServer::attach(home, None, reader, writer);
    let fake = Fake {
        requests: BufReader::new(theirs_read),
        to_client: theirs_write,
    };
    (app, fake)
}

impl Fake {
    pub(crate) async fn next_request(&mut self) -> Value {
        let mut line = String::new();
        self.requests
            .read_line(&mut line)
            .await
            .expect("read the client's request");
        serde_json::from_str(line.trim()).expect("the request is JSON")
    }

    async fn send(&mut self, frame: Value) {
        let line = format!("{frame}\n");
        self.to_client
            .write_all(line.as_bytes())
            .await
            .expect("write to the client");
    }

    pub(crate) async fn respond(&mut self, id: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await;
    }

    pub(crate) async fn fail(&mut self, id: &Value, code: i32, message: &str) {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }))
        .await;
    }

    async fn notify(&mut self, notification: Value) {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "turn/notification",
            "params": notification,
        }))
        .await;
    }

    async fn raw(&mut self, text: &str) {
        let line = format!("{text}\n");
        self.to_client
            .write_all(line.as_bytes())
            .await
            .expect("write to the client");
    }
}

fn spawn_call(
    app: &Arc<AppServer>,
    method: &'static str,
) -> tokio::task::JoinHandle<Result<Value, ErrorObject>> {
    let app = Arc::clone(app);
    tokio::spawn(async move { app.call(method, json!({})).await })
}

#[tokio::test]
async fn shutdown_wakes_late_subscribers_rejects_new_calls_and_keeps_existing_reply() {
    let (app, mut fake) = fake();
    let app = Arc::new(app);
    let in_flight = spawn_call(&app, "turn/start");
    let request = fake.next_request().await;
    app.begin_shutdown();
    tokio::time::timeout(std::time::Duration::from_millis(100), app.stopping())
        .await
        .unwrap();
    assert!(app.call("session/list", json!({})).await.is_err());
    fake.respond(&request["id"], json!({"finished":true})).await;
    assert_eq!(in_flight.await.unwrap().unwrap()["finished"], true);
}

#[tokio::test]
async fn child_eof_signals_shutdown_and_refuses_later_requests() {
    let (app, fake) = fake();
    drop(fake);
    tokio::time::timeout(std::time::Duration::from_secs(1), app.stopping())
        .await
        .unwrap();
    assert!(app.call("session/list", json!({})).await.is_err());
}

#[tokio::test]
async fn a_cancelled_request_does_not_leak_its_pending_entry() {
    let (app, mut fake) = fake();
    let app = Arc::new(app);
    let call = spawn_call(&app, "session/list");
    let _request = fake.next_request().await;
    assert_eq!(app.pending().len(), 1);
    call.abort();
    let _ = call.await;
    assert!(app.pending().is_empty());
}

#[tokio::test]
async fn responses_pair_by_id_even_when_they_come_back_out_of_order() {
    let (app, mut fake) = fake();
    let app = Arc::new(app);

    let first = spawn_call(&app, "memory/list");
    let second = spawn_call(&app, "session/list");

    let one = fake.next_request().await;
    let two = fake.next_request().await;
    assert_eq!(one["method"], "memory/list");
    assert_eq!(two["method"], "session/list");
    assert_ne!(
        one["id"], two["id"],
        "every request has to carry its own id"
    );

    fake.respond(&two["id"], json!({"data": ["second"]})).await;
    fake.respond(&one["id"], json!({"data": ["first"]})).await;

    assert_eq!(second.await.unwrap().unwrap()["data"][0], "second");
    assert_eq!(first.await.unwrap().unwrap()["data"][0], "first");
}

#[tokio::test]
async fn notifications_are_broadcast_unchanged() {
    let (app, mut fake) = fake();
    let mut events = app.subscribe();

    let call = spawn_call(&Arc::new(app), "turn/start");
    let request = fake.next_request().await;
    fake.notify(json!({"type": "textDelta", "turnId": "t1", "delta": "you"}))
        .await;
    fake.respond(&request["id"], json!({"turnId": "t1"})).await;

    match events.recv().await.expect("the notice arrives") {
        ServerNotification::TextDelta(delta) => {
            assert_eq!(delta.turn_id, "t1");
            assert_eq!(delta.delta, "you");
        }
        other => panic!("this should be textDelta, got {other:?}"),
    }
    assert!(call.await.unwrap().is_ok());
}

#[tokio::test]
async fn an_error_carries_its_code_back() {
    let (app, mut fake) = fake();
    let app = Arc::new(app);

    let call = spawn_call(&app, "turn/start");
    let request = fake.next_request().await;
    fake.fail(
        &request["id"],
        codes::PROVIDER_ERROR,
        "no API key: add ANTHROPIC_API_KEY to .env",
    )
    .await;

    let error = call.await.unwrap().expect_err("it should be an error");
    assert_eq!(error.code, codes::PROVIDER_ERROR);
    assert!(
        error.message.contains("ANTHROPIC_API_KEY"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn an_unreadable_line_does_not_break_the_frames_after_it() {
    let (app, mut fake) = fake();
    let app = Arc::new(app);

    let call = spawn_call(&app, "memory/list");
    let request = fake.next_request().await;
    fake.raw("this is not JSON").await;
    fake.respond(&request["id"], json!({"data": []})).await;

    assert!(
        call.await.unwrap().is_ok(),
        "after a bad line the responses still have to pair up"
    );
}

#[tokio::test]
async fn a_departed_server_wakes_the_pending_requests() {
    let (app, mut fake) = fake();
    let app = Arc::new(app);

    let call = spawn_call(&app, "memory/list");
    let _ = fake.next_request().await;

    drop(fake);

    let error = call
        .await
        .unwrap()
        .expect_err("a dropped connection should fail, not hang forever");
    assert_eq!(error.code, codes::INTERNAL_ERROR);
    assert!(error.message.contains("app-server"), "{}", error.message);
}
