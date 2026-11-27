//! What a tool actually accepts, learned from its own answers.
//!
//! A JSON Schema says a field is a string. It does not say the string must be `2026-11-27`
//! and not `11/27/2026`, that an id is a uuid and not a number, or that a list wants
//! `a,b` and not `a, b`. The model cannot see those constraints, so it discovers them by
//! failing — and on a later call, in a later turn, discovers them again.
//!
//! This layer sits between the registry's schema check and the tool handler. It watches
//! which argument **shapes** a tool accepted and which it rejected, and hands the next
//! rejected call one line of evidence: this tool took `date` as `YYYY-MM-DD`, and refused
//! it as `MM/DD/YYYY`. The model then corrects the format instead of guessing again.
//!
//! Three rules keep this honest:
//!
//! 1. **Shapes, never values.** A recorded shape is a coarse pattern (`YYYY-MM-DD`,
//!    `<digits>`, `<uuid>`) derived from the string's character classes. Argument values
//!    may hold a path, an account number or a person's name, so nothing that could carry
//!    one is stored or shown. A string that fits no known pattern records nothing.
//! 2. **Observed, never inferred.** A shape is "accepted" only after a call carrying it
//!    succeeded, and "rejected" only after one failed on its arguments. A schema violation
//!    the registry caught is not evidence about the tool: it never reached it.
//! 3. **A hint, never a gate.** Calibration only appends a sentence to a failure the tool
//!    already returned. It cannot block a call, rewrite arguments, or turn a success into
//!    a failure.
//!
//! Per process, bounded, and lost on restart: this is a cache that saves a round trip,
//! not a store of record.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{LazyLock, Mutex};

use serde_json::Value;

/// Tools tracked before the table starts over. Dropping it only loses hints.
const MAX_TOOLS: usize = 256;

/// Fields tracked per tool, and shapes per field. A tool with more fields than this keeps
/// the ones seen first; a field with more shapes stops recording new ones.
const MAX_FIELDS: usize = 16;
const MAX_SHAPES: usize = 8;

/// Strings longer than this are prose or a document body, not a formatted scalar.
const MAX_SHAPED_CHARS: usize = 64;

/// What one field accepted and what it refused.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Field {
    accepted: BTreeSet<&'static str>,
    rejected: BTreeSet<&'static str>,
}

type Table = BTreeMap<String, BTreeMap<String, Field>>;

static LEARNED: LazyLock<Mutex<Table>> = LazyLock::new(|| Mutex::new(Table::new()));

/// The coarse pattern of a scalar argument, or `None` when it carries no recognizable
/// shape. Deliberately lossy: every arm describes a format, never its content.
fn shape(value: &Value) -> Option<&'static str> {
    let text = match value {
        Value::String(text) => text.as_str(),
        Value::Bool(_) => return Some("<bool>"),
        Value::Number(number) if number.is_i64() || number.is_u64() => return Some("<integer>"),
        Value::Number(_) => return Some("<number>"),
        _ => return None,
    };
    if text.is_empty() || text.chars().count() > MAX_SHAPED_CHARS {
        return None;
    }
    let digits =
        |part: &str, len: usize| part.len() == len && part.bytes().all(|b| b.is_ascii_digit());
    let hex =
        |part: &str, len: usize| part.len() == len && part.bytes().all(|b| b.is_ascii_hexdigit());
    let dashed: Vec<&str> = text.split('-').collect();
    if dashed.len() == 3 && digits(dashed[0], 4) && digits(dashed[1], 2) && digits(dashed[2], 2) {
        return Some("YYYY-MM-DD");
    }
    let slashed: Vec<&str> = text.split('/').collect();
    if slashed.len() == 3
        && slashed
            .iter()
            .all(|part| digits(part, 2) || digits(part, 4))
        && slashed.iter().any(|part| part.len() == 4)
    {
        return Some("date with / separators");
    }
    if dashed.len() == 5
        && hex(dashed[0], 8)
        && hex(dashed[1], 4)
        && hex(dashed[2], 4)
        && hex(dashed[3], 4)
        && hex(dashed[4], 12)
    {
        return Some("<uuid>");
    }
    if text.len() >= 20
        && text.ends_with('Z')
        && text.contains('T')
        && text.starts_with(|c: char| c.is_ascii_digit())
    {
        return Some("RFC 3339 timestamp");
    }
    if text.bytes().all(|b| b.is_ascii_digit()) {
        return Some("<digits>");
    }
    if text.contains(", ") {
        return Some("comma-space separated list");
    }
    if text.contains(',') {
        return Some("comma separated list");
    }
    None
}

/// The shaped top-level fields of one call. Nested objects are not walked: a hint is only
/// useful when the model can see which argument to change.
fn shaped(args: &Value) -> Vec<(String, &'static str)> {
    args.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(field, value)| Some((field.clone(), shape(value)?)))
        .take(MAX_FIELDS)
        .collect()
}

/// Whether a failure is the tool rejecting these arguments, as opposed to the work itself
/// going wrong. Only the former says anything about the accepted format.
fn about_arguments(error: &str) -> bool {
    let text = error.to_lowercase();
    [
        "invalid",
        "malformed",
        "not a valid",
        "could not parse",
        "cannot parse",
        "failed to parse",
        "unparseable",
        "expected format",
        "bad format",
        "wrong format",
        "unsupported format",
        "unrecognized",
        "must be",
        "must match",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// Record one finished call. `success` is the tool's own verdict; `error` is its message.
pub(crate) fn observe(name: &str, args: &Value, success: bool, error: &str) {
    if !success && !about_arguments(error) {
        return;
    }
    let shapes = shaped(args);
    if shapes.is_empty() {
        return;
    }
    let Ok(mut table) = LEARNED.lock() else {
        return;
    };
    if table.len() >= MAX_TOOLS && !table.contains_key(name) {
        return;
    }
    let fields = table.entry(name.to_owned()).or_default();
    for (field, shape) in shapes {
        if fields.len() >= MAX_FIELDS && !fields.contains_key(&field) {
            continue;
        }
        let entry = fields.entry(field).or_default();
        let (add, drop) = if success {
            (&mut entry.accepted, &mut entry.rejected)
        } else {
            (&mut entry.rejected, &mut entry.accepted)
        };
        if add.len() < MAX_SHAPES {
            add.insert(shape);
        }
        // The newer verdict wins: a format that works now is no longer a rejected one.
        drop.remove(shape);
    }
}

/// One line of evidence for a call that just failed on its arguments, or `None` when
/// nothing was learned that contradicts it.
pub(crate) fn hint(name: &str, args: &Value) -> Option<String> {
    let table = LEARNED.lock().ok()?;
    let fields = table.get(name)?;
    let mut notes = Vec::new();
    for (field, shape) in shaped(args) {
        let Some(entry) = fields.get(&field) else {
            continue;
        };
        // Only speak where this call's own shape is one this tool refused before.
        if !entry.rejected.contains(shape) || entry.accepted.is_empty() {
            continue;
        }
        let accepted = entry
            .accepted
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .join(" or ");
        notes.push(format!("{field} as {accepted}, not {shape}"));
    }
    (!notes.is_empty()).then(|| {
        format!(
            "Earlier calls to {name} succeeded with {}. Send the accepted format rather than another variation.",
            notes.join("; ")
        )
    })
}

/// `LEARNED` is process-wide, so a test that clears it would wipe a concurrent test's
/// records. Every recording test takes this lock and clears on entry, which serializes
/// them without making any one of them depend on the order.
#[cfg(test)]
pub(crate) fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut table) = LEARNED.lock() {
        table.clear();
    }
    guard
}

#[cfg(test)]
mod tests;
