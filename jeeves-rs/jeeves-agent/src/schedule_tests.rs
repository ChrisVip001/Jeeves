use super::*;

fn body(name: &str, cron: &str) -> JobBody {
    JobBody {
        name: name.to_string(),
        cron: cron.to_string(),
        prompt: "write a briefing".to_string(),
        delivery: String::new(),
        catch_up: None,
    }
}

#[test]
fn a_good_body_is_normalised() {
    let job = validate(&body(" digest ", "0  8 * * *"), &[]).expect("valid");
    assert_eq!(job.name, "digest");
    assert_eq!(job.cron, "0 8 * * *");
    assert_eq!(job.delivery, "outbox");
    assert!(job.catch_up);
}

#[test]
fn bad_input_is_refused_with_a_sentence() {
    assert!(validate(&body("", "0 8 * * *"), &[]).is_err());
    assert!(validate(&body("Has Caps", "0 8 * * *"), &[])
        .expect_err("caps")
        .contains("lowercase"));
    assert!(validate(&body("x", "99 8 * * *"), &[])
        .expect_err("cron")
        .contains("cron"));
    let mut empty_prompt = body("x", "0 8 * * *");
    empty_prompt.prompt = "  ".into();
    assert!(validate(&empty_prompt, &[]).is_err());
    let mut odd = body("x", "0 8 * * *");
    odd.delivery = "carrier-pigeon".into();
    assert!(validate(&odd, &[])
        .expect_err("delivery")
        .contains("outbox"));
}

#[test]
fn a_name_a_skill_owns_is_reserved() {
    let reserved = vec!["weekly".to_string()];
    assert!(validate(&body("weekly", "0 8 * * 1"), &reserved)
        .expect_err("taken")
        .contains("skill"));
    assert!(refuse_skill_owned("weekly", &reserved).is_err());
    assert!(refuse_skill_owned("other", &reserved).is_ok());
}
