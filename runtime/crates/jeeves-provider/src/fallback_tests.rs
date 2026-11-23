//! Fallback chain: which errors move the call, which stay, and what the backup is asked.

use std::sync::{Arc, Mutex};

use super::{Fallback, Target};
use crate::mock::Mock;
use crate::retry::{RetryNotice, with_note_sink};
use crate::{
    ContentBlock, CreateRequest, CreateResponse, Message, Provider, ProviderError, Role, TextSink,
};

fn http(status: u16) -> ProviderError {
    ProviderError::Http {
        status,
        body: "down".into(),
        retry_after_secs: None,
    }
}

fn target(label: &str, mock: &Arc<Mock>, vision: bool) -> Target {
    Target {
        label: label.into(),
        client: mock.clone(),
        model: format!("{label}-main"),
        small_model: format!("{label}-small"),
        vision,
        max_output_tokens: Some(8),
    }
}

fn request(model: &str) -> CreateRequest {
    CreateRequest {
        model: model.into(),
        cache_prefix: None,
        system: None,
        messages: vec![Message::user_text("hi")],
        tools: Vec::new(),
        max_tokens: 16,
    }
}

fn reply_text(response: &CreateResponse) -> String {
    Message {
        role: Role::Assistant,
        content: response.content.clone(),
    }
    .text()
}

/// Run `call` with a notice outlet and return what it produced plus every notice.
async fn noticed<F: Future<Output = T>, T>(call: F) -> (T, Vec<RetryNotice>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let seen = seen.clone();
        Arc::new(move |notice: RetryNotice| seen.lock().expect("lock").push(notice))
    };
    let out = with_note_sink(sink, call).await;
    let notices = seen.lock().expect("lock").clone();
    (out, notices)
}

#[tokio::test]
async fn a_server_fault_moves_the_call_to_the_backup_with_its_own_model() {
    let primary = Arc::new(Mock::with_outcomes(vec![Err(http(503))]));
    let backup = Arc::new(Mock::new(vec![Mock::text("from backup")]));
    let chain = Fallback::new(
        target("main", &primary, false),
        vec![target("spare", &backup, false)],
    );

    let (out, notices) = noticed(chain.create(request("main-main"))).await;

    assert_eq!(reply_text(&out.expect("the backup answers")), "from backup");
    let asked = backup.received.lock().expect("lock");
    assert_eq!(
        asked[0].model, "spare-main",
        "the backup gets its own model name"
    );
    assert_eq!(asked[0].max_tokens, 8, "and its own output ceiling");
    let [notice] = notices.as_slice() else {
        panic!("one switch, one notice: {notices:?}");
    };
    assert!(
        notice.reason.contains("spare") && notice.reason.contains("503"),
        "{notice:?}"
    );
}

#[tokio::test]
async fn the_small_model_maps_to_the_backups_small_model() {
    let primary = Arc::new(Mock::with_outcomes(vec![Err(http(402))]));
    let backup = Arc::new(Mock::new(vec![Mock::text("ok")]));
    let chain = Fallback::new(
        target("main", &primary, false),
        vec![target("spare", &backup, false)],
    );
    chain.create(request("main-small")).await.expect("answers");
    assert_eq!(
        backup.received.lock().expect("lock")[0].model,
        "spare-small"
    );
}

#[tokio::test]
async fn a_request_error_is_reported_instead_of_switching() {
    for error in [http(400), http(404), Mock::context_overflow().unwrap_err()] {
        let primary = Arc::new(Mock::with_outcomes(vec![Err(error)]));
        let backup = Arc::new(Mock::new(vec![Mock::text("should not run")]));
        let chain = Fallback::new(
            target("main", &primary, false),
            vec![target("spare", &backup, false)],
        );
        assert!(chain.create(request("main-main")).await.is_err());
        assert!(backup.received.lock().expect("lock").is_empty());
    }
}

/// Shows one piece of text, then breaks: the shape of a stream that died mid-answer.
struct BreaksAfterText;
impl Provider for BreaksAfterText {
    fn create(&self, request: CreateRequest) -> super::Reply<'_> {
        self.stream(request, Arc::new(|_| {}))
    }
    fn stream(&self, _request: CreateRequest, on_text: TextSink) -> super::Reply<'_> {
        Box::pin(async move {
            on_text("half an ans");
            Err(ProviderError::Incomplete("connection reset".into()))
        })
    }
}

#[tokio::test]
async fn a_stream_that_already_showed_text_does_not_switch() {
    let backup = Arc::new(Mock::new(vec![Mock::text("second answer")]));
    let chain = Fallback::new(
        Target {
            label: "main".into(),
            client: Arc::new(BreaksAfterText),
            model: "main-main".into(),
            small_model: "main-small".into(),
            vision: false,
            max_output_tokens: None,
        },
        vec![target("spare", &backup, false)],
    );
    let shown = Arc::new(Mutex::new(String::new()));
    let sink: TextSink = {
        let shown = shown.clone();
        Arc::new(move |piece: &str| shown.lock().expect("lock").push_str(piece))
    };
    let error = chain
        .stream(request("main-main"), sink)
        .await
        .expect_err("the break is reported");
    assert!(matches!(error, ProviderError::Incomplete(_)), "{error}");
    assert_eq!(*shown.lock().expect("lock"), "half an ans");
    assert!(backup.received.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn an_image_request_skips_a_backup_that_cannot_see() {
    let primary = Arc::new(Mock::with_outcomes(vec![Err(http(500))]));
    let blind = Arc::new(Mock::new(vec![Mock::text("blind")]));
    let sighted = Arc::new(Mock::new(vec![Mock::text("sighted")]));
    let chain = Fallback::new(
        target("main", &primary, true),
        vec![
            target("blind", &blind, false),
            target("eyes", &sighted, true),
        ],
    );
    let mut with_image = request("main-main");
    with_image.messages[0].content.push(ContentBlock::Image {
        media_type: "image/png".into(),
        data: "AAAA".into(),
    });
    let out = chain
        .create(with_image)
        .await
        .expect("the sighted backup answers");
    assert_eq!(reply_text(&out), "sighted");
    assert!(blind.received.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn when_every_target_fails_the_last_error_is_returned() {
    let primary = Arc::new(Mock::with_outcomes(vec![Err(http(503))]));
    let backup = Arc::new(Mock::with_outcomes(vec![Err(http(401))]));
    let chain = Fallback::new(
        target("main", &primary, false),
        vec![target("spare", &backup, false)],
    );
    let error = chain
        .create(request("main-main"))
        .await
        .expect_err("all down");
    assert!(
        matches!(error, ProviderError::Http { status: 401, .. }),
        "{error}"
    );
}

#[tokio::test]
async fn after_a_switch_later_calls_start_at_the_backup() {
    let primary = Arc::new(Mock::with_outcomes(vec![Err(http(503))]));
    let backup = Arc::new(Mock::new(vec![Mock::text("one"), Mock::text("two")]));
    let chain = Fallback::new(
        target("main", &primary, false),
        vec![target("spare", &backup, false)],
    );
    chain.create(request("main-main")).await.expect("first");
    let (out, notices) = noticed(chain.create(request("main-main"))).await;
    assert_eq!(reply_text(&out.expect("second")), "two");
    assert!(notices.is_empty(), "no second switch: {notices:?}");
    assert_eq!(primary.received.lock().expect("lock").len(), 1);
}

#[tokio::test]
async fn a_model_only_the_primary_knows_goes_straight_to_it() {
    let primary = Arc::new(Mock::with_outcomes(vec![Err(http(503))]));
    let backup = Arc::new(Mock::new(vec![Mock::text("should not run")]));
    let chain = Fallback::new(
        target("main", &primary, false),
        vec![target("spare", &backup, false)],
    );
    assert!(chain.create(request("other-model")).await.is_err());
    assert!(backup.received.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn with_no_backups_the_chain_is_the_primary() {
    let primary = Arc::new(Mock::new(vec![Mock::text("plain")]));
    let chain = Fallback::new(target("main", &primary, false), Vec::new());
    let out = chain.create(request("main-main")).await.expect("answers");
    assert_eq!(reply_text(&out), "plain");
}

#[tokio::test]
async fn a_backup_that_refuses_the_request_is_skipped_and_the_first_error_kept() {
    let primary = Arc::new(Mock::with_outcomes(vec![Err(http(503))]));
    let small_window = Arc::new(Mock::with_outcomes(vec![Mock::context_overflow()]));
    let last = Arc::new(Mock::with_outcomes(vec![Err(http(400))]));
    let chain = Fallback::new(
        target("main", &primary, false),
        vec![
            target("small", &small_window, false),
            target("last", &last, false),
        ],
    );
    let error = chain
        .create(request("main-main"))
        .await
        .expect_err("nobody answers");
    assert!(
        matches!(error, ProviderError::Http { status: 503, .. }),
        "the outage is the error to report, not a backup's refusal: {error}"
    );
    assert_eq!(
        last.received.lock().expect("lock").len(),
        1,
        "the chain went on"
    );
}
