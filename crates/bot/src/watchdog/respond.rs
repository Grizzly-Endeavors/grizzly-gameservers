//! The watchdog responder: the `Data`-holding consumer of fired incidents.
//!
//! One sequential loop per process — incidents are rare, and running them one
//! at a time is itself a guardrail (never two proactive Gary turns in flight).
//! Every incident is recorded to the product event log whatever its
//! disposition; that record is what makes shadow mode auditable with one query.
//!
//! In `on` mode a fired incident becomes a manager-tier Gary turn, shaped
//! exactly like a deferred batch (ADR-007): fresh two-message transcript, no
//! session, interactive confirmations, delivered to the guild's home channel —
//! or into an operator's DM when the guild has none. Escalations DM operators
//! through the same notifier every other surface uses.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use poise::serenity_prelude as serenity;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::{Incident, IncidentFacts, IncidentKind};
use crate::agent::{ChatMessage, SessionOutcome, TurnOrigin};
use crate::agones::ServerScope;
use crate::config::WatchdogMode;
use crate::defer::Condition;
use crate::discord::gary::{
    ConfirmMode, GaryTurn, build_system_prompt, game_catalog_list, run_gary_turn,
};
use crate::discord::{AccessLevel, Data};
use crate::events::mapping;
use crate::notify::{Escalation, EscalationContext, summarize_attempts};
use crate::prompts;

/// How long to wait for the gateway's first `Ready` before giving up on a turn
/// and falling back to an operator DM with the raw facts. Generous: Discord
/// outages are rare and an incident is worth waiting out a slow reconnect for.
const GATEWAY_WAIT: Duration = Duration::from_mins(5);
/// Storm guard: with this many proactive turns already started inside
/// [`STORM_WINDOW`], further incidents are suppressed to a one-line operator
/// DM. A node failure fires many instances at once; three investigations is
/// signal, ten is noise (and ten model turns).
const STORM_MAX_TURNS: usize = 3;
const STORM_WINDOW: Duration = Duration::from_mins(30);

/// Consume incidents until shutdown, on the tracked task set so an in-flight
/// response is drained rather than cut off.
pub(crate) fn spawn_responder(data: Data, mut incidents: mpsc::Receiver<Incident>) {
    let tasks = data.tasks.clone();
    let mode = data.settings.snapshot().watchdog;
    tasks.spawn(async move {
        let mut turn_starts: VecDeque<Instant> = VecDeque::new();
        loop {
            let incident = tokio::select! {
                () = data.shutdown.cancelled() => return,
                received = incidents.recv() => match received {
                    Some(incident) => incident,
                    None => return,
                },
            };
            handle_incident(&data, &incident, &mut turn_starts).await;
        }
    });
    info!(mode = mode.as_str(), "proactive watchdog responder started");
}

/// Dispose of one incident according to the watchdog mode, recording exactly
/// one incident event for it whatever happens.
///
/// The mode is read here, per incident, rather than captured when the responder
/// spawned: an operator who turns the watchdog down mid-storm expects the next
/// incident to be recorded rather than acted on, not the one after the restart.
async fn handle_incident(data: &Data, incident: &Incident, turn_starts: &mut VecDeque<Instant>) {
    warn!(
        instance = %incident.instance,
        signal = incident.kind.as_str(),
        facts = ?incident.facts,
        "watchdog incident fired"
    );
    match data.settings.snapshot().watchdog {
        WatchdogMode::On => respond(data, incident, turn_starts).await,
        // `Off` reaches here only for an incident the detector had already fired
        // when the mode changed under it. Recording it like shadow keeps the
        // event log's one-row-per-incident promise through the transition.
        WatchdogMode::Shadow | WatchdogMode::Off => {
            data.events
                .record(mapping::incident_event(incident, mapping::SHADOW, None))
                .await;
        }
    }
}

/// The `on`-mode disposition chain: dedup against the defer watchdog, apply
/// the storm guard, find a channel, run the turn, escalate if it gave up.
async fn respond(data: &Data, incident: &Incident, turn_starts: &mut VecDeque<Instant>) {
    if incident.kind == IncidentKind::Crash
        && data
            .defer
            .is_watching(incident.instance.as_str(), Condition::Startup)
    {
        info!(
            instance = %incident.instance,
            "watchdog crash incident left to the armed deferred startup watcher"
        );
        data.events
            .record(mapping::incident_event(
                incident,
                mapping::DEFERRED_TO_BATCH,
                None,
            ))
            .await;
        return;
    }

    let now = Instant::now();
    if storm_suppressed(turn_starts, now) {
        warn!(
            instance = %incident.instance,
            signal = incident.kind.as_str(),
            "watchdog storm guard: too many proactive turns; suppressing this incident"
        );
        data.events
            .record(mapping::incident_event(
                incident,
                mapping::SUPPRESSED_STORM,
                None,
            ))
            .await;
        data.notifier
            .notify_plain(&format!(
                "**Gary's watchdog (storm guard):** server `{}` fired `{}` — {} — but too many \
                 proactive turns already ran in the last half hour, so nothing was done.",
                incident.instance,
                incident.kind.as_str(),
                facts_line(&incident.facts),
            ))
            .await;
        return;
    }

    let Some(ctx) = wait_for_gateway(data).await else {
        error!(
            instance = %incident.instance,
            "watchdog: no gateway context within the wait window; incident undeliverable"
        );
        data.events
            .record(mapping::incident_event(
                incident,
                mapping::UNDELIVERABLE,
                None,
            ))
            .await;
        data.notifier
            .notify_plain(&format!(
                "**Gary's watchdog:** server `{}` fired `{}` — {} — but Discord's gateway never \
                 came up, so no proactive turn could run.",
                incident.instance,
                incident.kind.as_str(),
                facts_line(&incident.facts),
            ))
            .await;
        return;
    };

    let Some((channel_id, delivery)) = resolve_channel(data, &ctx, incident).await else {
        error!(
            instance = %incident.instance,
            "watchdog: no home channel and no reachable operator DM; incident undeliverable"
        );
        data.events
            .record(mapping::incident_event(
                incident,
                mapping::UNDELIVERABLE,
                None,
            ))
            .await;
        return;
    };

    turn_starts.push_back(now);
    run_incident_turn(data, &ctx, incident, channel_id, delivery).await;
}

/// Retain only turn-starts inside the storm window and say whether another
/// turn would exceed the cap. Pure so the window arithmetic is testable.
fn storm_suppressed(turn_starts: &mut VecDeque<Instant>, now: Instant) -> bool {
    while turn_starts
        .front()
        .is_some_and(|start| now.duration_since(*start) >= STORM_WINDOW)
    {
        turn_starts.pop_front();
    }
    turn_starts.len() >= STORM_MAX_TURNS
}

/// Wait for a live gateway Context, bounded by [`GATEWAY_WAIT`] and shutdown.
async fn wait_for_gateway(data: &Data) -> Option<serenity::Context> {
    let mut gateway = data.gateway.subscribe();
    tokio::select! {
        () = data.shutdown.cancelled() => None,
        waited = tokio::time::timeout(
            GATEWAY_WAIT,
            gateway.wait_for(Option::is_some),
        ) => match waited {
            Ok(Ok(current)) => current.clone(),
            // The sender half only drops when the process is tearing down.
            Ok(Err(_)) | Err(_) => None,
        },
    }
}

/// Where the proactive turn runs: the guild's home channel, else the first
/// operator DM that opens. `None` means there is genuinely nowhere to go.
async fn resolve_channel(
    data: &Data,
    ctx: &serenity::Context,
    incident: &Incident,
) -> Option<(serenity::ChannelId, &'static str)> {
    let guild = incident
        .guild
        .as_ref()
        .and_then(|guild| guild.as_str().parse::<u64>().ok());
    if let Some(guild) = guild
        && let Some(channel) = data.home_channels.home_for_guild(guild).await
    {
        return Some((serenity::ChannelId::new(channel), "home_channel"));
    }
    for &operator in data.operator_ids.iter() {
        match serenity::UserId::new(operator)
            .create_dm_channel(&ctx.http)
            .await
        {
            Ok(channel) => return Some((channel.id, "operator_dm")),
            Err(err) => {
                warn!(error = ?err, operator, "watchdog: couldn't open an operator DM channel");
            }
        }
    }
    None
}

/// Run the incident as a one-shot manager-tier Gary turn — the deferred-batch
/// shape — and record how it went.
/// The tenant an incident's turn is confined to. An instance carrying no owning
/// guild label gets no turn at all: scoping it `All` would hand an unattended
/// manager-tier turn the cross-guild operator view over one missing label.
fn incident_scope(incident: &Incident) -> Option<ServerScope> {
    let Some(guild) = incident.guild.as_ref() else {
        warn!(
            instance = %incident.instance,
            "watchdog: incident on an instance with no owning guild; not responding"
        );
        return None;
    };
    Some(ServerScope::Guild(guild.as_str().to_owned()))
}

async fn run_incident_turn(
    data: &Data,
    ctx: &serenity::Context,
    incident: &Incident,
    channel_id: serenity::ChannelId,
    delivery: &'static str,
) {
    let games = game_catalog_list(data);
    let memories = data.memory.render_for_prompt().await;
    let request = compose_incident_prompt(incident);
    let mut messages = vec![
        ChatMessage::system(build_system_prompt(AccessLevel::Manager, &games, &memories)),
        ChatMessage::user(request.clone()),
    ];

    let guild_id = incident
        .guild
        .as_ref()
        .and_then(|guild| guild.as_str().parse::<u64>().ok());
    let Some(scope) = incident_scope(incident) else {
        return;
    };
    // The turn has no requester; the bot's own snowflake is the honest author
    // ("Gary did it, unprompted"), and the watchdog surface disambiguates it.
    let author_id = ctx.cache.current_user().id;

    let turn = GaryTurn {
        ctx,
        data,
        origin: TurnOrigin::Watchdog,
        session: None,
        channel_id,
        guild: guild_id,
        author_id,
        access: AccessLevel::Manager,
        scope,
        confirmations: ConfirmMode::Interactive,
    };
    match run_gary_turn(&turn, &mut messages).await {
        Ok(SessionOutcome {
            escalation: Some(reason),
            ..
        }) => {
            data.events
                .record(mapping::incident_event(
                    incident,
                    mapping::RESPONDED,
                    Some(delivery),
                ))
                .await;
            warn!(
                instance = %incident.instance,
                kind = reason.as_str(),
                "watchdog turn escalated to operators"
            );
            let escalation = Escalation::from_turn(
                &reason,
                EscalationContext::Watchdog {
                    server: incident.instance.as_str().to_owned(),
                    signal: incident.kind.as_str().to_owned(),
                    guild: guild_id,
                },
                request,
                summarize_attempts(&messages),
            );
            data.events
                .record(mapping::escalation_event(
                    &escalation,
                    &crate::events::Attribution::new(
                        crate::events::Actor::System,
                        crate::events::Surface::Watchdog,
                    ),
                    incident.guild.clone(),
                ))
                .await;
            data.notifier.notify(&escalation).await;
        }
        Ok(SessionOutcome {
            escalation: None, ..
        }) => {
            data.events
                .record(mapping::incident_event(
                    incident,
                    mapping::RESPONDED,
                    Some(delivery),
                ))
                .await;
        }
        Err(err) => {
            error!(
                error = ?err,
                instance = %incident.instance,
                "watchdog turn failed before producing a reply"
            );
            data.events
                .record(mapping::incident_event(
                    incident,
                    mapping::TURN_FAILED,
                    Some(delivery),
                ))
                .await;
        }
    }
}

/// The incident's user turn: the frame prompt around a kind-specific trigger
/// note carrying the fired signal's numbers.
fn compose_incident_prompt(incident: &Incident) -> String {
    let trigger_note = trigger_note(&incident.facts);
    prompts::WatchdogIncident {
        server: incident.instance.as_str(),
        trigger_note: &trigger_note,
    }
    .render()
}

/// The kind-specific predicate phrase completing `The server "<name>" …`.
fn trigger_note(facts: &IncidentFacts) -> String {
    match facts {
        IncidentFacts::CrashLoop {
            restarts_delta,
            window_minutes,
            ..
        } => prompts::WatchdogCrashLoop {
            restarts: &restarts_delta.to_string(),
            minutes: &window_minutes.to_string(),
        }
        .render(),
        IncidentFacts::Crashed { total_restarts } => prompts::WatchdogCrashed {
            restarts: &total_restarts.to_string(),
        }
        .render(),
        IncidentFacts::Unreachable { minutes } => prompts::WatchdogUnreachable {
            minutes: &minutes.to_string(),
        }
        .render(),
        IncidentFacts::Unhealthy => prompts::WatchdogUnhealthy::render(),
        IncidentFacts::MemoryPressure {
            used_bytes,
            limit_bytes,
            percent,
        } => prompts::WatchdogMemoryPressure {
            used_mib: &as_mib(*used_bytes).to_string(),
            limit_mib: &as_mib(*limit_bytes).to_string(),
            percent: &percent.to_string(),
        }
        .render(),
    }
}

/// One human-readable line of an incident's numbers, for operator DMs (which
/// are human-facing text, not model-visible prompt content).
fn facts_line(facts: &IncidentFacts) -> String {
    match facts {
        IncidentFacts::CrashLoop {
            restarts_delta,
            window_minutes,
            total_restarts,
            ..
        } => format!(
            "{restarts_delta} unexpected exits in the last {window_minutes} minutes \
             ({total_restarts} total this pod)"
        ),
        IncidentFacts::Crashed { total_restarts } => format!(
            "the supervisor gave up relaunching after {total_restarts} unexpected exits; \
             the process is down"
        ),
        IncidentFacts::Unreachable { minutes } => {
            format!("its control API has been unreachable for {minutes} minutes")
        }
        IncidentFacts::Unhealthy => {
            "agones marked it unhealthy after crash escalation; its pod is torn down and it \
             stays down until recovered"
                .to_owned()
        }
        IncidentFacts::MemoryPressure {
            used_bytes,
            limit_bytes,
            percent,
        } => format!(
            "memory at {} MiB of {} MiB ({percent}%)",
            as_mib(*used_bytes),
            as_mib(*limit_bytes)
        ),
    }
}

fn as_mib(bytes: i64) -> i64 {
    bytes / (1 << 20)
}

#[cfg(test)]
#[path = "tests/respond.rs"]
mod tests;
