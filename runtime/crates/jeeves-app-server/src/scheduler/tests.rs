//! Matching and loading of scheduled jobs: cron semantics, once per minute, two sources.

use std::collections::HashMap;

use chrono::NaiveDate;

use super::{cron_matches, due_now, last_missed, load_jobs, Delivery, Job};

fn at(y: i32, m: u32, d: u32, hh: u32, mm: u32) -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .expect("a legal date")
        .and_hms_opt(hh, mm, 0)
        .expect("a legal time")
}

fn job(name: &str, cron: &str) -> Job {
    Job {
        name: name.to_string(),
        cron: cron.to_string(),
        prompt: "do something".to_string(),
        catch_up: true,
        delivery: Delivery::Outbox,
    }
}

#[test]
fn every_minute_matches_everything() {
    assert!(cron_matches("* * * * *", at(2026, 9, 21, 3, 7)));
}

#[test]
fn a_daily_time_matches_only_that_minute() {
    assert!(cron_matches("0 8 * * *", at(2026, 9, 21, 8, 0)));
    assert!(!cron_matches("0 8 * * *", at(2026, 9, 21, 8, 1)));
    assert!(!cron_matches("0 8 * * *", at(2026, 9, 21, 9, 0)));
}

#[test]
fn steps_ranges_and_lists_all_work() {
    assert!(cron_matches("*/15 * * * *", at(2026, 9, 21, 0, 30)));
    assert!(!cron_matches("*/15 * * * *", at(2026, 9, 21, 0, 31)));
    assert!(cron_matches("0 9-17 * * 1-5", at(2026, 9, 21, 9, 0)));
    assert!(cron_matches("0 9-17 * * 1-5", at(2026, 9, 25, 17, 0)));
    assert!(
        !cron_matches("0 9-17 * * 1-5", at(2026, 9, 26, 9, 0)),
        "Saturday should not fire"
    );
    assert!(cron_matches("0 8,20 * * *", at(2026, 9, 21, 20, 0)));
    assert!(!cron_matches("0 8,20 * * *", at(2026, 9, 21, 12, 0)));
    assert!(cron_matches("0 0 * * 7", at(2026, 9, 27, 0, 0)), "Sunday");
}

#[test]
fn a_malformed_cron_never_fires() {
    assert!(
        !cron_matches("0 8 * *", at(2026, 9, 21, 8, 0)),
        "four fields"
    );
    assert!(!cron_matches("banana", at(2026, 9, 21, 8, 0)));
    assert!(!cron_matches("0 8 * * 1-", at(2026, 9, 21, 8, 0)));
}

#[test]
fn the_same_minute_only_fires_once() {
    let jobs = vec![job("brief", "0 8 * * *")];
    let fired = HashMap::new();
    let first = due_now(&jobs, at(2026, 9, 21, 8, 0), &fired);
    assert_eq!(first.len(), 1);

    let mut fired = HashMap::new();
    fired.insert("brief".to_string(), "2026-09-21T08:00".to_string());
    let second = due_now(&jobs, at(2026, 9, 21, 8, 0), &fired);
    assert!(
        second.is_empty(),
        "the same minute must not fire a second time"
    );
    let later = due_now(&jobs, at(2026, 9, 21, 8, 1), &fired);
    assert!(later.is_empty());
}

#[tokio::test]
async fn firing_runs_a_turn_and_writes_the_reply_to_the_outbox() {
    use std::sync::Arc;

    use jeeves_provider::mock::Mock;
    use jeeves_provider::Resolved;

    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().to_path_buf();
    let settings = jeeves_config::Settings {
        home: home.clone(),
        api_key: Some("dummy".to_string()),
        ..Default::default()
    };
    let server = crate::open(&settings).await.expect("assemble");
    server.install_provider(Resolved {
        vision: false,
        provider_id: "mock".to_string(),
        client: Arc::new(Mock::new(vec![Mock::text("check-in done.")])),
        model: "test".to_string(),
        small_model: "test-small".to_string(),
        context_window: jeeves_provider::DEFAULT_CONTEXT_WINDOW,
    });

    let job = job("tick", "* * * * *");
    super::fire(&server, &job, at(2026, 9, 21, 9, 0))
        .await
        .expect("the trigger succeeds");

    let path = home.join("outbox").join("schedule-tick-20260921-0900.md");
    let written = std::fs::read_to_string(&path).expect("the result file");
    assert!(written.contains("check-in done."), "{written}");
    assert!(
        written.contains("cron '* * * * *'"),
        "it has to say where the trigger came from: {written}"
    );

    let sessions = server.chat().sessions().await.expect("the session list");
    assert!(
        sessions.iter().any(|s| s.id == "schedule:tick"),
        "a scheduled job needs a session of its own: {sessions:?}"
    );
}

async fn pool_in(home: &std::path::Path) -> sqlx::SqlitePool {
    jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store")
}

#[tokio::test]
async fn jobs_load_from_both_skills_and_the_table() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path();
    let skill_dir = home.join("skills").join("weekly-review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: weekly-review\ndescription: summarize the week\nschedule: 0 8 * * 1\n---\nsteps",
    )
    .unwrap();
    let plain = home.join("skills").join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(
        plain.join("SKILL.md"),
        "---\nname: plain\ndescription: nothing scheduled\n---\nsteps",
    )
    .unwrap();
    let pool = pool_in(home).await;
    jeeves_state::ScheduleJobs::new(pool.clone())
        .save(&jeeves_state::ScheduleJobRow {
            name: "standup".into(),
            cron: "0 9 * * 1-5".into(),
            prompt: "what is on today?".into(),
            delivery: "outbox".into(),
            catch_up: true,
        })
        .await
        .unwrap();

    let jobs = load_jobs(home, &pool).await;
    let names: Vec<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
    assert_eq!(names, vec!["weekly-review", "standup"]);
    let weekly = jobs.iter().find(|j| j.name == "weekly-review").unwrap();
    assert_eq!(weekly.cron, "0 8 * * 1");
    assert!(
        weekly.prompt.contains("summarize the week"),
        "a skill job's request has to carry its own description: {}",
        weekly.prompt
    );
}

#[tokio::test]
async fn per_job_policy_fields_are_loaded() {
    let dir = tempfile::tempdir().expect("temp dir");
    let pool = pool_in(dir.path()).await;
    let store = jeeves_state::ScheduleJobs::new(pool.clone());
    for (name, cron, delivery, catch_up) in [
        ("morning-briefing", "0 8 * * *", "notify", false),
        ("watch-ci", "*/15 * * * *", "quiet", true),
        ("default", "0 12 * * *", "outbox", true),
        ("misspelt", "0 13 * * *", "a daily report", true),
    ] {
        store
            .save(&jeeves_state::ScheduleJobRow {
                name: name.into(),
                cron: cron.into(),
                prompt: "do it".into(),
                delivery: delivery.into(),
                catch_up,
            })
            .await
            .unwrap();
    }
    let jobs = load_jobs(dir.path(), &pool).await;
    assert_eq!(jobs.len(), 4);
    let by = |name: &str| jobs.iter().find(|j| j.name == name).expect("the job");
    assert!(
        !by("morning-briefing").catch_up,
        "catch_up=false means at most once"
    );
    assert_eq!(by("morning-briefing").delivery, Delivery::Notify);
    assert_eq!(by("watch-ci").delivery, Delivery::Quiet);
    assert!(by("default").catch_up);
    assert_eq!(by("default").delivery, Delivery::Outbox);
    assert_eq!(
        by("misspelt").delivery,
        Delivery::Outbox,
        "an unknown value falls back"
    );
}

#[tokio::test]
async fn the_legacy_file_is_imported_once_with_the_same_cron() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path();
    std::fs::write(
        home.join("schedules.json"),
        r#"{"jobs":[
            {"name":"standup","cron":"0 9 * * 1-5","prompt":"what is on today?"},
            {"name":"ci","cron":"*/15 * * * *","prompt":"look","delivery":"quiet","catch_up":false},
            {"name":"broken","cron":"0 9 * * 1-5"},
            {"name":"bad-cron","cron":"99 9 * * *","prompt":"x"}
        ]}"#,
    )
    .unwrap();
    let pool = pool_in(home).await;
    let imported = jeeves_tools::schedules::import_legacy_file(home, &pool)
        .await
        .expect("import");
    assert_eq!(
        imported, 2,
        "the half-written and the invalid entries are skipped"
    );
    assert!(!home.join("schedules.json").exists());
    assert!(home.join("schedules.json.migrated").exists());

    let jobs = load_jobs(home, &pool).await;
    let standup = jobs.iter().find(|j| j.name == "standup").expect("standup");
    assert_eq!(
        standup.cron, "0 9 * * 1-5",
        "the same cron before and after"
    );
    let ci = jobs.iter().find(|j| j.name == "ci").expect("ci");
    assert_eq!(ci.delivery, Delivery::Quiet);
    assert!(!ci.catch_up);
    assert!(cron_matches(&standup.cron, at(2026, 9, 21, 9, 0)));

    assert_eq!(
        jeeves_tools::schedules::import_legacy_file(home, &pool)
            .await
            .unwrap(),
        0
    );
}

#[test]
fn a_missed_fire_is_caught_up_once() {
    let daily = job("morning briefing", "0 8 * * *");

    let missed = last_missed(&daily, at(2026, 10, 27, 7, 0), at(2026, 10, 27, 9, 0));
    assert_eq!(missed, Some(at(2026, 10, 27, 8, 0)));

    assert_eq!(
        last_missed(&daily, at(2026, 10, 27, 9, 0), at(2026, 10, 27, 9, 30)),
        None
    );

    let missed = last_missed(&daily, at(2026, 10, 26, 7, 59), at(2026, 10, 27, 8, 30));
    assert_eq!(
        missed,
        Some(at(2026, 10, 27, 8, 0)),
        "three days down, still only the newest is caught up"
    );
}

#[test]
fn an_every_minute_job_yields_only_the_latest_missed_minute() {
    let frequent = job("heartbeat", "* * * * *");
    let missed = last_missed(&frequent, at(2026, 10, 20, 0, 0), at(2026, 10, 27, 0, 0));
    assert_eq!(missed, Some(at(2026, 10, 27, 0, 0)));
}

#[test]
fn the_run_log_line_has_all_the_fields() {
    let line = super::run_log_line("morning briefing", "2026-10-30T08:00", "ok", "", 42, true);
    let parsed: serde_json::Value = serde_json::from_str(&line).expect("legal JSON");
    for key in [
        "ts",
        "name",
        "minute",
        "status",
        "detail",
        "reply_chars",
        "delivered",
    ] {
        assert!(parsed.get(key).is_some(), "missing field {key}");
    }
    assert_eq!(parsed["name"], "morning briefing");
    assert_eq!(parsed["minute"], "2026-10-30T08:00");
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["reply_chars"], 42);
    assert_eq!(parsed["delivered"], true);

    let err = super::run_log_line(
        "watch CI",
        "2026-10-30T09:00",
        "error",
        "the turn failed: timed out",
        0,
        false,
    );
    let parsed: serde_json::Value = serde_json::from_str(&err).expect("legal JSON");
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["detail"], "the turn failed: timed out");
    assert_eq!(parsed["delivered"], false);
}

#[test]
fn the_run_log_appends_and_never_panics() {
    let dir = tempfile::tempdir().expect("temp dir");
    super::append_run_log(
        dir.path(),
        &super::run_log_line("a", "m1", "ok", "", 1, true),
    );
    super::append_run_log(
        dir.path(),
        &super::run_log_line("b", "m2", "empty", "", 0, false),
    );

    let text =
        std::fs::read_to_string(dir.path().join("schedule-runs.jsonl")).expect("the ledger exists");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "two appends make two lines");
    assert!(
        lines[0].contains("\"name\":\"a\""),
        "the older line is still there"
    );
    assert!(
        lines[1].contains("\"status\":\"empty\""),
        "the new line comes after it"
    );

    super::append_run_log(dir.path().join("no such directory").as_path(), "x");
}

#[tokio::test]
async fn an_uncertain_trigger_is_not_replayed_and_requires_explicit_review() {
    if !jeeves_tools::exec::sandbox_available() {
        return;
    }
    use jeeves_provider::{mock::Mock, Resolved};
    use std::sync::Arc;
    let home = tempfile::tempdir().unwrap();
    let effects = tempfile::tempdir().unwrap();
    let marker = effects.path().join("committed.txt");
    let settings = jeeves_config::Settings {
        home: home.path().into(),
        api_key: Some("dummy".into()),
        exec_enabled: true,
        exec_allow: vec!["*".into()],
        exec_timeout_secs: 1,
        ..Default::default()
    };
    let server = crate::open(&settings).await.unwrap();
    let mock = Arc::new(Mock::new(vec![
        Mock::tool_use(
            "effect",
            "run_command",
            serde_json::json!({"command":format!("printf committed >> '{}'; sleep 3",marker.display()),"cwd":effects.path()}),
        ),
        Mock::text("Must not be reached"),
    ]));
    server.install_provider(Resolved {
        vision: false,
        provider_id: "mock".into(),
        client: mock.clone(),
        model: "test".into(),
        small_model: "test".into(),
        context_window: 100_000,
    });
    let job = job("uncertain", "* * * * *");
    let error = super::fire(&server, &job, at(2026, 11, 13, 8, 0))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("requires review"), "{error}");
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "committed");
    assert_eq!(mock.received.lock().unwrap().len(), 1);
    super::fire(&server, &job, at(2026, 11, 13, 8, 1))
        .await
        .unwrap();
    assert_eq!(mock.received.lock().unwrap().len(), 1);
    let schedules = jeeves_state::Schedules::new(server.state_pool());
    assert_eq!(
        schedules
            .pending_claim("uncertain")
            .await
            .unwrap()
            .as_deref(),
        Some("2026-11-13T08:00")
    );
    super::acknowledge(&server, "uncertain").await.unwrap();
    assert!(schedules
        .pending_claim("uncertain")
        .await
        .unwrap()
        .is_none());
    assert_eq!(mock.received.lock().unwrap().len(), 1);
}

mod gap_tests {
    use super::*;
    use crate::scheduler::gap::{is_gap, missed_in_gap, verdict, Verdict};

    #[test]
    fn only_a_forward_jump_beyond_a_tick_is_a_gap() {
        let t = at(2026, 9, 21, 3, 0);
        assert!(!is_gap(t, t + chrono::TimeDelta::try_seconds(25).unwrap()));
        assert!(is_gap(t, t + chrono::TimeDelta::try_minutes(30).unwrap()));
        assert!(
            !is_gap(t, t - chrono::TimeDelta::try_hours(1).unwrap()),
            "backwards is not a gap"
        );
    }

    #[test]
    fn a_trigger_inside_the_gap_is_found_once_as_the_latest() {
        let daily = job("brief", "0 8 * * *");
        let missed = missed_in_gap(&daily, at(2026, 9, 21, 23, 0), at(2026, 9, 22, 9, 30));
        assert_eq!(missed, Some(at(2026, 9, 22, 8, 0)));
    }

    #[test]
    fn several_missed_days_still_give_one_trigger() {
        let hourly = job("h", "0 * * * *");
        let missed = missed_in_gap(&hourly, at(2026, 9, 21, 0, 0), at(2026, 9, 24, 12, 30));
        assert_eq!(missed, Some(at(2026, 9, 24, 12, 0)));
    }

    #[test]
    fn the_current_minute_is_left_to_the_normal_path() {
        let every = job("e", "* * * * *");
        let now = at(2026, 9, 22, 9, 30);
        assert_eq!(
            missed_in_gap(&every, at(2026, 9, 22, 9, 0), now),
            Some(at(2026, 9, 22, 9, 29))
        );
    }

    #[test]
    fn nothing_missed_means_nothing_to_do() {
        let daily = job("brief", "0 8 * * *");
        assert_eq!(
            missed_in_gap(&daily, at(2026, 9, 22, 8, 1), at(2026, 9, 22, 9, 30)),
            None
        );
    }

    #[test]
    fn a_gap_longer_than_the_lookback_only_scans_seven_days() {
        let daily = job("d", "0 8 * * *");
        let missed = missed_in_gap(&daily, at(2026, 8, 1, 0, 0), at(2026, 9, 22, 9, 30));
        assert_eq!(missed, Some(at(2026, 9, 22, 8, 0)));
        let monthly = job("m", "0 8 1 * *");
        assert_eq!(
            missed_in_gap(&monthly, at(2026, 8, 1, 0, 0), at(2026, 9, 22, 9, 30)),
            None
        );
    }

    #[test]
    fn claimed_triggers_are_never_replayed_and_catch_up_off_is_only_reported() {
        let mut j = job("brief", "0 8 * * *");
        let miss = Some(at(2026, 9, 22, 8, 0));
        assert_eq!(verdict(&j, miss, true), Verdict::Blocked);
        assert_eq!(
            verdict(&j, miss, false),
            Verdict::Fire(at(2026, 9, 22, 8, 0))
        );
        j.catch_up = false;
        assert_eq!(
            verdict(&j, miss, false),
            Verdict::Report(at(2026, 9, 22, 8, 0))
        );
        assert_eq!(verdict(&j, None, false), Verdict::Nothing);
    }
}

mod gap_reconcile_tests {
    use super::*;
    use crate::{scheduler::gap, Server};
    use jeeves_provider::Resolved;
    use std::sync::Arc;

    async fn boot(replies: usize) -> (tempfile::TempDir, Server, Arc<jeeves_provider::mock::Mock>) {
        let home = tempfile::tempdir().unwrap();
        let pool = jeeves_state::open(&home.path().join("state.db"))
            .await
            .unwrap();
        let server = Server::boot(
            pool,
            jeeves_config::Settings {
                home: home.path().into(),
                ..Default::default()
            },
        )
        .await;
        let model = Arc::new(jeeves_provider::mock::Mock::new(
            (0..replies)
                .map(|_| jeeves_provider::mock::Mock::text("Brief ready."))
                .collect(),
        ));
        server.install_provider(Resolved {
            client: model.clone(),
            model: "test".into(),
            small_model: "test".into(),
            provider_id: "mock".into(),
            vision: false,
            context_window: 100_000,
        });
        (home, server, model)
    }

    #[tokio::test]
    async fn a_trigger_missed_in_the_gap_runs_once() {
        let (_home, server, model) = boot(2).await;
        let brief = job("brief", "0 8 * * *");
        let mut fired = HashMap::new();
        let jobs = [brief];
        gap::reconcile(
            &server,
            &jobs,
            at(2026, 9, 21, 23, 0),
            at(2026, 9, 22, 9, 30),
            &mut fired,
        )
        .await;
        assert_eq!(
            model.received.lock().unwrap().len(),
            1,
            "one turn, not a chain"
        );
        assert_eq!(
            fired.get("brief").map(String::as_str),
            Some("2026-09-22T09:30")
        );
        gap::reconcile(
            &server,
            &jobs,
            at(2026, 9, 21, 23, 0),
            at(2026, 9, 22, 9, 30),
            &mut fired,
        )
        .await;
        assert_eq!(model.received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn catch_up_off_is_reported_and_not_run() {
        let (_home, server, model) = boot(1).await;
        let mut quiet = job("brief", "0 8 * * *");
        quiet.catch_up = false;
        let mut fired = HashMap::new();
        gap::reconcile(
            &server,
            &[quiet],
            at(2026, 9, 21, 23, 0),
            at(2026, 9, 22, 9, 30),
            &mut fired,
        )
        .await;
        assert!(model.received.lock().unwrap().is_empty());
        assert!(fired.is_empty());
    }

    #[tokio::test]
    async fn a_claimed_unsettled_trigger_is_not_replayed() {
        let (_home, server, model) = boot(1).await;
        let brief = job("brief", "0 8 * * *");
        let schedules = jeeves_state::Schedules::new(server.state_pool());
        schedules.ensure("brief", true, "outbox").await.unwrap();
        schedules.claim("brief", "2026-09-21T08:00").await.unwrap();
        let mut fired = HashMap::new();
        gap::reconcile(
            &server,
            &[brief],
            at(2026, 9, 21, 23, 0),
            at(2026, 9, 22, 9, 30),
            &mut fired,
        )
        .await;
        assert!(model.received.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_gap_and_no_miss_runs_nothing() {
        let (_home, server, model) = boot(1).await;
        let brief = job("brief", "0 8 * * *");
        let mut fired = HashMap::new();
        gap::reconcile(
            &server,
            &[brief],
            at(2026, 9, 22, 8, 5),
            at(2026, 9, 22, 9, 30),
            &mut fired,
        )
        .await;
        assert!(model.received.lock().unwrap().is_empty());
    }
}
