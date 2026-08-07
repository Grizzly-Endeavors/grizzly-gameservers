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
use axum::extract::{Extension, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use maud::{Markup, html};
use serde::Deserialize;
use tracing::error;

use crate::agones::{guild_usage, list_live_instances};
use crate::events::EventFilter;
use crate::limits::format_cores;
use crate::settings::{Reload, Section, SettingKey, SettingsSnapshot, StoredOverride};

use super::AdminDeps;
use super::actions::{self, InstanceAction, SignupAction};
use super::auth::AdminIdentity;
use super::render::{
    Flash, Nav, Tone, availability, badge, html_response, notice, page, percent_encode, table, tile,
};

/// Rows shown on one page of the event log. Generous — this is a scan-and-close
/// view, and paging is cheap — but bounded so one query can't hold the pool.
const EVENT_PAGE_SIZE: i64 = 100;

/// Beta requests shown at once. The queue is worked front to back, so a bound
/// here costs nothing: what falls off the end is what you get to next.
const SIGNUP_QUEUE_SIZE: i64 = 50;

/// Decided requests kept in view, as recent context rather than an archive —
/// `/events` is where the full history lives.
const SIGNUP_HISTORY_SIZE: i64 = 20;

/// Note length shown in the queue table. The field is free text from a public
/// form, so one pasted essay must not blow out the row.
const NOTE_PREVIEW_CHARS: usize = 300;

/// Cap on the updates-list export. Generous, and the page says so when it bites —
/// a mailing list that quietly stops halfway is worse than one that says it did.
const UPDATES_EXPORT_LIMIT: i64 = 10_000;

pub(crate) fn router(deps: AdminDeps) -> Router {
    Router::new()
        .route("/", get(dashboard))
        .route("/settings", get(settings))
        .route("/settings/{key}", post(actions::set_setting))
        .route("/signups", get(signups))
        // Top level, not under /signups/: axum matches the path first, so a
        // GET at /signups/export.csv would collide with the action route
        // below and 405.
        .route("/signups.csv", get(signups_csv))
        .route("/signups/{action}", post(actions::run_signup_action))
        .route("/guilds", get(guilds))
        .route("/guilds/{guild}/limits", post(actions::set_guild_limit))
        .route("/guilds/{guild}/admission", post(actions::set_admission))
        .route("/instances", get(instances))
        .route("/instances/{name}", get(instance_detail))
        .route(
            "/instances/{name}/{action}",
            post(actions::run_instance_action),
        )
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
    Query(flash): Query<Flash>,
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
        (flash.render())
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
            subsystem_row("Approval mail (SMTP)", data.mailer.is_enabled(),
                "Approvals are still recorded, but nobody is told; mail them later with Resend."),
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
    Query(flash): Query<Flash>,
) -> Response {
    let settings = &deps.data.settings;
    let effective = settings.snapshot();
    let booted = settings.booted();
    let baseline = settings.baseline();
    let overrides = settings.overrides().await;
    let editable = settings.is_available();

    let body = html! {
        (flash.render())
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
                .map(|key| setting_row(*key, &effective, baseline, overrides.get(key), editable))
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
    editable: bool,
) -> Markup {
    let value = key.render(effective);
    let deployed = key.render(baseline);
    let action = format!("/settings/{}", key.as_str());
    html! {
        tr {
            td {
                div .mono { (key.as_str()) }
                div .hint { (key.summary()) }
            }
            td {
                form .inline method="post" action=(action) {
                    // The rendered value is what round-trips, so the field starts
                    // as exactly what is in force — an operator who saves without
                    // editing changes nothing.
                    input type="text" name="value" value=(value) disabled[!editable]
                        aria-label=(format!("{} value", key.as_str()));
                    button type="submit" disabled[!editable] { "Save" }
                    @if stored.is_some() {
                        button type="submit" name="clear" value="1" disabled[!editable]
                            title=(format!("Back to the deployed value: {deployed}")) { "Reset" }
                    }
                }
                div .hint { "Needs " (key.expected()) "." }
            }
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
                        div .hint { "Deployed: " span .mono { (deployed) } }
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

// ----------------------------------------------------------------- signups

/// Warn that admitting a guild isn't gating anything while the allowlist is
/// empty.
///
/// This is the notice that matters most on this page. Approving a beta request
/// mails out a bot-invite link, and `guild_is_admitted` treats an empty
/// allowlist as admitting *every* guild — so with the list empty, clicking
/// approve grants access the moment the person follows that link, and the
/// deliberate second step on the Guilds page is decorative. Saying so here puts
/// it in front of whoever is about to approve someone, rather than in a runbook.
fn allowlist_open_notice(allowlist_empty: bool) -> Markup {
    if !allowlist_empty {
        return html! {};
    }
    notice(
        Tone::Warn,
        &html! {
            strong { "The guild allowlist is empty, so every Discord server is served." }
            " Approving someone mails them an invite link, and their server is served the moment \
              they use it — the admission step on the Guilds page isn't gating anything yet. \
              Admit the servers you mean to serve before approving anyone."
        },
    )
}

/// Note the person left, bounded for display.
fn note_preview(note: Option<&String>) -> Markup {
    let Some(note) = note.filter(|note| !note.is_empty()) else {
        return html! { span .hint { "—" } };
    };
    html! { (crate::signups::truncate_chars(note, NOTE_PREVIEW_CHARS)) }
}

/// What a signup asked for. Both are independent opt-ins, so both can show.
fn wants_badges(wants_beta: bool, wants_updates: bool) -> Markup {
    html! {
        @if wants_beta { (badge("beta", Tone::Accent)) }
        @if wants_updates { (badge("updates", Tone::Ok)) }
    }
}

fn pending_row(row: &crate::signups::SignupRow, allowlist_empty: bool, confirming: bool) -> Markup {
    html! {
        tr {
            td .mono { (row.created_at) }
            td { (row.display_email) }
            td { (wants_badges(row.wants_beta, row.wants_updates)) }
            td { (note_preview(row.note.as_ref())) }
            td .actions {
                form .inline method="post" action=(signup_path(SignupAction::Approve)) {
                    input type="hidden" name="email" value=(row.email);
                    @if allowlist_empty {
                        label .hint {
                            input type="checkbox" name="confirm" value="1" required;
                            " serves everyone"
                        }
                    }
                    button type="submit" { "Approve" }
                }
                form .inline method="post" action=(signup_path(SignupAction::Deny)) {
                    input type="hidden" name="email" value=(row.email);
                    @if confirming {
                        input type="text" name="confirm" placeholder="type the address";
                    }
                    button .danger type="submit" { "Deny" }
                }
            }
        }
    }
}

/// One CSV record, RFC 4180 quoted.
///
/// Every field is quoted rather than only those that need it: the rule is then
/// one line rather than a predicate somebody later gets wrong. Control
/// characters are dropped — a newline inside a quoted field is legal but breaks
/// naive readers, and nothing in this export legitimately contains one.
fn csv_row(fields: &[&str]) -> String {
    let mut out = String::new();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        for ch in field.chars().filter(|ch| !ch.is_control()) {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    }
    out.push('\n');
    out
}

/// The updates list as a CSV download.
///
/// Inside `pages::router`, so `require_admin` covers it with no per-route check.
async fn signups_csv(State(deps): State<AdminDeps>) -> Response {
    let rows = deps.data.signups.updates_export(UPDATES_EXPORT_LIMIT).await;
    let mut body = csv_row(&["email", "first_asked_at", "wants_beta"]);
    for row in &rows {
        body.push_str(&csv_row(&[
            &row.email,
            &row.first_asked_at,
            if row.wants_beta { "true" } else { "false" },
        ]));
    }
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"gary-updates.csv\"",
            ),
        ],
        body,
    )
        .into_response()
}

/// Where a signup action posts. Built from the enum rather than a literal so the
/// route, the handler's `parse`, and the form can't drift apart.
fn signup_path(action: SignupAction) -> String {
    format!("/signups/{}", action.as_str())
}

/// One approval whose email never landed, with the reason and a way to retry.
fn unmailed_row(row: &crate::signups::SignupRow) -> Markup {
    html! {
        tr {
            td .mono { (row.decided_at.as_deref().unwrap_or("—")) }
            td { (row.display_email) }
            td .hint { (row.email_error.as_deref().unwrap_or("never attempted")) }
            td .actions {
                form .inline method="post" action=(signup_path(SignupAction::Resend)) {
                    input type="hidden" name="email" value=(row.email);
                    button type="submit" { "Resend" }
                }
            }
        }
    }
}

fn decided_row(row: &crate::signups::SignupRow) -> Markup {
    let outcome = row.status.as_deref().unwrap_or("—");
    let tone = if outcome == "approved" {
        Tone::Ok
    } else {
        Tone::Bad
    };
    html! {
        tr {
            td .mono { (row.decided_at.as_deref().unwrap_or("—")) }
            td { (row.display_email) }
            td { (badge(outcome, tone)) }
            td { (row.decided_by.as_deref().unwrap_or("—")) }
        }
    }
}

/// Whether the page is mid-confirmation for a deny, which is what makes the
/// "type the address" box appear.
#[derive(Debug, Default, Deserialize)]
struct SignupConfirm {
    confirm: Option<String>,
}

impl SignupConfirm {
    fn is_denying(&self) -> bool {
        self.confirm.as_deref() == Some("deny")
    }
}

async fn signups(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Query(flash): Query<Flash>,
    Query(confirm): Query<SignupConfirm>,
) -> Response {
    let data = &deps.data;
    let enabled = data.signups.is_enabled();
    let pending = data.signups.pending_beta(SIGNUP_QUEUE_SIZE).await;
    let unmailed = data.signups.approved_not_emailed(SIGNUP_HISTORY_SIZE).await;
    let decided = data.signups.recent_decisions(SIGNUP_HISTORY_SIZE).await;
    let updates = data.signups.updates_count().await;
    let last_day = data.signups.submissions_last_24h().await;
    let allowlist_empty = data.settings.snapshot().guild_allowlist.is_empty();

    let body = html! {
        (flash.render())
        @if !enabled {
            (notice(Tone::Bad, &html! {
                strong { "No signup queue." }
                " There's no database connection, so the landing page can't record anyone and \
                  there's nothing to show here."
            }))
        }
        (allowlist_open_notice(allowlist_empty))
        div .tiles {
            (tile("Waiting on you", &pending.len().to_string()))
            (tile("On the updates list", &updates.to_string()))
            (tile("Signed up today", &last_day.to_string()))
        }
        p .hint {
            a href="/signups.csv" { "Download the updates list (CSV)" }
            @if updates >= UPDATES_EXPORT_LIMIT {
                " — capped at " (UPDATES_EXPORT_LIMIT) " rows."
            }
        }
        @if !data.mailer.is_enabled() {
            (notice(Tone::Warn, &html! {
                strong { "Approval mail is off." }
                " Approving still records the decision, but nobody is told. Once mail is \
                  configured, use Resend on the rows below."
            }))
        }
        h2 { "Waiting on you" }
        (table(
            &["Asked", "Email", "Wants", "Note", ""],
            pending.iter()
                .map(|row| pending_row(row, allowlist_empty, confirm.is_denying()))
                .collect(),
            "Nobody is waiting for a decision.",
        ))
        @if !unmailed.is_empty() {
            h2 { "Approved, not emailed" }
            (notice(Tone::Warn, &html! {
                "These people were let in but never told. Resend once the problem is fixed."
            }))
            (table(
                &["Decided", "Email", "What happened", ""],
                unmailed.iter().map(unmailed_row).collect(),
                "—",
            ))
        }
        h2 { "Recently decided" }
        (table(
            &["Decided", "Email", "Outcome", "By"],
            decided.iter().map(decided_row).collect(),
            "No decisions yet.",
        ))
    };
    html_response(page("Signups", Nav::Signups, &identity, &body))
}

// ------------------------------------------------------------------ guilds

async fn guilds(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Query(flash): Query<Flash>,
    Query(confirm): Query<ConfirmQuery>,
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
                    (cap_form(&key, "servers", "Servers"))
                    (cap_form(&key, "cpu", "CPU"))
                    (cap_form(&key, "memory", "Memory"))
                }
                td {
                    form .inline method="post" action=(format!("/guilds/{key}/admission")) {
                        @if admitted {
                            button type="submit" { "Stop serving" }
                        } @else {
                            button type="submit" name="admit" value="1" { "Serve" }
                        }
                    }
                }
            }
        });
    }

    let body = html! {
        (flash.render())
        (admission_confirm(&confirm))
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
        (table(&["Discord server", "Admission", "Servers", "CPU", "Memory", "Caps", ""],
            rows, "The bot isn't in any Discord servers."))
    };
    html_response(page("Guilds", Nav::Guilds, &identity, &body))
}

/// A one-field form for a single cap. Blank means "back to the global default",
/// matching what `/config limits … default` does in Discord.
fn cap_form(guild: &str, field: &str, label: &str) -> Markup {
    html! {
        form .inline method="post" action=(format!("/guilds/{guild}/limits")) {
            input type="hidden" name="field" value=(field);
            input type="text" name="value" placeholder="default" size="7"
                aria-label=(format!("{label} cap for {guild}"));
            button type="submit" { (label) }
        }
    }
}

/// The two allowlist edits that change what *every other* guild gets, spelled
/// out before they happen.
///
/// Admitting the first guild flips the service from "serves everyone" to "serves
/// this one"; removing the last one flips it back. Both read as a change to one
/// guild and are a change to all of them, which is exactly the shape that
/// deserves a second click.
fn admission_confirm(confirm: &ConfirmQuery) -> Markup {
    let Some(guild) = confirm.guild.as_deref() else {
        return html! {};
    };
    match confirm.confirm.as_deref() {
        Some("admit") => notice(
            Tone::Warn,
            &html! {
                strong { "This closes the door on every other Discord server." }
                " The allowlist is empty right now, which serves everyone. Adding "
                span .mono { (guild) }
                " makes it the only one served — every other server loses its commands."
                form .inline method="post" action=(format!("/guilds/{guild}/admission")) {
                    input type="hidden" name="admit" value="1";
                    input type="hidden" name="confirm" value="1";
                    button type="submit" { "Serve only this one" }
                }
            },
        ),
        Some("open") => notice(
            Tone::Warn,
            &html! {
                strong { "This opens the door to every Discord server." }
                " Removing "
                span .mono { (guild) }
                " empties the allowlist, and an empty allowlist serves every server the bot is \
                  invited to — the opposite of what stopping one usually means."
                form .inline method="post" action=(format!("/guilds/{guild}/admission")) {
                    input type="hidden" name="confirm" value="1";
                    button type="submit" { "Serve everyone" }
                }
            },
        ),
        _ => html! {},
    }
}

// --------------------------------------------------------------- instances

async fn instances(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Query(flash): Query<Flash>,
) -> Response {
    let data = &deps.data;
    let live = list_live_instances(&data.kube_client, &data.namespace).await;

    let listing = match live {
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
                        td .mono { a href=(format!("/instances/{name}")) { (name) } }
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
    let body = html! { (flash.render()) (listing) };
    html_response(page("Servers", Nav::Instances, &identity, &body))
}

/// One server: what it is, what it's allowed, and the buttons that act on it.
///
/// Destroy sits behind a typed confirmation on this page rather than a checkbox
/// in the list — losing a world should take a deliberate act, and typing the
/// server's own name is one.
async fn instance_detail(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Path(name): Path<String>,
    Query(flash): Query<Flash>,
    Query(confirm): Query<ConfirmQuery>,
) -> Response {
    let data = &deps.data;
    let live = list_live_instances(&data.kube_client, &data.namespace)
        .await
        .unwrap_or_default();
    let found = live
        .iter()
        .find(|instance| instance.instance.as_str() == name);
    let sizing = data.sizing.get(&name).await;
    let backups_on = data.backup.is_some();
    let recent = data
        .events
        .recent(
            &EventFilter {
                instance: Some(name.clone()),
                ..EventFilter::default()
            },
            20,
        )
        .await;

    let body = html! {
        (flash.render())
        @if confirm.confirm.as_deref() == Some("destroy") {
            (notice(Tone::Bad, &html! {
                strong { "Destroy " span .mono { (name) } "?" }
                " This deletes the server and its world. Archives it already has are kept, \
                  but anything not archived is gone. Type the server's name to confirm."
                form .inline method="post" action=(format!("/instances/{name}/destroy")) {
                    input type="text" name="confirm" placeholder=(name)
                        aria-label="Type the server name to confirm";
                    button type="submit" .danger { "Destroy it" }
                }
            }))
        }

        @match found {
            Some(instance) => {
                h2 { "State" }
                div .tiles {
                    (tile("Server", &name))
                    (tile("Game", instance.game.as_str()))
                    (tile("Discord server", instance.guild.as_str()))
                    (tile("Process", if instance.unhealthy {
                        "unhealthy"
                    } else if instance.running { "running" } else { "stopped" }))
                }
            }
            None => {
                (notice(Tone::Warn, &html! {
                    strong { "No pod for " span .mono { (name) } "." }
                    " It's either shut down — its world is still on disk — or it doesn't \
                      exist. Starting it will bring the pod back if the world is there."
                }))
            }
        }

        h2 { "Allowed" }
        (table(&["", ""], vec![
            html! { tr {
                td { "CPU / memory" }
                td .value {
                    @match sizing {
                        Some(sizing) => {
                            (format_cores(sizing.cpu_limit_millicores))
                            " · " (human_bytes(sizing.mem_limit_bytes))
                        }
                        None => { span .hint { "the game's catalog default" } }
                    }
                }
            } },
        ], "—"))

        h2 { "Actions" }
        div .card {
            div .actions {
                (action_button(&name, InstanceAction::Stop, "Stop", true))
                (action_button(&name, InstanceAction::Start, "Start", true))
                (action_button(&name, InstanceAction::Restart, "Restart", true))
                (action_button(&name, InstanceAction::Backup, "Back up now", backups_on))
                (action_button(&name, InstanceAction::Shutdown, "Shut down", true))
                (action_button(&name, InstanceAction::Destroy, "Destroy", true))
            }
            div .hint style="padding: 0 0.95rem 0.9rem" {
                "Stop leaves the pod up, so starting again is instant. Shut down removes the \
                 pod and keeps the world. Destroy removes both."
            }
        }

        h2 { "Recent events" }
        (table(&["When", "Kind", "Actor", "Surface", "Outcome"],
            recent.iter().map(|row| html! {
                tr {
                    td .mono { (row.created_at) }
                    td .mono { (row.kind) }
                    td .mono { (row.actor) }
                    td { (row.surface) }
                    td { (row.outcome) }
                }
            }).collect(),
            "Nothing recorded for this server."))
    };
    html_response(page(&name, Nav::Instances, &identity, &body))
}

/// One action button. Destructive actions send the operator to a confirmation
/// instead of acting, which the handler enforces independently — this only
/// decides what the button looks like.
fn action_button(name: &str, action: InstanceAction, label: &str, enabled: bool) -> Markup {
    let path = format!("/instances/{name}/{}", action.as_str());
    html! {
        form .inline method="post" action=(path) {
            button type="submit" disabled[!enabled]
                class=[action.is_destructive().then_some("danger")] { (label) }
        }
    }
}

/// A `?confirm=…` marker, set by a handler that bounced an action back for
/// confirmation.
#[derive(Debug, Default, Deserialize)]
struct ConfirmQuery {
    confirm: Option<String>,
    guild: Option<String>,
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
                parts.push(format!("{name}={}", percent_encode(value.trim())));
            }
        }
        format!("/events?{}", parts.join("&"))
    }
}

async fn events(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Query(query): Query<EventQuery>,
    Query(flash): Query<Flash>,
) -> Response {
    let data = &deps.data;
    let rows = data.events.recent(&query.filter(), EVENT_PAGE_SIZE).await;
    let kinds = data.events.observed_kinds().await;
    let oldest = rows.last().map(|row| row.id);
    let full_page = i64::try_from(rows.len()).unwrap_or(i64::MAX) == EVENT_PAGE_SIZE;

    let body = html! {
        (flash.render())
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
