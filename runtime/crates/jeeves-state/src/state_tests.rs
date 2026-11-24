//! state's integration tests: run against a real temporary SQLite file; in-memory won't do —
//! every connection in sqlx's pool gets its own independent `:memory:` database.

use sqlx::sqlite::SqlitePool;

use crate::{Chat, Episodes, Facts, NewTask, TaskStatus, Tasks};

#[tokio::test]
async fn latest_task_prefers_open_but_never_hides_a_finished_one() {
    let pool = temp_db("task_latest").await;
    let tasks = Tasks::new(pool);

    tasks
        .create(bare_task("old-done", "s1", "finished"))
        .await
        .expect("create the task");
    tasks
        .finish("old-done", TaskStatus::Done)
        .await
        .expect("finish it");
    tasks
        .create(bare_task("new-open", "s1", "still running"))
        .await
        .expect("create the task");

    let latest = tasks
        .latest("s1")
        .await
        .expect("query")
        .expect("there should be one");
    assert_eq!(latest.id, "new-open", "the open one wins");

    tasks
        .finish("new-open", TaskStatus::Done)
        .await
        .expect("finish it");
    let latest = tasks
        .latest("s1")
        .await
        .expect("query")
        .expect("a finished one has to be readable too");
    assert_eq!(
        latest.id, "new-open",
        "a finished task must not vanish from the card"
    );
    assert_eq!(latest.status, TaskStatus::Done);

    assert!(
        tasks.latest("s2").await.expect("query").is_none(),
        "other sessions cannot see it"
    );
}

fn bare_task<'a>(id: &'a str, session: &'a str, title: &'a str) -> NewTask<'a> {
    NewTask {
        id,
        session_id: Some(session),
        title,
        goal_condition: None,
        goal_draft: None,
        acceptance_json: "[]",
        budget_json: "{}",
        cwd: None,
        max_rounds: 3,
        tier: None,
    }
}

async fn temp_db(name: &str) -> SqlitePool {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join(name);
    let pool = crate::open(&path).await.expect("open the database");
    let _ = dir.keep();
    pool
}

#[tokio::test]
async fn a_duplicate_fact_is_not_stored_twice() {
    let facts = Facts::new(temp_db("dedup.db").await);

    let (first, is_new) = facts
        .add("alex", "likes morning meetings", "user", "user")
        .await
        .unwrap();
    assert!(is_new, "the first time is of course new");

    let (again, is_new) = facts
        .add("alex", "likes morning meetings", "user", "user")
        .await
        .unwrap();
    assert!(!is_new);
    assert_eq!(again.id, first.id);

    let (spaced, is_new) = facts
        .add(
            "alex",
            "  likes morning meetings  ",
            "consolidation",
            "fact",
        )
        .await
        .unwrap();
    assert!(
        !is_new,
        "surrounding whitespace must not make it a new fact"
    );
    assert_eq!(spaced.id, first.id);

    let (other, is_new) = facts
        .add("alex", "likes the afternoon meeting", "user", "user")
        .await
        .unwrap();
    assert!(is_new);
    assert!(other.id > first.id);

    assert_eq!(
        facts.recent(10, 0).await.unwrap().len(),
        2,
        "the store holds exactly two"
    );
}

#[tokio::test]
async fn context_observations_pair_measured_with_estimated() {
    let chat = Chat::new(temp_db("context.db").await);
    assert_eq!(
        chat.context_factor("s").await.unwrap(),
        None,
        "no record, no calibration"
    );

    chat.observe_context("s", 1000, 800).await.unwrap();
    let factor = chat.context_factor("s").await.unwrap().unwrap();
    assert!(
        (factor - 1.25).abs() < 1e-9,
        "the measurement beats the estimate → 1.25, actually {factor}"
    );

    chat.observe_context("s", 900, 1800).await.unwrap();
    let factor = chat.context_factor("s").await.unwrap().unwrap();
    assert!(factor < 1.0, "the estimate came out too high: {factor}");

    chat.observe_context("zero", 0, 100).await.unwrap();
    assert_eq!(chat.context_factor("zero").await.unwrap(), None);

    chat.observe_context("wild", 100_000, 10).await.unwrap();
    assert_eq!(chat.context_factor("wild").await.unwrap(), Some(2.0));
}

#[tokio::test]
async fn fact_kinds_are_normalised_at_the_write_entry() {
    let facts = Facts::new(temp_db("kinds.db").await);
    facts
        .add("a", "a misshapen category", "user", "PREFERENCE")
        .await
        .unwrap();
    let rows = facts.recent(10, 0).await.unwrap();
    assert_eq!(rows[0].kind, "fact", "an unknown kind falls back");

    facts
        .add("b", "a proper category", "user", "feedback")
        .await
        .unwrap();
    let rows = facts.recent(10, 0).await.unwrap();
    assert!(rows.iter().any(|row| row.kind == "feedback"));
    facts.add("c", "case", "user", "  Project ").await.unwrap();
    let rows = facts.recent(10, 0).await.unwrap();
    assert!(rows.iter().any(|row| row.kind == "project"));
}

#[tokio::test]
async fn failed_consolidation_rows_are_backed_off() {
    let chat = Chat::new(temp_db("backoff.db").await);
    chat.append_exchange("m", "r", "default", "cli", None)
        .await
        .unwrap();
    let rows = chat.unconsolidated().await.unwrap();
    assert_eq!(rows.len(), 2);
    let ids: Vec<i64> = rows.iter().map(|(id, _, _)| *id).collect();

    chat.mark_consolidation_failed(&ids).await.unwrap();
    assert_eq!(
        chat.unconsolidated().await.unwrap().len(),
        0,
        "nothing is picked up during the backoff"
    );

    assert_eq!(chat.session_history("default").await.unwrap().len(), 1);
}

#[tokio::test]
async fn open_creates_the_schema() {
    let pool = temp_db("schema.db").await;
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(&pool)
            .await
            .expect("query the table names");
    for expected in ["facts", "facts_fts", "episodes", "episodes_fts", "chat_log"] {
        assert!(
            tables.contains(&expected.to_string()),
            "missing table {expected}: {tables:?}"
        );
    }
}

#[tokio::test]
async fn facts_search_finds_by_keyword() {
    let pool = temp_db("facts.db").await;
    let facts = Facts::new(pool);
    facts
        .add("alex", "Alex prefers morning meetings", "user", "user")
        .await
        .unwrap();
    facts
        .add("project", "The Acme demo is on Friday", "user", "user")
        .await
        .unwrap();

    let hit = facts.search("morning", 4).await.unwrap();
    assert_eq!(hit.len(), 1, "exactly one hit expected: {hit:?}");
    assert_eq!(hit[0].subject, "alex");

    let hit = facts.search("acme", 4).await.unwrap();
    assert_eq!(hit[0].content, "The Acme demo is on Friday");
}

#[tokio::test]
async fn search_never_errors_on_junk_input() {
    let pool = temp_db("junk.db").await;
    let facts = Facts::new(pool);
    facts
        .add("alex", "Alex prefers morning meetings", "user", "user")
        .await
        .unwrap();

    for junk in ["", "???", "\"", "a:b", "(unclosed", "*", "alex OR", "---"] {
        let out = facts.search(junk, 4).await;
        assert!(
            out.is_ok(),
            "{junk:?} made the search fail: {:?}",
            out.err()
        );
    }
}

#[tokio::test]
async fn deleting_a_fact_keeps_the_index_in_sync() {
    let pool = temp_db("delete.db").await;
    let facts = Facts::new(pool);
    facts
        .add("alex", "Alex prefers morning meetings", "user", "user")
        .await
        .unwrap();
    assert_eq!(facts.search("morning", 4).await.unwrap().len(), 1);

    let removed = facts.forget_subject("alex").await.unwrap();
    assert_eq!(removed, 1);
    assert!(
        facts.search("morning", 4).await.unwrap().is_empty(),
        "the FTS index was not deleted along with it"
    );
}

#[tokio::test]
async fn episodes_search_and_recent() {
    let pool = temp_db("episodes.db").await;
    let eps = Episodes::new(pool);
    eps.add("2026-09-01", "planned the Acme demo with Alex")
        .await
        .unwrap();
    eps.add("2026-09-10", "booked a catch-up on Friday")
        .await
        .unwrap();

    assert_eq!(eps.search("acme", 4).await.unwrap().len(), 1);

    let recent = eps.recent(10).await.unwrap();
    assert_eq!(
        recent[0].happened_at, "2026-09-10",
        "the most recent comes first"
    );
}

#[tokio::test]
async fn chat_log_round_trip_and_consolidation_marking() {
    let pool = temp_db("chat.db").await;
    let chat = Chat::new(pool);
    chat.append_exchange("hi", "hello", "default", "cli", None)
        .await
        .unwrap();
    chat.append_exchange(
        "remember alex likes mornings",
        "noted",
        "default",
        "cli",
        Some("{\"iterations\":1}"),
    )
    .await
    .unwrap();

    let pending = chat.unconsolidated().await.unwrap();
    assert_eq!(pending.len(), 4, "4 rows (2 turns × user/assistant)");

    let claimed = chat.claim_unconsolidated().await.unwrap();
    assert_eq!(claimed.len(), 4, "a claim returns the 4 rows it claimed");
    assert!(chat.unconsolidated().await.unwrap().is_empty());

    let history = chat.session_history("default").await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].0, "hi");
}

#[tokio::test]
async fn sessions_are_just_labels_on_the_chat_log() {
    let pool = temp_db("sessions.db").await;
    let chat = Chat::new(pool);
    chat.append_exchange("first topic", "a", "s1", "cli", None)
        .await
        .unwrap();
    chat.append_exchange("second topic", "b", "s2", "cli", None)
        .await
        .unwrap();
    chat.append_exchange("more", "c", "s2", "cli", None)
        .await
        .unwrap();

    let sessions = chat.sessions().await.unwrap();
    assert_eq!(
        sessions.len(),
        2,
        "writing a message creates the session row; no `new` first"
    );
    let s2 = sessions.iter().find(|s| s.id == "s2").expect("s2 exists");
    assert_eq!(s2.messages, 4, "s2 has 2 turns = 4 rows");
    assert_eq!(
        s2.title, "second topic",
        "the title should be the session's first user message"
    );
}

#[tokio::test]
async fn an_empty_session_is_listed_with_its_name_and_working_directory() {
    let pool = temp_db("sessions_empty.db").await;
    let chat = Chat::new(pool);
    chat.create_session("s1", Some("release check"), Some("/tmp/jeeves-work"))
        .await
        .expect("create the session");

    let all = chat.sessions().await.expect("list");
    assert_eq!(all.len(), 1, "not a single message, and it still exists");
    assert_eq!(all[0].title, "release check");
    assert_eq!(all[0].messages, 0);
    assert_eq!(all[0].cwd.as_deref(), Some("/tmp/jeeves-work"));
}

#[tokio::test]
async fn update_renames_and_clears_back_to_the_first_message() {
    let pool = temp_db("sessions_update.db").await;
    let chat = Chat::new(pool);
    chat.append_exchange("the first line", "answer", "s1", "cli", None)
        .await
        .expect("write one turn");

    chat.update_session("s1", Some("a name I picked"), Some("/tmp/w"))
        .await
        .expect("rename");
    let row = chat
        .session("s1")
        .await
        .expect("query")
        .expect("there should be one");
    assert_eq!(row.title, "a name I picked");
    assert_eq!(row.cwd.as_deref(), Some("/tmp/w"));

    chat.update_session("s1", Some(""), Some(""))
        .await
        .expect("clear it");
    let row = chat
        .session("s1")
        .await
        .expect("query")
        .expect("there should be one");
    assert_eq!(
        row.title, "the first line",
        "clearing falls back to the first user message"
    );
    assert_eq!(row.cwd, None, "cleared means no directory chosen");
}

#[tokio::test]
async fn creating_twice_keeps_the_name_that_is_already_there() {
    let pool = temp_db("sessions_idem.db").await;
    let chat = Chat::new(pool);
    chat.create_session("s1", Some("the original name"), None)
        .await
        .expect("create");
    chat.create_session("s1", Some("another name"), Some("/tmp/x"))
        .await
        .expect("create it again");

    let row = chat
        .session("s1")
        .await
        .expect("query")
        .expect("there should be one");
    assert_eq!(row.title, "the original name");
    assert_eq!(
        row.cwd.as_deref(),
        Some("/tmp/x"),
        "the empty field gets filled in"
    );
}

#[tokio::test]
async fn messages_page_backwards_from_the_newest() {
    let pool = temp_db("messages.db").await;
    let chat = Chat::new(pool);
    for round in 1..=2 {
        chat.append_exchange(
            &format!("question {round}"),
            &format!("answer {round}"),
            "default",
            "cli",
            None,
        )
        .await
        .unwrap();
    }
    chat.append_exchange(
        "question 3",
        "answer 3",
        "default",
        "cli",
        Some(r#"{"iterations":7}"#),
    )
    .await
    .unwrap();
    chat.append_exchange("another session", "must not leak in", "other", "cli", None)
        .await
        .unwrap();

    let page = chat.messages("default", None, 2).await.unwrap();
    let contents: Vec<&str> = page.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        contents,
        ["answer 3", "question 3"],
        "the newest comes first"
    );
    assert_eq!(page[0].role, "assistant");

    assert_eq!(page[0].meta.as_deref(), Some(r#"{"iterations":7}"#));
    assert!(page[1].meta.is_none());

    let older = chat
        .messages("default", Some(page[1].id), 10)
        .await
        .unwrap();
    let contents: Vec<&str> = older.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        contents,
        ["answer 2", "question 2", "answer 1", "question 1"]
    );

    let other = chat.messages("other", None, 10).await.unwrap();
    assert_eq!(other.len(), 2);
    assert_eq!(
        other[0].content, "must not leak in",
        "another session must not leak in"
    );
}

#[tokio::test]
async fn a_chinese_question_finds_facts_that_share_its_words() {
    let pool = temp_db("cjk.db").await;
    let facts = Facts::new(pool.clone());
    facts
        .add("阿明", "对花生过敏", "user", "user")
        .await
        .unwrap();
    facts
        .add("阿明", "喜欢早上开会", "user", "user")
        .await
        .unwrap();
    facts.add("小红", "住在上海", "user", "user").await.unwrap();

    let hits = facts.search("阿明能吃花生酱吗？", 4).await.unwrap();
    assert_eq!(
        hits.first().map(|h| h.content.as_str()),
        Some("对花生过敏"),
        "{hits:?}"
    );
    assert!(!hits.iter().any(|h| h.content == "住在上海"), "{hits:?}");

    facts
        .add("Alex", "Alex likes morning meetings", "user", "user")
        .await
        .unwrap();
    assert_eq!(facts.search("morning", 4).await.unwrap().len(), 1);

    let episodes = Episodes::new(pool);
    episodes
        .add("2026-11-01", "和阿明定了周五的演示")
        .await
        .unwrap();
    let found = episodes.search("周五的演示准备好了吗", 3).await.unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
}

#[tokio::test]
async fn a_rollup_only_moves_forward() {
    let chat = Chat::new(temp_db("rollup.db").await);

    assert!(
        chat.save_rollup("s", 5, "summary of the first five turns")
            .await
            .unwrap()
    );
    assert!(
        chat.save_rollup("s", 10, "summary of the first ten turns")
            .await
            .unwrap()
    );
    let (covered, summary) = chat
        .load_rollup("s")
        .await
        .unwrap()
        .expect("there is a summary");
    assert_eq!(covered, 10);
    assert_eq!(summary, "summary of the first ten turns");

    assert!(
        !chat
            .save_rollup("s", 7, "a summary of round seven")
            .await
            .unwrap(),
        "the rollback has to be blocked"
    );
    let (covered, summary) = chat
        .load_rollup("s")
        .await
        .unwrap()
        .expect("there is a summary");
    assert_eq!(covered, 10, "not overwritten by the older one");
    assert_eq!(summary, "summary of the first ten turns");

    assert!(!chat.save_rollup("s", 10, "one more version").await.unwrap());
}

#[tokio::test]
async fn the_pool_waits_for_locks_and_uses_wal() {
    let pool = temp_db("pragmas.db").await;

    let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        busy_timeout, 10_000,
        "the lock wait should be 10 seconds (SQLite's own advice)"
    );

    let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        journal.to_lowercase(),
        "wal",
        "reads must not be blocked by writes"
    );
}
#[tokio::test]
async fn checkpoints_keep_the_newest_working_memory_open() {
    use crate::Checkpoints;

    let checkpoints = Checkpoints::new(temp_db("checkpoints.db").await);

    checkpoints
        .save(
            "t1",
            "s",
            "schedule three days of meetings for me",
            r#"[{"m":1}]"#,
            2,
            r#"{"input_tokens":100}"#,
            Some("turn-t1"),
        )
        .await
        .unwrap();
    checkpoints
        .save(
            "t1",
            "s",
            "schedule three days of meetings for me",
            r#"[{"m":2}]"#,
            5,
            r#"{"input_tokens":300}"#,
            Some("turn-t1"),
        )
        .await
        .unwrap();

    let rows = checkpoints.open_rows().await.unwrap();
    assert_eq!(rows.len(), 1, "one row per turn");
    assert_eq!(rows[0].iterations, 5, "only the latest is kept");
    assert!(
        rows[0].messages_json.contains(r#"{"m":2}"#),
        "working memory is the new version"
    );
    assert_eq!(rows[0].status, "open");

    checkpoints.finish("t1", "done").await.unwrap();
    assert!(checkpoints.open_rows().await.unwrap().is_empty());

    checkpoints
        .save("t2", "s", "a long task", "[]", 1, "{}", None)
        .await
        .unwrap();
    checkpoints.finish("t2", "abandoned").await.unwrap();
    assert!(checkpoints.open_rows().await.unwrap().is_empty());

    checkpoints
        .save("t3", "s", "the one that came first", "[]", 1, "{}", None)
        .await
        .unwrap();
    checkpoints
        .save("t4", "s", "the one that came later", "[]", 1, "{}", None)
        .await
        .unwrap();
    let rows = checkpoints.open_rows().await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].turn_id, "t3", "first in, first resumed");
}

#[tokio::test]
async fn the_follow_up_queue_is_fifo_per_session_and_pops_once() {
    let pool = temp_db("session_queue").await;
    let queue = crate::Queue::new(pool);

    assert_eq!(
        queue.push("s1", "the first line").await.expect("enqueue"),
        1
    );
    assert_eq!(
        queue.push("s1", "the second line").await.expect("enqueue"),
        2
    );
    assert_eq!(
        queue.push("s2", "another session").await.expect("enqueue"),
        1,
        "each session counts its own"
    );

    let first = queue
        .pop("s1")
        .await
        .expect("take")
        .expect("there should be one");
    assert_eq!(first.message, "the first line", "first in, first out");
    assert!(queue.pop("s1").await.expect("claim blocked").is_none());
    queue.release(first.id).await.expect("release");
    let reclaimed = queue.pop("s1").await.unwrap().unwrap();
    assert_eq!(reclaimed.id, first.id);
    queue.complete(first.id).await.expect("complete");
    let second = queue
        .pop("s1")
        .await
        .expect("take")
        .expect("there should be one");
    assert_eq!(second.message, "the second line");
    let interrupted = queue.reconcile_interrupted().await.unwrap();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].id, second.id);
    assert_eq!(interrupted[0].message, second.message);
    assert!(queue.reconcile_interrupted().await.unwrap().is_empty());
    assert!(
        queue.pop("s1").await.expect("take").is_none(),
        "once taken, it does not come again"
    );

    assert_eq!(
        queue.count("s2").await.expect("count"),
        1,
        "another session is untouched"
    );
    assert_eq!(
        queue.sessions_with_pending().await.expect("list"),
        vec!["s2".to_string()],
        "the startup reconcile has to know who is still owed a line"
    );
}

fn entry(kind: &'static str, text: &str) -> crate::NewEntry {
    crate::NewEntry::exact(kind, format!(r#"{{"text":"{text}"}}"#))
}

#[tokio::test]
async fn a_turn_round_trips_in_append_order() {
    let ledger = crate::Ledger::new(temp_db("ledger_roundtrip").await);
    let entries = vec![
        entry("user", "take a look at this error for me"),
        entry("assistant", "read the log first"),
        entry("tool_use", "read_file"),
        entry("tool_result", "no such file"),
    ];

    let digest = ledger
        .append("s1", "t1", &entries, "take a look at this error for me")
        .await
        .expect("write the ledger");
    assert_eq!(digest.len(), 64, "the digest is lowercase sha256 hex");

    let rows = ledger.entries("s1", "t1").await.expect("read the ledger");
    assert_eq!(
        rows.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>(),
        vec!["user", "assistant", "tool_use", "tool_result"],
        "the order is the order things happened"
    );
    assert_eq!(rows[3].payload, r#"{"text":"no such file"}"#);
    assert!(
        ledger.verify("s1", "t1").await.expect("verify"),
        "written just now, so verification has to pass"
    );
}

#[tokio::test]
async fn appending_the_same_turn_twice_writes_once() {
    let ledger = crate::Ledger::new(temp_db("ledger_idempotent").await);
    let entries = vec![entry("user", "only this one sentence")];

    let append = || ledger.append("s1", "t1", &entries, "only this one sentence");
    let (first, second) = tokio::join!(append(), append());
    let (first, second) = (
        first.expect("first append"),
        second.expect("concurrent retry"),
    );
    assert_eq!(
        first, second,
        "the second call returns the first digest without writing again"
    );
    assert_eq!(
        ledger.entries("s1", "t1").await.expect("read").len(),
        1,
        "only one line"
    );
    assert_eq!(
        ledger.count().await.expect("count"),
        1,
        "a turn is recorded once"
    );
}

#[tokio::test]
async fn editing_a_row_breaks_the_turn_digest() {
    let ledger = crate::Ledger::new(temp_db("ledger_tamper").await);
    ledger
        .append(
            "s1",
            "t1",
            &[entry("user", "the original content")],
            "the original content",
        )
        .await
        .expect("write the ledger");
    assert!(ledger.verify("s1", "t1").await.expect("verify"));

    sqlx::query("UPDATE ledger SET payload = '{\"text\":\"edited\"}' WHERE turn_id = 't1'")
        .execute(ledger.pool())
        .await
        .expect("tamper");

    assert!(
        !ledger.verify("s1", "t1").await.expect("verify"),
        "the content changed, so the turn digest has to disagree"
    );
}

#[tokio::test]
async fn a_folded_turn_is_declared_as_such() {
    let ledger = crate::Ledger::new(temp_db("ledger_folded").await);
    ledger
        .append(
            "s1",
            "t0",
            &[
                crate::NewEntry::folded("user", r#"{"text":"one old line"}"#),
                crate::NewEntry::folded(
                    "assistant",
                    r#"{"text":"old reply\n[tools used: read_file(…)]"}"#,
                ),
            ],
            "one old line",
        )
        .await
        .expect("backfill");

    let turns = ledger.turns("s1", 10).await.expect("list");
    assert_eq!(turns[0].fidelity, "folded", "fidelity is labelled honestly");
    assert_eq!(turns[0].events, 2);
    assert_eq!(turns[0].preview, "one old line");
}

#[tokio::test]
async fn claiming_pending_turns_has_one_winner() {
    let ledger = crate::Ledger::new(temp_db("ledger_claim").await);
    for turn in ["t1", "t2"] {
        ledger
            .append("s1", turn, &[entry("user", "…")], "…")
            .await
            .expect("write the ledger");
    }

    assert_eq!(ledger.pending(10).await.expect("pending").len(), 2);
    let claimed = ledger.claim_pending(10).await.expect("claim");
    assert_eq!(claimed.len(), 2, "one claim takes all of them");
    assert!(
        ledger.pending(10).await.expect("pending").is_empty(),
        "once claimed, it is no longer pending"
    );
    assert!(
        ledger
            .claim_pending(10)
            .await
            .expect("claim again")
            .is_empty(),
        "nobody else gets it"
    );

    ledger.mark_failed("s1", "t1").await.expect("hand it back");
    assert!(
        ledger.pending(10).await.expect("pending").is_empty(),
        "it must not reappear inside the backoff"
    );
}

#[tokio::test]
async fn a_fact_carries_its_freshness() {
    let facts = Facts::new(temp_db("facts_freshness").await);
    let (fact, is_new) = facts
        .add("alex", "likes morning meetings", "user", "user")
        .await
        .expect("write");
    assert!(is_new);

    let meta = facts.meta(fact.id).await.expect("query").expect("present");
    assert_eq!(meta.confirm_count, 1);
    assert!(
        meta.last_confirmed_at.is_some(),
        "a write is one confirmation"
    );
    assert!(
        meta.page_path.is_none(),
        "not mirrored into the plaintext pages yet"
    );

    let (again, is_new) = facts
        .add("alex", "likes morning meetings", "user", "user")
        .await
        .expect("write again");
    assert!(!is_new, "a repeat adds no row");
    assert_eq!(again.id, fact.id);
    let meta = facts.meta(fact.id).await.expect("query").expect("present");
    assert_eq!(
        meta.confirm_count, 2,
        "seeing it once more makes it a little more credible"
    );

    facts
        .set_page(fact.id, "memory/alex.md", "abc123")
        .await
        .expect("record the page");
    let meta = facts.meta(fact.id).await.expect("query").expect("present");
    assert_eq!(meta.page_path.as_deref(), Some("memory/alex.md"));
    assert_eq!(meta.digest.as_deref(), Some("abc123"));
}

#[tokio::test]
async fn backfilling_marks_history_as_folded() {
    let pool = temp_db("ledger_backfill").await;
    let chat = Chat::new(pool.clone());
    chat.append_exchange_for(
        "old question one",
        "old answer one",
        "default",
        "app-server",
        None,
        None,
    )
    .await
    .expect("write one question and answer");
    chat.append_exchange_for(
        "old question two",
        "old answer two",
        "default",
        "app-server",
        None,
        None,
    )
    .await
    .expect("write the second question");

    let report = crate::ledger::backfill_from_chat(&pool)
        .await
        .expect("backfill");
    assert_eq!(report.sessions, 1);
    assert_eq!(
        report.turns, 2,
        "one question and one answer count as a turn"
    );
    assert_eq!(report.rows, 4, "two user lines and two assistant lines");

    let turns = crate::Ledger::new(pool.clone())
        .turns("default", 10)
        .await
        .expect("list the ledger");
    assert_eq!(turns.len(), 2);
    assert!(
        turns.iter().all(|turn| turn.fidelity == "folded"),
        "the history can only be the fidelity of the fold"
    );

    let again = crate::ledger::backfill_from_chat(&pool)
        .await
        .expect("backfill again");
    assert_eq!(again.turns, 2);
    assert_eq!(
        crate::Ledger::new(pool).count().await.expect("count"),
        2,
        "backfilling must not write a second round"
    );
}

#[tokio::test]
async fn fresh_home_creates_the_full_baseline_schema() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("state.db");
    let pool = crate::open(&path).await?;
    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx%'",
    ).fetch_one(&pool).await?;
    assert!(
        tables > 60,
        "the single baseline must create the full schema, got {tables} tables"
    );
    let forked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'forked_from'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(forked, 0);
    Ok(())
}

#[tokio::test]
async fn an_unrecognized_incremental_database_is_refused_without_changing_its_history()
-> anyhow::Result<()> {
    use sqlx::{Connection, SqliteConnection, migrate::Migrate, sqlite::SqliteConnectOptions};

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("state.db");
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let mut conn = SqliteConnection::connect_with(&options).await?;
    conn.ensure_migrations_table().await?;

    for version in 1..=25 {
        sqlx::query("INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time) VALUES (?, '', datetime('now'), 1, zeroblob(48), 0)")
            .bind(version)
            .execute(&mut conn)
            .await?;
    }
    conn.close().await?;

    let err = crate::open(&path).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("Unrecognized incremental schema history"),
        "{err}"
    );
    let mut conn = SqliteConnection::connect_with(&options).await?;
    let intact: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE checksum=zeroblob(48)")
            .fetch_one(&mut conn)
            .await?;
    assert_eq!(intact, 25);
    Ok(())
}

#[tokio::test]
async fn concurrent_queue_drains_claim_an_instruction_only_once() {
    let queue = crate::Queue::new(temp_db("queue_concurrent_claim").await);
    queue.push("s", "first").await.unwrap();
    queue.push("s", "second").await.unwrap();
    let (a, b) = tokio::join!(queue.pop("s"), queue.pop("s"));
    let claimed: Vec<_> = [a.unwrap(), b.unwrap()].into_iter().flatten().collect();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].message, "first");
    assert_eq!(queue.count("s").await.unwrap(), 1);
    queue.complete(claimed[0].id).await.unwrap();
    assert_eq!(queue.pop("s").await.unwrap().unwrap().message, "second");
}

#[tokio::test]
async fn reopening_an_interrupted_run_preserves_spend_and_refuses_terminal_runs() {
    let pool = temp_db("run_reopen").await;
    let tasks = crate::Tasks::new(pool.clone());
    tasks
        .create(crate::NewTask {
            id: "task-reopen",
            title: "resume",
            session_id: Some("s"),
            goal_condition: None,
            goal_draft: None,
            cwd: None,
            acceptance_json: "[]",
            budget_json: "{}",
            max_rounds: 1,
            tier: None,
        })
        .await
        .unwrap();
    let runs = crate::Runs::new(pool);
    runs.start("r", "task-reopen", Some("s"), Some("t"))
        .await
        .unwrap();
    runs.add_round("r", 10, 5, 1, 0).await.unwrap();
    runs.close("r", crate::RunStatus::Waiting, Some("restarted"))
        .await
        .unwrap();
    assert!(runs.reopen("r").await.unwrap());
    let row = runs.get("r").await.unwrap().unwrap();
    assert_eq!(row.status, "running");
    assert_eq!(row.tokens_in, 10);
    assert_eq!(row.tools_ok, 1);
    assert!(row.ended_at.is_none());
    runs.close("r", crate::RunStatus::Completed, None)
        .await
        .unwrap();
    assert!(!runs.reopen("r").await.unwrap());
}

#[tokio::test]
async fn recovered_round_charges_only_new_work_and_accounting_rolls_back_together() {
    let pool = temp_db("round_accounting").await;
    let tasks = crate::Tasks::new(pool.clone());
    tasks
        .create(crate::NewTask {
            id: "account-task",
            title: "accounting",
            session_id: None,
            goal_condition: None,
            goal_draft: None,
            cwd: None,
            acceptance_json: "[]",
            budget_json: "{}",
            max_rounds: 2,
            tier: None,
        })
        .await
        .unwrap();
    let runs = crate::Runs::new(pool.clone());
    runs.start("account-run", "account-task", None, None)
        .await
        .unwrap();
    let original = crate::runs::RoundAccount {
        tokens_in: 100,
        tokens_out: 20,
        tools_ok: 2,
        wall_ms: 50,
        ..Default::default()
    };
    runs.account_round("account-run", "round-1", &original)
        .await
        .unwrap();
    runs.account_round("account-run", "round-1", &original)
        .await
        .unwrap();
    let recovered = crate::runs::RoundAccount {
        tokens_in: 150,
        tokens_out: 30,
        tools_ok: 3,
        wall_ms: 80,
        ..Default::default()
    };
    runs.account_round("account-run", "round-1", &recovered)
        .await
        .unwrap();
    let task = tasks.get("account-task").await.unwrap().unwrap();
    assert_eq!(
        (task.spent_tokens, task.spent_tools, task.spent_wall_ms),
        (180, 3, 80)
    );
    let run = runs.get("account-run").await.unwrap().unwrap();
    assert_eq!(
        (run.rounds, run.tokens_in, run.tokens_out, run.tools_ok),
        (1, 150, 30, 3)
    );
    sqlx::query("CREATE TRIGGER fail_run_account BEFORE UPDATE ON task_runs BEGIN SELECT RAISE(ABORT, 'injected accounting failure'); END").execute(&pool).await.unwrap();
    assert!(
        runs.account_round("account-run", "round-2", &original)
            .await
            .is_err()
    );
    let task = tasks.get("account-task").await.unwrap().unwrap();
    assert_eq!(
        (task.spent_tokens, task.spent_tools, task.spent_wall_ms),
        (180, 3, 80)
    );
    assert_eq!(
        runs.round_account("account-run", "round-2")
            .await
            .unwrap()
            .tokens_in,
        0
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM run_round_accounts WHERE round_id = 'round-2'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "failed receipt must not suppress the eventual retry"
    );
}

#[tokio::test]
async fn jobs_bind_to_the_exact_host_run_and_reject_cross_session_binding() {
    let pool = temp_db("job_run_binding").await;
    let tasks = crate::Tasks::new(pool.clone());
    for id in ["older", "newer"] {
        tasks
            .create(crate::NewTask {
                id,
                title: id,
                session_id: Some("s"),
                goal_condition: None,
                goal_draft: None,
                cwd: None,
                acceptance_json: "[]",
                budget_json: "{}",
                max_rounds: 1,
                tier: None,
            })
            .await
            .unwrap();
    }
    let runs = crate::Runs::new(pool.clone());
    runs.start("older-run", "older", Some("s"), None)
        .await
        .unwrap();
    runs.start("newer-run", "newer", Some("s"), None)
        .await
        .unwrap();
    let jobs = crate::Jobs::new(pool);
    jobs.record_started_for_run("job-old", "s", "echo", "log", Some("older-run"))
        .await
        .unwrap();
    let row = jobs.get("job-old").await.unwrap().unwrap();
    assert_eq!(row.run_id.as_deref(), Some("older-run"));
    assert_eq!(row.task_id.as_deref(), Some("older"));
    assert!(
        jobs.record_started_for_run("foreign", "other", "echo", "log", Some("older-run"))
            .await
            .is_err()
    );
    assert!(jobs.get("foreign").await.unwrap().is_none());
    runs.close("older-run", crate::RunStatus::Completed, None)
        .await
        .unwrap();
    assert!(
        jobs.record_started_for_run("stale", "s", "echo", "log", Some("older-run"))
            .await
            .is_err()
    );
    assert!(jobs.get("stale").await.unwrap().is_none());
}

#[tokio::test]
async fn job_outcome_and_completion_event_commit_together() {
    let pool = temp_db("job_completion_atomic").await;
    let jobs = crate::Jobs::new(pool.clone());
    jobs.record_started_with_delivery("job", "s", "echo", "log", None, true, Some("target"))
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_completion BEFORE INSERT ON job_completions BEGIN SELECT RAISE(ABORT, 'injected completion failure'); END").execute(&pool).await.unwrap();
    assert!(
        jobs.record_finished("job", "completed", Some(0))
            .await
            .is_err()
    );
    assert_eq!(jobs.get("job").await.unwrap().unwrap().status, "running");
    assert!(jobs.pending_notifications().await.unwrap().is_empty());
    sqlx::query("DROP TRIGGER fail_completion")
        .execute(&pool)
        .await
        .unwrap();
    jobs.record_finished("job", "completed", Some(0))
        .await
        .unwrap();
    jobs.record_finished("job", "failed", Some(1))
        .await
        .unwrap();
    let pending = jobs.pending_notifications().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].status, "completed",
        "repeated completion must not rewrite the known outcome"
    );
    assert_eq!(pending[0].notify_target.as_deref(), Some("target"));
    jobs.notification_enqueued("job").await.unwrap();
    assert!(jobs.pending_notifications().await.unwrap().is_empty());
}

#[tokio::test]
async fn durable_job_waits_roll_back_bad_dependencies_and_honor_human_pause() {
    let pool = temp_db("durable_job_waits").await;
    let tasks = crate::Tasks::new(pool.clone());
    tasks
        .create(crate::NewTask {
            id: "wait-task",
            title: "wait",
            session_id: Some("s"),
            goal_condition: None,
            goal_draft: None,
            cwd: None,
            acceptance_json: "[]",
            budget_json: "{}",
            max_rounds: 2,
            tier: None,
        })
        .await
        .unwrap();
    tasks.set_running("wait-task", "wait-run").await.unwrap();
    let runs = crate::Runs::new(pool.clone());
    runs.start("wait-run", "wait-task", Some("s"), Some("wait-run"))
        .await
        .unwrap();
    let checkpoints = crate::Checkpoints::new(pool.clone());
    checkpoints
        .save_round(
            "wait-run",
            "s",
            "work",
            "[]",
            0,
            "{}",
            Some("wait-run"),
            Some("round"),
        )
        .await
        .unwrap();
    let jobs = crate::Jobs::new(pool.clone());
    jobs.record_started_for_run("old", "s", "historical", "log", Some("wait-run"))
        .await
        .unwrap();
    jobs.record_finished("old", "interrupted", None)
        .await
        .unwrap();
    jobs.record_started_for_run("current", "s", "work", "log", Some("wait-run"))
        .await
        .unwrap();
    let waits = crate::waits::JobWaits::new(pool.clone());
    assert!(
        waits
            .register(
                "wait-task",
                "wait-run",
                "wait-run",
                "s",
                &["missing".into()]
            )
            .await
            .is_err()
    );
    assert_eq!(
        tasks.get("wait-task").await.unwrap().unwrap().status,
        crate::TaskStatus::Running
    );
    assert_eq!(
        checkpoints.get("wait-run").await.unwrap().unwrap().status,
        "open"
    );
    assert_eq!(
        runs.get("wait-run").await.unwrap().unwrap().status,
        "running"
    );
    waits
        .register(
            "wait-task",
            "wait-run",
            "wait-run",
            "s",
            &["current".into()],
        )
        .await
        .unwrap();
    let wait = waits.get("wait-task").await.unwrap().unwrap();
    assert_eq!(waits.jobs("wait-task").await.unwrap().len(), 1);
    assert!(!waits.claim(&wait, "[]").await.unwrap());
    tasks
        .suspend("wait-task", Some("human pause"))
        .await
        .unwrap();
    jobs.record_finished("current", "completed", Some(0))
        .await
        .unwrap();
    assert!(!waits.claim(&wait, "[]").await.unwrap());
    assert_eq!(
        tasks
            .get("wait-task")
            .await
            .unwrap()
            .unwrap()
            .suspend_reason
            .as_deref(),
        Some("human pause")
    );
    crate::human_pauses::HumanPauses::new(pool.clone())
        .resume_checkpoint("wait-task", "wait-run", "[]")
        .await
        .unwrap();
    runs.reopen("wait-run").await.unwrap();
    waits
        .register(
            "wait-task",
            "wait-run",
            "wait-run",
            "s",
            &["current".into()],
        )
        .await
        .unwrap();
    let resumed_memory = r#"[{"role":"user","content":[{"type":"text","text":"job completed"}]}]"#;
    assert!(waits.claim(&wait, resumed_memory).await.unwrap());
    assert!(!waits.claim(&wait, resumed_memory).await.unwrap());
    let checkpoint = checkpoints.get("wait-run").await.unwrap().unwrap();
    assert_eq!(checkpoint.status, "open");
    assert_eq!(checkpoint.messages_json, resumed_memory);
}

#[tokio::test]
async fn a_baseline_database_can_be_reopened() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    crate::open(&path).await.unwrap();
    crate::open(&path)
        .await
        .expect("a database written by the same baseline opens again");
}

#[tokio::test]
async fn events_since_pages_filters_and_survives_a_bad_cursor() {
    let pool = temp_db("events_since.db").await;
    let events = crate::Events::new(pool);

    let (rows, next) = events.since(0, None, 10).await.unwrap();
    assert!(
        rows.is_empty() && next == 0,
        "empty log: nothing, cursor unchanged"
    );
    assert_eq!(events.latest_cursor(None).await.unwrap(), 0);

    for i in 0..5 {
        let task = if i % 2 == 0 { "a" } else { "b" };
        events
            .append(task, None, "verify", Some(&format!("e{i}")))
            .await
            .unwrap();
    }

    let (first, c1) = events.since(0, None, 3).await.unwrap();
    let (second, c2) = events.since(c1, None, 3).await.unwrap();
    let details: Vec<_> = first
        .iter()
        .chain(&second)
        .map(|r| r.detail.clone().unwrap())
        .collect();
    assert_eq!(details, ["e0", "e1", "e2", "e3", "e4"]);
    assert_eq!(second.len(), 2);
    assert_eq!(events.latest_cursor(None).await.unwrap(), c2);

    let (only_a, _) = events.since(0, Some("a"), 10).await.unwrap();
    assert_eq!(only_a.iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(events.latest_cursor(Some("zzz")).await.unwrap(), 0);

    let (none, same) = events.since(c2 + 1000, None, 10).await.unwrap();
    assert!(none.is_empty() && same == c2 + 1000);

    let (one, _) = events.since(0, None, 0).await.unwrap();
    assert_eq!(one.len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn database_path_with_url_punctuation_is_a_literal_filename() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes#1?draft.db");
    let pool = crate::open(&path).await.unwrap();
    assert_eq!(
        crate::db::schema_version(&pool).await.unwrap(),
        crate::db::known_schema_version()
    );
    pool.close().await;
    assert_eq!(
        crate::db::snapshot_version(&path).await.unwrap(),
        crate::db::known_schema_version()
    );
}

#[tokio::test]
async fn a_generated_title_lands_only_on_an_unnamed_session_after_its_first_reply() {
    let pool = temp_db("sessions_generated_title.db").await;
    let chat = Chat::new(pool);
    assert_eq!(chat.untitled_opening("s1").await.unwrap(), None);
    chat.append_exchange(
        "here is my long pasted error log",
        "answer",
        "s1",
        "cli",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        chat.untitled_opening("s1").await.unwrap().as_deref(),
        Some("here is my long pasted error log")
    );
    assert!(
        chat.set_generated_title("s1", "Pasted error log")
            .await
            .unwrap()
    );
    assert_eq!(
        chat.session("s1").await.unwrap().unwrap().title,
        "Pasted error log"
    );
    assert!(
        !chat
            .set_generated_title("s1", "Another guess")
            .await
            .unwrap(),
        "a stored title is never replaced by a generated one"
    );

    chat.create_session("named", Some("my name"), None)
        .await
        .unwrap();
    chat.append_exchange("hi", "hello", "named", "cli", None)
        .await
        .unwrap();
    assert_eq!(chat.untitled_opening("named").await.unwrap(), None);
    assert!(!chat.set_generated_title("named", "Greeting").await.unwrap());

    chat.append_exchange("one", "two", "busy", "cli", None)
        .await
        .unwrap();
    chat.append_exchange("three", "four", "busy", "cli", None)
        .await
        .unwrap();
    assert!(
        !chat
            .set_generated_title("busy", "Late title")
            .await
            .unwrap(),
        "a title that arrives after the second turn is dropped"
    );
}
