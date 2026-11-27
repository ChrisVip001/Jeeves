//! Loop guardrails: when the model is stuck in repeated or alternating tool calls, tell it **early**.
//!
//! Repeated calls can spend tokens without making progress. `fold_tool_activity`
//! guards **cross-turn** forgetting (history); this layer guards **within-turn** dead loops.
//!
//! Three criteria:
//!
//! 1. **Consecutive identical**: the same `(tool, args, result)` appearing 3 times in a row;
//! 2. **Alternating cycles**: batches like `A,B,A,B…` repeating on a period;
//! 3. **One tool failing in a row**: the same tool failing 3 times with *different* arguments.
//!    Criteria 1 and 2 cannot see this — every attempt differs — yet a model guessing paths or
//!    names against a wall burns its budget the same way. This one only warns: a failing test
//!    command is ordinary output, so it never counts toward stopping the turn.
//!
//! A successful poll (reading a running background job's output) is left out of criteria 1
//! and 2: waiting on a long build returns the same text each time, and that is not a loop.
//!
//! A successful file change (`write_file`, `edit`, `apply_patch`) clears the window, so an
//! edit followed by the same test run starts counting again.
//!
//! Criterion 2 must be judged separately, because every alternation **resets** criterion 1's
//! consecutive count — with consecutive detection alone, a model replaying the same 2–4 calls
//! each round could run the budget down without ever tripping the alarm.
//!
//! On a hit, two things happen: the model gets a message (still in time to change course), and
//! **byte-identical large results are replaced by a reference stub** (repeated long output only
//! burns context). Both act on the fed-back text only — `LoopResult.tool_calls` keeps the truth.

use std::collections::VecDeque;

use serde_json::Value;

/// Consecutive identical calls that count as "stuck". 3, not 2: one repeat may be a recheck.
const IDENTICAL_THRESHOLD: u32 = 3;

/// Consecutive failures of one tool (any arguments) before the model is told to change approach.
const FAILURE_THRESHOLD: u32 = 3;

/// Max period of an alternating cycle (`A,B` has period 2, `A,B,C` has 3).
const MAX_CYCLE_PERIOD: usize = 4;

/// Results shorter than this are not worth stubbing — the stub itself costs tens of chars.
const STUB_MIN_CHARS: usize = 512;

/// Args preview kept in the stub: with the original compacted away, the call stays identifiable.
const STUB_ARGS_PREVIEW_CHARS: usize = 120;

/// Why a streak of calls failed, decided from the error text alone. Three failures of one
/// tool are not one situation: a timeout wants another attempt, a rejected argument wants a
/// corrected call, and a missing permission wants the user. Telling the model to "take
/// another route" in every case sends it guessing against a wall it cannot move.
///
/// Classification is a deterministic read of the tool's own error text, never the model's
/// account of it. Anything unrecognized stays `Unclassified`, which keeps the existing
/// generic advice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// A timeout, a dropped connection, a 429 or a 5xx: the same call may yet work.
    Transient,
    /// The target does not exist. Another guessed name or path is not a new attempt.
    Missing,
    /// Permission, auth or an unapproved effect: the user has to open the door.
    Permission,
    /// The arguments were rejected on their own terms (schema, parse, format, encoding).
    Arguments,
    /// The call was refused because of state it did not expect (a conflict, a lock, a
    /// precondition): the environment has to be re-read before trying again.
    State,
    Unclassified,
}

impl FailureKind {
    /// Read the tool's error text. Ordered so that the narrower reading wins: a 403 is a
    /// permission problem even though "403" also reads as an HTTP status.
    fn of(output: &str) -> Self {
        let text = output.to_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
        if has(&[
            "permission denied",
            "not permitted",
            "unauthorized",
            "forbidden",
            "access denied",
            "403",
            "401",
            "requires approval",
            "not approved",
            "api key",
            "credential",
        ]) {
            return Self::Permission;
        }
        if has(&[
            "no such file",
            "not found",
            "does not exist",
            "unknown tool",
            "no tool named",
            "404",
            "nonexistent",
        ]) {
            return Self::Missing;
        }
        if has(&[
            "schema",
            "invalid argument",
            "invalid input",
            "invalid json",
            "parse error",
            "failed to parse",
            "missing field",
            "unknown field",
            "malformed",
            "expected",
        ]) {
            return Self::Arguments;
        }
        if has(&[
            "conflict",
            "409",
            "precondition",
            "already exists",
            "locked",
            "stale",
            "modified since",
            "out of date",
            "dirty",
        ]) {
            return Self::State;
        }
        if has(&[
            "timed out",
            "timeout",
            "connection reset",
            "connection refused",
            "temporarily unavailable",
            "rate limit",
            "too many requests",
            "429",
            "500",
            "502",
            "503",
            "504",
            "network",
            "unreachable",
        ]) {
            return Self::Transient;
        }
        Self::Unclassified
    }

    /// The move this failure actually calls for. One line, no menu of options.
    fn advice(self) -> &'static str {
        match self {
            Self::Transient => {
                "The errors read as transient (timeout, rate limit or a 5xx). One more attempt is reasonable; if it fails again, say so instead of continuing to retry."
            }
            Self::Missing => {
                "Every attempt says the target does not exist. Stop guessing names or paths: list or search the containing location once to find the real one, or tell the user it is missing."
            }
            Self::Permission => {
                "These are permission, credential or approval failures, which another attempt cannot fix. Tell the user exactly what access is needed."
            }
            Self::Arguments => {
                "The arguments themselves were rejected. Re-read the tool's schema and the error text, then send one corrected call — do not vary the values at random."
            }
            Self::State => {
                "The environment is not in the state the call assumed (a conflict, a lock or a stale precondition). Re-read the current state before calling again."
            }
            Self::Unclassified => {
                "Read the error text, check the assumption behind it (a path, a name, a permission) with one small read, or tell the user what blocks you."
            }
        }
    }
}

/// Fingerprint of one call. Store hashes, not text: this path runs every turn, keep it light.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    name: String,
    args: u64,
    result: u64,
}

/// The guard's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardVerdict {
    Ok,
    /// The same call (same tool, args, result) for the `times`-th consecutive time.
    Repeated {
        times: u32,
    },
    /// The same group of calls repeating on a `period`-step cycle.
    Cyclic {
        period: usize,
    },
    /// The same tool failed for the `times`-th consecutive time, with varying arguments.
    Failing {
        name: String,
        times: u32,
        kind: FailureKind,
    },
}

impl GuardVerdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, GuardVerdict::Ok)
    }

    /// Whether this verdict means the loop made no progress (repetition), as opposed to a
    /// warning that leaves the turn running.
    pub fn is_stall(&self) -> bool {
        matches!(
            self,
            GuardVerdict::Repeated { .. } | GuardVerdict::Cyclic { .. }
        )
    }
}

/// A within-turn loop detector. **Create a fresh one per turn** — cross-turn repetition is
/// handled by the `fold_tool_activity` machinery (folding tool activity into history), not here.
#[derive(Default)]
pub struct StallGuard {
    recent: VecDeque<Call>,
    failures: std::collections::HashMap<String, u32>,
    /// The failure kind seen across the current streak, per tool. Mixed causes fall back to
    /// `Unclassified` rather than advising for whichever error happened to come last.
    kinds: std::collections::HashMap<String, FailureKind>,
    hits: i32,
    last_note: Option<String>,
}

impl StallGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many times it has tripped (for `TurnMeta.guard_hits`).
    pub fn hits(&self) -> i32 {
        self.hits
    }

    /// What the last hit said (for `TurnMeta.guard_note`).
    pub fn last_note(&self) -> Option<String> {
        self.last_note.clone()
    }

    /// Observe one call, return the verdict. The caller decides what text to feed back.
    pub fn observe(
        &mut self,
        name: &str,
        args: &Value,
        output: &str,
        success: bool,
    ) -> GuardVerdict {
        let failures = self.failures.entry(name.to_string()).or_default();
        *failures = if success { 0 } else { *failures + 1 };
        let failures = *failures;
        let kind = match (success, FailureKind::of(output)) {
            (true, _) => {
                self.kinds.remove(name);
                FailureKind::Unclassified
            }
            (false, observed) => *self
                .kinds
                .entry(name.to_string())
                .and_modify(|known| {
                    if *known != observed {
                        *known = FailureKind::Unclassified;
                    }
                })
                .or_insert(observed),
        };
        if success && is_poll(name, args) {
            return GuardVerdict::Ok;
        }
        if success && FILE_CHANGES.contains(&name) {
            self.recent.clear();
        }
        let call = Call {
            name: name.to_string(),
            args: hash(&args.to_string()),
            result: hash(output),
        };
        self.recent.push_back(call.clone());
        while self.recent.len() > MAX_CYCLE_PERIOD * 2 {
            self.recent.pop_front();
        }

        let mut verdict = self.classify(&call);
        if verdict.is_ok() && failures >= FAILURE_THRESHOLD {
            verdict = GuardVerdict::Failing {
                name: name.to_string(),
                times: failures,
                kind,
            };
        }
        if !verdict.is_ok() {
            self.hits += 1;
            self.last_note = reminder(&verdict);
        }
        verdict
    }

    fn classify(&self, call: &Call) -> GuardVerdict {
        let identical = self
            .recent
            .iter()
            .rev()
            .take_while(|recent| *recent == call)
            .count() as u32;
        if identical >= IDENTICAL_THRESHOLD {
            return GuardVerdict::Repeated { times: identical };
        }

        for period in 2..=MAX_CYCLE_PERIOD {
            let len = self.recent.len();
            if len < period * 2 {
                continue;
            }
            let window: Vec<&Call> = self.recent.iter().skip(len - period).collect();
            if window.iter().all(|item| *item == window[0]) {
                continue;
            }
            let repeats = (0..period)
                .all(|i| self.recent[len - 2 * period + i] == self.recent[len - period + i]);
            if repeats {
                return GuardVerdict::Cyclic { period };
            }
        }
        GuardVerdict::Ok
    }
}

/// File tools whose success changes what the next call can see. After one lands, rerunning
/// the same test command is a new experiment even when its output has not changed yet, so
/// the repeat and cycle window starts over.
const FILE_CHANGES: [&str; 3] = ["write_file", "edit", "apply_patch"];

/// A call that is meant to be repeated unchanged: reading a background job's output while it
/// runs returns the same "still running" text until the job moves, and a wait is the right
/// move, not a loop. Covers `job` with `action=output`, `job_output`, and tools named like
/// pollers (`…_poll`, `…_get_result`, the MCP convention). Failures still count per tool.
fn is_poll(name: &str, args: &Value) -> bool {
    match name {
        "job_output" => true,
        "job" => args.get("action").and_then(Value::as_str) == Some("output"),
        _ => name.ends_with("_poll") || name.ends_with("_get_result"),
    }
}

/// A message appended for the model on a hit. Returns `None` when there is no hit.
pub fn reminder(verdict: &GuardVerdict) -> Option<String> {
    match verdict {
        GuardVerdict::Ok => None,
        GuardVerdict::Repeated { times } => Some(format!(
            "\n[guard] the same call (same arguments, same result) has already appeared {times} times.\n             Stop repeating it — take another route, or tell the user where you are stuck."
        )),
        GuardVerdict::Cyclic { period } => Some(format!(
            "\n[guard] you are repeating the same group of calls on a {period}-step cycle.\n             Stop repeating it — take another route, or tell the user where you are stuck."
        )),
        GuardVerdict::Failing { name, times, kind } => Some(format!(
            "\n[guard] {name} has now failed {times} times in a row with different arguments.\n             {}",
            kind.advice()
        )),
    }
}

/// Long enough result repeated for the 2nd time or more → worth stubbing.
fn should_stub(output: &str, times: u32) -> bool {
    times >= 2 && output.chars().count() >= STUB_MIN_CHARS
}

/// Reference stub: states which identical call this is, and keeps an args preview.
fn stub(name: &str, args: &Value, times: u32) -> String {
    let preview: String = args
        .to_string()
        .chars()
        .take(STUB_ARGS_PREVIEW_CHARS)
        .collect();
    format!(
        "[{name} called #{times} with identical arguments: the result is the same as last time, omitted here (args: {preview})]"
    )
}

/// Decides the "text fed back to the model": stub on a hit when warranted, and append the
/// reminder; return as-is when there is no hit.
pub fn for_model(output: &str, name: &str, args: &Value, verdict: &GuardVerdict) -> String {
    let Some(note) = reminder(verdict) else {
        return output.to_string();
    };
    let body = match verdict {
        GuardVerdict::Repeated { times } if should_stub(output, *times) => stub(name, args, *times),
        _ => output.to_string(),
    };
    format!("{body}{note}")
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The within-turn guard tally: hit count + the last note.
/// Goes into `LoopResult`, then `TurnMeta` (so trace and UI can see "this turn stalled").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardReport {
    pub hits: i32,
    pub note: Option<String>,
}

impl GuardReport {
    /// Settle up with the detector.
    pub fn close(guard: &StallGuard) -> Self {
        Self {
            hits: guard.hits(),
            note: guard.last_note(),
        }
    }
}
