//! The IO shell: build a request, check the status, hand the body to
//! [`types`](super::types). Holds no client of its own — it borrows the bot's
//! shared one and overrides the timeout per request, the same way the
//! chat-completions client does.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode, header::USER_AGENT};

use super::query::{SEARCH_LIMIT, SearchQuery};
use super::types::{Project, SearchHit, SearchResponse};

/// Production Modrinth API v2. v2 is the current stable version; a future
/// deprecation surfaces as `410 Gone`, which the status check below reports
/// verbatim rather than swallowing.
const API_BASE: &str = "https://api.modrinth.com/v2";

/// Per-request ceiling. Sized for public-internet latency rather than the shared
/// client's short default, which is tuned for in-cluster supervisor hops.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Modrinth's usage policy requires an identifying User-Agent. Built from the
/// crate metadata so it names this project without carrying any personal
/// identifier into a public repo.
const AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// Search the registry, newest-relevance first, bounded to [`SEARCH_LIMIT`].
pub(crate) async fn search_projects(http: &Client, query: &SearchQuery) -> Result<Vec<SearchHit>> {
    let url = format!("{API_BASE}/search");
    let limit = SEARCH_LIMIT.to_string();
    let mut params: Vec<(&str, &str)> = vec![("query", query.text.as_str()), ("limit", &limit)];
    let facets = query.facets();
    if let Some(facets) = facets.as_deref() {
        params.push(("facets", facets));
    }

    let response = http
        .get(&url)
        .header(USER_AGENT, AGENT)
        .timeout(REQUEST_TIMEOUT)
        .query(&params)
        .send()
        .await
        .with_context(|| format!("failed to reach modrinth search at {url}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("modrinth search returned {status}: {body}");
    }

    let parsed: SearchResponse = response
        .json()
        .await
        .with_context(|| format!("failed to parse modrinth search reply from {url}"))?;
    Ok(parsed.hits)
}

/// Look one project up by slug. `Ok(None)` means the registry has no such slug —
/// an ordinary answer for a name the model guessed, kept out of the error
/// channel so the caller can tell it apart from the API being unreachable.
pub(crate) async fn fetch_project(http: &Client, slug: &str) -> Result<Option<Project>> {
    let url = format!("{API_BASE}/project/{slug}");

    let response = http
        .get(&url)
        .header(USER_AGENT, AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .with_context(|| format!("failed to reach modrinth project lookup at {url}"))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("modrinth project lookup returned {status}: {body}");
    }

    let parsed: Project = response
        .json()
        .await
        .with_context(|| format!("failed to parse modrinth project reply from {url}"))?;
    Ok(Some(parsed))
}
