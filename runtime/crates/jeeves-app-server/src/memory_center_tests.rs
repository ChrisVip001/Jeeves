//! The memory center preserves readable data and reports a failed authoritative read.

use crate::memory_center::{preview_of, usage_of};

#[test]
fn the_meter_warns_at_the_threshold_it_was_given() {
    let full = usage_of(
        vec![("soul", 100), ("memory", 50), ("history", 850)],
        1000,
        85,
    );
    assert_eq!(full.used_tokens, 1_000);
    assert!(
        (full.ratio - 1.0).abs() < 1e-9,
        "the ratio is computed against the budget: {}",
        full.ratio
    );
    assert!(full.notice.is_some(), "100% has to warn");

    let calm = usage_of(vec![("soul", 100), ("history", 400)], 1000, 85);
    assert_eq!(calm.used_tokens, 500);
    assert!(calm.notice.is_none(), "50% must not warn");
}

#[test]
fn a_missing_budget_reads_as_zero_rather_than_infinity() {
    let usage = usage_of(vec![("soul", 120)], 0, 85);
    assert_eq!(usage.ratio, 0.0);
    assert!(usage.notice.is_none());
}

#[test]
fn a_ledger_preview_uses_the_payloads_own_text() {
    assert_eq!(
        preview_of("{\"text\":\"first line\\nsecond line\"}"),
        "first line"
    );
    assert_eq!(
        preview_of("{\"preview\":\"ptr → spill/…\",\"bytes\":12}"),
        "ptr → spill/…"
    );
    assert_eq!(preview_of("{\"kind\":\"image\"}"), "{\"kind\":\"image\"}");
    assert_eq!(
        preview_of("one line of plain text"),
        "one line of plain text"
    );
}

#[test]
fn a_long_preview_is_clipped_not_squeezed() {
    let long = "char".repeat(400);
    let clipped = preview_of(&format!("{{\"text\":\"{long}\"}}"));
    assert_eq!(clipped.chars().count(), 160);
}

#[tokio::test]
async fn manual_memory_reads_distinguish_absence_from_failed_storage() {
    use jeeves_protocol::methods;
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let server = crate::Server::boot(
        pool,
        jeeves_config::Settings {
            home: home.path().into(),
            ..Default::default()
        },
    )
    .await;
    let index = home.path().join("memory/index.md");
    if index.is_file() {
        std::fs::remove_file(&index).unwrap();
    }
    let rpc = |method| crate::memory_center::handle(&server, method, None);
    assert_eq!(
        rpc(methods::MEMORY_INDEX).await.unwrap().unwrap()["catalog"],
        ""
    );
    std::fs::create_dir_all(&index).unwrap();
    assert!(rpc(methods::MEMORY_INDEX).await.unwrap().is_err());
    assert!(rpc(methods::SKILL_STATS).await.unwrap().is_ok());
    std::fs::create_dir(home.path().join("skill-hits.jsonl")).unwrap();
    assert!(rpc(methods::SKILL_STATS).await.unwrap().is_err());
    sqlx::query("ALTER TABLE context_rollups RENAME TO fixture_unavailable")
        .execute(&server.pool)
        .await
        .unwrap();
    assert!(
        crate::memory_center::compact_session(&server, "default", None)
            .await
            .is_err()
    );
}
