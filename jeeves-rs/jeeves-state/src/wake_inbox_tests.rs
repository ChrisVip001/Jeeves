use super::*;

async fn store() -> (WakeInbox, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::open(&dir.path().join("state.db")).await.unwrap();
    (WakeInbox::new(pool), dir)
}

#[tokio::test]
async fn same_source_key_delivers_once() {
    let (inbox, _dir) = store().await;
    assert_eq!(
        inbox.wake("schedule:a:09:00", "t1", "x").await.unwrap(),
        WakeOutcome::Queued
    );
    assert_eq!(
        inbox.wake("schedule:a:09:00", "t1", "x").await.unwrap(),
        WakeOutcome::Duplicate
    );
    assert_eq!(inbox.pending().await.unwrap().len(), 1);
    inbox.delivered("schedule:a:09:00").await.unwrap();
    assert_eq!(
        inbox.wake("schedule:a:09:00", "t1", "x").await.unwrap(),
        WakeOutcome::Duplicate
    );
    assert!(inbox.pending().await.unwrap().is_empty());
}

#[tokio::test]
async fn reconcile_of_one_job_does_not_block_another() {
    let (inbox, _dir) = store().await;
    inbox.wake("schedule:a:09:00", "a", "").await.unwrap();
    inbox.reconcile("schedule:a:09:00").await.unwrap();
    assert_eq!(
        inbox.wake("schedule:b:09:00", "b", "").await.unwrap(),
        WakeOutcome::Queued
    );
    assert_eq!(inbox.pending().await.unwrap().len(), 1);
    assert_eq!(inbox.needs_reconcile().await.unwrap().len(), 1);
}

#[tokio::test]
async fn restart_redelivers_pending_never_reconcile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    {
        let inbox = WakeInbox::new(crate::open(&path).await.unwrap());
        inbox.wake("k1", "t", "").await.unwrap();
        inbox.wake("k2", "t", "").await.unwrap();
        inbox.reconcile("k2").await.unwrap();
    }
    let inbox = WakeInbox::new(crate::open(&path).await.unwrap());
    let keys: Vec<_> = inbox
        .pending()
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.source_key)
        .collect();
    assert_eq!(keys, vec!["k1".to_string()]);
}
