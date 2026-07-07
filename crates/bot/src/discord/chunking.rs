//! Code-fence-aware text chunking for Discord's per-message size cap. Ported from
//! the residuum Discord adapter: long replies are split into sendable pieces
//! without leaving a `` ``` `` code fence dangling across the boundary.

/// Discord's hard cap on a single message's content. Byte-budgeting against this
/// is conservative versus Discord's character cap (bytes ≥ chars), so a chunk
/// never overflows.
pub(crate) const DISCORD_MAX_CHARS: usize = 2000;

/// Split text into chunks that fit within `max_chars`, preserving code fence
/// integrity.
///
/// - Prefers splitting at newline or whitespace boundaries over mid-word
/// - If a `` ``` `` code fence spans a chunk boundary, the fence is closed at the
///   end of the current chunk and reopened at the start of the next
/// - Returns a single-element vec if the text fits within the limit
/// - Returns an empty vec for empty input
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

        // Build prefix for this chunk (reopen fence if needed)
        let prefix = if in_fence {
            format!("{fence_header}\n")
        } else {
            String::new()
        };

        // Suffix we may need to append (close fence if still open at end of chunk)
        let suffix_reserve: usize = if in_fence { 4 } else { 0 }; // "\n```"

        let budget = max_chars
            .saturating_sub(prefix.len())
            .saturating_sub(suffix_reserve);

        if budget == 0 {
            break;
        }

        if remaining.len() <= budget {
            // Last chunk — fits entirely
            let mut chunk = prefix;
            chunk.push_str(remaining);
            update_fence_state(remaining, &mut in_fence, &mut fence_header);
            if in_fence {
                chunk.push_str("\n```");
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
            chunk.push_str("\n```");
        }

        chunks.push(chunk);

        // Advance past the split, skipping a leading newline
        pos += split_at;
        if text.get(pos..pos + 1) == Some("\n") {
            pos += 1;
        } else if text.get(pos..pos + 2) == Some("\r\n") {
            pos += 2;
        }
    }

    chunks
}

/// Update fence tracking state by scanning lines in `text`.
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

/// Find the best byte offset to split at, preferring newline > whitespace > hard cut.
fn find_split_point(text: &str, max: usize) -> usize {
    if max >= text.len() {
        return text.len();
    }

    // Clamp to a char boundary
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    if end == 0 {
        // Find first char boundary > 0
        let mut first = 1;
        while first < text.len() && !text.is_char_boundary(first) {
            first += 1;
        }
        return first;
    }

    let search_region = text.split_at(end).0;

    // Prefer splitting at a newline
    if let Some(pos) = search_region.rfind('\n')
        && pos > 0
    {
        return pos;
    }

    // Fall back to whitespace
    if let Some(pos) = search_region.rfind(char::is_whitespace)
        && pos > 0
    {
        return pos;
    }

    // Hard cut at char boundary
    end
}

#[cfg(test)]
#[path = "tests/chunking.rs"]
mod tests;
