//! Skill install/update tests: version comparison, validate-before-write, backup, atomic swap.

use super::install::{
    install, installed_version, needs_update, parse_index, update_all, IndexEntry, Outcome,
};
use crate::skills::{install_from_text, is_slug};

const GOOD: &str = "---\nname: weekly-review\ndescription: summarize the week\nversion: 1.1.0\n---\n1. pull episodes\n";

fn entry(name: &str, version: &str, url: &str) -> IndexEntry {
    IndexEntry {
        name: name.to_string(),
        version: version.to_string(),
        url: url.to_string(),
    }
}

#[test]
fn a_bad_entry_is_skipped_not_fatal() {
    let (entries, warnings) = parse_index(
        r#"{"skills":[
            {"name":"a","version":"1.0.0","url":"https://x/a.md"},
            {"name":"b","url":"https://x/b.md"},
            {"name":"c","version":"1.0.0"}
        ]}"#,
    )
    .expect("parse");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "a");
    assert_eq!(
        warnings.len(),
        2,
        "each bad entry has to be named: {warnings:?}"
    );

    assert!(parse_index("this is not JSON").is_err(), "this is not JSON");
    assert!(parse_index(r#"{"nope":[]}"#).is_err());
}

#[test]
fn version_comparison_never_downgrades() {
    assert!(
        needs_update(None, "1.0.0"),
        "never installed means install it"
    );
    assert!(needs_update(Some("1.0.0"), "1.1.0"));
    assert!(
        needs_update(Some("1.9.0"), "1.10.0"),
        "compare sizes segment by segment"
    );
    assert!(!needs_update(Some("1.1.0"), "1.1.0"));
    assert!(!needs_update(Some("2.0.0"), "1.9.0"), "no downgrade");
    assert!(!needs_update(Some("2024-05"), "2024-06"));
}

#[test]
fn every_writer_agrees_on_what_a_valid_name_is() {
    assert!(is_slug("weekly-review"));
    assert!(is_slug("v2"));
    assert!(!is_slug(""));
    assert!(
        !is_slug("-evil-"),
        "a leading or trailing hyphen is a bad directory name"
    );
    assert!(!is_slug("Weekly Review"));
    assert!(!is_slug("../evil"));

    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path();
    let text = "---\nname: -evil-\ndescription: x\n---\nb";
    assert!(
        install_from_text(home, text).is_err(),
        "the install path has to refuse"
    );
    assert!(
        install(home, "-evil-", text).is_err(),
        "the update path has to refuse too"
    );
}

#[test]
fn install_validates_before_touching_the_disk() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path();

    let wrong = install(
        home,
        "weekly-review",
        "---\nname: other\ndescription: x\n---\nb",
    );
    assert!(
        wrong.is_err(),
        "a name that does not match its content is refused"
    );
    assert!(
        install(home, "weekly-review", "no frontmatter").is_err(),
        "no frontmatter, no install"
    );
    assert!(install(home, "../evil", GOOD).is_err());
    assert!(!home.join("skills").join("evil").exists());
    assert!(installed_version(home, "weekly-review").is_none());

    let path = install(home, "weekly-review", GOOD).expect("install it");
    assert!(path.exists());
    assert_eq!(
        installed_version(home, "weekly-review").as_deref(),
        Some("1.1.0")
    );
}

#[test]
fn a_reinstall_backs_up_the_previous_version() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path();
    install(home, "weekly-review", GOOD).expect("the first version");

    let second = GOOD
        .replace("version: 1.1.0", "version: 1.2.0")
        .replace("pull episodes", "pull episodes and summarize");
    install(home, "weekly-review", &second).expect("the second version");

    assert_eq!(
        installed_version(home, "weekly-review").as_deref(),
        Some("1.2.0")
    );
    let backups = std::fs::read_dir(home.join("skills").join(".backup"))
        .expect("the backup directory")
        .count();
    assert_eq!(backups, 1, "the old one is backed up before the swap");
    let staged = std::fs::read_dir(home.join("skills").join(".staging"))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(staged, 0, "the staging directory should be empty again");
}

#[tokio::test]
async fn update_all_installs_skips_and_reports_failures() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path();
    install(home, "weekly-review", GOOD).expect("install one version first");

    let entries = vec![
        entry("weekly-review", "1.1.0", "same"),
        entry("roadmap", "2.0.0", "good"),
        entry("broken", "1.0.0", "bad-content"),
        entry("missing", "1.0.0", "unreachable"),
    ];
    let outcomes = update_all(home, &entries, |url: String| async move {
        match url.as_str() {
            "same" => Ok(GOOD.to_string()),
            "good" => Ok(
                "---\nname: roadmap\ndescription: plan quarters\nversion: 2.0.0\n---\nsteps"
                    .to_string(),
            ),
            "bad-content" => Ok("this is not a SKILL.md".to_string()),
            _ => Err("404".to_string()),
        }
    })
    .await;

    assert!(
        matches!(outcomes[0], Outcome::UpToDate { .. }),
        "{:?}",
        outcomes[0]
    );
    assert!(matches!(outcomes[1], Outcome::Installed { .. }));
    assert!(
        matches!(outcomes[2], Outcome::Failed { .. }),
        "bad content has to be reported"
    );
    assert!(
        matches!(outcomes[3], Outcome::Failed { .. }),
        "failing to fetch has to be reported"
    );

    assert!(installed_version(home, "broken").is_none());
    assert_eq!(installed_version(home, "roadmap").as_deref(), Some("2.0.0"));
    assert_eq!(outcomes.iter().filter(|o| o.ok()).count(), 2);
    assert!(
        outcomes.iter().all(|o| !o.line().is_empty()),
        "each one needs a sentence fit to show a human"
    );
}
