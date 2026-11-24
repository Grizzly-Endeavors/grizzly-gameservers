const TITLE_LIMIT: usize = 80;

const DESCRIPTION_LIMIT: usize = 200;

pub(crate) fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(limit).collect();
    cut.push('…');
    cut
}

pub(crate) fn short_title(title: &str) -> String {
    truncate(title, TITLE_LIMIT)
}

pub(crate) fn short_description(description: &str) -> String {
    let flattened = description.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&flattened, DESCRIPTION_LIMIT)
}

pub(crate) fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        return "none listed".to_owned();
    }
    items.join(", ")
}

#[cfg(test)]
#[path = "tests/text.rs"]
mod tests;
