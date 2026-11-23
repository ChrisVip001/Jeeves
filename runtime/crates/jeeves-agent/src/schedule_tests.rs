use super::*;

fn body(name: &str, cron: &str) -> JobBody {
    JobBody {
        name: name.to_string(),
        cron: cron.to_string(),
        prompt: "write a briefing".to_string(),
        delivery: String::new(),
        catch_up: None,
        run_at: None,
    }
}

fn now() -> NaiveDateTime {
    parse_run_at("2026-11-24 10:00").expect("fixed clock")
}

fn once(at: &str) -> JobBody {
    JobBody {
        run_at: Some(at.to_string()),
        ..body("dentist", "")
    }
}

#[test]
fn a_good_body_is_normalised() {
    let job = validate(&body(" digest ", "0  8 * * *"), &[], now()).expect("valid");
    assert_eq!(job.name, "digest");
    assert_eq!(job.cron, "0 8 * * *");
    assert_eq!(job.delivery, "outbox");
    assert!(job.catch_up);
}

#[test]
fn bad_input_is_refused_with_a_sentence() {
    assert!(validate(&body("", "0 8 * * *"), &[], now()).is_err());
    assert!(
        validate(&body("Has Caps", "0 8 * * *"), &[], now())
            .expect_err("caps")
            .contains("lowercase")
    );
    assert!(
        validate(&body("x", "99 8 * * *"), &[], now())
            .expect_err("cron")
            .contains("cron")
    );
    let mut empty_prompt = body("x", "0 8 * * *");
    empty_prompt.prompt = "  ".into();
    assert!(validate(&empty_prompt, &[], now()).is_err());
    let mut odd = body("x", "0 8 * * *");
    odd.delivery = "carrier-pigeon".into();
    assert!(
        validate(&odd, &[], now())
            .expect_err("delivery")
            .contains("outbox")
    );
}

#[test]
fn a_name_a_skill_owns_is_reserved() {
    let reserved = vec!["weekly".to_string()];
    assert!(
        validate(&body("weekly", "0 8 * * 1"), &reserved, now())
            .expect_err("taken")
            .contains("skill")
    );
    assert!(refuse_skill_owned("weekly", &reserved).is_err());
    assert!(refuse_skill_owned("other", &reserved).is_ok());
}

#[test]
fn a_one_shot_job_pins_its_minute_in_the_cron() {
    let job = validate(&once("2026-11-25T15:00"), &[], now()).expect("valid");
    assert_eq!(job.run_at, parse_run_at("2026-11-25 15:00"));
    assert_eq!(job.cron, "0 15 25 11 *");
    let seconds = validate(&once("2026-11-25 15:00:42"), &[], now()).expect("seconds");
    assert_eq!(seconds.run_at, job.run_at);
}

#[test]
fn a_one_shot_time_is_refused_when_past_far_garbled_or_doubled() {
    let past = validate(&once("2026-11-24 09:59"), &[], now()).expect_err("past");
    assert!(past.contains("already passed"), "{past}");
    assert!(validate(&once("2026-11-24 10:00"), &[], now()).is_err());
    let far = validate(&once("2028-01-01 09:00"), &[], now()).expect_err("far");
    assert!(far.contains("more than a year"), "{far}");
    let garbled = validate(&once("tomorrow 3pm"), &[], now()).expect_err("garbled");
    assert!(garbled.contains("YYYY-MM-DD HH:MM"), "{garbled}");
    let both = JobBody {
        cron: "0 8 * * *".to_string(),
        ..once("2026-11-25 15:00")
    };
    assert!(
        validate(&both, &[], now())
            .expect_err("both")
            .contains("not both")
    );
}

#[test]
fn a_blank_run_at_means_a_repeating_job() {
    let job = validate(
        &JobBody {
            run_at: Some("  ".to_string()),
            ..body("digest", "0 8 * * *")
        },
        &[],
        now(),
    )
    .expect("valid");
    assert_eq!(job.run_at, None);
}
