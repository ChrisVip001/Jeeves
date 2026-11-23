//! Worker tests: it must work off the turn's clock, and it must stop when asked.
//!
//! The model is never called here — with fewer rows than the threshold, distillation is
//! not due, which is exactly the branch that makes "the worker runs every 60 s" cheap.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use jeeves_config::Settings;
use jeeves_provider::Resolved;
use jeeves_provider::mock::Mock;

use crate::memory_worker::{Deps, MemoryWorker};

async fn fixtures(
    name: &str,
) -> (
    Deps,
    std::path::PathBuf,
    Arc<std::sync::RwLock<Option<Resolved>>>,
) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let home = dir.path().join(name);
    std::fs::create_dir_all(&home).expect("create the directory");
    let _ = dir.keep();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");

    let settings = Settings {
        home: home.clone(),
        ..Settings::default()
    };
    let resolved = Arc::new(std::sync::RwLock::new(Some(Resolved {
        context_window: jeeves_provider::DEFAULT_CONTEXT_WINDOW,
        vision: false,
        provider_id: "mock".to_string(),
        client: Arc::new(Mock::new(Vec::new())),
        model: "test-model".to_string(),
        small_model: "test-small".to_string(),
    })));

    let deps = Deps {
        pool: pool.clone(),
        chat: jeeves_state::Chat::new(pool.clone()),
        facts: jeeves_state::Facts::new(pool.clone()),
        episodes: jeeves_state::Episodes::new(pool),
        settings: Arc::new(std::sync::RwLock::new(settings)),
        resolved: resolved.clone(),
        _execution_owner: None,
    };
    (deps, home, resolved)
}

#[tokio::test]
async fn the_worker_mirrors_and_then_stops() {
    let (deps, home, _resolved) = fixtures("worker_mirror").await;
    deps.facts
        .add("alex", "likes morning meetings", "user", "user")
        .await
        .expect("write a fact");

    let worker = MemoryWorker::spawn(deps);
    worker.wake();
    let page = home.join("memory").join("alex.md");
    let mut waited = 0;
    while !page.exists() && waited < 50 {
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        waited += 1;
    }
    assert!(
        page.exists(),
        "the mirror should write the page: {}",
        page.display()
    );

    worker.stop().await;
    std::fs::remove_file(&page).expect("delete the page");
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    assert!(!page.exists(), "a stopped worker must not write files");
}

#[tokio::test]
async fn without_a_model_the_worker_stays_quiet() {
    let (deps, _home, resolved) = fixtures("worker_no_model").await;
    *resolved.write().expect("lock") = None;
    deps.facts
        .add("alex", "one sentence", "user", "user")
        .await
        .expect("write");

    let worker = MemoryWorker::spawn(deps);
    worker.wake();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        worker.take_pending(),
        0,
        "no model means no new facts to announce"
    );
    worker.stop().await;
}

#[test]
fn pending_facts_are_taken_once() {
    let worker = MemoryWorker {
        stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        wake: Arc::new(tokio::sync::Notify::new()),
        pending: Arc::new(std::sync::atomic::AtomicI32::new(3)),
        handle: std::sync::Mutex::new(None),
    };
    assert_eq!(worker.take_pending(), 3);
    assert_eq!(
        worker.take_pending(),
        0,
        "the same batch is announced once only"
    );
    assert_eq!(worker.pending.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn correcting_an_existing_fact_refreshes_generated_memory_without_a_count_change() {
    let (deps, home, _) = fixtures("worker_correction").await;
    let (fact, _) = deps
        .facts
        .add("alex", "prefers the old time", "user", "user")
        .await
        .unwrap();
    let mut last = Default::default();
    let pending = std::sync::atomic::AtomicI32::new(0);
    super::run_once(&deps, &pending, &mut last).await.unwrap();
    deps.facts
        .update(fact.id, "prefers the corrected time")
        .await
        .unwrap();
    super::run_once(&deps, &pending, &mut last).await.unwrap();
    let page = std::fs::read_to_string(home.join("memory/alex.md")).unwrap();
    assert!(page.contains("prefers the corrected time"));
    assert!(!page.contains("prefers the old time"));
}

#[tokio::test]
async fn existing_memory_can_be_projected_without_a_model() {
    let (deps, home, resolved) = fixtures("worker_offline_projection").await;
    *resolved.write().unwrap() = None;
    deps.facts
        .add("alex", "local memory works offline", "user", "user")
        .await
        .unwrap();
    let mut last = Default::default();
    let pending = std::sync::atomic::AtomicI32::new(0);
    super::run_once(&deps, &pending, &mut last).await.unwrap();
    assert!(
        std::fs::read_to_string(home.join("memory/alex.md"))
            .unwrap()
            .contains("local memory works offline")
    );
    assert_eq!(pending.load(Ordering::SeqCst), 0);
}
