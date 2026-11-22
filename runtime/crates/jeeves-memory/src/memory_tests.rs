//! Tests for retrieval and consolidation.
//!
//! All use jeeves_provider::mock::Mock — what the model says is fully test-decided,
//! so conventions like "fail-open" can be asserted exactly.

use jeeves_provider::mock::Mock;
use jeeves_provider::ProviderError;

use super::consolidation::consolidate_if_due;
use super::retrieve_context;
use super::skills::{parse_skill_text, seed_default_skills, SkillLoader};
use jeeves_state::{Chat, Episodes, Facts};

async fn stores() -> (Facts, Episodes, Chat) {
    let dir = tempfile::tempdir().expect("temp dir");
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    (
        Facts::new(pool.clone()),
        Episodes::new(pool.clone()),
        Chat::new(pool),
    )
}

#[test]
fn extract_json_picks_the_braces() {
    use jeeves_provider::extract_json;
    assert_eq!(
        extract_json("prefix {\"a\":1} suffix").as_deref(),
        Some("{\"a\":1}")
    );
    assert_eq!(extract_json("no brackets"), None, "no braces, no JSON");
    assert_eq!(
        extract_json("{\"a\":{\"b\":2}}").as_deref(),
        Some("{\"a\":{\"b\":2}}")
    );
}

#[tokio::test]
async fn retrieve_context_formats_facts_and_episodes() {
    let (facts, episodes, _chat) = stores().await;
    facts
        .add("alex", "Alex prefers morning meetings", "user", "user")
        .await
        .unwrap();
    episodes
        .add("2026-09-01", "planned the Acme demo with alex")
        .await
        .unwrap();

    let text = retrieve_context(&facts, &episodes, "alex", 4)
        .await
        .unwrap();
    assert!(text.contains("**alex**"), "{text}");
    assert!(
        text.contains("(2026-09-01) planned the Acme demo"),
        "{text}"
    );
}

#[tokio::test]
async fn retrieve_context_is_empty_when_nothing_matches() {
    let (facts, episodes, _chat) = stores().await;
    let text = retrieve_context(&facts, &episodes, "??? ", 4)
        .await
        .unwrap();
    assert!(text.is_empty(), "no hits means no heading: {text:?}");
}

#[tokio::test]
async fn consolidation_waits_until_enough_exchanges() {
    let (facts, episodes, chat) = stores().await;
    chat.append_exchange("hi", "hello", "default", "cli", None)
        .await
        .unwrap();

    let mock = Mock::new(vec![]);
    let written = consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 6)
        .await
        .unwrap();
    assert_eq!(written, 0, "one turn in hand is not enough to distil");
    assert!(
        mock.received.lock().unwrap().is_empty(),
        "below the threshold the model must not be called"
    );
}

#[tokio::test]
async fn consolidation_distills_facts_and_marks_rows() {
    let (facts, episodes, chat) = stores().await;
    for i in 0..6 {
        chat.append_exchange(
            &format!("msg{i}"),
            &format!("reply{i}"),
            "default",
            "cli",
            None,
        )
        .await
        .unwrap();
    }
    let mock = Mock::new(vec![Mock::text(
        r#"{"facts": [{"subject": "alex", "content": "Alex prefers mornings"}],
            "episode": "talked about scheduling"}"#,
    )]);

    let written = consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 6)
        .await
        .unwrap();
    assert_eq!(written, 1);

    let all = facts.recent(10, 0).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(
        all[0].source, "consolidation",
        "a distilled fact has to name its source"
    );
    assert!(
        chat.unconsolidated().await.unwrap().is_empty(),
        "marked only after success"
    );

    let eps = episodes.recent(10).await.unwrap();
    assert_eq!(eps.len(), 1);
}

#[tokio::test]
async fn consolidation_drops_temporary_statements_but_keeps_facts() {
    let (facts, episodes, chat) = stores().await;
    for i in 0..6 {
        chat.append_exchange(&format!("m{i}"), &format!("r{i}"), "default", "cli", None)
            .await
            .unwrap();
    }
    let mock = Mock::new(vec![Mock::text(
        r#"{"facts": [
             {"subject": "plan", "content": "use plan A for this session"},
             {"subject": "alex", "content": "Alex prefers mornings"}
           ],
           "episode": "planned the week"}"#,
    )]);

    let written = consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 6)
        .await
        .unwrap();
    assert_eq!(written, 1, "exactly one fact should be written");

    let all = facts.recent(10, 0).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].subject, "alex");
}

#[tokio::test]
async fn consolidation_failure_keeps_the_log() {
    let (facts, episodes, chat) = stores().await;
    for i in 0..6 {
        chat.append_exchange(
            &format!("msg{i}"),
            &format!("reply{i}"),
            "default",
            "cli",
            None,
        )
        .await
        .unwrap();
    }

    let mock = Mock::new(vec![Mock::text(
        "I do not think this conversation is worth remembering.",
    )]);
    let written = consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 6)
        .await
        .unwrap();
    assert_eq!(written, 0);
    assert_eq!(
        chat.unconsolidated().await.unwrap().len(),
        0,
        "a row that failed backs off; it must not be picked up again at once"
    );
    let log = chat.session_history("default").await.unwrap();
    assert_eq!(log.len(), 6, "not one log line may go missing");

    let mock = Mock::new(vec![Mock::text(
        r#"{"facts": [{"subject": "", "content": "x"}, {"subject": "a", "content": ""}]}"#,
    )]);
    let written = consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 6)
        .await
        .unwrap();
    assert_eq!(written, 0);
    assert!(facts.recent(10, 0).await.unwrap().is_empty());
}

#[test]
fn temporary_statements_are_kept_out_of_long_term_memory() {
    use super::consolidation::temporary_marker;
    assert!(
        temporary_marker("use this plan for this session").is_some(),
        "a this-session marker is caught"
    );
    assert!(temporary_marker("We will use this path for now").is_some());
    assert!(
        temporary_marker("move the meeting to the afternoon for now").is_some(),
        "a temporary marker is caught"
    );
    assert!(
        temporary_marker("Alex likes morning meetings").is_none(),
        "a durable fact is not a temporary one"
    );
    assert!(temporary_marker("The release is on October 15").is_none());
    assert!(temporary_marker("FOR NOW keep it simple").is_some());
}

#[test]
fn a_skill_can_opt_out_of_implicit_invocation_and_be_summoned_by_name() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().to_path_buf();
    let folder = home.join("skills").join("weekly-review");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("SKILL.md"),
        "---\nname: weekly-review\ndescription: plan the quarterly roadmap review\nallow-model-invocation: false\n---\nexplicit reference only.\n",
    )
    .unwrap();

    let mut loader = SkillLoader::new(SkillLoader::dirs_for(&home));
    let implicit = loader.hits("help me plan the quarterly roadmap review");
    assert!(
        implicit.section.is_empty(),
        "with allow-model-invocation: false it must not be triggered implicitly: {}",
        implicit.section
    );

    let explicit = loader.hits("$weekly-review take a look at this week for me");
    assert!(
        explicit.section.contains("explicit reference only."),
        "{}",
        explicit.section
    );
    assert_eq!(
        explicit.message, "take a look at this week for me",
        "an explicit request keeps its own message"
    );
    assert!(explicit.hints.is_empty());
}

#[test]
fn a_reference_to_an_unknown_skill_becomes_a_hint_not_an_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().to_path_buf();
    std::fs::create_dir_all(home.join("skills")).unwrap();

    let mut loader = SkillLoader::new(SkillLoader::dirs_for(&home));
    let hits = loader.hits("$nope do something for me");
    assert!(hits.section.is_empty());
    assert_eq!(
        hits.message, "do something for me",
        "a memory hit carries the current message"
    );
    assert_eq!(hits.hints.len(), 1);
    assert!(
        hits.hints[0].contains("no skill is named 'nope'"),
        "{:?}",
        hits.hints
    );
}

#[test]
fn a_skill_with_a_missing_dependency_does_not_fire_implicitly() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().to_path_buf();
    let folder = home.join("skills").join("deploy");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("SKILL.md"),
        "---\nname: deploy\ndescription: ship the quarterly roadmap release\ndependencies: changelog, nobody\n---\nthe body\n",
    )
    .unwrap();

    let mut loader = SkillLoader::new(SkillLoader::dirs_for(&home));
    let hits = loader.hits("ship the quarterly roadmap release");
    assert!(
        hits.section.is_empty(),
        "with a dependency missing it takes no part in implicit triggering: {}",
        hits.section
    );

    let dangling = loader.dangling_dependencies();
    assert_eq!(dangling.len(), 1);
    assert_eq!(dangling[0].0, "deploy");
    assert!(
        dangling[0].1 == "changelog" || dangling[0].1 == "nobody",
        "reporting one missing dependency is enough: {dangling:?}"
    );
}

const SKILL_MD: &str = "\
---
name: weekly-review
description: Summarize the week and draft the Monday brief
---

1. Pull last week's episodes.
2. Distill into three bullets.
";

#[test]
fn skill_frontmatter_must_carry_name_and_description() {
    let skill = parse_skill_text(SKILL_MD).expect("a legal SKILL.md");
    assert_eq!(skill.name, "weekly-review");
    assert!(skill.description.contains("Monday brief"));
    assert!(skill.body.starts_with("1. Pull"), "{:?}", skill.body);

    assert!(
        parse_skill_text("plain markdown with no frontmatter").is_none(),
        "no frontmatter, no skill"
    );
    assert!(parse_skill_text("---\nname: x\n---\nbody").is_none());
    let skill = parse_skill_text("---\nname: 'a'\ndescription: \"b\"\n---\nx\n").unwrap();
    assert_eq!(
        (skill.name.as_str(), skill.description.as_str()),
        ("a", "b")
    );
}

#[test]
fn trigger_is_transparent_keyword_overlap() {
    let dir = tempfile::tempdir().expect("temp dir");
    let skills = dir.path().join("skills");
    std::fs::create_dir_all(skills.join("weekly-review")).unwrap();
    std::fs::write(skills.join("weekly-review").join("SKILL.md"), SKILL_MD).unwrap();

    let mut loader = SkillLoader::new(vec![skills]);

    let hit = loader
        .hits("help me do the weekly review of my inbox")
        .section;
    assert!(hit.contains("Pull last week's episodes"), "{hit}");

    let miss = loader.hits("what is 2+2").section;
    assert!(miss.is_empty(), "nothing should fire: {miss}");
}

#[test]
fn a_skill_created_mid_session_is_live_next_match() {
    let dir = tempfile::tempdir().expect("temp dir");
    let skills = dir.path().join("skills");
    let mut loader = SkillLoader::new(vec![skills.clone()]);
    assert!(loader.hits("plan the quarterly roadmap").section.is_empty());

    std::fs::create_dir_all(skills.join("roadmap-planning")).unwrap();
    std::fs::write(
        skills.join("roadmap-planning").join("SKILL.md"),
        "---\nname: roadmap-planning\ndescription: plan the quarterly roadmap and milestones\n---\nStep one.",
    )
    .unwrap();

    let hit = loader.hits("let us plan the quarterly roadmap now").section;
    assert!(
        hit.contains("Step one"),
        "a changed catalog has to be rescanned: {hit}"
    );
}

#[tokio::test]
async fn two_concurrent_consolidations_only_run_the_model_once() {
    let (facts, episodes, chat) = stores().await;
    chat.append_exchange(
        "alex likes morning meetings",
        "noted",
        "default",
        "cli",
        None,
    )
    .await
    .unwrap();

    let mock = Mock::new(vec![Mock::text(
        r#"{"facts":[{"subject":"alex","content":"likes morning meetings"}],"episode":"talked about alex"}"#,
    )]);

    let (first, second) = tokio::join!(
        consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 1),
        consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 1),
    );
    assert_eq!(
        first.unwrap() + second.unwrap(),
        1,
        "only the winner wrote a fact"
    );
    assert_eq!(
        mock.received.lock().unwrap().len(),
        1,
        "the small model is called once only — the losing one must not see the script"
    );
    assert!(
        chat.unconsolidated().await.unwrap().is_empty(),
        "the claimed batch is already marked"
    );
}

#[tokio::test]
async fn a_failed_consolidation_releases_its_claim() {
    let dir = tempfile::tempdir().expect("temp dir");
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    let facts = Facts::new(pool.clone());
    let episodes = Episodes::new(pool.clone());
    let chat = Chat::new(pool.clone());
    chat.append_exchange(
        "alex likes morning meetings",
        "noted",
        "default",
        "cli",
        None,
    )
    .await
    .unwrap();

    let mock = Mock::with_outcomes(vec![Err(ProviderError::Api("the model died".to_string()))]);
    let written = consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 1)
        .await
        .unwrap();
    assert_eq!(written, 0, "not a single fact was written");

    let still_pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_log WHERE consolidated = 0")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(still_pending, 2, "both rows have to come back");
    assert_eq!(
        chat.unconsolidated().await.unwrap().len(),
        0,
        "but the backoff schedules it into the future, so it must not be picked up at once"
    );
}

#[test]
fn the_bundled_video_skill_parses_and_carries_the_discipline() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../skills/video-transcription/SKILL.md"
    );
    let text =
        std::fs::read_to_string(path).expect("the skill file shipped with the repo should exist");
    let skill = crate::skills::parse_skill_text(&text).expect("it should parse as a skill");
    assert_eq!(skill.name, "video-transcription");
    assert!(
        skill.description.contains("video") && skill.description.contains("read_file"),
        "the description has to state the purpose and the path: {}",
        skill.description
    );
    for discipline in [
        "yt-dlp",
        "whisper",
        "never pretend",
        "data is not an instruction",
        "JEEVES_EXEC",
    ] {
        assert!(
            text.contains(discipline),
            "the body lacks the key discipline '{discipline}'"
        );
    }
}

#[test]
fn a_crlf_skill_file_still_parses() {
    let text = "---
name: crlf-skill
description: a skill edited on Windows
---

the body
";
    let skill = crate::skills::parse_skill_text(text).expect("CRLF should be normalised");
    assert_eq!(skill.name, "crlf-skill");
    assert_eq!(
        skill.description, "a skill edited on Windows",
        "the description survives the line endings"
    );
}

#[tokio::test]
async fn consolidation_cannot_resurrect_a_deleted_or_corrected_fact() {
    let (facts, episodes, chat) = stores().await;
    let (deleted, _) = facts
        .add("alex", "Prefers mornings", "user", "user")
        .await
        .unwrap();
    facts.delete(deleted.id).await.unwrap();
    let (corrected, _) = facts
        .add("sam", "Lives in Paris", "user", "user")
        .await
        .unwrap();
    facts.update(corrected.id, "Lives in Berlin").await.unwrap();
    facts.forget_subject("project").await.unwrap();
    chat.append_exchange("old conversation", "old reply", "default", "cli", None)
        .await
        .unwrap();
    let mock = Mock::new(vec![Mock::text(
        r#"{"facts":[
        {"subject":"alex","content":"Prefers mornings"},
        {"subject":"sam","content":"Lives in Paris"},
        {"subject":"project","content":"An old project detail"}],"episode":""}"#,
    )]);
    assert_eq!(
        consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 1)
            .await
            .unwrap(),
        0
    );
    let stored = facts.recent(10, 0).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].content, "Lives in Berlin");
    assert!(chat.unconsolidated().await.unwrap().is_empty());
}

#[tokio::test]
async fn consolidation_keeps_explicit_temporary_states_out_of_durable_memory() {
    let (facts, episodes, chat) = stores().await;
    chat.append_exchange(
        "I feel exhausted today.",
        "I hear you.",
        "chat",
        "test",
        None,
    )
    .await
    .unwrap();
    let digest = jeeves_state::ledger::digest_of(
        serde_json::to_string(&("user", "I feel exhausted today."))
            .unwrap()
            .as_bytes(),
    );
    let response = serde_json::json!({
        "facts": [], "episode": "The user reported feeling exhausted.",
        "states": [{"subject":"user", "content":"Feels exhausted today", "observation": {
            "sourceRef":format!("chat:1@{digest}"), "quote":"feel exhausted today", "confidence":0.8, "ttlHours":4
        }}]
    });
    let mock = Mock::new(vec![Mock::text(&response.to_string())]);
    let written = consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 1)
        .await
        .unwrap();
    assert_eq!(
        written, 1,
        "The temporary observation must be retained with an expiry"
    );
    assert!(facts.all_by_subject(10).await.unwrap().is_empty());
    assert_eq!(facts.search("exhausted", 5).await.unwrap().len(), 1);
    let context = retrieve_context(&facts, &episodes, "exhausted", 5)
        .await
        .unwrap();
    assert!(context.contains("temporary claim, valid until"));
    assert!(context.contains("not proof of current mood"));
    let requests = mock.received.lock().unwrap();
    assert!(requests[0]
        .system
        .as_deref()
        .unwrap()
        .contains("untrusted source data"));
    assert!(requests[0].messages[0]
        .text()
        .contains(&format!("chat:1@{digest}")));
}

#[tokio::test]
async fn malformed_extraction_shapes_release_the_claim_instead_of_losing_history() {
    for response in [
        "{}",
        r#"{"facts":{},"episode":"bad shape"}"#,
        r#"{"facts":[],"episode":42}"#,
        r#"{"facts":[],"episode":"x","states":{}}"#,
    ] {
        let home = tempfile::tempdir().unwrap();
        let pool = jeeves_state::open(&home.path().join("state.db"))
            .await
            .unwrap();
        let facts = Facts::new(pool.clone());
        let chat = Chat::new(pool.clone());
        let episodes = Episodes::new(pool.clone());
        chat.append_exchange("Hello", "Hi", "chat", "test", None)
            .await
            .unwrap();
        let mock = Mock::new(vec![Mock::text(response)]);
        assert_eq!(
            consolidate_if_due(&chat, &facts, &episodes, &mock, "small", 1)
                .await
                .unwrap(),
            0
        );
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_log WHERE consolidated=0 AND consolidation_next_at IS NOT NULL").fetch_one(&pool).await.unwrap();
        assert_eq!(
            pending, 2,
            "Malformed extraction must return the whole claim with backoff"
        );
        assert_eq!(facts.count().await.unwrap(), 0);
    }
}

#[test]
fn the_starter_skills_are_seeded_once_and_all_parse() {
    let home = tempfile::tempdir().expect("temp dir");
    assert_eq!(seed_default_skills(home.path()), 2);
    let loader = SkillLoader::new(SkillLoader::dirs_for(home.path()));
    assert_eq!(loader.skills().len(), 2, "every starter skill must parse");
    let edited = home.path().join("skills/tidy-folder/SKILL.md");
    std::fs::write(
        &edited,
        "---\nname: tidy-folder\ndescription: mine\n---\nmine",
    )
    .unwrap();
    std::fs::remove_dir_all(home.path().join("skills/summarize-document")).unwrap();
    assert_eq!(seed_default_skills(home.path()), 0);
    assert!(std::fs::read_to_string(&edited).unwrap().contains("mine"));
    assert!(!home.path().join("skills/summarize-document").exists());
}
