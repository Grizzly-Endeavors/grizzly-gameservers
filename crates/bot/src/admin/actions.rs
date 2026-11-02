//! Everything the panel can change.
//!
//! Each handler is the *executing layer* for its action, which is where ADR-014
//! puts event emission — so a panel-driven stop records exactly one
//! `server_stopped` with `Actor::Admin` and `Surface::Admin`, the same way a
//! slash command records one with the friend who ran it.
//!
//! Actions call the same shared functions the Discord commands and Gary's tools
//! call, never Gary's tool dispatcher: a tool result is model-facing prose and
//! wants a channel and a tier this surface hasn't got. Going through the shared
//! functions is also what makes the per-guild caps apply here for free.
//!
//! Every mutation answers with a redirect rather than a rendered page, so a
//! reload can't repeat it and the browser's back button lands on a view instead
//! of a resubmission prompt.

use axum::Form;
use axum::extract::{Extension, Path, State};
use axum::response::Response;
use serde::Deserialize;
use tracing::error;

use crate::agones::{
    SupervisorOutcome, begin_start, destroy_instance, shutdown_instance, supervisor_restart,
    supervisor_start, supervisor_stop,
};
use crate::discord::{backup_ctx, cold_start_refusal, settings_changed};
use crate::domain::{GuildId, InstanceName};
use crate::events::{Actor, Attribution, EventKind, ProductEvent, Surface, mapping};
use crate::limits::LimitChange;
use crate::settings::SettingKey;

use super::AdminDeps;
use super::auth::AdminIdentity;
use super::render::{Flash, redirect};

/// How the panel attributes what it does.
fn attribution(identity: &AdminIdentity) -> Attribution {
    Attribution::new(Actor::Admin(identity.username.clone()), Surface::Admin)
}

/// Record one panel-driven action against an instance. `record_lifecycle` fills
/// in the owning guild and game from the instance's labels, which the panel
/// doesn't otherwise know.
async fn record(
    deps: &AdminDeps,
    identity: &AdminIdentity,
    kind: EventKind,
    instance: &str,
    outcome: &'static str,
) {
    let event = ProductEvent::new(kind, &attribution(identity), outcome)
        .instance(InstanceName::new(instance));
    deps.data
        .events
        .record_lifecycle(&deps.data.kube_client, &deps.data.namespace, event)
        .await;
}

// ---------------------------------------------------------------- settings

/// One settings form. `value` is absent when the submit button was "reset",
/// which is how a clear is distinguished from setting an empty string — the
/// guild allowlist accepts an empty value meaningfully, so the two can't be
/// collapsed.
#[derive(Debug, Deserialize)]
pub(crate) struct SettingForm {
    value: Option<String>,
    clear: Option<String>,
}

pub(crate) async fn set_setting(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Path(key): Path<String>,
    Form(form): Form<SettingForm>,
) -> Response {
    let Some(key) = SettingKey::from_key(&key) else {
        return redirect(
            "/settings",
            &Flash::error(&format!("There's no setting called {key}.")),
        );
    };
    let clearing = form.clear.is_some();
    let raw = if clearing {
        None
    } else {
        form.value.as_deref()
    };
    let previous = key.render(&deps.data.settings.snapshot());

    match deps.data.settings.apply(key, raw, &identity.username).await {
        Ok(snapshot) => {
            // Push before recording, so the event's timestamp is never earlier
            // than the moment the change actually bound.
            settings_changed(&deps.data, key, &snapshot).await;
            let now = key.render(&snapshot);
            deps.data
                .events
                .record(
                    ProductEvent::new(
                        EventKind::SettingChanged,
                        &attribution(&identity),
                        mapping::CHANGED,
                    )
                    .details(serde_json::json!({
                        "key": key.as_str(),
                        "old": previous,
                        "new": now,
                        "cleared": clearing,
                    })),
                )
                .await;
            let message = if clearing {
                format!("Reset {} to the deployed value ({now}).", key.as_str())
            } else {
                format!("{} is now {now}.", key.as_str())
            };
            redirect("/settings", &Flash::ok(&message))
        }
        Err(err) => {
            // The message is written for the operator, so it is shown as-is
            // rather than replaced with something generic.
            redirect("/settings", &Flash::error(&format!("{err}")))
        }
    }
}

// ------------------------------------------------------------------ guilds

/// A per-guild cap edit. An empty value clears the override back to the global
/// default, matching what `/config limits … default` does in Discord.
#[derive(Debug, Deserialize)]
pub(crate) struct LimitForm {
    field: String,
    value: String,
}

pub(crate) async fn set_guild_limit(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Path(guild): Path<String>,
    Form(form): Form<LimitForm>,
) -> Response {
    let raw = form.value.trim();
    let cleared = raw.is_empty() || raw.eq_ignore_ascii_case("default");
    let change = match parse_limit_change(&form.field, raw, cleared) {
        Ok(change) => change,
        Err(message) => return redirect("/guilds", &Flash::error(&message)),
    };

    match deps
        .data
        .guild_limits
        .apply(&guild, change, &format!("admin:{}", identity.username))
        .await
    {
        Ok(()) => {
            deps.data
                .events
                .record(
                    ProductEvent::new(
                        EventKind::SettingChanged,
                        &attribution(&identity),
                        mapping::CHANGED,
                    )
                    .guild(Some(GuildId::new(guild.clone())))
                    .details(serde_json::json!({
                        "key": format!("guild_limits.{}", form.field),
                        "guild": guild,
                        "new": if cleared { "default".to_owned() } else { raw.to_owned() },
                        "cleared": cleared,
                    })),
                )
                .await;
            let message = if cleared {
                format!("{} for {guild} is back to the global default.", form.field)
            } else {
                format!("{} for {guild} is now {raw}.", form.field)
            };
            redirect("/guilds", &Flash::ok(&message))
        }
        Err(err) => {
            error!(error = ?err, guild = %guild, "admin panel could not set a guild cap");
            redirect("/guilds", &Flash::error(&format!("{err}")))
        }
    }
}

/// Turn a form field and value into a typed cap change, or say why it can't be.
fn parse_limit_change(field: &str, raw: &str, cleared: bool) -> Result<LimitChange, String> {
    match field {
        "servers" => {
            if cleared {
                return Ok(LimitChange::Servers(None));
            }
            raw.parse::<u32>()
                .ok()
                .filter(|&count| count > 0)
                .map(|count| LimitChange::Servers(Some(count)))
                .ok_or_else(|| {
                    format!("Server count needs a whole number of 1 or more, got {raw:?}.")
                })
        }
        "cpu" => {
            if cleared {
                return Ok(LimitChange::CpuLimitMillicores(None));
            }
            crate::events::parse_cpu_millicores(raw)
                .filter(|&value| value > 0)
                .map(|value| LimitChange::CpuLimitMillicores(Some(value)))
                .ok_or_else(|| {
                    format!("CPU needs an amount like \"24\" or \"24000m\", got {raw:?}.")
                })
        }
        "memory" => {
            if cleared {
                return Ok(LimitChange::MemLimitBytes(None));
            }
            crate::events::parse_bytes(raw)
                .filter(|&value| value > 0)
                .map(|value| LimitChange::MemLimitBytes(Some(value)))
                .ok_or_else(|| format!("Memory needs an amount like \"48Gi\", got {raw:?}."))
        }
        other => Err(format!("There's no cap called {other}.")),
    }
}

/// Admit or revoke one guild, by editing the allowlist through the settings
/// façade — so admission has exactly one representation rather than a second
/// table that could disagree with it.
#[derive(Debug, Deserialize)]
pub(crate) struct AdmissionForm {
    admit: Option<String>,
    confirm: Option<String>,
}

pub(crate) async fn set_admission(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Path(guild): Path<String>,
    Form(form): Form<AdmissionForm>,
) -> Response {
    let Ok(id) = guild.parse::<u64>() else {
        return redirect("/guilds", &Flash::error("That isn't a Discord server id."));
    };
    let admitting = form.admit.is_some();
    let current = deps.data.settings.snapshot();
    let mut list: Vec<u64> = current.guild_allowlist.to_vec();

    if admitting {
        if list.is_empty() {
            // Adding the first entry flips the whole service from "serves
            // everyone" to "serves this one guild". Every other guild silently
            // loses its commands, so it takes a second click.
            if form.confirm.is_none() {
                return redirect(
                    &format!("/guilds?confirm=admit&guild={guild}"),
                    &Flash::none(),
                );
            }
        }
        if !list.contains(&id) {
            list.push(id);
        }
    } else {
        list.retain(|entry| *entry != id);
        if list.is_empty() && form.confirm.is_none() {
            // Removing the last entry re-opens the service to every guild — the
            // opposite of what "revoke" sounds like it does.
            return redirect(
                &format!("/guilds?confirm=open&guild={guild}"),
                &Flash::none(),
            );
        }
    }

    let value = list
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    match deps
        .data
        .settings
        .apply(SettingKey::GuildAllowlist, Some(&value), &identity.username)
        .await
    {
        Ok(snapshot) => {
            settings_changed(&deps.data, SettingKey::GuildAllowlist, &snapshot).await;
            deps.data
                .events
                .record(
                    ProductEvent::new(
                        EventKind::SettingChanged,
                        &attribution(&identity),
                        mapping::CHANGED,
                    )
                    .guild(Some(GuildId::new(guild.clone())))
                    .details(serde_json::json!({
                        "key": SettingKey::GuildAllowlist.as_str(),
                        "guild": guild,
                        "admitted": admitting,
                        "new": value,
                    })),
                )
                .await;
            let message = if admitting {
                format!("{guild} is served. Its commands are registering now.")
            } else if snapshot.guild_allowlist.is_empty() {
                "The allowlist is empty again, so every Discord server is served.".to_owned()
            } else {
                format!("{guild} is no longer served. Its commands have been withdrawn.")
            };
            redirect("/guilds", &Flash::ok(&message))
        }
        Err(err) => redirect("/guilds", &Flash::error(&format!("{err}"))),
    }
}

// --------------------------------------------------------------- instances

/// Which lifecycle action a button asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstanceAction {
    Stop,
    Start,
    Restart,
    Shutdown,
    Destroy,
    Backup,
}

impl InstanceAction {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "stop" => Some(Self::Stop),
            "start" => Some(Self::Start),
            "restart" => Some(Self::Restart),
            "shutdown" => Some(Self::Shutdown),
            "destroy" => Some(Self::Destroy),
            "backup" => Some(Self::Backup),
            _ => None,
        }
    }

    /// The path segment this action is posted to.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Start => "start",
            Self::Restart => "restart",
            Self::Shutdown => "shutdown",
            Self::Destroy => "destroy",
            Self::Backup => "backup",
        }
    }

    /// Whether the action needs a typed confirmation. Destroy is the only one
    /// that loses a world: everything else is recoverable by doing the opposite.
    pub(crate) fn is_destructive(self) -> bool {
        self == Self::Destroy
    }

    fn kind(self) -> EventKind {
        match self {
            Self::Stop => EventKind::ServerStopped,
            Self::Start => EventKind::ServerStarted,
            Self::Restart => EventKind::ServerRestarted,
            Self::Shutdown => EventKind::ServerShutdown,
            Self::Destroy => EventKind::ServerDestroyed,
            Self::Backup => EventKind::BackupTaken,
        }
    }
}

/// The confirmation a destructive action carries back on its second submit.
#[derive(Debug, Deserialize)]
pub(crate) struct ActionForm {
    confirm: Option<String>,
}

pub(crate) async fn run_instance_action(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Path((name, action)): Path<(String, String)>,
    Form(form): Form<ActionForm>,
) -> Response {
    let Some(action) = InstanceAction::parse(&action) else {
        return redirect("/instances", &Flash::error("That isn't an action."));
    };
    // Typing the server's own name is the confirmation. A checkbox is one
    // mis-click; retyping `minecraft-survival` is a decision.
    if action.is_destructive() && form.confirm.as_deref() != Some(name.as_str()) {
        return redirect(
            &format!("/instances/{name}?confirm=destroy"),
            &Flash::none(),
        );
    }

    let (outcome, message) = match perform(&deps, &identity, action, &name).await {
        Ok(done) => done,
        Err(err) => {
            error!(error = ?err, server = %name, action = ?action, "admin panel action failed");
            (mapping::FAILED, format!("{err}"))
        }
    };
    record(&deps, &identity, action.kind(), &name, outcome).await;

    let target = if action == InstanceAction::Destroy {
        "/instances".to_owned()
    } else {
        format!("/instances/{name}")
    };
    // The outcome vocabulary distinguishes "did nothing, and here's why"
    // (already_stopped, not_running) from an outright failure. Only the latter
    // reads as an error; the rest are answers.
    let flash = if outcome == mapping::FAILED {
        Flash::error(&message)
    } else {
        Flash::ok(&message)
    };
    redirect(&target, &flash)
}

/// Run the action, returning its recorded outcome and what to tell the operator.
async fn perform(
    deps: &AdminDeps,
    identity: &AdminIdentity,
    action: InstanceAction,
    name: &str,
) -> anyhow::Result<(&'static str, String)> {
    let data = &deps.data;
    match action {
        InstanceAction::Stop => {
            let outcome = supervisor_stop(data.servers(), &data.control_http, name).await?;
            Ok((
                mapping::supervisor(&outcome),
                format!("Stopped {name} — its pod stays up, so starting it again is instant."),
            ))
        }
        InstanceAction::Start => start(deps, name).await,
        InstanceAction::Restart => {
            let outcome = supervisor_restart(data.servers(), &data.control_http, name).await?;
            Ok((mapping::supervisor(&outcome), format!("Restarted {name}.")))
        }
        InstanceAction::Shutdown => {
            let outcome = shutdown_instance(&data.kube_client, &data.namespace, name).await?;
            Ok((
                mapping::shutdown(&outcome),
                format!("Shut {name} down. Its world is kept — starting it brings the pod back."),
            ))
        }
        InstanceAction::Destroy => {
            let outcome = destroy_instance(&data.kube_client, &data.namespace, name).await?;
            Ok((
                mapping::destroy(&outcome),
                format!("Destroyed {name}. Any archives it had are still there."),
            ))
        }
        InstanceAction::Backup => {
            let backups = data.backup.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Backups aren't configured, so there's nowhere to put one.")
            })?;
            let outcome = backups
                .backup_instance(&backup_ctx(data), name, &attribution(identity))
                .await?;
            Ok((mapping::backup(&outcome), format!("Backed up {name}.")))
        }
    }
}

/// Start a server: warm if its pod is still there, cold otherwise.
///
/// The cold path goes under the provision lock and through the same cap gate a
/// friend's `/start` does, so the panel inherits ADR-022's enforcement rather
/// than working around it — an operator raising a cap should be a deliberate act,
/// not a side effect of starting a server from here.
async fn start(deps: &AdminDeps, name: &str) -> anyhow::Result<(&'static str, String)> {
    let data = &deps.data;
    let warm = supervisor_start(data.servers(), &data.control_http, name).await?;
    // A pod that isn't there can't be resumed in place; every other outcome is
    // an answer about the warm path and stands on its own.
    if !matches!(
        warm,
        SupervisorOutcome::PodNotReady | SupervisorOutcome::NotFound
    ) {
        return Ok((mapping::supervisor(&warm), format!("Started {name}.")));
    }

    let lease = data.provision_lock.lock().await;
    if let Some(refusal) = cold_start_refusal(data, name).await? {
        return Ok((
            mapping::limit_refusal(refusal),
            format!(
                "Starting {name} would put its Discord server over its allowance. \
                 Raise that server's caps first, or stop something else."
            ),
        ));
    }
    let sizing = data.sizing.get(name).await;
    let begun = begin_start(
        &data.kube_client,
        &data.namespace,
        &data.domain,
        &data.catalog,
        name,
        sizing,
    )
    .await?;
    drop(lease);
    Ok((
        mapping::start_begin(&begun),
        format!("Starting {name} from cold — its pod is coming back up."),
    ))
}

#[cfg(test)]
#[path = "tests/actions.rs"]
mod tests;

// ----------------------------------------------------------------- signups

/// What an operator can do to a beta request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignupAction {
    Approve,
    Deny,
    /// Retry the approval email for someone already approved. Sends only — it
    /// never re-decides, so it is safe to click twice.
    Resend,
}

impl SignupAction {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "approve" => Some(Self::Approve),
            "deny" => Some(Self::Deny),
            "resend" => Some(Self::Resend),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
            Self::Resend => "resend",
        }
    }

    /// Whether the action needs a typed confirmation.
    ///
    /// Deny only. It is the irreversible one — a denied applicant is off the
    /// queue, and in six months nobody will remember why. Approve is recoverable
    /// (you can admit or refuse their guild afterwards) and Resend does nothing
    /// a second click would worsen.
    pub(crate) fn is_destructive(self) -> bool {
        self == Self::Deny
    }
}

/// A decision, with the address in the **body** rather than the URL.
///
/// Deliberate: query strings and path segments land in the ingress access log
/// and in `Referer` headers, which is a different trust boundary from the page
/// body. A signup address is a stranger's contact detail we solicited on a public
/// form, so it stays out of anything a proxy writes down.
#[derive(Debug, Deserialize)]
pub(crate) struct SignupForm {
    email: String,
    confirm: Option<String>,
}

/// Approve, deny, or re-send — the panel's whole write surface for the queue.
pub(crate) async fn run_signup_action(
    State(deps): State<AdminDeps>,
    Extension(identity): Extension<AdminIdentity>,
    Path(action): Path<String>,
    Form(form): Form<SignupForm>,
) -> Response {
    let Some(action) = SignupAction::parse(&action) else {
        return redirect("/signups", &Flash::error("That isn't an action."));
    };
    let Some(email) = crate::signups::normalize_email(&form.email) else {
        return redirect("/signups", &Flash::error("That isn't an email address."));
    };

    // Typing the address back is the confirmation, matching how destroying a
    // server works. Compared normalized-to-normalized because the page shows the
    // lowercased form but an operator may type what they remember.
    if action.is_destructive() {
        let typed = form
            .confirm
            .as_deref()
            .and_then(crate::signups::normalize_email);
        if typed.as_deref() != Some(email.as_str()) {
            return redirect(
                "/signups?confirm=deny",
                &Flash::error("Type the address exactly to confirm."),
            );
        }
    }

    match action {
        SignupAction::Approve => approve(&deps, &identity, &email, form.confirm.as_deref()).await,
        SignupAction::Deny => deny(&deps, &identity, &email).await,
        SignupAction::Resend => {
            let outcome = send_approval(&deps, &identity, &email, true).await;
            let flash = if outcome.is_sent() {
                Flash::ok(&outcome.describe())
            } else {
                Flash::error(&outcome.describe())
            };
            redirect("/signups", &flash)
        }
    }
}

async fn deny(deps: &AdminDeps, identity: &AdminIdentity, email: &str) -> Response {
    match deps
        .data
        .signups
        .decide(
            email,
            crate::signups::SignupStatus::Denied,
            &identity.username,
        )
        .await
    {
        crate::signups::SignupDecision::Recorded => {
            record_signup(
                deps,
                identity,
                EventKind::BetaDenied,
                mapping::DENIED,
                email,
                false,
            )
            .await;
            redirect("/signups", &Flash::ok("Turned down."))
        }
        crate::signups::SignupDecision::AlreadyDecided => redirect(
            "/signups",
            &Flash::error("Somebody already decided that one."),
        ),
        crate::signups::SignupDecision::Unavailable => redirect(
            "/signups",
            &Flash::error("There's no database, so nothing was recorded."),
        ),
    }
}

async fn approve(
    deps: &AdminDeps,
    identity: &AdminIdentity,
    email: &str,
    confirm: Option<&str>,
) -> Response {
    // Refuse before writing anything: an email pointing nowhere can't be
    // unsent, so a missing invite link is worth a refusal rather than a guess.
    if deps
        .data
        .settings
        .snapshot()
        .beta_invite_url
        .trim()
        .is_empty()
    {
        return redirect(
            "/signups",
            &Flash::error(
                "Set the invite link under Settings → Beta signups first — the email would have \
                 nowhere to point.",
            ),
        );
    }
    // With the allowlist empty every guild is already served, so approving hands
    // out working access immediately and the Guilds page gates nothing. The
    // checkbox lives in the form rather than a `?confirm=` bounce because the
    // state that would have to survive the bounce is *which person*.
    if deps.data.settings.snapshot().guild_allowlist.is_empty() && confirm.is_none() {
        return redirect(
            "/signups",
            &Flash::error(
                "The guild allowlist is empty, so approving grants access straight away. Tick the \
                 box on the row to confirm you mean that.",
            ),
        );
    }

    // Decision first, delivery second: an approval that isn't recorded returns
    // to the queue and gets approved twice, which means two emails to a stranger.
    match deps
        .data
        .signups
        .decide(
            email,
            crate::signups::SignupStatus::Approved,
            &identity.username,
        )
        .await
    {
        crate::signups::SignupDecision::Recorded => {}
        crate::signups::SignupDecision::AlreadyDecided => {
            return redirect(
                "/signups",
                &Flash::error("Somebody already decided that one."),
            );
        }
        crate::signups::SignupDecision::Unavailable => {
            return redirect(
                "/signups",
                &Flash::error("There's no database, so nothing was recorded."),
            );
        }
    }
    record_signup(
        deps,
        identity,
        EventKind::BetaApproved,
        mapping::APPROVED,
        email,
        false,
    )
    .await;

    let outcome = send_approval(deps, identity, email, false).await;
    let flash = if outcome.is_sent() {
        Flash::ok("Approved — the invite email is on its way.")
    } else {
        Flash::error(&format!("Approved, but {}", outcome.describe()))
    };
    redirect("/signups", &flash)
}

/// Send (or re-send) the approval email and record what happened, both in the
/// row's own columns and in the product log.
///
/// `resend` rides into the event rather than being inferred, because the two
/// callers are the only thing that knows which this is — and without it a first
/// send and a retry are indistinguishable in the log, which is exactly the
/// distinction "approved but never told" is measured on.
async fn send_approval(
    deps: &AdminDeps,
    identity: &AdminIdentity,
    email: &str,
    resend: bool,
) -> crate::mail::MailOutcome {
    let snapshot = deps.data.settings.snapshot();
    let community = Some(snapshot.community_url.as_str()).filter(|url| !url.trim().is_empty());
    let body = crate::mail::approval_body(&snapshot.beta_invite_url, community);
    let outcome = deps
        .data
        .mailer
        .send(email, crate::mail::APPROVAL_SUBJECT, &body)
        .await;

    if outcome.is_sent() {
        deps.data.signups.mark_emailed(email).await;
    } else {
        deps.data
            .signups
            .mark_email_failed(email, &outcome.describe())
            .await;
    }
    record_signup(
        deps,
        identity,
        EventKind::SignupEmailed,
        mapping::mail(&outcome),
        email,
        resend,
    )
    .await;
    outcome
}

/// Record one signup event.
///
/// Carries the address's **domain**, never the address: `events` is browsed in
/// this panel, exported to Metabase, and read through a SELECT-only analytics
/// role, and solicited contact details are hard to un-spill once they are in the
/// aggregate plane. The audit trail for who was approved and when lives in the
/// `signups` row itself, behind Authentik.
async fn record_signup(
    deps: &AdminDeps,
    identity: &AdminIdentity,
    kind: EventKind,
    outcome: &'static str,
    email: &str,
    resend: bool,
) {
    let domain = email.rsplit('@').next().unwrap_or("unknown");
    deps.data
        .events
        .record(
            ProductEvent::new(kind, &attribution(identity), outcome)
                .details(serde_json::json!({ "domain": domain, "resend": resend })),
        )
        .await;
}
