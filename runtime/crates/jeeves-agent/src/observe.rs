//! Build task steps and artifact observations from settled operation records.

use jeeves_state::{Artifacts, Events, NewStep, Steps};

/// Tools whose arguments name a file they claim to have written.
const WRITE_TOOLS: [&str; 2] = ["write_file", "edit"];

/// What the filesystem says about one claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    Wrote {
        path: String,
        bytes: u64,
        written_text: Option<String>,
    },
    Missing {
        path: String,
    },
    /// The tool said it succeeded and the fact disagrees — the one signal that must not be
    /// talked away.
    Contradiction {
        path: String,
        said: String,
    },
}

/// One tool call, ready to be written to the step ledger.
#[derive(Debug, Clone)]
pub struct StepDraft {
    pub seq: i32,
    pub tool: String,
    pub input_json: Option<String>,
    pub ok: bool,
    /// Short digest of the output — never the whole thing (context and disk both have limits).
    pub digest: String,
    pub idempotency_key: String,
}

/// Everything one round left behind.
#[derive(Debug, Clone, Default)]
pub struct Observation {
    pub steps: Vec<StepDraft>,
    pub facts: Vec<Fact>,
}

impl Observation {
    /// Files that really exist, with their sizes.
    pub fn artifacts(&self) -> Vec<(&str, u64, Option<&str>)> {
        self.facts
            .iter()
            .filter_map(|fact| match fact {
                Fact::Wrote {
                    path,
                    bytes,
                    written_text,
                } => Some((path.as_str(), *bytes, written_text.as_deref())),
                _ => None,
            })
            .collect()
    }

    /// "The tool said it wrote, the file is not there" — one sentence per case.
    pub fn contradictions(&self) -> Vec<String> {
        self.facts
            .iter()
            .filter_map(|fact| match fact {
                Fact::Contradiction { path, said } => Some(format!(
                    "the tool said it wrote {path}, but the file is not there (the tool said: {said})"
                )),
                _ => None,
            })
            .collect()
    }

    /// Whether the round contains a lie about a side effect. The turn reads
    /// `contradictions()` for the sentences; this is the cheap boolean the tests assert on.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has_contradiction(&self) -> bool {
        self.facts
            .iter()
            .any(|fact| matches!(fact, Fact::Contradiction { .. }))
    }
}

/// A settled tool call. Human reconciliation remains separate from an executor-confirmed success.
pub struct SettledCall {
    pub seq: i32,
    pub tool: String,
    pub input: serde_json::Value,
    pub success: bool,
    pub output: String,
}

pub fn observe(
    operations: &[SettledCall],
    turn_id: &str,
    cwd: Option<&std::path::Path>,
) -> Observation {
    let mut observation = Observation::default();
    for call in operations {
        let seq = call.seq;
        let input = &call.input;
        if WRITE_TOOLS.contains(&call.tool.as_str()) {
            if let Some(path) = input.get("path").and_then(|p| p.as_str()) {
                let written = if call.tool == "write_file" {
                    input["content"].as_str()
                } else {
                    None
                };
                observation.facts.push(check_written(
                    path,
                    call.success,
                    &call.output,
                    cwd,
                    written,
                ));
            }
        }
        observation.steps.push(StepDraft {
            seq,
            tool: call.tool.clone(),
            input_json: Some(input.to_string()),
            ok: call.success,
            digest: digest(&call.output),
            idempotency_key: format!("{turn_id}:{seq}"),
        });
    }
    observation
}

/// Does the file a write tool claims to have written actually exist?
fn check_written(
    path: &str,
    ok: bool,
    output: &str,
    cwd: Option<&std::path::Path>,
    text: Option<&str>,
) -> Fact {
    let full = match cwd {
        Some(root) if !std::path::Path::new(path).is_absolute() => root.join(path),
        _ => std::path::PathBuf::from(path),
    };
    match std::fs::metadata(&full) {
        Ok(meta) if ok && meta.is_file() => Fact::Wrote {
            path: full.display().to_string(),
            bytes: meta.len(),
            written_text: text
                .filter(|s| {
                    s.len() <= 64 * 1024
                        && s.len() as u64 == meta.len()
                        && std::fs::read(&full).is_ok_and(|bytes| bytes == s.as_bytes())
                })
                .map(str::to_owned),
        },
        _ if !ok => Fact::Missing {
            path: full.display().to_string(),
        },
        _ => Fact::Contradiction {
            path: full.display().to_string(),
            said: digest(output),
        },
    }
}

/// First line, whitespace-collapsed, capped — a digest, not the output.
fn digest(output: &str) -> String {
    let one_line = output.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped: String = one_line.chars().take(120).collect();
    if one_line.chars().count() > 120 {
        format!("{capped}…")
    } else {
        capped
    }
}

/// Persist required step/artifact evidence; optional event failures remain visible on stderr.
pub struct Recorder<'a> {
    pub steps: &'a Steps,
    pub artifacts: &'a Artifacts,
    pub events: &'a Events,
    pub task_id: &'a str,
    pub run_id: Option<&'a str>,
}

impl Recorder<'_> {
    pub async fn record(&self, observation: &Observation) -> Result<(), String> {
        for step in &observation.steps {
            let id = self
                .steps
                .begin(NewStep {
                    task_id: self.task_id,
                    run_id: self.run_id,
                    seq: step.seq,
                    kind: "tool",
                    tool: Some(&step.tool),
                    input_json: step.input_json.as_deref(),
                    idempotency_key: Some(&step.idempotency_key),
                })
                .await
                .map_err(|error| format!("Step observation could not be recorded: {error}"))?;
            let status = if step.ok { "ok" } else { "failed" };
            self.steps
                .close(id, status, Some(&step.digest), None)
                .await
                .map_err(|error| format!("Step result could not be recorded: {error}"))?;
        }
        for (path, bytes, text) in observation.artifacts() {
            self.artifacts
                .add_with_text(self.task_id, self.run_id, path, bytes as i64, text)
                .await
                .map_err(|error| format!("Artifact observation could not be recorded: {error}"))?;
        }
        for contradiction in observation.contradictions() {
            eprintln!(
                "(jeeves) task {} found a contradiction: {contradiction}",
                self.task_id
            );
            if let Err(e) = self
                .events
                .append(
                    self.task_id,
                    self.run_id,
                    "contradiction",
                    Some(&contradiction),
                )
                .await
            {
                eprintln!("(jeeves) the contradiction event was not recorded: {e}");
            }
        }
        Ok(())
    }
}
