//! research-quality — a real-model benchmark that separates the three failures the
//! November 14 study blurred into one `comprehensiveQualityPassed: false`:
//!
//!   * `citation`: are the required source passages quoted verbatim? Machine-checked,
//!     never the model's own claim.
//!   * `review`: can a separate review round be decoded into the expected JSON at all?
//!     A transport/decoding failure scores only this dimension, not the whole case.
//!   * `overall`: does an independent referee judge the brief correct and grounded?
//!
//! Uses a fresh, unseen source set in `evals/research/quality.jsonl`. Needs a key;
//! without one it skips honestly. Scores are recorded, never a release gate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use jeeves_provider::{CreateRequest, Message, Provider, Resolved};
use serde::Deserialize;

use crate::{judge_model, report, server_from_env};

#[derive(Debug, Clone, Deserialize)]
pub struct ResearchCase {
    pub id: String,
    #[serde(default)]
    pub description: String,
    pub task: String,
    pub sources: Vec<Source>,
    /// Source id -> verbatim snippets from that source; any one hit means the source was cited.
    #[serde(rename = "expect_citations", default)]
    pub expect_citations: BTreeMap<String, Vec<String>>,
    pub judge: CaseJudge,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaseJudge {
    pub criteria: String,
    #[serde(default)]
    pub min_scores: MinScores,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MinScores {
    #[serde(default)]
    pub citation: usize,
    #[serde(default)]
    pub review: usize,
    #[serde(default)]
    pub overall: i64,
}

#[derive(Debug, Default)]
struct Verdict {
    citation: Option<usize>,
    citation_total: usize,
    review_ok: bool,
    review_error: String,
    overall: Option<i64>,
    overall_reason: String,
    draft: String,
    review: String,
}

fn sources_block(case: &ResearchCase) -> String {
    case.sources
        .iter()
        .map(|s| format!("[{}] {}", s.id, s.text))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn ask(
    client: &dyn Provider,
    model: &str,
    system: &str,
    user: &str,
    max: i32,
) -> Result<String> {
    let response = client
        .create(CreateRequest {
            cache_prefix: None,
            model: model.to_string(),
            system: Some(system.to_string()),
            messages: vec![Message::user_text(user)],
            tools: Vec::new(),
            max_tokens: max,
        })
        .await
        .map_err(|e| anyhow::anyhow!("model call failed: {e}"))?;
    Ok(response.text())
}

fn citation_score(case: &ResearchCase, draft: &str) -> (usize, usize) {
    let total = case.expect_citations.len();
    let hits = case
        .expect_citations
        .values()
        .filter(|quotes| quotes.iter().any(|quote| draft.contains(quote.as_str())))
        .count();
    (hits, total)
}

fn parse_review(text: &str) -> Result<serde_json::Value> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        anyhow::bail!("review returned no JSON object");
    };
    if end < start {
        anyhow::bail!("review returned an inverted JSON span");
    }
    let value: serde_json::Value = serde_json::from_str(&text[start..=end])
        .map_err(|e| anyhow::anyhow!("review JSON broken: {e}"))?;
    anyhow::ensure!(
        value.get("verdict").is_some(),
        "review JSON has no verdict field"
    );
    Ok(value)
}

fn parse_score(text: &str) -> Result<(i64, String)> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        anyhow::bail!("judge returned no JSON object");
    };
    if end < start {
        anyhow::bail!("judge returned an inverted JSON span");
    }
    let value: serde_json::Value = serde_json::from_str(&text[start..=end])
        .map_err(|e| anyhow::anyhow!("judge JSON broken: {e}"))?;
    let score = value
        .get("score")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("judge JSON has no integer score"))?;
    anyhow::ensure!((0..=10).contains(&score), "judge score out of range");
    let reason = value
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok((score, reason))
}

async fn grade_one(
    player: &dyn Provider,
    player_model: &str,
    judge: &dyn Provider,
    judge_model: &str,
    case: &ResearchCase,
) -> Verdict {
    let mut verdict = Verdict::default();
    let sources = sources_block(case);

    let draft = match ask(
        player,
        player_model,
        "You are a careful research assistant. Answer using only the given sources. Quote required passages verbatim.",
        &format!("{}\n\nSOURCES:\n{}", case.task, sources),
        1800,
    )
    .await
    {
        Ok(text) => text,
        Err(e) => {
            verdict.review_error = format!("draft call failed: {e}");
            return verdict;
        }
    };
    verdict.draft = draft.clone();
    let (hits, total) = citation_score(case, &draft);
    verdict.citation = Some(hits);
    verdict.citation_total = total;

    let review_user = format!(
        "BRIEF:\n{draft}\n\nSOURCES:\n{sources}\n\nReview this brief for unsupported claims, wrong citations and missing requirements. Reply with ONLY a JSON object: {{\"verdict\": \"pass|revise\", \"issues\": [\"...\"]}}"
    );
    match ask(
        judge,
        judge_model,
        "You are a strict research reviewer. Reply with only the requested JSON object.",
        &review_user,
        700,
    )
    .await
    {
        Ok(text) => {
            verdict.review = text.clone();
            match parse_review(&text) {
                Ok(_) => verdict.review_ok = true,
                Err(e) => verdict.review_error = e.to_string(),
            }
        }
        Err(e) => verdict.review_error = format!("review call failed: {e}"),
    }

    let overall_user = format!(
        "TASK:\n{}\n\nSOURCES:\n{}\n\nBRIEF:\n{}\n\nCRITERIA:\n{}\n\nScore the brief 0-10. Reply with ONLY a JSON object: {{\"score\": <int>, \"reason\": \"<one sentence>\"}}",
        case.task, sources, draft, case.judge.criteria
    );
    match ask(
        judge,
        judge_model,
        "You are a strict, fair judge. Reply with only the requested JSON object.",
        &overall_user,
        500,
    )
    .await
    {
        Ok(text) => match parse_score(&text) {
            Ok((score, reason)) => {
                verdict.overall = Some(score);
                verdict.overall_reason = reason;
            }
            Err(e) => verdict.overall_reason = format!("overall decode failed: {e}"),
        },
        Err(e) => verdict.overall_reason = format!("overall call failed: {e}"),
    }
    verdict
}

fn passed(verdict: &Verdict, case: &ResearchCase) -> bool {
    let citation_ok = match verdict.citation {
        Some(hits) => hits >= case.judge.min_scores.citation,
        None => false,
    };
    let overall_ok = verdict
        .overall
        .map(|score| score >= case.judge.min_scores.overall)
        .unwrap_or(false);
    citation_ok && verdict.review_ok && overall_ok
}

/// Runs the research-quality benchmark. Exit 0 always (records, never blocks release).
pub async fn run_research(paths: &[PathBuf], report_home: &Path) -> Result<i32> {
    let cases = crate::jsonl::read_jsonl(paths, "research case", |_, _| Ok(()))?;
    if cases.is_empty() {
        anyhow::bail!("no research cases were loaded — check the path (evals/research/)");
    }
    let Some(server) = server_from_env(report_home).await? else {
        println!(
            "research-quality needs an API key (none configured); skipping — this does not block a release."
        );
        return Ok(0);
    };
    let resolved: Resolved = server
        .resolved()
        .expect("resolved was checked by server_from_env")
        .clone();
    let judge_model = judge_model(&resolved);

    println!(
        "research-quality: player {}, reviewer/judge {} ({} cases)",
        resolved.model,
        judge_model,
        cases.len()
    );

    let mut results = Vec::new();
    let mut all_passed = true;
    for case in &cases {
        let verdict = grade_one(
            resolved.client.as_ref(),
            &resolved.model,
            resolved.client.as_ref(),
            &judge_model,
            case,
        )
        .await;
        let ok = passed(&verdict, case);
        all_passed &= ok;
        println!(
            "  {}: citation {}/{} review {} overall {} -> {}",
            case.id,
            verdict.citation.unwrap_or(0),
            verdict.citation_total,
            if verdict.review_ok { "ok" } else { "FAIL" },
            verdict
                .overall
                .map(|n| n.to_string())
                .unwrap_or_else(|| "FAIL".into()),
            if ok { "pass" } else { "fail" }
        );
        if !verdict.review_error.is_empty() {
            println!("      review: {}", verdict.review_error);
        }
        results.push(serde_json::json!({
            "id": case.id,
            "description": case.description,
            "citation": {"hits": verdict.citation, "expected": verdict.citation_total},
            "review": {"decoded": verdict.review_ok, "error": verdict.review_error},
            "overall": {"score": verdict.overall, "reason": verdict.overall_reason},
            "passed": ok,
            "draft": verdict.draft,
            "review_reply": verdict.review,
        }));
    }
    report::write(
        report_home,
        "research",
        serde_json::json!({"all_passed": all_passed, "cases": results}),
    );
    println!(
        "research-quality: {}/{} cases passed (recorded only)",
        results.iter().filter(|r| r["passed"] == true).count(),
        results.len()
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case() -> ResearchCase {
        let mut expect = BTreeMap::new();
        expect.insert("S1".into(), vec!["exact phrase".into()]);
        expect.insert(
            "S2".into(),
            vec!["another phrase".into(), "second option".into()],
        );
        ResearchCase {
            id: "t".into(),
            description: String::new(),
            task: "t".into(),
            sources: vec![],
            expect_citations: expect,
            judge: CaseJudge {
                criteria: String::new(),
                min_scores: MinScores::default(),
            },
        }
    }

    #[test]
    fn citation_is_exact_substring_not_self_report() {
        let c = case();
        assert_eq!(citation_score(&c, "an exact phrase here"), (1, 2));
        assert_eq!(
            citation_score(&c, "exact phrase and a second option"),
            (2, 2)
        );
        assert_eq!(citation_score(&c, "a paraphrase of it"), (0, 2));
    }

    #[test]
    fn review_decode_failure_does_not_hide_citation_and_overall() {
        assert!(parse_review("no json here").is_err());
        assert!(parse_review("{\"verdict\": \"pass\"}").is_ok());
        assert!(parse_review("{\"issues\": []}").is_err());
    }

    #[test]
    fn overall_score_bounds_are_enforced() {
        for bad in ["-1", "11", "\"7\"", "null"] {
            assert!(parse_score(&format!(r#"{{"score":{bad},"reason":"x"}}"#)).is_err());
        }
        assert_eq!(
            parse_score(r#"{"score":7,"reason":"ok"}"#).unwrap(),
            (7, "ok".into())
        );
    }
}
