//! Bounding applied to registry text before it enters the model's context.
//! Shared by both registries so a Thunderstore description can't be longer than
//! a Modrinth one just because a different module rendered it.

/// Longest title kept. Titles are author-controlled third-party text, so they
/// are cut before entering the model's context.
const TITLE_LIMIT: usize = 80;

/// Longest description kept, same reasoning as [`TITLE_LIMIT`].
const DESCRIPTION_LIMIT: usize = 200;

/// Cut `text` to `limit` characters, marking it when something was dropped.
///
/// Counts characters rather than bytes: the input is arbitrary author-supplied
/// text and a byte cut would split a multi-byte character.
pub(crate) fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(limit).collect();
    cut.push('…');
    cut
}

/// A title, bounded.
pub(crate) fn short_title(title: &str) -> String {
    truncate(title, TITLE_LIMIT)
}

/// A description, bounded and flattened — newlines would break the one-line-per-
/// hit rendering the tools use.
pub(crate) fn short_description(description: &str) -> String {
    let flattened = description.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&flattened, DESCRIPTION_LIMIT)
}

/// Join a list for display, or name it empty so a blank never reads as an
/// omission the model should fill in.
pub(crate) fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        return "none listed".to_owned();
    }
    items.join(", ")
}

#[cfg(test)]
#[path = "tests/text.rs"]
mod tests;
