//! Repair tool arguments the model sent as strings: `"5"` for an integer, `"true"` for a
//! boolean, `"[\"a\"]"` for an array.
//!
//! Weaker models do this often, and each rejection costs a model round. The registry tries a
//! repair only after validation fails, and keeps the repair only when the result validates.
//! Each repair parses the string as JSON and keeps it only when the parse is exactly the
//! declared type, so `"abc"` for an integer, or `"1.5"` for an integer, stays as sent and the
//! schema error reaches the model unchanged.
//!
//! Only top-level properties with a single declared `type` are touched. Numbers are never
//! turned into strings and scalars are never wrapped into arrays: those guesses change meaning.

use serde_json::{Map, Value};

/// The repaired arguments and the keys that changed, or `None` when nothing could be repaired.
pub fn coerce(schema: &Value, args: &Value) -> Option<(Value, Vec<String>)> {
    let properties = schema.get("properties")?.as_object()?;
    let given = args.as_object()?;
    let mut repaired = Map::with_capacity(given.len());
    let mut keys = Vec::new();
    for (key, value) in given {
        let wanted = properties
            .get(key)
            .and_then(|property| property.get("type"))
            .and_then(Value::as_str);
        match (value, wanted.and_then(|wanted| parse_as(value, wanted))) {
            (Value::String(_), Some(parsed)) => {
                repaired.insert(key.clone(), parsed);
                keys.push(key.clone());
            }
            _ => {
                repaired.insert(key.clone(), value.clone());
            }
        }
    }
    (!keys.is_empty()).then_some((Value::Object(repaired), keys))
}

/// `value` parsed as JSON when it is a string and the parse has exactly the `wanted` type.
fn parse_as(value: &Value, wanted: &str) -> Option<Value> {
    let text = value.as_str()?.trim();
    let parsed: Value = serde_json::from_str(text).ok()?;
    let matches = match wanted {
        "integer" => parsed.is_i64() || parsed.is_u64(),
        "number" => parsed.is_number(),
        "boolean" => parsed.is_boolean(),
        "array" => parsed.is_array(),
        "object" => parsed.is_object(),
        _ => false,
    };
    matches.then_some(parsed)
}
