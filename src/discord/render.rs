use crate::agones::ServerSummary;

const EMPTY_MESSAGE: &str = "No game servers are running right now.";
const NO_ADDRESS: &str = "(not exposed yet)";

/// Render a list of servers into a Discord message. Each server becomes one
/// bullet line with its state and connection address; an empty list yields a
/// friendly "nothing running" message.
pub(crate) fn format_server_list(servers: &[ServerSummary]) -> String {
    if servers.is_empty() {
        return EMPTY_MESSAGE.to_owned();
    }

    let lines: Vec<String> = servers
        .iter()
        .map(|server| {
            let address = server.address.as_deref().unwrap_or(NO_ADDRESS);
            format!("• **{}** — {} — `{}`", server.name, server.state, address)
        })
        .collect();
    lines.join("\n")
}

#[cfg(test)]
#[path = "tests/render.rs"]
mod tests;
