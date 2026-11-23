//! Map a misspelled tool name onto the registered one: `ReadFile`, `READ_FILE`, `read-file`,
//! `read file` and `read_file_tool` all mean `read_file`.
//!
//! Some providers also leak XML fragments into the name (`read_file" parameter="path`), so the
//! name is cut at the first quote or angle bracket.
//!
//! Only spelling conventions are forgiven. There is no edit-distance matching: `reed_file` stays
//! unknown, and a disabled `write_file` must never turn into `read_file`. A name that matches
//! more than one registered tool is not repaired either.

/// The registered name `requested` spells, when exactly one registered name matches.
pub fn resolve<'a>(requested: &str, registered: impl Iterator<Item = &'a str>) -> Option<String> {
    let wanted = canonical(requested)?;
    let mut found = None;
    for name in registered {
        if canonical(name).as_deref() == Some(wanted.as_str()) {
            if found.is_some() {
                return None;
            }
            found = Some(name.to_string());
        }
    }
    found
}

/// Lowercase, cut at a leaked quote or bracket, drop separators and one trailing `tool`.
fn canonical(name: &str) -> Option<String> {
    let cut = name
        .find(['"', '\'', '<', '>'])
        .map_or(name, |end| &name[..end]);
    let flat: String = cut
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect();
    let flat = match flat.strip_suffix("tool") {
        Some(stem) if !stem.is_empty() => stem.to_string(),
        _ => flat,
    };
    (!flat.is_empty()).then_some(flat)
}
