//! Identifier grammar and case conversion.
//!
//! Ids are `PascalCase` matching `([A-Z][a-z0-9]*)+` — each segment a capital
//! letter followed by lowercase letters or digits, with no acronym runs
//! (`HttpProxy`, never `HTTPProxy`). That restriction is what makes case
//! conversion unambiguous in both directions, so the id can *be* the generated
//! Rust type name with no lookup table to drift.

/// Whether `id` is a valid `PascalCase` id: one or more segments, each a capital
/// letter followed by zero or more lowercase letters or digits.
pub(crate) fn is_valid_id(id: &str) -> bool {
    let mut chars = id.chars().peekable();
    let mut saw_segment = false;
    while let Some(&c) = chars.peek() {
        if !c.is_ascii_uppercase() {
            return false;
        }
        chars.next();
        saw_segment = true;
        while let Some(&d) = chars.peek() {
            if d.is_ascii_lowercase() || d.is_ascii_digit() {
                chars.next();
            } else {
                break;
            }
        }
    }
    saw_segment
}

/// Convert a valid `PascalCase` id to `snake_case`: lowercase each character,
/// inserting `_` before each segment boundary (each interior capital).
pub(crate) fn pascal_to_snake(id: &str) -> String {
    let mut out = String::with_capacity(id.len() + 4);
    for (idx, c) in id.char_indices() {
        if c.is_ascii_uppercase() && idx != 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Convert a `snake_case` string to `PascalCase`: capitalize the first letter of
/// each `_`-separated segment. Used for generated enum type/variant names.
pub(crate) fn snake_to_pascal(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for segment in name.split('_') {
        let mut chars = segment.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Whether `name` is a legal placeholder / parameter name: matches
/// `[a-z][a-z0-9_]*` and is not a Rust keyword (it becomes a struct field).
pub(crate) fn is_valid_placeholder_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return false;
    }
    !is_rust_keyword(name)
}

/// Whether an enum value matches its grammar `[a-z][a-z0-9_]*`. Same shape as a
/// placeholder name but keywords are fine (values become serde-renamed variants,
/// not identifiers).
pub(crate) fn is_valid_enum_value(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The strict and reserved Rust keywords that cannot be bare identifiers. Kept
/// as a flat set because placeholder names are always lowercase, so the
/// uppercase `Self` variant never applies.
fn is_rust_keyword(name: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true",
        "type", "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final",
        "gen", "macro", "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
    ];
    KEYWORDS.contains(&name)
}

#[cfg(test)]
#[path = "tests/ident.rs"]
mod tests;
