pub(crate) const BEGIN_MARKER: &str = "-----BEGIN UNTRUSTED";

pub(crate) const END_MARKER: &str = "-----END UNTRUSTED";

const REDACTED_LINE: &str = "[line removed: it contained an untrusted-data marker]";

pub(crate) fn scrub(body: &str) -> String {
    let begin = BEGIN_MARKER.to_ascii_lowercase();
    let end = END_MARKER.to_ascii_lowercase();
    body.lines()
        .map(|line| {
            let lowered = line.to_ascii_lowercase();
            if lowered.contains(&begin) || lowered.contains(&end) {
                REDACTED_LINE
            } else {
                line
            }
        })
        .collect::<Vec<&str>>()
        .join("\n")
}

#[cfg(test)]
#[path = "tests/untrusted.rs"]
mod tests;
