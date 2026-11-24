use std::time::Duration;

use poise::serenity_prelude as serenity;
use tokio::time::Instant;
use tracing::{debug, error, warn};

use super::condition::{Condition, compose_batch_prompt, next_empty_since, wait_key};
use super::task::DeferredTask;
use crate::agent::{ChatMessage, SessionOutcome, TurnEnding, TurnOrigin};
use crate::agones::{
    FsOutcome, ReadyWait, ServerScope, owner_of, supervisor_occupancy, wait_for_ready_within,
};
use crate::discord::gary::{
    ConfirmMode, GaryTurn, build_system_prompt, game_catalog_list, run_gary_turn, send_chunks,
};
use crate::discord::{AccessLevel, Data};
use crate::domain::GuildId;
use crate::events::{Actor, Attribution, Surface, mapping};
use crate::notify::{Escalation, EscalationContext, summarize_attempts};
use crate::prompts;

const STARTUP_CEILING: Duration = Duration::from_mins(20);
const EMPTY_POLL_INTERVAL: Duration = Duration::from_secs(30);
const IDLE_POLL_INTERVAL: Duration = Duration::from_mins(1);
const IDLE_GRACE: Duration = Duration::from_mins(5);
const OCCUPANCY_CEILING: Duration = Duration::from_hours(24);

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

        data.defer.deregister(&server, condition);
        match data.defer.valkey.is_empty(&key).await {
            Ok(true) => return,
            Ok(false) => data.defer.register_silent(&server, condition),
            Err(err) => {
                warn!(error = ?err, server = %server, "deferred watcher: couldn't confirm queue empty; exiting (queue persists in valkey; re-arms only on next process restart)");
                return;
            }
        }
    }
}

async fn wait_for_condition(data: &Data, server: &str, condition: Condition) -> String {
    match condition {
        Condition::Startup => wait_startup(data, server).await,
        Condition::Empty => wait_occupancy(data, server, EMPTY_POLL_INTERVAL, None).await,
        Condition::Idle => wait_occupancy(data, server, IDLE_POLL_INTERVAL, Some(IDLE_GRACE)).await,
    }
}

async fn wait_startup(data: &Data, server: &str) -> String {
    match wait_for_ready_within(
        &data.kube_client,
        &data.control_http,
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
            &data.control_http,
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

    let owner = match owner_of(&data.kube_client, &data.namespace, server).await {
        Ok(owner) => owner.unwrap_or_default(),
        Err(err) => {
            error!(error = ?err, server = %server, "deferred batch: failed to resolve guild scope");
            return;
        }
    };
    let Some(guild) = owner.guild.map(|id| id.as_str().to_owned()) else {
        error!(
            server = %server,
            "deferred batch: server has no owning guild label; refusing to run it unscoped"
        );
        return;
    };
    let scope = ServerScope::Guild(guild.clone());
    let guild = Some(guild);
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
        origin: TurnOrigin::Deferred,
        session: None,
        channel_id: serenity::ChannelId::new(primary_channel),
        guild: guild_id,
        author_id: serenity::UserId::new(author_id),
        access: AccessLevel::Manager,
        scope,
        confirmations: ConfirmMode::Interactive,
    };
    let outcome = run_gary_turn(&turn, &mut messages).await;
    deliver_extras(ctx, primary_channel, &tasks, &outcome).await;
    if let Ok(SessionOutcome {
        ending: TurnEnding::Escalated(reason),
        ..
    }) = &outcome
    {
        warn!(
            server = %server,
            kind = reason.as_str(),
            "deferred batch escalated to operators"
        );
        let escalation = Escalation::from_turn(
            reason,
            EscalationContext::Deferred {
                server: server.to_owned(),
                condition: condition.as_str().to_owned(),
                guild: guild_id,
            },
            compose_batch_prompt(server, trigger_note, &tasks),
            summarize_attempts(&messages),
        );
        data.events
            .record(mapping::escalation_event(
                &escalation,
                &Attribution::new(Actor::Discord(author_id), Surface::Deferred),
                guild_id.map(|id| GuildId::new(id.to_string())),
            ))
            .await;
        data.notifier.notify(&escalation).await;
    }
}

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
