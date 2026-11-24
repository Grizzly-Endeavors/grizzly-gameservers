//! Code-fence-aware text chunking for Discord's per-message size cap. Ported from
//! the residuum Discord adapter: long replies are split into sendable pieces
//! without leaving a `` ``` `` code fence dangling across the boundary.

/// Discord's hard cap on a single message's content. Byte-budgeting against this
/// is conservative versus Discord's character cap (bytes ≥ chars), so a chunk
/// never overflows.
pub(crate) const DISCORD_MAX_CHARS: usize = 2000;

const FENCE_CLOSE: &str = "\n```";

pub(crate) fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    if text.is_empty() || max_chars == 0 {
        return Vec::new();
    }
    if text.len() <= max_chars {
        return vec![text.to_owned()];
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut pos = 0;
    let mut in_fence = false;
    let mut fence_header = String::new();

    while pos < text.len() {
        let remaining = text.split_at(pos).1;

        let prefix = if in_fence {
            format!("{fence_header}\n")
        } else {
            String::new()
        };

        let suffix_reserve: usize = if in_fence { FENCE_CLOSE.len() } else { 0 };

        let budget = max_chars
            .saturating_sub(prefix.len())
            .saturating_sub(suffix_reserve);

        if budget == 0 {
            break;
        }

        if remaining.len() <= budget {
            let mut chunk = prefix;
            chunk.push_str(remaining);
            update_fence_state(remaining, &mut in_fence, &mut fence_header);
            if in_fence {
                chunk.push_str(FENCE_CLOSE);
            }
            chunks.push(chunk);
            break;
        }

        let split_at = find_split_point(remaining, budget);

        let slice = remaining.split_at(split_at).0;

        update_fence_state(slice, &mut in_fence, &mut fence_header);

        let mut chunk = prefix;
        chunk.push_str(slice);

        if in_fence {
            chunk.push_str(FENCE_CLOSE);
        }

        chunks.push(chunk);

        pos += split_at;
        if text.get(pos..pos + 1) == Some("\n") {
            pos += 1;
        } else if text.get(pos..pos + 2) == Some("\r\n") {
            pos += 2;
        }
    }

    chunks
}

fn update_fence_state(text: &str, in_fence: &mut bool, fence_header: &mut String) {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if *in_fence {
                *in_fence = false;
                fence_header.clear();
            } else {
                *in_fence = true;
                trimmed.clone_into(fence_header);
            }
        }
    }
}

fn find_split_point(text: &str, max: usize) -> usize {
    if max >= text.len() {
        return text.len();
    }

    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    if end == 0 {
        let mut first = 1;
        while first < text.len() && !text.is_char_boundary(first) {
            first += 1;
        }
        return first;
    }

    let search_region = text.split_at(end).0;

    if let Some(pos) = search_region.rfind('\n')
        && pos > 0
    {
        return pos;
    }

    if let Some(pos) = search_region.rfind(char::is_whitespace)
        && pos > 0
    {
        return pos;
    }

    end
}

#[cfg(test)]
#[path = "tests/chunking.rs"]
mod tests;
