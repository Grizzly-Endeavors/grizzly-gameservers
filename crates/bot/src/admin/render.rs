use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use maud::{DOCTYPE, Markup, html};
use serde::Deserialize;

use super::auth::{AdminIdentity, Denial};

const STYLESHEET: &str = include_str!("panel.css");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Nav {
    Dashboard,
    Settings,
    Signups,
    Guilds,
    Instances,
    Events,
}

impl Nav {
    const ALL: &'static [Self] = &[
        Self::Dashboard,
        Self::Settings,
        Self::Signups,
        Self::Guilds,
        Self::Instances,
        Self::Events,
    ];

    fn path(self) -> &'static str {
        match self {
            Self::Dashboard => "/",
            Self::Settings => "/settings",
            Self::Signups => "/signups",
            Self::Guilds => "/guilds",
            Self::Instances => "/instances",
            Self::Events => "/events",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Overview",
            Self::Settings => "Settings",
            Self::Signups => "Signups",
            Self::Guilds => "Guilds",
            Self::Instances => "Servers",
            Self::Events => "Events",
        }
    }
}

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

pub(crate) fn tile(label: &str, figure: &str) -> Markup {
    html! {
        div .tile {
            div .label { (label) }
            div .figure { (figure) }
        }
    }
}

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

pub(crate) fn notice(tone: Tone, body: &Markup) -> Markup {
    let class = match tone {
        Tone::Bad => "notice bad",
        Tone::Ok => "notice ok",
        Tone::Neutral | Tone::Warn | Tone::Accent => "notice",
    };
    html! { div class=(class) { (body) } }
}

pub(crate) fn availability(enabled: bool) -> Markup {
    if enabled {
        badge("on", Tone::Ok)
    } else {
        badge("off", Tone::Neutral)
    }
}

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

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Flash {
    pub(crate) flash: Option<String>,
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

    pub(crate) fn none() -> Self {
        Self::default()
    }

    fn query(&self) -> String {
        let Some(message) = self.flash.as_deref() else {
            return String::new();
        };
        let failed = if self.err.is_some() { "&err=1" } else { "" };
        format!("flash={}{failed}", percent_encode(message))
    }

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

fn hex_digit(nibble: u8) -> char {
    char::from(match nibble {
        0..=9 => b'0' + nibble,
        _ => b'A' + (nibble - 10),
    })
}

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

pub(crate) fn html_response(markup: Markup) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        markup.into_string(),
    )
        .into_response()
}

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
