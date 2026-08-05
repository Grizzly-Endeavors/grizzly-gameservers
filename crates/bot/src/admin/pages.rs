//! The panel's read-only views.
//!
//! Each handler gathers what it needs and hands it to `maud`; there is no
//! caching layer, because the whole point of the page is to show what is true
//! *now* and the underlying reads are a snapshot load, an in-memory map, or one
//! Kubernetes list.
//!
//! Where a read can fail, the page degrades to showing less rather than
//! returning an error: an unreachable Kubernetes API should cost the servers
//! table, not the settings page it sits beside.

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::{Extension, Query, State};
use axum::response::Response;
use axum::routing::get;
use maud::{Markup, html};
use serde::Deserialize;
use tracing::error;

use crate::agones::{guild_usage, list_live_instances};
use crate::events::EventFilter;
use crate::limits::format_cores;
use crate::settings::{Reload, Section, SettingKey, SettingsSnapshot, StoredOverride};

use super::AdminDeps;
use super::auth::AdminIdentity;
use super::render::{Nav, Tone, availability, badge, html_response, notice, page, table, tile};

/// Rows shown on one page of the event log. Generous — this is a scan-and-close
/// view, and paging is cheap — but bounded so one query can't hold the pool.
const EVENT_PAGE_SIZE: i64 = 100;

pub(crate) fn router(deps: AdminDeps) -> Router {
    Router::new()
        .route("/", get(dashboard))
        .route("/settings", get(settings))
        .route("/guilds", get(guilds))
        .route("/instances", get(instances))
        .route("/events", get(events))
        .with_state(deps)
}

/// Human-readable byte size for display. Not round-trippable, and deliberately
/// not used anywhere a value is edited — see `crate::settings` for that.
fn human_bytes(bytes: i64) -> String {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "GiB"), (1 << 20, "MiB"), (1 << 10, "KiB")];
    let bytes = bytes.unsigned_abs();
    for (scale, suffix) in UNITS {
        if bytes >= scale {
            let whole = bytes / scale;
            let tenths = (bytes % scale) * 10 / scale;
            return if tenths == 0 {
                format!("{whole} {suffix}")
            } else {
                format!("{whole}.{tenths} {suffix}")
            };
        }
    }
    format!("{bytes} B")
}

// ---------------------------------------------------------------- overview

async fn dashboard(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
) -> Response {
    let data = &deps.data;
    let effective = data.settings.snapshot();
    let booted = data.settings.booted();
    let overrides = data.settings.overrides().await;

    let live = list_live_instances(&data.kube_client, &data.namespace)
        .await
        .unwrap_or_else(|err| {
            error!(error = ?err, "admin panel could not list servers for the overview");
            Vec::new()
        });
    let running = live.iter().filter(|instance| instance.running).count();
    let guilds = data
        .gateway
        .current()
        .map_or(0, |ctx| ctx.cache.guilds().len());

    let body = html! {
        (pending_restart_notice(&effective, &booted, &overrides))
        @if !data.settings.is_available() {
            (notice(Tone::Bad, &html! {
                strong { "Settings are read-only." }
                " There's no database connection, so the bot is running on its deployed \
                  configuration and no override can be recorded."
            }))
        }

        h2 { "Right now" }
        div .tiles {
            (tile("Servers", &format!("{} live / {} total", running, live.len())))
            (tile("Discord servers", &guilds.to_string()))
            (tile("Model", &effective.gary_model))
            (tile("Watchdog", effective.watchdog.as_str()))
            (tile("Turn burst", &effective.throttle.burst.to_string()))
            (tile("Turns at once", &effective.throttle.max_concurrent.to_string()))
        }

        h2 { "Subsystems" }
        (table(&["Subsystem", "State", "What it costs when off"], vec![
            subsystem_row("Discord gateway", data.gateway.current().is_some(),
                "Everything — no commands, no Gary, no watchdog response."),
            subsystem_row("Database", data.events.enabled(),
                "Home channels, per-guild config, events, sampling, and the watchdog."),
            subsystem_row("Gary", data.ollama.is_some(),
                "Mentions are declined; slash commands still work."),
            subsystem_row("Backups (S3)", data.backup.is_some(),
                "Backup, archive, and restore report they aren't configured."),
            subsystem_row("Deferred tasks (Valkey)", data.defer.is_enabled(),
                "Gary can't schedule `run_when`; he offers to act now instead."),
            subsystem_row("Telemetry (Langfuse)", data.telemetry.is_enabled(),
                "Turns aren't traced; nothing else changes."),
            subsystem_row("Settings overrides", data.settings.is_available(),
                "This panel is read-only; the deployed configuration still applies."),
        ], "No subsystems."))

        h2 { "This build" }
        (table(&["", ""], vec![
            html! { tr { td { "Release" } td .mono { (data.telemetry.release()) } } },
            html! { tr { td { "Environment" } td .mono { (data.environment.as_str()) } } },
            html! { tr { td { "Namespace" } td .mono { (data.namespace) } } },
        ], "—"))
    };
    html_response(page("Overview", Nav::Dashboard, &identity, &body))
}

fn subsystem_row(name: &str, enabled: bool, cost: &str) -> Markup {
    html! {
        tr {
            td { (name) }
            td { (availability(enabled)) }
            td .hint { (cost) }
        }
    }
}

/// Warn when a restart-class override is recorded but the running process is
/// still on the value it booted with — otherwise the settings page would show a
/// number the bot isn't actually using, with nothing to say so.
fn pending_restart_notice(
    effective: &SettingsSnapshot,
    booted: &SettingsSnapshot,
    overrides: &BTreeMap<SettingKey, StoredOverride>,
) -> Markup {
    let pending: Vec<&'static str> = SettingKey::ALL
        .iter()
        .filter(|key| key.reload() == Reload::Restart && overrides.contains_key(key))
        .filter(|key| key.render(effective) != key.render(booted))
        .map(|key| key.as_str())
        .collect();
    if pending.is_empty() {
        return html! {};
    }
    notice(
        Tone::Warn,
        &html! {
            strong { "Waiting for a restart." }
            " These are recorded but the running bot is still on the value it started with: "
            span .mono { (pending.join(", ")) }
            ". They take effect on the next deploy."
        },
    )
}

// ---------------------------------------------------------------- settings

async fn settings(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
) -> Response {
    let settings = &deps.data.settings;
    let effective = settings.snapshot();
    let booted = settings.booted();
    let baseline = settings.baseline();
    let overrides = settings.overrides().await;

    let body = html! {
        (pending_restart_notice(&effective, &booted, &overrides))
        @if !settings.is_available() {
            (notice(Tone::Bad, &html! {
                strong { "Read-only." }
                " There's no database connection, so these show the deployed configuration \
                  and can't be changed from here."
            }))
        }
        p .hint {
            "Each setting resolves database override, then environment variable, then the \
             built-in default. Clearing an override returns it to the deployed value shown \
             beside it."
        }
        @for section in Section::ALL {
            h2 { (section.title()) }
            (table(&["Setting", "Value", "Source", "When"], SettingKey::ALL
                .iter()
                .filter(|key| key.section() == *section)
                .map(|key| setting_row(*key, &effective, baseline, overrides.get(key)))
                .collect(), "No settings in this section."))
        }
    };
    html_response(page("Settings", Nav::Settings, &identity, &body))
}

fn setting_row(
    key: SettingKey,
    effective: &SettingsSnapshot,
    baseline: &SettingsSnapshot,
    stored: Option<&StoredOverride>,
) -> Markup {
    let value = key.render(effective);
    let deployed = key.render(baseline);
    html! {
        tr {
            td {
                div .mono { (key.as_str()) }
                div .hint { (key.summary()) }
            }
            td .value { (value) }
            td {
                @match stored {
                    Some(override_row) => {
                        (badge("override", Tone::Accent))
                        div .hint {
                            @if let Some(who) = override_row.updated_by.as_ref() {
                                (who) " · "
                            }
                            (override_row.updated_at)
                        }
                        div .hint { "Deployed value: " span .mono { (deployed) } }
                    }
                    None => { (badge("deployed", Tone::Neutral)) }
                }
            }
            td {
                @match key.reload() {
                    Reload::Hot => (badge("live", Tone::Ok)),
                    Reload::Restart => (badge("on restart", Tone::Warn)),
                }
            }
        }
    }
}

// ------------------------------------------------------------------ guilds

async fn guilds(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
) -> Response {
    let data = &deps.data;
    let effective = data.settings.snapshot();
    let allowlist = &effective.guild_allowlist;

    // The gateway cache is the live truth of which guilds the bot is actually
    // in; the allowlist says which of them it serves. Showing both together is
    // what makes an unadmitted guild visible at all.
    let Some(ctx) = data.gateway.current() else {
        let body = html! {
            (notice(Tone::Bad, &html! {
                strong { "Not connected to Discord." }
                " The guild list comes from the gateway, so there's nothing to show until \
                  it reconnects."
            }))
        };
        return html_response(page("Guilds", Nav::Guilds, &identity, &body));
    };

    let mut rows = Vec::new();
    for guild in ctx.cache.guilds() {
        let id = guild.get();
        let name = guild
            .to_guild_cached(&ctx.cache)
            .map_or_else(|| "(uncached)".to_owned(), |cached| cached.name.clone());
        let key = id.to_string();
        let caps = data.guild_limits.caps_for(&key).await;
        let usage = guild_usage(&data.kube_client, &data.namespace, &key, None)
            .await
            .ok();
        let admitted = allowlist.is_empty() || allowlist.contains(&id);
        let overridden = data.guild_limits.overrides_for(&key).await.is_some();
        rows.push(html! {
            tr {
                td {
                    div { (name) }
                    div .hint .mono { (key) }
                }
                td {
                    @if admitted { (badge("served", Tone::Ok)) }
                    @else { (badge("not admitted", Tone::Bad)) }
                }
                td .value {
                    @match usage.as_ref() {
                        Some(usage) => { (usage.server_count) " / " (caps.servers) }
                        None => { "? / " (caps.servers) }
                    }
                }
                td .value {
                    @match usage.as_ref() {
                        Some(usage) => {
                            (format_cores(usage.live_cpu_limit_millicores))
                            " / " (format_cores(caps.cpu_limit_millicores))
                        }
                        None => { "? / " (format_cores(caps.cpu_limit_millicores)) }
                    }
                }
                td .value {
                    @match usage.as_ref() {
                        Some(usage) => {
                            (human_bytes(usage.live_mem_limit_bytes))
                            " / " (human_bytes(caps.mem_limit_bytes))
                        }
                        None => { "? / " (human_bytes(caps.mem_limit_bytes)) }
                    }
                }
                td {
                    @if overridden { (badge("custom caps", Tone::Accent)) }
                }
            }
        });
    }

    let body = html! {
        @if allowlist.is_empty() {
            (notice(Tone::Warn, &html! {
                strong { "Every Discord server is served." }
                " The allowlist is empty, which admits any server the bot is invited to. \
                  That's the historical posture and safe for an un-configured deploy, but \
                  it means admission isn't gated."
            }))
        }
        p .hint {
            "CPU and memory count each live server's configured maximum, so stopped servers \
             use none. A \"?\" means the cluster couldn't be read just now."
        }
        (table(&["Discord server", "Admission", "Servers", "CPU", "Memory", ""],
            rows, "The bot isn't in any Discord servers."))
    };
    html_response(page("Guilds", Nav::Guilds, &identity, &body))
}

// --------------------------------------------------------------- instances

async fn instances(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
) -> Response {
    let data = &deps.data;
    let live = list_live_instances(&data.kube_client, &data.namespace).await;

    let body = match live {
        Err(err) => {
            error!(error = ?err, "admin panel could not list servers");
            notice(
                Tone::Bad,
                &html! {
                    strong { "Couldn't read the cluster." }
                    " The Kubernetes API didn't answer, so there's no server list to show. \
                      The bot logs the reason."
                },
            )
        }
        Ok(live) => {
            let mut rows = Vec::new();
            for instance in &live {
                let name = instance.instance.as_str();
                let sizing = data.sizing.get(name).await;
                rows.push(html! {
                    tr {
                        td .mono { (name) }
                        td { (instance.game.as_str()) }
                        td .mono { (instance.guild.as_str()) }
                        td {
                            @if instance.unhealthy { (badge("unhealthy", Tone::Bad)) }
                            @else if instance.running { (badge("running", Tone::Ok)) }
                            @else { (badge("stopped", Tone::Neutral)) }
                        }
                        td .value {
                            @match sizing {
                                Some(sizing) => {
                                    (format_cores(sizing.cpu_limit_millicores))
                                    " · " (human_bytes(sizing.mem_limit_bytes))
                                }
                                None => { span .hint { "from the catalog" } }
                            }
                        }
                    }
                });
            }
            html! {
                p .hint {
                    "Servers with no pod at all — shut down, PVC still held — don't appear \
                     here; there is nothing in the cluster to read. A blank size means the \
                     server runs on its game's catalog default rather than a per-server \
                     override."
                }
                (table(&["Server", "Game", "Discord server", "State", "Limits"],
                    rows, "No game servers exist right now."))
            }
        }
    };
    html_response(page("Servers", Nav::Instances, &identity, &body))
}

// ------------------------------------------------------------------ events

/// The event browser's filters, straight off the query string. Every field is
/// optional and an empty string means "don't filter", so the form can submit
/// blanks without the handler special-casing them.
#[derive(Debug, Default, Deserialize)]
struct EventQuery {
    kind: Option<String>,
    guild: Option<String>,
    instance: Option<String>,
    surface: Option<String>,
    before: Option<i64>,
}

impl EventQuery {
    fn filter(&self) -> EventFilter {
        fn some_if_set(value: Option<&String>) -> Option<String> {
            value
                .map(|raw| raw.trim().to_owned())
                .filter(|raw| !raw.is_empty())
        }
        EventFilter {
            kind: some_if_set(self.kind.as_ref()),
            guild: some_if_set(self.guild.as_ref()),
            instance: some_if_set(self.instance.as_ref()),
            surface: some_if_set(self.surface.as_ref()),
            before_id: self.before,
        }
    }

    /// The query string for the next page, preserving the active filters.
    fn next_page(&self, before: i64) -> String {
        let mut parts = vec![format!("before={before}")];
        for (name, value) in [
            ("kind", self.kind.as_ref()),
            ("guild", self.guild.as_ref()),
            ("instance", self.instance.as_ref()),
            ("surface", self.surface.as_ref()),
        ] {
            if let Some(value) = value.filter(|raw| !raw.trim().is_empty()) {
                parts.push(format!("{name}={}", urlencode(value.trim())));
            }
        }
        format!("/events?{}", parts.join("&"))
    }
}

/// Percent-encode a query-string value. Only the handful of characters that can
/// appear in an event filter (ids, instance names, kinds) need escaping, but
/// anything outside the unreserved set is encoded rather than guessed at.
fn urlencode(value: &str) -> String {
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

async fn events(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Query(query): Query<EventQuery>,
) -> Response {
    let data = &deps.data;
    let rows = data.events.recent(&query.filter(), EVENT_PAGE_SIZE).await;
    let kinds = data.events.observed_kinds().await;
    let oldest = rows.last().map(|row| row.id);
    let full_page = i64::try_from(rows.len()).unwrap_or(i64::MAX) == EVENT_PAGE_SIZE;

    let body = html! {
        @if !data.events.enabled() {
            (notice(Tone::Bad, &html! {
                strong { "No event log." }
                " There's no database connection, so nothing is being recorded."
            }))
        }
        form .filters method="get" action="/events" {
            div {
                label for="kind" { "Kind" }
                select name="kind" id="kind" {
                    option value="" { "any" }
                    @for kind in &kinds {
                        option value=(kind) selected[query.kind.as_deref() == Some(kind.as_str())] {
                            (kind)
                        }
                    }
                }
            }
            div {
                label for="surface" { "Surface" }
                input type="text" name="surface" id="surface" placeholder="command, agent…"
                    value=[query.surface.as_deref()];
            }
            div {
                label for="guild" { "Discord server" }
                input type="text" name="guild" id="guild" placeholder="id"
                    value=[query.guild.as_deref()];
            }
            div {
                label for="instance" { "Server" }
                input type="text" name="instance" id="instance" placeholder="name"
                    value=[query.instance.as_deref()];
            }
            button type="submit" { "Filter" }
        }
        (table(&["When", "Kind", "Server", "Actor", "Surface", "Outcome", ""],
            rows.iter().map(event_row).collect(),
            "Nothing matches those filters."))
        @if full_page && let Some(oldest) = oldest {
            div .pager { a href=(query.next_page(oldest)) { "Older →" } }
        }
    };
    html_response(page("Events", Nav::Events, &identity, &body))
}

fn event_row(row: &crate::events::EventRow) -> Markup {
    html! {
        tr {
            td .mono { (row.created_at) }
            td .mono { (row.kind) }
            td {
                div .mono {
                    @match row.instance.as_deref() {
                        Some(instance) => (instance),
                        None => "—",
                    }
                }
                // Guild and game are what turn a bare instance name into
                // something you can act on — which community, which game.
                @if row.guild.is_some() || row.game.is_some() {
                    div .hint .mono {
                        @if let Some(game) = row.game.as_deref() { (game) }
                        @if row.guild.is_some() && row.game.is_some() { " · " }
                        @if let Some(guild) = row.guild.as_deref() { (guild) }
                    }
                }
            }
            td .mono { (row.actor) }
            td { (row.surface) }
            td {
                @if row.outcome == "ok" { (badge("ok", Tone::Ok)) }
                @else { (badge(&row.outcome, Tone::Warn)) }
            }
            td {
                // `{}` is every event that carried no extra facts, which is most
                // of them — a disclosure triangle over an empty object is noise.
                @if row.details != "{}" {
                    details .details {
                        summary { "details" }
                        pre { (row.details) }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/pages.rs"]
mod tests;
