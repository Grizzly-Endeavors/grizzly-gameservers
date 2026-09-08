//! The IO shell: build a request, check the status, hand the body to
//! [`types`](super::types) and the narrowing to [`query`](super::query). Holds
//! no client of its own — it borrows the bot's shared one and overrides the
//! timeout per request, the same way the Modrinth client does.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode, header::USER_AGENT};

use super::query::{ORDERING, PAGE_SIZE, SearchQuery, narrow};
use super::types::{Listing, ListingResponse, Package};

/// Production Thunderstore. Two API surfaces live under it and both are used
/// here; see the module docs for why.
const API_BASE: &str = "https://thunderstore.io/api";

/// Per-request ceiling. Sized for public-internet latency rather than the shared
/// client's short default, which is tuned for in-cluster supervisor hops.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// An identifying User-Agent. Thunderstore publishes no rate limit, so naming
/// this project is what lets them contact us rather than block us if these calls
/// ever become a problem. Built from crate metadata so it carries no personal
/// identifier into a public repo.
const AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// Search one community, most-downloaded first, narrowed by [`narrow`].
///
/// # Errors
///
/// Returns an error if the API is unreachable, answers with a non-success
/// status, or returns a body this crate can't parse.
pub(crate) async fn search_packages(http: &Client, query: &SearchQuery) -> Result<Vec<Listing>> {
    let url = format!("{API_BASE}/cyberstorm/listing/{}/", query.community);
    let page_size = PAGE_SIZE.to_string();
    let params: Vec<(&str, &str)> = vec![
        ("q", query.text.as_str()),
        ("ordering", ORDERING),
        ("page_size", &page_size),
    ];

    let response = http
        .get(&url)
        .header(USER_AGENT, AGENT)
        .timeout(REQUEST_TIMEOUT)
        .query(&params)
        .send()
        .await
        .with_context(|| format!("failed to reach thunderstore search at {url}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("thunderstore search returned {status}: {body}");
    }

    let parsed: ListingResponse = response
        .json()
        .await
        .with_context(|| format!("failed to parse thunderstore search reply from {url}"))?;
    Ok(narrow(parsed.results, query.side))
}

/// Look one package up by `Namespace` and `Name`. `Ok(None)` means the registry
/// has no such package — an ordinary answer for a name the model guessed, kept
/// out of the error channel so the caller can tell it apart from the API being
/// unreachable.
///
/// # Errors
///
/// Returns an error if the API is unreachable, answers with a non-success status
/// other than 404, or returns a body this crate can't parse.
pub(crate) async fn fetch_package(
    http: &Client,
    namespace: &str,
    name: &str,
) -> Result<Option<Package>> {
    let url = format!("{API_BASE}/experimental/package/{namespace}/{name}/");

    let response = http
        .get(&url)
        .header(USER_AGENT, AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .with_context(|| format!("failed to reach thunderstore package lookup at {url}"))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("thunderstore package lookup returned {status}: {body}");
    }

    let parsed: Package = response
        .json()
        .await
        .with_context(|| format!("failed to parse thunderstore package reply from {url}"))?;
    Ok(Some(parsed))
}
