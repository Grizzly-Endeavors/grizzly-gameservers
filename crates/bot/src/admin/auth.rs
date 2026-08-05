//! Who the panel lets in.
//!
//! Authentication happens before the request reaches this process: ingress-nginx
//! sends every request to the Authentik outpost first, and only forwards it here
//! once the outpost has established who is asking, stamping the answer into
//! `X-authentik-*` headers. That mechanism *overwrites* whatever the client sent,
//! so a browser can't forge its way in through the front door.
//!
//! What it cannot do is stop something already inside the cluster from talking to
//! the panel's Service directly with headers of its own choosing. That is the
//! `CiliumNetworkPolicy`'s job — the panel's port admits ingress-nginx only — and
//! it, not this module, is the boundary. The check here is defense in depth, and
//! it buys one thing the outpost's own policy binding doesn't: a signed-in
//! non-member gets a page telling them which group they're missing, instead of a
//! bare 403 from a proxy they've never heard of.

use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use tracing::warn;

/// Set by the Authentik proxy outpost on an authenticated request. Listed in the
/// ingress' `auth-response-headers`, which is what copies them onto the request
/// forwarded here.
const USERNAME_HEADER: &str = "x-authentik-username";
const GROUPS_HEADER: &str = "x-authentik-groups";
const EMAIL_HEADER: &str = "x-authentik-email";

/// Who the outpost says is making this request. Placed in the request extensions
/// by [`require_admin`], so a handler that reads it can only be reached by
/// someone who passed the check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdminIdentity {
    pub(crate) username: String,
    pub(crate) email: Option<String>,
}

/// Why a request didn't get in. Distinguished because they call for different
/// pages: one is a misconfiguration to fix, the other is a person to explain
/// themselves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Denial {
    /// No identity headers at all. Either the request bypassed the outpost — the
    /// case the network policy exists to prevent — or the ingress is missing its
    /// `auth-response-headers` annotation.
    Unidentified,
    /// A real signed-in user who isn't in the required group.
    NotAMember {
        username: String,
        required: String,
        held: Vec<String>,
    },
}

/// Split Authentik's group header. The proxy outpost joins group names with `|`;
/// a comma is accepted too, so a change in that formatting degrades to "some
/// groups parse oddly" rather than "nobody can log in".
fn groups(raw: &str) -> impl Iterator<Item = &str> {
    raw.split(['|', ','])
        .map(str::trim)
        .filter(|group| !group.is_empty())
}

/// Read one header as a trimmed, non-empty string.
fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Establish who is asking, or say why they don't get in.
///
/// # Errors
///
/// Returns [`Denial::Unidentified`] when the outpost's headers are absent, and
/// [`Denial::NotAMember`] when they name someone outside `required_group`.
pub(crate) fn identify(headers: &HeaderMap, required_group: &str) -> Result<AdminIdentity, Denial> {
    let Some(username) = header(headers, USERNAME_HEADER) else {
        return Err(Denial::Unidentified);
    };
    let raw_groups = header(headers, GROUPS_HEADER).unwrap_or_default();
    if !groups(&raw_groups).any(|group| group == required_group) {
        return Err(Denial::NotAMember {
            username,
            required: required_group.to_owned(),
            held: groups(&raw_groups).map(str::to_owned).collect(),
        });
    }
    Ok(AdminIdentity {
        username,
        email: header(headers, EMAIL_HEADER),
    })
}

/// The group this panel admits, as the middleware's state.
#[derive(Clone, Debug)]
pub(crate) struct AdminGate {
    pub(crate) required_group: String,
}

/// Reject anyone the outpost hasn't vouched for, and put the identity of anyone
/// it has into the request extensions.
pub(crate) async fn require_admin(
    State(gate): State<AdminGate>,
    mut request: Request,
    next: Next,
) -> Response {
    match identify(request.headers(), &gate.required_group) {
        Ok(identity) => {
            request.extensions_mut().insert(identity);
            next.run(request).await
        }
        Err(denial) => {
            match &denial {
                Denial::Unidentified => warn!(
                    path = %request.uri().path(),
                    "admin panel request carried no identity; the outpost or the network policy is misconfigured"
                ),
                Denial::NotAMember {
                    username, required, ..
                } => warn!(
                    user = %username,
                    required_group = %required,
                    path = %request.uri().path(),
                    "refused an admin panel request from a non-member"
                ),
            }
            super::render::denied(&denial)
        }
    }
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
