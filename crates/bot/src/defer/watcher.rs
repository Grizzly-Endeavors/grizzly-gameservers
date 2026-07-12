//! The background poll loops behind the deferred-task queue. One watcher per
//! `(server, condition)` waits — cancellably — until the condition holds, then
//! drains the queue and runs the batch as one manager-tier Gary turn. Detection
//! reuses the bot's existing supervisor clients (`wait_for_ready_within` for
//! `startup`, `supervisor_occupancy` for `empty`/`idle`); nothing new touches the
//! cluster.

use std::time::Duration;

use poise::serenity_prelude as serenity;
use tokio::time::Instant;
use tracing::{debug, error, warn};

use super::condition::{Condition, compose_batch_prompt, next_empty_since, wait_key};
use super::task::DeferredTask;
use crate::agent::{ChatMessage, SessionOutcome};
use crate::agones::{
    FsOutcome, ReadyWait, ServerScope, guild_of, supervisor_occupancy, wait_for_ready_within,
};
use crate::discord::gary::{
    GaryTurn, build_system_prompt, game_catalog_list, run_gary_turn, send_chunks,
};
use crate::discord::{AccessLevel, Data};
use crate::notify::{Escalation, EscalationContext, summarize_attempts};
use crate::prompts;

/// How long the `startup` watchdog waits for a (re)start to settle before calling
/// it stuck. No real server takes this long to come up, so exceeding it is itself
/// the signal that a boot is wedged.
const STARTUP_CEILING: Duration = Duration::from_mins(20);
/// Poll cadence for the `empty` condition — brisk, since "empty now" changes are
/// wanted promptly as the last player logs off.
const EMPTY_POLL_INTERVAL: Duration = Duration::from_secs(30);
/// Poll cadence for the `idle` condition — relaxed; idle waits are no-rush.
const IDLE_POLL_INTERVAL: Duration = Duration::from_mins(1);
/// How long a server must stay continuously empty before `idle` fires.
const IDLE_GRACE: Duration = Duration::from_mins(5);
/// Give-up ceiling for an occupancy wait that never resolves (server unreachable
/// forever, etc.). Matches the queue TTL so the watcher and the key retire together.
const OCCUPANCY_CEILING: Duration = Duration::from_hours(24);

/// Drive one `(server, condition)` watcher to completion. Waits for the condition
/// (cancellable on shutdown), drains and runs the batch, then retires — re-arming
/// only if a task slipped in after the drain. On shutdown it exits leaving the
/// queue in Valkey, which [`super::DeferRuntime::reconcile`] rebuilds next boot.
pub(crate) async fn run_watcher(
    data: Data,
    ctx: serenity::Context,
    server: String,
    condition: Condition,
) {
    let key = wait_key(&server, condition);
    loop {
        let trigger_note = tokio::select! {
            () = data.shutdown.cancelled() => {
                debug!(
                    server = %server,
                    condition = condition.as_str(),
                    "deferred watcher stopping for shutdown; queue persists in valkey"
                );
                return;
            }
            note = wait_for_condition(&data, &server, condition) => note,
        };

        let raw = match data.defer.valkey.drain(&key).await {
            Ok(raw) => raw,
            Err(err) => {
                error!(error = ?err, server = %server, "deferred watcher: failed to drain queue; exiting");
                data.defer.deregister(&server, condition);
                return;
            }
        };
        if !raw.is_empty() {
            run_batch(&data, &ctx, &server, condition, &trigger_note, raw).await;
        }

        // Retire, then re-check: a task pushed after the drain but before we left
        // the registry would otherwise be stranded (its enqueue saw us registered
        // and didn't spawn). If one landed, re-arm and keep going.
        data.defer.deregister(&server, condition);
        match data.defer.valkey.is_empty(&key).await {
            Ok(true) => return,
            Ok(false) => data.defer.register_silent(&server, condition),
            Err(err) => {
                warn!(error = ?err, server = %server, "deferred watcher: couldn't confirm queue empty; exiting (reconcile re-arms)");
                return;
            }
        }
    }
}

/// Wait until `condition` holds for `server`, returning a plain-language note for
/// the batch prompt describing what happened (which is *not* always success —
/// `startup` also returns when a boot fails or stalls, so Gary can react).
async fn wait_for_condition(data: &Data, server: &str, condition: Condition) -> String {
    match condition {
        Condition::Startup => wait_startup(data, server).await,
        Condition::Empty => wait_occupancy(data, server, EMPTY_POLL_INTERVAL, None).await,
        Condition::Idle => wait_occupancy(data, server, IDLE_POLL_INTERVAL, Some(IDLE_GRACE)).await,
    }
}

/// Watch a (re)start to its outcome and describe it. Every [`ReadyWait`] variant
/// is terminal and maps to a note; a cluster error can't be watched through, so it
/// too resolves to a note rather than hanging.
async fn wait_startup(data: &Data, server: &str) -> String {
    match wait_for_ready_within(
        &data.kube_client,
        &data.http,
        &data.namespace,
        server,
        data.control_port,
        STARTUP_CEILING,
    )
    .await
    {
        Ok(ReadyWait::Ready) => prompts::StartupReady::render(),
        Ok(ReadyWait::Crashed) => prompts::StartupCrashed::render(),
        Ok(ReadyWait::Stopped) => prompts::StartupStopped::render(),
        Ok(ReadyWait::TimedOut) => prompts::StartupTimedOut::render(),
        Ok(ReadyWait::NotFound) => prompts::StartupNotFound::render(),
        Ok(ReadyWait::NotManaged) => prompts::StartupNotManaged::render(),
        Err(err) => {
            error!(error = ?err, server = %server, "deferred startup watch: cluster query failed");
            prompts::StartupUnchecked::render()
        }
    }
}

/// Poll occupancy until the server is empty. With `grace = None` this fires the
/// moment the count reaches zero (`empty`); with `Some(window)` it fires only
/// after the server has stayed empty for `window` (`idle`). An unknown count never
/// counts as empty and resets any idle streak.
async fn wait_occupancy(
    data: &Data,
    server: &str,
    poll_interval: Duration,
    grace: Option<Duration>,
) -> String {
    let deadline = Instant::now() + OCCUPANCY_CEILING;
    let mut empty_since: Option<Instant> = None;
    loop {
        match supervisor_occupancy(
            &data.kube_client,
            &data.http,
            &data.namespace,
            server,
            data.control_port,
        )
        .await
        {
            Ok(FsOutcome::Ok(reading)) => match grace {
                None => {
                    if reading == Some(0) {
                        return prompts::OccupancyEmpty::render();
                    }
                }
                Some(window) => {
                    empty_since = next_empty_since(empty_since, reading, Instant::now());
                    if empty_since
                        .is_some_and(|since| Instant::now().duration_since(since) >= window)
                    {
                        return prompts::OccupancyIdle::render();
                    }
                }
            },
            Ok(FsOutcome::NotFound) => {
                return prompts::OccupancyNotFound::render();
            }
            Ok(FsOutcome::NotManaged) => {
                return prompts::OccupancyNotManaged::render();
            }
            // Transient: the console isn't reachable this tick. Treat as "unknown"
            // (reset any idle streak) and keep polling until the ceiling.
            Ok(FsOutcome::PodNotReady | FsOutcome::Unreachable | FsOutcome::Rejected(_)) => {
                empty_since = None;
            }
            Err(err) => {
                error!(error = ?err, server = %server, "deferred occupancy watch: read failed");
                empty_since = None;
            }
        }
        if Instant::now() >= deadline {
            return prompts::OccupancyTimedOut::render();
        }
        tokio::time::sleep(poll_interval).await;
    }
}

/// Run the drained tasks as one manager-tier Gary turn scoped to the server's
/// guild, delivering the result to the channel(s) the tasks came from.
async fn run_batch(
    data: &Data,
    ctx: &serenity::Context,
    server: &str,
    condition: Condition,
    trigger_note: &str,
    raw: Vec<String>,
) {
    let tasks: Vec<DeferredTask> = raw
        .iter()
        .filter_map(|payload| match serde_json::from_str::<DeferredTask>(payload) {
            Ok(task) => Some(task),
            Err(err) => {
                error!(error = ?err, server = %server, "dropping unparseable deferred task payload");
                None
            }
        })
        .collect();
    let Some(latest) = tasks.last() else {
        return;
    };
    let primary_channel = latest.channel_id;
    let author_id = latest.requested_by;

    // Scope the batch to the server's own guild (the enqueue was already gated on
    // the caller's scope), running at the manager tier per the shared-queue design.
    let guild = match guild_of(&data.kube_client, &data.namespace, server).await {
        Ok(guild) => guild,
        Err(err) => {
            error!(error = ?err, server = %server, "deferred batch: failed to resolve guild scope");
            None
        }
    };
    let scope = guild.clone().map_or(ServerScope::All, ServerScope::Guild);
    let guild_id = guild.as_deref().and_then(|id| id.parse::<u64>().ok());

    let games = game_catalog_list(data);
    let memories = data.memory.render_for_prompt().await;
    let mut messages = vec![
        ChatMessage::system(build_system_prompt(AccessLevel::Manager, &games, &memories)),
        ChatMessage::user(compose_batch_prompt(server, trigger_note, &tasks)),
    ];

    let turn = GaryTurn {
        ctx,
        data,
        channel_id: serenity::ChannelId::new(primary_channel),
        guild: guild_id,
        author_id: serenity::UserId::new(author_id),
        access: AccessLevel::Manager,
        scope,
    };
    let outcome = run_gary_turn(&turn, &mut messages).await;
    deliver_extras(ctx, primary_channel, &tasks, &outcome).await;
    if let Ok(SessionOutcome {
        escalated: true, ..
    }) = &outcome
    {
        warn!(server = %server, "deferred batch hit the round budget; escalating to operators");
        data.notifier
            .notify(&Escalation::RoundBudgetExhausted {
                context: EscalationContext::Deferred {
                    server: server.to_owned(),
                    condition: condition.as_str().to_owned(),
                    guild: guild_id,
                },
                request: compose_batch_prompt(server, trigger_note, &tasks),
                attempts: summarize_attempts(&messages),
                rounds: messages.len(),
            })
            .await;
    }
}

/// Echo the final reply to any channels a task came from *other* than the primary
/// (which `run_gary_turn` already posted to), so a friend who queued from a
/// different channel still hears the outcome. Almost always a no-op (one channel).
async fn deliver_extras(
    ctx: &serenity::Context,
    primary_channel: u64,
    tasks: &[DeferredTask],
    outcome: &anyhow::Result<SessionOutcome>,
) {
    let Ok(SessionOutcome { reply, .. }) = outcome else {
        return;
    };
    let mut seen = std::collections::HashSet::from([primary_channel]);
    for task in tasks {
        if seen.insert(task.channel_id) {
            send_chunks(ctx, serenity::ChannelId::new(task.channel_id), reply).await;
        }
    }
}
