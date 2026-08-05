//! The panel's shared markup: the page shell, the small pieces every page
//! reuses, and the two responses that aren't a page at all.
//!
//! Templates are `maud` macros rather than files in a template language, so a
//! page referring to a field that stopped existing fails the build instead of
//! at render time, and the whole panel is embedded in the binary — which the
//! pod's read-only root filesystem requires regardless.

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use maud::{DOCTYPE, Markup, html};
use serde::Deserialize;

use super::auth::{AdminIdentity, Denial};

/// The panel's stylesheet, served from the binary at `/panel.css`.
const STYLESHEET: &str = include_str!("panel.css");

/// Which nav entry is the current page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Nav {
    Dashboard,
    Settings,
    Guilds,
    Instances,
    Events,
}

impl Nav {
    const ALL: &'static [Self] = &[
        Self::Dashboard,
        Self::Settings,
        Self::Guilds,
        Self::Instances,
        Self::Events,
    ];

    fn path(self) -> &'static str {
        match self {
            Self::Dashboard => "/",
            Self::Settings => "/settings",
            Self::Guilds => "/guilds",
            Self::Instances => "/instances",
            Self::Events => "/events",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Overview",
            Self::Settings => "Settings",
            Self::Guilds => "Guilds",
            Self::Instances => "Servers",
            Self::Events => "Events",
        }
    }
}

/// Wrap a page body in the shell: title, nav, and who is signed in.
pub(crate) fn page(title: &str, active: Nav, identity: &AdminIdentity, body: &Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "grizzly gameservers — " (title) }
                link rel="stylesheet" href="/panel.css";
            }
            body {
                header .bar {
                    h1 { "grizzly gameservers" }
                    nav {
                        @for entry in Nav::ALL {
                            a href=(entry.path()) class=[(*entry == active).then_some("active")] {
                                (entry.label())
                            }
                        }
                    }
                    span .who { (identity.username) }
                }
                main { (body) }
            }
        }
    }
}

/// A labelled figure on the overview.
pub(crate) fn tile(label: &str, figure: &str) -> Markup {
    html! {
        div .tile {
            div .label { (label) }
            div .figure { (figure) }
        }
    }
}

/// Severity of a badge or notice, so the colour is chosen by meaning rather than
/// spelled at each call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Neutral,
    Ok,
    Warn,
    Bad,
    Accent,
}

impl Tone {
    fn class(self) -> &'static str {
        match self {
            Self::Neutral => "badge",
            Self::Ok => "badge ok",
            Self::Warn => "badge warn",
            Self::Bad => "badge bad",
            Self::Accent => "badge accent",
        }
    }
}

pub(crate) fn badge(text: &str, tone: Tone) -> Markup {
    html! { span class=(tone.class()) { (text) } }
}

/// A standing message above a page's content — an empty allowlist, a database
/// that isn't there, a setting recorded but not yet in force.
pub(crate) fn notice(tone: Tone, body: &Markup) -> Markup {
    let class = match tone {
        Tone::Bad => "notice bad",
        Tone::Ok => "notice ok",
        Tone::Neutral | Tone::Warn | Tone::Accent => "notice",
    };
    html! { div class=(class) { (body) } }
}

/// Whether a subsystem is wired, as the overview shows it. Every optional
/// subsystem degrades rather than fails, so "off" here is a configuration fact,
/// not an error.
pub(crate) fn availability(enabled: bool) -> Markup {
    if enabled {
        badge("on", Tone::Ok)
    } else {
        badge("off", Tone::Neutral)
    }
}

/// A table, or a line saying there's nothing to show. Saves every page from
/// rendering a header row over no rows.
pub(crate) fn table(headers: &[&str], rows: Vec<Markup>, empty: &str) -> Markup {
    html! {
        div .card {
            @if rows.is_empty() {
                div .empty { (empty) }
            } @else {
                div .scroll {
                    table {
                        thead { tr { @for head in headers { th { (head) } } } }
                        tbody { @for row in rows { (row) } }
                    }
                }
            }
        }
    }
}

/// The one-line result of the last action, carried across the redirect that
/// follows every mutation.
///
/// A query parameter rather than a session or a cookie: the panel has no
/// server-side session to hang one on, and a message that survives exactly one
/// navigation is all this needs. Nothing here is trusted — it is escaped into
/// the page like any other value.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Flash {
    pub(crate) flash: Option<String>,
    /// Present and `1` when the message is a failure.
    pub(crate) err: Option<u8>,
}

impl Flash {
    pub(crate) fn ok(message: &str) -> Self {
        Self {
            flash: Some(message.to_owned()),
            err: None,
        }
    }

    pub(crate) fn error(message: &str) -> Self {
        Self {
            flash: Some(message.to_owned()),
            err: Some(1),
        }
    }

    /// No message — used by a redirect whose destination speaks for itself.
    pub(crate) fn none() -> Self {
        Self::default()
    }

    /// This flash as query parameters, ready to append to a path.
    fn query(&self) -> String {
        let Some(message) = self.flash.as_deref() else {
            return String::new();
        };
        let failed = if self.err.is_some() { "&err=1" } else { "" };
        format!("flash={}{failed}", percent_encode(message))
    }

    /// The message as a notice, or nothing.
    pub(crate) fn render(&self) -> Markup {
        let Some(message) = self.flash.as_deref() else {
            return html! {};
        };
        let tone = if self.err.is_some() {
            Tone::Bad
        } else {
            Tone::Ok
        };
        notice(tone, &html! { (message) })
    }
}

/// Percent-encode a query-string value, encoding everything outside the
/// unreserved set rather than guessing at what needs it.
pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
    }
    out
}

/// One nibble as an uppercase hex digit.
fn hex_digit(nibble: u8) -> char {
    char::from(match nibble {
        0..=9 => b'0' + nibble,
        _ => b'A' + (nibble - 10),
    })
}

/// Answer a mutation with a redirect to a view.
///
/// Every mutation ends this way (POST → 303 → GET) so a reload can't repeat the
/// action and the back button lands on a page rather than a resubmission
/// prompt. `path` may already carry a query string.
pub(crate) fn redirect(path: &str, flash: &Flash) -> Response {
    let query = flash.query();
    let location = if query.is_empty() {
        path.to_owned()
    } else if path.contains('?') {
        format!("{path}&{query}")
    } else {
        format!("{path}?{query}")
    };
    (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response()
}

/// A rendered page.
pub(crate) fn html_response(markup: Markup) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        markup.into_string(),
    )
        .into_response()
}

/// The stylesheet. Static for the life of the process, so it is safe to cache
/// hard — a redeploy changes the binary, and the browser gets a new connection.
pub(crate) async fn stylesheet() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        STYLESHEET,
    )
        .into_response()
}

/// The 403 page. Deliberately a page rather than a bare status: whoever is
/// reading it has already signed in successfully, so the useful thing to tell
/// them is which group they're missing and who can add them to it.
pub(crate) fn denied(denial: &Denial) -> Response {
    let markup = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "grizzly gameservers — no access" }
                link rel="stylesheet" href="/panel.css";
            }
            body { (denial_body(denial)) }
        }
    };
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        markup.into_string(),
    )
        .into_response()
}

/// What the denial page actually says. Split from the response so the wording —
/// the part a locked-out person reads — is testable.
fn denial_body(denial: &Denial) -> Markup {
    match denial {
        Denial::Unidentified => html! {
            div .denied {
                h1 { "Not signed in" }
                p {
                    "This page didn't arrive with a sign-in attached. If you reached it \
                     through a link, go to the address directly and let it send you to \
                     the sign-in page."
                }
                p .hint {
                    "If you are already signed in and still seeing this, the panel is \
                     reachable by a route that skips authentication — worth telling Bear \
                     about."
                }
            }
        },
        Denial::NotAMember {
            username,
            required,
            held,
        } => html! {
            div .denied {
                h1 { "You're signed in, but not an admin" }
                p {
                    "This panel is limited to members of the "
                    span .mono { (required) }
                    " group. You're signed in as "
                    span .mono { (username) }
                    "."
                }
                @if held.is_empty() {
                    p .hint { "You're not in any groups yet." }
                } @else {
                    p .hint { "Your groups: " span .mono { (held.join(", ")) } }
                }
                p { "Ask Bear to add you if you should have access." }
            }
        },
    }
}

#[cfg(test)]
#[path = "tests/render.rs"]
mod tests;
