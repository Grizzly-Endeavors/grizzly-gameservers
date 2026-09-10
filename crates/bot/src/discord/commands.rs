use poise::serenity_prelude as serenity;
use serenity::{
    ButtonStyle, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow, CreateButton,
    CreateEmbed, CreateInteractionResponse, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption, MessageId,
};
use tracing::{error, warn};

use super::auth::{require_admin, require_manager, require_operator, visibility_scope};
use super::render::{
    archive_confirm_embed, archive_result_embed, archives_list_embed, archives_unavailable_embed,
    backup_result_embed, backups_disabled_embed, backups_list_embed, create_result_embed,
    delete_archive_confirm_embed, delete_archive_result_embed, destroy_confirm_embed,
    destroy_result_embed, error_embed, guild_required_embed, limit_refusal_embed, neutral_embed,
    recover_result_embed, restore_confirm_embed, restore_result_embed, server_list_embed,
    shutdown_result_embed, start_result_embed, supervisor_result_embed, working_embed,
};
use super::{COMPONENT_TIMEOUT, Context, Error, backup_ctx};
use crate::agent::SessionKey;
use crate::agones::{
    CreateOutcome, ProvisionOutcome, ProvisionSpec, RuntimeState, ServerScope, StartBegin,
    StartOutcome, begin_start, build_instance_name, destroy_instance, guild_usage,
    instance_runtime_state, list_active_servers, list_instance_names, now_entropy,
    provision_instance, shutdown_instance, supervisor_restart, supervisor_start, supervisor_stop,
    validate_world_name, wait_for_instance_ready,
};
use crate::backup::{ArchiveDeleteOutcome, ArtifactSummary, BackupService};
use crate::domain::{GuildId, InstanceName};
use crate::events::{Actor, Attribution, EventKind, ProductEvent, Surface, mapping};
use crate::limits::{LimitChange, format_cores, human_size};
use crate::memory::{ForgetOutcome, Memory};
use crate::store::{ConfigChange, HomeToggle};

/// Maximum options a Discord string select menu accepts. Pickers that can exceed
/// this show only the newest entries (they are built newest-first); older ones
/// fall off the menu.
const MAX_SELECT_OPTIONS: usize = 25;

/// Discord's hard cap on a select-option label (and value). Labels are
/// display-only, so an over-long one is truncated; values that must round-trip
/// (e.g. the /restore pick) carry a short index instead of a long key.
const MAX_SELECT_LABEL: usize = 100;

/// Truncate a select-option label to Discord's 100-character cap, on a char
/// boundary with an ellipsis, so an over-long label doesn't get the whole menu
/// send rejected. Only for display-only labels — never for a value that must
/// round-trip to identify the pick.
fn truncate_option_label(label: &str) -> String {
    if label.chars().count() <= MAX_SELECT_LABEL {
        return label.to_owned();
    }
    let mut truncated: String = label.chars().take(MAX_SELECT_LABEL - 1).collect();
    truncated.push('…');
    truncated
}

/// List the game servers currently running and how to connect to them.
#[poise::command(slash_command)]
pub(crate) async fn servers(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();
    let Some(scope) = command_scope(ctx) else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let reply = ctx
        .send(reply_with(working_embed(
            "Looking up servers",
            "Checking what's running…",
        )))
        .await?;

    match list_active_servers(
        data.kube_client.clone(),
        &data.namespace,
        &data.domain,
        &scope,
    )
    .await
    {
        Ok(summaries) => {
            reply
                .edit(ctx, cleared(server_list_embed(&summaries)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, namespace = %data.namespace, "failed to list game servers");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't reach the cluster right now. Try again in a moment.",
                    )),
                )
                .await?;
        }
    }

    Ok(())
}

/// Spin up a new game server. Pick the game from the menu that appears.
#[poise::command(slash_command, check = "require_manager")]
pub(crate) async fn create(
    ctx: Context<'_>,
    #[description = "Optional name for this world"] name: Option<String>,
) -> Result<(), Error> {
    let data = ctx.data();

    // Validate a supplied name before the game picker so a bad one fails fast
    // rather than after the friend has already picked a game (the check is
    // game-independent — build_instance_name uses the same validation).
    if let Some(supplied) = name.as_deref()
        && let Err(err) = validate_world_name(supplied)
    {
        ctx.send(reply_with(error_embed(&format!("That name won't work: {err}"))).ephemeral(true))
            .await?;
        return Ok(());
    }

    let options: Vec<CreateSelectMenuOption> = data
        .catalog
        .game_ids()
        .take(MAX_SELECT_OPTIONS)
        .map(|id| CreateSelectMenuOption::new(id, id))
        .collect();
    if options.is_empty() {
        ctx.send(
            reply_with(neutral_embed(
                "No games available",
                "There aren't any games in the catalog to launch yet.",
            ))
            .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let menu = CreateSelectMenu::new("create_game", CreateSelectMenuKind::String { options })
        .placeholder("Pick a game to launch");

    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(neutral_embed(
                    "Launch a server",
                    "Pick a game from the menu below.",
                ))
                .components(vec![CreateActionRow::SelectMenu(menu)])
                .ephemeral(true),
        )
        .await?;

    let message = reply.message().await?;
    let Some(interaction) = collect_component(ctx, message.id).await else {
        reply
            .edit(
                ctx,
                cleared(neutral_embed(
                    "Timed out",
                    "No game picked — run `/create` again when you're ready.",
                )),
            )
            .await?;
        return Ok(());
    };
    acknowledge(ctx, &interaction).await?;

    let Some(game) = string_select_value(&interaction) else {
        reply
            .edit(
                ctx,
                cleared(error_embed(
                    "Couldn't read your selection. Try `/create` again.",
                )),
            )
            .await?;
        return Ok(());
    };

    finish_create(ctx, &reply, &game, name.as_deref()).await
}

/// Validate the picked game and provision the instance, writing the result back
/// over the original ephemeral `/create` reply. Split out of [`create`] so the
/// command body stays under the line cap.
async fn finish_create(
    ctx: Context<'_>,
    reply: &poise::ReplyHandle<'_>,
    game: &str,
    name: Option<&str>,
) -> Result<(), Error> {
    let data = ctx.data();
    let Some(entry) = data.catalog.get(game) else {
        reply
            .edit(
                ctx,
                cleared(error_embed(&format!(
                    "'{game}' isn't in the catalog anymore. Try `/create` again."
                ))),
            )
            .await?;
        return Ok(());
    };

    let server = match build_instance_name(game, name, now_entropy()) {
        Ok(server) => server,
        Err(err) => {
            reply
                .edit(
                    ctx,
                    cleared(error_embed(&format!("That name won't work: {err}"))),
                )
                .await?;
            return Ok(());
        }
    };

    reply
        .edit(
            ctx,
            cleared(working_embed(
                &format!("Launching {game}"),
                &format!("Setting up **{server}**…"),
            )),
        )
        .await?;

    let guild_key = guild_id_string(ctx);
    let caps = if guild_key.is_empty() {
        None
    } else {
        Some(data.guild_limits.caps_for(&guild_key).await)
    };
    let provisioned = provision_instance(
        &data.kube_client,
        &data.namespace,
        &data.domain,
        &data.provision_lock,
        ProvisionSpec {
            entry,
            instance: &server,
            guild: &guild_key,
            caps: caps.as_ref(),
        },
    )
    .await;

    record_provision(ctx, &server, &provisioned).await;

    match provisioned {
        Ok(ProvisionOutcome::Provisioned { address }) => {
            await_ready(
                ctx,
                reply,
                &server,
                address,
                EventKind::ServerCreated,
                |address, ready| {
                    create_result_embed(&CreateOutcome::Created { address, ready }, &server)
                },
            )
            .await?;
        }
        Ok(
            refused @ (ProvisionOutcome::AlreadyExists
            | ProvisionOutcome::NameUnavailable
            | ProvisionOutcome::PortsExhausted
            | ProvisionOutcome::GuildLimitReached(_)
            | ProvisionOutcome::ClusterFull(_)),
        ) => {
            reply
                .edit(
                    ctx,
                    cleared(create_result_embed(&provision_to_create(refused), &server)),
                )
                .await?;
        }
        Err(err) => {
            error!(error = ?err, game, %server, "failed to create game server");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't create the server right now. Try again in a moment.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// Map a provision outcome onto its [`CreateOutcome`] for rendering. `Provisioned`
/// is finalized via [`await_ready`] instead, so its arm here only exists to keep
/// the match exhaustive.
fn provision_to_create(outcome: ProvisionOutcome) -> CreateOutcome {
    match outcome {
        ProvisionOutcome::Provisioned { address } => CreateOutcome::Created {
            address,
            ready: false,
        },
        ProvisionOutcome::AlreadyExists => CreateOutcome::AlreadyExists,
        ProvisionOutcome::NameUnavailable => CreateOutcome::NameUnavailable,
        ProvisionOutcome::PortsExhausted => CreateOutcome::PortsExhausted,
        ProvisionOutcome::GuildLimitReached(refusal) => CreateOutcome::GuildLimitReached(refusal),
        ProvisionOutcome::ClusterFull(refusal) => CreateOutcome::ClusterFull(refusal),
    }
}

/// Record a create that never got as far as booting. A successful provision is
/// deliberately silent here: its terminal outcome is the readiness resolution,
/// which [`await_ready`] records once it knows how long the boot took.
async fn record_provision(
    ctx: Context<'_>,
    server: &str,
    provisioned: &Result<ProvisionOutcome, anyhow::Error>,
) {
    match provisioned {
        Ok(ProvisionOutcome::Provisioned { .. }) => {}
        Ok(
            rejected @ (ProvisionOutcome::AlreadyExists
            | ProvisionOutcome::NameUnavailable
            | ProvisionOutcome::PortsExhausted
            | ProvisionOutcome::GuildLimitReached(_)
            | ProvisionOutcome::ClusterFull(_)),
        ) => {
            record_action(
                ctx,
                EventKind::ServerCreated,
                server,
                mapping::provision(rejected),
            )
            .await;
        }
        Err(_) => record_action(ctx, EventKind::ServerCreated, server, mapping::FAILED).await,
    }
}

/// Show the connection address immediately, then poll until the server reports
/// Ready and write the final embed. Shared by `/create` and `/start`, whose
/// readiness wait can run for minutes — `finalize` builds the done-embed from the
/// resolved address and readiness so each caller keeps its own wording.
async fn await_ready(
    ctx: Context<'_>,
    reply: &poise::ReplyHandle<'_>,
    server: &str,
    address: String,
    kind: EventKind,
    finalize: impl FnOnce(String, bool) -> serenity::CreateEmbed,
) -> Result<(), Error> {
    let data = ctx.data();
    reply
        .edit(
            ctx,
            cleared(working_embed(
                &format!("{server} is booting"),
                &format!(
                    "Address: `{address}` — it'll be playable in a minute or two. Hang tight."
                ),
            )),
        )
        .await?;

    // This is where a create or cold start becomes terminal, so it is also where
    // it is recorded — and the only place that can time the boot.
    let started = std::time::Instant::now();
    let booted = address.clone();
    match wait_for_instance_ready(&data.kube_client, &data.namespace, server).await {
        Ok(ready) => {
            reply.edit(ctx, cleared(finalize(address, ready))).await?;
            let elapsed = ready.then(|| started.elapsed());
            record_boot(
                ctx,
                kind,
                server,
                mapping::readiness(ready),
                Some(mapping::boot_details(Some(&booted), elapsed)),
            )
            .await;
        }
        Err(err) => {
            error!(error = ?err, %server, "failed to wait for readiness");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "I lost track of whether the server came online. \
                         Check `/servers` in a minute.",
                    )),
                )
                .await?;
            // The objects exist; only the answer to "is it up yet" is missing.
            record_boot(
                ctx,
                kind,
                server,
                mapping::readiness(false),
                Some(mapping::boot_details(Some(&booted), None)),
            )
            .await;
        }
    }
    Ok(())
}

/// Pause a server — keep it warm so /start resumes in seconds. World is saved.
#[poise::command(slash_command, check = "require_manager")]
pub(crate) async fn stop(
    ctx: Context<'_>,
    #[description = "Which server to pause"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    let reply = ctx
        .send(reply_with(working_embed(
            &format!("Pausing {server}"),
            "Saving the world and pausing…",
        )))
        .await?;
    match supervisor_stop(
        &data.kube_client,
        &data.control_http,
        &data.namespace,
        &server,
        data.control_port,
    )
    .await
    {
        Ok(outcome) => {
            reply
                .edit(ctx, cleared(supervisor_result_embed(&outcome, &server)))
                .await?;
            record_action(
                ctx,
                EventKind::ServerStopped,
                &server,
                mapping::supervisor(&outcome),
            )
            .await;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to pause game server");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't pause the server right now. Try again in a moment.",
                    )),
                )
                .await?;
            record_action(ctx, EventKind::ServerStopped, &server, mapping::FAILED).await;
        }
    }
    Ok(())
}

/// Fully shut a server down to free the slot, keeping the world so /start can bring it back.
#[poise::command(slash_command, check = "require_manager")]
pub(crate) async fn shutdown(
    ctx: Context<'_>,
    #[description = "Which server to shut down"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    let reply = ctx
        .send(reply_with(working_embed(
            &format!("Stopping {server}"),
            "Shutting it down and saving the world…",
        )))
        .await?;
    match shutdown_instance(&data.kube_client, &data.namespace, &server).await {
        Ok(outcome) => {
            reply
                .edit(ctx, cleared(shutdown_result_embed(&outcome, &server)))
                .await?;
            record_action(
                ctx,
                EventKind::ServerShutdown,
                &server,
                mapping::shutdown(&outcome),
            )
            .await;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to shut down game server");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't shut the server down right now. Try again in a moment.",
                    )),
                )
                .await?;
            record_action(ctx, EventKind::ServerShutdown, &server, mapping::FAILED).await;
        }
    }
    Ok(())
}

/// Restart a running server in place — a quick reboot that keeps the world and address.
#[poise::command(slash_command, check = "require_manager")]
pub(crate) async fn restart(
    ctx: Context<'_>,
    #[description = "Which server to restart"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    let reply = ctx
        .send(reply_with(working_embed(
            &format!("Restarting {server}"),
            "Bouncing it…",
        )))
        .await?;
    match supervisor_restart(
        &data.kube_client,
        &data.control_http,
        &data.namespace,
        &server,
        data.control_port,
    )
    .await
    {
        Ok(outcome) => {
            reply
                .edit(ctx, cleared(supervisor_result_embed(&outcome, &server)))
                .await?;
            record_action(
                ctx,
                EventKind::ServerRestarted,
                &server,
                mapping::supervisor(&outcome),
            )
            .await;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to restart game server");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't restart the server right now. Try again in a moment.",
                    )),
                )
                .await?;
            record_action(ctx, EventKind::ServerRestarted, &server, mapping::FAILED).await;
        }
    }
    Ok(())
}

/// Start a server: a paused one resumes in seconds, a shut-down one is rescheduled (slower).
#[poise::command(slash_command, check = "require_manager")]
pub(crate) async fn start(
    ctx: Context<'_>,
    #[description = "Which server to start"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    let reply = ctx
        .send(reply_with(working_embed(
            &format!("Starting {server}"),
            "Waking it up…",
        )))
        .await?;

    match instance_runtime_state(&data.kube_client, &data.namespace, &server).await {
        Ok(RuntimeState::PodUp) => {
            // Warm path: the pod is alive, just resume the process in place.
            match supervisor_start(
                &data.kube_client,
                &data.control_http,
                &data.namespace,
                &server,
                data.control_port,
            )
            .await
            {
                Ok(outcome) => {
                    reply
                        .edit(ctx, cleared(supervisor_result_embed(&outcome, &server)))
                        .await?;
                    record_action(
                        ctx,
                        EventKind::ServerStarted,
                        &server,
                        mapping::supervisor(&outcome),
                    )
                    .await;
                }
                Err(err) => {
                    error!(error = ?err, server = %server, "failed to resume game server");
                    reply
                        .edit(
                            ctx,
                            cleared(error_embed(
                                "Couldn't resume the server right now. Try again in a moment.",
                            )),
                        )
                        .await?;
                    record_action(ctx, EventKind::ServerStarted, &server, mapping::FAILED).await;
                }
            }
        }
        Ok(RuntimeState::Down) => start_cold(ctx, &reply, &server).await?,
        // Neither path is available mid-teardown: there's no live pod to resume
        // and the outgoing GameServer still owns the name. Say so plainly rather
        // than dialling a pod that is on its way out.
        Ok(RuntimeState::Terminating) => {
            reply
                .edit(
                    ctx,
                    cleared(start_result_embed(&StartOutcome::StillStopping, &server)),
                )
                .await?;
        }
        Ok(RuntimeState::Absent) => {
            reply
                .edit(
                    ctx,
                    cleared(start_result_embed(&StartOutcome::NotFound, &server)),
                )
                .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to resolve server state");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't reach the cluster right now. Try again in a moment.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// Cold start: recreate a shut-down instance's `GameServer` and wait for it to come
/// up. Split out of [`start`] so the command body stays under the line cap.
async fn start_cold(
    ctx: Context<'_>,
    reply: &poise::ReplyHandle<'_>,
    server: &str,
) -> Result<(), Error> {
    let data = ctx.data();
    // One hold of the provision lock covers the cap gate and the GameServer
    // create, so a concurrent create can't add load in between (ADR-022). The
    // returns inside the block release it before any long wait.
    let begin = {
        let _guard = data.provision_lock.lock().await;
        match super::cold_start_refusal(data, server).await {
            Ok(None) => {}
            Ok(Some(refusal)) => {
                record_action(
                    ctx,
                    EventKind::ServerStarted,
                    server,
                    mapping::limit_refusal(refusal),
                )
                .await;
                reply
                    .edit(ctx, cleared(limit_refusal_embed(refusal)))
                    .await?;
                return Ok(());
            }
            Err(err) => {
                error!(error = ?err, server = %server, "failed to check guild caps for cold start");
                reply
                    .edit(
                        ctx,
                        cleared(error_embed(
                            "Couldn't start the server right now. Try again in a moment.",
                        )),
                    )
                    .await?;
                record_action(ctx, EventKind::ServerStarted, server, mapping::FAILED).await;
                return Ok(());
            }
        }
        match begin_start(
            &data.kube_client,
            &data.namespace,
            &data.domain,
            &data.catalog,
            server,
            data.sizing.get(server).await,
        )
        .await
        {
            Ok(begin) => begin,
            Err(err) => {
                error!(error = ?err, server = %server, "failed to start game server");
                reply
                    .edit(
                        ctx,
                        cleared(error_embed(
                            "Couldn't start the server right now. Try again in a moment.",
                        )),
                    )
                    .await?;
                record_action(ctx, EventKind::ServerStarted, server, mapping::FAILED).await;
                return Ok(());
            }
        }
    };

    if let StartBegin::Starting { address } = begin {
        // Terminal outcome and timing come from the readiness wait, exactly as
        // they do for a create — a cold start is the same shape.
        await_ready(
            ctx,
            reply,
            server,
            address,
            EventKind::ServerStarted,
            |address, ready| start_result_embed(&StartOutcome::Started { address, ready }, server),
        )
        .await?;
    } else {
        let outcome = mapping::start_begin(&begin);
        reply
            .edit(
                ctx,
                cleared(start_result_embed(&begin_to_outcome(begin), server)),
            )
            .await?;
        record_action(ctx, EventKind::ServerStarted, server, outcome).await;
    }
    Ok(())
}

/// Map the non-`Starting` start outcomes onto their [`StartOutcome`] for
/// rendering. `Starting` is excluded — it carries an address and is finalized via
/// [`await_ready`] only after readiness is known.
fn begin_to_outcome(begin: StartBegin) -> StartOutcome {
    match begin {
        StartBegin::Starting { address } => StartOutcome::Started {
            address,
            ready: false,
        },
        StartBegin::AlreadyRunning => StartOutcome::AlreadyRunning,
        StartBegin::NotFound => StartOutcome::NotFound,
        StartBegin::NotManaged => StartOutcome::NotManaged,
        StartBegin::UnknownGame(game) => StartOutcome::UnknownGame(game),
        StartBegin::ClusterFull(refusal) => StartOutcome::ClusterFull(refusal),
    }
}

/// Permanently destroy a server and delete its world. Cannot be undone.
#[poise::command(slash_command, check = "require_admin")]
pub(crate) async fn destroy(
    ctx: Context<'_>,
    #[description = "Which server to destroy (this permanently deletes its world)"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();

    let Some(reply) =
        confirm_action(ctx, destroy_confirm_embed(&server), "destroy_confirm").await?
    else {
        return Ok(());
    };

    reply
        .edit(
            ctx,
            cleared(working_embed(
                &format!("Deleting {server}"),
                "Destroying the server and its world…",
            )),
        )
        .await?;

    match destroy_instance(&data.kube_client, &data.namespace, &server).await {
        Ok(outcome) => {
            // Same reason as Gary's destroy tool: names get reused, and a stale
            // override would size an unrelated future world.
            if let Err(err) = data.sizing.forget(&server).await {
                error!(error = ?err, server = %server, "failed to clear the destroyed server's resource override");
            }
            reply
                .edit(ctx, cleared(destroy_result_embed(&outcome, &server)))
                .await?;
            record_action(
                ctx,
                EventKind::ServerDestroyed,
                &server,
                mapping::destroy(&outcome),
            )
            .await;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to destroy game server");
            record_action(ctx, EventKind::ServerDestroyed, &server, mapping::FAILED).await;
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't destroy the server right now. Try again in a moment.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// Permanently delete a server's archives from cold storage. Cannot be undone.
///
/// Admin-only and separate from `/destroy` on purpose: destroying a server leaves
/// its archive standing as the way back, so this is the only command that makes a
/// world unrecoverable.
#[poise::command(slash_command, rename = "delete-archive", check = "require_admin")]
pub(crate) async fn delete_archive(
    ctx: Context<'_>,
    #[description = "Which archive to delete (this permanently deletes that saved world)"]
    #[autocomplete = "autocomplete_archive"]
    name: String,
) -> Result<(), Error> {
    let data = ctx.data();
    let Some(service) = data.backup.clone() else {
        ctx.send(reply_with(backups_disabled_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    if !service.archives_enabled() {
        ctx.send(reply_with(archives_unavailable_embed()).ephemeral(true))
            .await?;
        return Ok(());
    }
    let Some(scope) = command_scope(ctx) else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };

    // Priced before the confirmation: `/archives` lists only the newest copy per
    // name, so the entry the admin is looking at routinely understates what a
    // delete removes.
    let (generations, freed) = match service.archive_footprint(&scope, &name).await {
        Ok(Some(footprint)) => footprint,
        Ok(None) => {
            ctx.send(
                reply_with(delete_archive_result_embed(&ArchiveDeleteOutcome::NotFound))
                    .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
        Err(err) => {
            error!(error = ?err, %name, "failed to read archive footprint");
            ctx.send(
                reply_with(error_embed(
                    "Couldn't look that archive up right now. Try again in a moment.",
                ))
                .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
    };

    let Some(reply) = confirm_action(
        ctx,
        delete_archive_confirm_embed(&name, generations, &human_size(freed)),
        "delete_archive_confirm",
    )
    .await?
    else {
        return Ok(());
    };

    reply
        .edit(
            ctx,
            cleared(working_embed(
                &format!("Deleting the {name} archive"),
                "Clearing it out of cold storage…",
            )),
        )
        .await?;

    match service
        .delete_archives(&backup_ctx(data), &scope, &name, &attribution(ctx))
        .await
    {
        Ok(outcome) => {
            if let Some(reason) = outcome.reason() {
                error!(reason, %name, "archive deletion did not complete");
            }
            reply
                .edit(ctx, cleared(delete_archive_result_embed(&outcome)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, %name, "failed to delete archive");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't delete that archive right now. Try again in a moment.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// Complete against archived names rather than live servers — an archived world
/// has no instance, so the server autocomplete would offer nothing.
async fn autocomplete_archive(ctx: Context<'_>, partial: &str) -> impl Iterator<Item = String> {
    let data = ctx.data();
    let needle = partial.to_owned();
    let names = match (data.backup.clone(), command_scope(ctx)) {
        (Some(service), Some(scope)) => match service.list_archives(&scope).await {
            Ok(list) => list.into_iter().map(|artifact| artifact.name).collect(),
            Err(err) => {
                error!(error = ?err, "failed to list archives for autocomplete");
                Vec::new()
            }
        },
        _ => Vec::new(),
    };
    names
        .into_iter()
        .filter(move |name: &String| name.starts_with(&needle))
}

async fn autocomplete_server(ctx: Context<'_>, partial: &str) -> impl Iterator<Item = String> {
    let data = ctx.data();
    let needle = partial.to_owned();
    let names = match command_scope(ctx) {
        Some(scope) => {
            match list_instance_names(&data.kube_client, &data.namespace, &scope).await {
                Ok(names) => names,
                Err(err) => {
                    error!(error = ?err, "failed to list instances for autocomplete");
                    Vec::new()
                }
            }
        }
        None => Vec::new(),
    };
    names
        .into_iter()
        .filter(move |name| name.starts_with(&needle))
}

/// Toggle whether Gary answers in this channel without being @mentioned.
#[poise::command(slash_command, rename = "gary-home", check = "require_admin")]
pub(crate) async fn gary_home(ctx: Context<'_>) -> Result<(), Error> {
    // DMs are always no-mention, so there's nothing to toggle there.
    let Some(guild_id) = ctx.guild_id() else {
        ctx.send(
            reply_with(neutral_embed(
                "Already listening",
                "This is a DM — I already answer here without being @mentioned.",
            ))
            .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    let embed = match ctx
        .data()
        .home_channels
        .toggle(ctx.channel_id().get(), guild_id.get())
        .await
    {
        Ok(HomeToggle::Added) => neutral_embed(
            "This is now Gary's channel",
            "I'll answer here without being @mentioned — separate from which servers exist, \
             which are shared across the whole Discord server. Run `/gary-home` again to turn it off.",
        ),
        Ok(HomeToggle::Removed) => neutral_embed(
            "Back to mentions only",
            "I'll only answer here when you @mention me now.",
        ),
        Ok(HomeToggle::Unavailable) => error_embed(
            "I couldn't set that right now — home-channel settings are unavailable. \
             You can still @mention me. Try again later.",
        ),
        Err(err) => {
            error!(error = ?err, channel = ctx.channel_id().get(), "failed to toggle home channel");
            error_embed("Something went wrong saving that. Try again in a moment.")
        }
    };
    ctx.send(reply_with(embed).ephemeral(true)).await?;
    Ok(())
}

/// Start fresh with Gary, forgetting the recent back-and-forth.
#[poise::command(slash_command, rename = "new-session")]
pub(crate) async fn new_session(ctx: Context<'_>) -> Result<(), Error> {
    // The conversation belongs to the channel, so this clears it for everyone in
    // it — which is what "start fresh" means when the back-and-forth was shared.
    ctx.data()
        .sessions
        .clear(SessionKey::Channel(ctx.channel_id().get()));
    ctx.send(
        reply_with(neutral_embed(
            "Fresh start",
            "Okay, starting fresh — I've cleared what we were just talking about in this channel.",
        ))
        .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Stop the Gary turn running in this channel.
#[poise::command(slash_command, rename = "gary-stop")]
pub(crate) async fn gary_stop(ctx: Context<'_>) -> Result<(), Error> {
    // Ungated, like /new-session and for the same reason: the conversation
    // belongs to the channel, so anyone who can add to it can end a turn against
    // it. The cost of a stop nobody wanted is that somebody asks again.
    let stopped = ctx.data().interrupts.cancel(ctx.channel_id().get());
    if stopped {
        let data = ctx.data();
        data.events
            .record(mapping::turn_cancelled_event(
                &attribution(ctx),
                ctx.guild_id()
                    .map(|guild| GuildId::new(guild.get().to_string())),
            ))
            .await;
    }
    let embed = if stopped {
        neutral_embed(
            "Stopping",
            "I'll stop as soon as I can. If I'm part-way through changing something on a              server I'll finish that step first rather than leave it half-done — nothing              new will start after it.",
        )
    } else {
        neutral_embed(
            "Nothing to stop",
            "I'm not working on anything in this channel right now.",
        )
    };
    // Not ephemeral: the conversation is shared, so a stop is something everyone
    // in the channel needs to see — otherwise Gary going quiet mid-answer looks
    // like a fault to whoever didn't run the command.
    ctx.send(reply_with(embed)).await?;
    Ok(())
}

/// Review and prune the durable facts Gary has learned about each game.
#[poise::command(
    slash_command,
    rename = "gary-memory",
    subcommands("gary_memory_list", "gary_memory_forget")
)]
pub(crate) async fn gary_memory(ctx: Context<'_>) -> Result<(), Error> {
    // Discord always resolves a subcommand, so this parent body is defensive.
    ctx.send(
        reply_with(neutral_embed(
            "Pick an option",
            "Use `/gary-memory list` to see what Gary has learned, or `/gary-memory forget` to \
             delete a fact by its id.",
        ))
        .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Show every durable fact Gary has saved, grouped by game.
#[poise::command(slash_command, rename = "list", check = "require_admin")]
async fn gary_memory_list(ctx: Context<'_>) -> Result<(), Error> {
    let memories = ctx.data().memory.all().await;
    let embed = if memories.is_empty() {
        neutral_embed(
            "Gary's memory",
            "Gary hasn't saved anything yet. He records durable facts about a game as he learns \
             them while running it.",
        )
    } else {
        neutral_embed("Gary's memory", &format_memory_list(&memories))
    };
    ctx.send(reply_with(embed).ephemeral(true)).await?;
    Ok(())
}

/// Delete one of Gary's saved facts by its id (from `/gary-memory list`).
#[poise::command(slash_command, rename = "forget", check = "require_admin")]
async fn gary_memory_forget(
    ctx: Context<'_>,
    #[description = "The id of the fact to delete (the number shown by /gary-memory list)"] id: i64,
) -> Result<(), Error> {
    let embed = match ctx.data().memory.forget(id).await {
        Ok(ForgetOutcome::Forgotten) => neutral_embed("Forgotten", &format!("Deleted fact #{id}.")),
        Ok(ForgetOutcome::NotFound) => error_embed(&format!(
            "There's no fact #{id} — check `/gary-memory list`."
        )),
        Ok(ForgetOutcome::Unavailable) => {
            error_embed("Gary's long-term memory is offline right now. Try again later.")
        }
        Err(err) => {
            error!(error = ?err, id, "failed to forget gary memory");
            error_embed("Something went wrong deleting that. Try again in a moment.")
        }
    };
    ctx.send(reply_with(embed).ephemeral(true)).await?;
    Ok(())
}

/// Gary's saved facts grouped by scope (game id or `general`), each with its id
/// so an admin can `/gary-memory forget` it.
fn format_memory_list(memories: &[Memory]) -> String {
    let mut by_scope: std::collections::BTreeMap<&str, Vec<&Memory>> =
        std::collections::BTreeMap::new();
    for memory in memories {
        by_scope
            .entry(memory.scope.as_str())
            .or_default()
            .push(memory);
    }
    by_scope
        .into_iter()
        .map(|(scope, facts)| {
            let lines = facts
                .iter()
                .map(|memory| format!("`#{}` {}", memory.id, memory.content))
                .collect::<Vec<_>>()
                .join("\n");
            format!("**{scope}**\n{lines}")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Configure who can administer and who can operate this Discord server's game
/// servers.
#[poise::command(
    slash_command,
    subcommands(
        "config_view",
        "config_admin_role",
        "config_admin_user",
        "config_manager_role",
        "config_manager_user",
        "config_limits"
    )
)]
pub(crate) async fn config(ctx: Context<'_>) -> Result<(), Error> {
    // Discord always resolves a subcommand, so this parent body is defensive.
    config_group_hint(ctx).await
}

/// The reply for a `/config` group invoked without a subcommand — defensive, as
/// Discord normally forces a subcommand choice.
async fn config_group_hint(ctx: Context<'_>) -> Result<(), Error> {
    ctx.send(
        reply_with(neutral_embed(
            "Pick an option",
            "Use `/config view`, `/config admin-role`, `/config admin-user`, \
             `/config manager-role`, `/config manager-user`, or `/config limits`.",
        ))
        .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Show this Discord server's admins and managers (roles and users) and config
/// status.
#[poise::command(slash_command, rename = "view", check = "require_admin")]
async fn config_view(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let config = &ctx.data().guild_config;
    let admins = config.admins(guild.get()).await;
    let managers = config.managers(guild.get()).await;
    let admin_roles = mention_list(&admins.roles, |id| format!("<@&{id}>"));
    let admin_users = mention_list(&admins.users, |id| format!("<@{id}>"));
    let manager_roles = mention_list(&managers.roles, |id| format!("<@&{id}>"));
    let manager_users = mention_list(&managers.users, |id| format!("<@{id}>"));
    let mut body = format!(
        "**Admin roles:** {admin_roles}\n**Admin users:** {admin_users}\n\
         **Manager roles:** {manager_roles}\n**Manager users:** {manager_users}\n\n\
         Admins can do everything, including deleting servers and changing this config. \
         Managers can run servers day-to-day — create, start, stop, restart, and back up — \
         but can't delete a server or change who has access. The Discord server's owner and \
         the bot operator are always admins. Game servers are shared across the whole Discord \
         server, not per-channel.",
    );
    if !config.is_available() {
        body.push_str(
            "\n\n⚠️ Access settings are unavailable right now, so only the owner and operator \
             are recognized until they're back.",
        );
    }
    ctx.send(reply_with(neutral_embed("Server access", &body)).ephemeral(true))
        .await?;
    Ok(())
}

/// Manage which roles can administer this server's game servers (full control).
#[poise::command(
    slash_command,
    rename = "admin-role",
    subcommands("config_admin_role_add", "config_admin_role_remove")
)]
async fn config_admin_role(ctx: Context<'_>) -> Result<(), Error> {
    config_group_hint(ctx).await
}

/// Let a role administer this Discord server's game servers (full control).
#[poise::command(slash_command, rename = "add", check = "require_admin")]
async fn config_admin_role_add(
    ctx: Context<'_>,
    #[description = "Role to grant server-admin"] role: serenity::Role,
) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let change = ctx
        .data()
        .guild_config
        .add_admin_role(guild.get(), role.id.get())
        .await;
    respond_grant_change(
        ctx,
        change,
        GrantRole::Admin,
        &format!("The **{}** role", role.name),
    )
    .await
}

/// Stop letting a role administer this Discord server's game servers.
#[poise::command(slash_command, rename = "remove", check = "require_admin")]
async fn config_admin_role_remove(
    ctx: Context<'_>,
    #[description = "Role to revoke server-admin from"] role: serenity::Role,
) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let change = ctx
        .data()
        .guild_config
        .remove_admin_role(guild.get(), role.id.get())
        .await;
    respond_grant_change(
        ctx,
        change,
        GrantRole::Admin,
        &format!("The **{}** role", role.name),
    )
    .await
}

/// Manage which people can administer this server's game servers (full control).
#[poise::command(
    slash_command,
    rename = "admin-user",
    subcommands("config_admin_user_add", "config_admin_user_remove")
)]
async fn config_admin_user(ctx: Context<'_>) -> Result<(), Error> {
    config_group_hint(ctx).await
}

/// Let a person administer this Discord server's game servers (full control).
#[poise::command(slash_command, rename = "add", check = "require_admin")]
async fn config_admin_user_add(
    ctx: Context<'_>,
    #[description = "Person to grant server-admin"] user: serenity::User,
) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let change = ctx
        .data()
        .guild_config
        .add_admin_user(guild.get(), user.id.get())
        .await;
    respond_grant_change(ctx, change, GrantRole::Admin, &format!("**{}**", user.name)).await
}

/// Stop letting a person administer this Discord server's game servers.
#[poise::command(slash_command, rename = "remove", check = "require_admin")]
async fn config_admin_user_remove(
    ctx: Context<'_>,
    #[description = "Person to revoke server-admin from"] user: serenity::User,
) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let change = ctx
        .data()
        .guild_config
        .remove_admin_user(guild.get(), user.id.get())
        .await;
    respond_grant_change(ctx, change, GrantRole::Admin, &format!("**{}**", user.name)).await
}

/// Manage which roles can operate this server's game servers day-to-day.
#[poise::command(
    slash_command,
    rename = "manager-role",
    subcommands("config_manager_role_add", "config_manager_role_remove")
)]
async fn config_manager_role(ctx: Context<'_>) -> Result<(), Error> {
    config_group_hint(ctx).await
}

/// Let a role operate this Discord server's game servers day-to-day.
#[poise::command(slash_command, rename = "add", check = "require_admin")]
async fn config_manager_role_add(
    ctx: Context<'_>,
    #[description = "Role to grant server-manager"] role: serenity::Role,
) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let change = ctx
        .data()
        .guild_config
        .add_manager_role(guild.get(), role.id.get())
        .await;
    respond_grant_change(
        ctx,
        change,
        GrantRole::Manager,
        &format!("The **{}** role", role.name),
    )
    .await
}

/// Stop letting a role operate this Discord server's game servers.
#[poise::command(slash_command, rename = "remove", check = "require_admin")]
async fn config_manager_role_remove(
    ctx: Context<'_>,
    #[description = "Role to revoke server-manager from"] role: serenity::Role,
) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let change = ctx
        .data()
        .guild_config
        .remove_manager_role(guild.get(), role.id.get())
        .await;
    respond_grant_change(
        ctx,
        change,
        GrantRole::Manager,
        &format!("The **{}** role", role.name),
    )
    .await
}

/// Manage which people can operate this server's game servers day-to-day.
#[poise::command(
    slash_command,
    rename = "manager-user",
    subcommands("config_manager_user_add", "config_manager_user_remove")
)]
async fn config_manager_user(ctx: Context<'_>) -> Result<(), Error> {
    config_group_hint(ctx).await
}

/// Let a person operate this Discord server's game servers day-to-day.
#[poise::command(slash_command, rename = "add", check = "require_admin")]
async fn config_manager_user_add(
    ctx: Context<'_>,
    #[description = "Person to grant server-manager"] user: serenity::User,
) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let change = ctx
        .data()
        .guild_config
        .add_manager_user(guild.get(), user.id.get())
        .await;
    respond_grant_change(
        ctx,
        change,
        GrantRole::Manager,
        &format!("**{}**", user.name),
    )
    .await
}

/// Stop letting a person operate this Discord server's game servers.
#[poise::command(slash_command, rename = "remove", check = "require_admin")]
async fn config_manager_user_remove(
    ctx: Context<'_>,
    #[description = "Person to revoke server-manager from"] user: serenity::User,
) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let change = ctx
        .data()
        .guild_config
        .remove_manager_user(guild.get(), user.id.get())
        .await;
    respond_grant_change(
        ctx,
        change,
        GrantRole::Manager,
        &format!("**{}**", user.name),
    )
    .await
}

/// This Discord server's resource caps: view for admins, adjust for the bot
/// operator.
#[poise::command(
    slash_command,
    rename = "limits",
    subcommands(
        "config_limits_view",
        "config_limits_max_servers",
        "config_limits_max_cpu",
        "config_limits_max_memory"
    )
)]
async fn config_limits(ctx: Context<'_>) -> Result<(), Error> {
    config_group_hint(ctx).await
}

/// Show this Discord server's resource caps and what's in use against them.
#[poise::command(slash_command, rename = "view", check = "require_admin")]
async fn config_limits_view(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let data = ctx.data();
    let guild_key = guild.get().to_string();
    let caps = data.guild_limits.caps_for(&guild_key).await;
    let usage = guild_usage(&data.kube_client, &data.namespace, &guild_key, None).await;
    let mut body = match usage {
        Ok(usage) => format!(
            "**Servers:** {} of {}\n**CPU:** {} of {} in use by live servers\n\
             **Memory:** {} of {} in use by live servers",
            usage.server_count,
            caps.servers,
            format_cores(usage.live_cpu_limit_millicores),
            format_cores(caps.cpu_limit_millicores),
            human_size(usage.live_mem_limit_bytes.unsigned_abs()),
            human_size(caps.mem_limit_bytes.unsigned_abs()),
        ),
        Err(err) => {
            error!(error = ?err, guild = %guild_key, "failed to measure guild usage for /config limits");
            format!(
                "**Servers:** up to {}\n**CPU:** up to {}\n**Memory:** up to {}\n\n\
                 ⚠️ I couldn't measure current usage just now, so only the caps are shown.",
                caps.servers,
                format_cores(caps.cpu_limit_millicores),
                human_size(caps.mem_limit_bytes.unsigned_abs()),
            )
        }
    };
    body.push_str(
        "\n\nCPU and memory count each live server's configured maximum, so stopped \
         servers use none. Ask the bot operator if this community needs more room.",
    );
    if data.guild_limits.overrides_for(&guild_key).await.is_some() {
        body.push_str("\n\nSome of these caps are set specifically for this community.");
    }
    ctx.send(reply_with(neutral_embed("Server limits", &body)).ephemeral(true))
        .await?;
    Ok(())
}

/// Set how many servers this Discord server may have, or `default` to clear.
#[poise::command(slash_command, rename = "max-servers", check = "require_operator")]
async fn config_limits_max_servers(
    ctx: Context<'_>,
    #[description = "A count like 8, or \"default\""] value: String,
) -> Result<(), Error> {
    let change = match parse_limit_value(&value, |raw| {
        raw.parse::<u32>().ok().filter(|&count| count > 0)
    }) {
        Ok(change) => LimitChange::Servers(change),
        Err(()) => return limit_value_rejected(ctx, &value, "a positive count like `8`").await,
    };
    apply_limit_change(ctx, change).await
}

/// Set this Discord server's total CPU allowance, or `default` to clear.
#[poise::command(slash_command, rename = "max-cpu", check = "require_operator")]
async fn config_limits_max_cpu(
    ctx: Context<'_>,
    #[description = "A CPU amount like 32 or 32000m, or \"default\""] value: String,
) -> Result<(), Error> {
    let change = match parse_limit_value(&value, |raw| {
        crate::events::parse_cpu_millicores(raw).filter(|&millicores| millicores > 0)
    }) {
        Ok(change) => LimitChange::CpuLimitMillicores(change),
        Err(()) => {
            return limit_value_rejected(ctx, &value, "a CPU amount like `32` or `32000m`").await;
        }
    };
    apply_limit_change(ctx, change).await
}

/// Set this Discord server's total memory allowance, or `default` to clear.
#[poise::command(slash_command, rename = "max-memory", check = "require_operator")]
async fn config_limits_max_memory(
    ctx: Context<'_>,
    #[description = "A memory amount like 64Gi, or \"default\""] value: String,
) -> Result<(), Error> {
    let change = match parse_limit_value(&value, |raw| {
        crate::events::parse_bytes(raw).filter(|&bytes| bytes > 0)
    }) {
        Ok(change) => LimitChange::MemLimitBytes(change),
        Err(()) => return limit_value_rejected(ctx, &value, "a memory amount like `64Gi`").await,
    };
    apply_limit_change(ctx, change).await
}

/// Parse a limit argument: the literal `default` clears the override, anything
/// else must survive `parse`.
fn parse_limit_value<T>(raw: &str, parse: impl Fn(&str) -> Option<T>) -> Result<Option<T>, ()> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("default") {
        return Ok(None);
    }
    parse(trimmed).map(Some).ok_or(())
}

async fn limit_value_rejected(ctx: Context<'_>, raw: &str, expected: &str) -> Result<(), Error> {
    ctx.send(
        reply_with(error_embed(&format!(
            "`{raw}` isn't something I can use — give me {expected}, or `default`."
        )))
        .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Persist one cap adjustment and confirm with the guild's new effective caps.
async fn apply_limit_change(ctx: Context<'_>, change: LimitChange) -> Result<(), Error> {
    let Some(guild) = ctx.guild_id() else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let data = ctx.data();
    let guild_key = guild.get().to_string();
    if !data.guild_limits.is_available() {
        ctx.send(
            reply_with(error_embed(
                "I can't record limit changes right now — my settings storage is offline. \
                 The current caps still apply; try again later.",
            ))
            .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let actor = ctx.author().id.get().to_string();
    if let Err(err) = data.guild_limits.apply(&guild_key, change, &actor).await {
        error!(error = ?err, guild = %guild_key, "failed to record a guild limit change");
        ctx.send(
            reply_with(error_embed(
                "Something went wrong recording the change, so nothing was altered. \
                 Try again later.",
            ))
            .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let caps = data.guild_limits.caps_for(&guild_key).await;
    ctx.send(
        reply_with(neutral_embed(
            "Limits updated",
            &format!(
                "This community's caps are now **{} servers**, **{}**, and **{}**.",
                caps.servers,
                format_cores(caps.cpu_limit_millicores),
                human_size(caps.mem_limit_bytes.unsigned_abs()),
            ),
        ))
        .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Join a set of ids into a Discord mention list, or an em dash when empty.
fn mention_list(ids: &std::collections::HashSet<u64>, mention: impl Fn(u64) -> String) -> String {
    if ids.is_empty() {
        return "—".to_owned();
    }
    ids.iter()
        .map(|id| mention(*id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Which access tier a `/config` grant mutation targets — picks the wording of
/// the confirmation reply.
#[derive(Clone, Copy)]
enum GrantRole {
    Admin,
    Manager,
}

impl GrantRole {
    /// What the grant lets the subject do, for the "Added" reply.
    fn can_now(self) -> &'static str {
        match self {
            Self::Admin => "administer this Discord server's game servers (full control)",
            Self::Manager => "run this Discord server's game servers day-to-day",
        }
    }

    /// The revoked capability, for the "Removed" reply.
    fn can_no_longer(self) -> &'static str {
        match self {
            Self::Admin => "administer this Discord server's game servers",
            Self::Manager => "run this Discord server's game servers",
        }
    }
}

/// The reply for a `/config` grant mutation, mapping the store outcome to plain
/// text tailored to the tier being changed.
async fn respond_grant_change(
    ctx: Context<'_>,
    change: Result<ConfigChange, anyhow::Error>,
    role: GrantRole,
    subject: &str,
) -> Result<(), Error> {
    let embed = match change {
        Ok(ConfigChange::Added) => {
            neutral_embed("Added", &format!("{subject} can now {}.", role.can_now()))
        }
        Ok(ConfigChange::Removed) => neutral_embed(
            "Removed",
            &format!("{subject} can no longer {}.", role.can_no_longer()),
        ),
        Ok(ConfigChange::Unchanged) => {
            neutral_embed("No change", &format!("{subject} was already set that way."))
        }
        Ok(ConfigChange::Unavailable) => error_embed(
            "I couldn't save that right now — access settings are unavailable. Try again later.",
        ),
        Err(err) => {
            error!(error = ?err, "failed to update guild access config");
            error_embed("Something went wrong saving that. Try again in a moment.")
        }
    };
    ctx.send(reply_with(embed).ephemeral(true)).await?;
    Ok(())
}

/// Take a backup of a running server's world to durable storage right now.
#[poise::command(slash_command, check = "require_manager")]
pub(crate) async fn backup(
    ctx: Context<'_>,
    #[description = "Which server to back up"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    let Some(service) = data.backup.clone() else {
        ctx.send(reply_with(backups_disabled_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let reply = ctx
        .send(reply_with(working_embed(
            &format!("Backing up {server}"),
            "Saving a snapshot of the world…",
        )))
        .await?;
    match service
        .backup_instance(&backup_ctx(data), &server, &attribution(ctx))
        .await
    {
        Ok(outcome) => {
            if let Some(reason) = outcome.reason() {
                warn!(reason, server = %server, "backup did not succeed");
            }
            reply
                .edit(ctx, cleared(backup_result_embed(&outcome, &server)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to back up game server");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't back up the server right now. Try again in a moment.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// List a server's saved backups, newest first.
#[poise::command(slash_command)]
pub(crate) async fn backups(
    ctx: Context<'_>,
    #[description = "Which server's backups to list"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    let Some(service) = data.backup.clone() else {
        ctx.send(reply_with(backups_disabled_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let reply = ctx
        .send(reply_with(working_embed(
            &format!("Backups of {server}"),
            "Looking them up…",
        )))
        .await?;
    match service.list_backups(&backup_ctx(data), &server).await {
        Ok(list) => {
            reply
                .edit(ctx, cleared(backups_list_embed(&server, &list)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to list backups");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't list backups right now. Try again in a moment.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// List the servers archived in this Discord server (in cold storage, recoverable).
#[poise::command(slash_command)]
pub(crate) async fn archives(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();
    let Some(service) = data.backup.clone() else {
        ctx.send(reply_with(backups_disabled_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    if !service.archives_enabled() {
        ctx.send(reply_with(archives_unavailable_embed()).ephemeral(true))
            .await?;
        return Ok(());
    }
    let Some(scope) = command_scope(ctx) else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let reply = ctx
        .send(reply_with(working_embed(
            "Archived servers",
            "Looking them up…",
        )))
        .await?;
    match service.list_archives(&scope).await {
        Ok(list) => {
            reply.edit(ctx, cleared(archives_list_embed(&list))).await?;
        }
        Err(err) => {
            error!(error = ?err, "failed to list archives");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't list archives right now. Try again in a moment.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// Archive a server: save a durable backup, then release its storage. Recoverable.
#[poise::command(slash_command, check = "require_admin")]
pub(crate) async fn archive(
    ctx: Context<'_>,
    #[description = "Which server to archive (frees its storage; recover it later)"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    let Some(service) = data.backup.clone() else {
        ctx.send(reply_with(backups_disabled_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    if !service.archives_enabled() {
        ctx.send(reply_with(archives_unavailable_embed()).ephemeral(true))
            .await?;
        return Ok(());
    }
    let Some(reply) =
        confirm_action(ctx, archive_confirm_embed(&server), "archive_confirm").await?
    else {
        return Ok(());
    };
    reply
        .edit(
            ctx,
            cleared(working_embed(
                &format!("Archiving {server}"),
                "Stopping, backing up, and releasing storage…",
            )),
        )
        .await?;
    match service
        .archive_instance(&backup_ctx(data), &server, &attribution(ctx))
        .await
    {
        Ok(outcome) => {
            if let Some(reason) = outcome.reason() {
                warn!(reason, server = %server, "archive did not succeed");
            }
            reply
                .edit(ctx, cleared(archive_result_embed(&outcome)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to archive game server");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't archive the server right now — nothing was released. \
                         Try again in a moment.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// Restore a server to one of its backups. Pick which backup from the menu.
#[poise::command(slash_command, check = "require_admin")]
pub(crate) async fn restore(
    ctx: Context<'_>,
    #[description = "Which server to restore"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    let Some(service) = data.backup.clone() else {
        ctx.send(reply_with(backups_disabled_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(working_embed(
                    &format!("Restore {server}"),
                    "Looking up its backups…",
                ))
                .ephemeral(true),
        )
        .await?;
    let backups = match service.list_backups(&backup_ctx(data), &server).await {
        Ok(backups) => backups,
        Err(err) => {
            error!(error = ?err, server = %server, "failed to list backups for restore");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't list backups right now. Try again in a moment.",
                    )),
                )
                .await?;
            return Ok(());
        }
    };
    if backups.is_empty() {
        reply
            .edit(
                ctx,
                cleared(neutral_embed(
                    "No backups",
                    &format!("**{server}** has no backups to restore from yet."),
                )),
            )
            .await?;
        return Ok(());
    }

    let options: Vec<CreateSelectMenuOption> = backups
        .iter()
        .take(MAX_SELECT_OPTIONS)
        .enumerate()
        // The value is the backup's index in `backups`, not its S3 key: a long
        // instance name pushes the key past Discord's 100-char value cap.
        // finish_restore resolves the index back to the key.
        .map(|(index, backup)| {
            CreateSelectMenuOption::new(
                truncate_option_label(&backup.created_at),
                index.to_string(),
            )
        })
        .collect();
    let menu = CreateSelectMenu::new("restore_pick", CreateSelectMenuKind::String { options })
        .placeholder("Pick a backup to restore");
    reply
        .edit(
            ctx,
            poise::CreateReply::default()
                .embed(neutral_embed(
                    &format!("Restore {server}"),
                    "Pick which backup to roll back to.",
                ))
                .components(vec![CreateActionRow::SelectMenu(menu)]),
        )
        .await?;
    finish_restore(ctx, &service, &reply, &server, &backups).await
}

/// Collect the backup pick and the overwrite confirmation, then run the restore.
/// Split out of [`restore`] so each stays under the line cap.
async fn finish_restore(
    ctx: Context<'_>,
    service: &BackupService,
    reply: &poise::ReplyHandle<'_>,
    server: &str,
    backups: &[ArtifactSummary],
) -> Result<(), Error> {
    let message = reply.message().await?;
    let Some(pick) = collect_component(ctx, message.id).await else {
        reply
            .edit(
                ctx,
                cleared(neutral_embed("Timed out", "Nothing was changed.")),
            )
            .await?;
        return Ok(());
    };
    acknowledge(ctx, &pick).await?;
    let Some(backup) = string_select_value(&pick)
        .and_then(|value| value.parse::<usize>().ok())
        .and_then(|index| backups.get(index))
    else {
        reply
            .edit(
                ctx,
                cleared(error_embed(
                    "Couldn't read your selection. Try `/restore` again.",
                )),
            )
            .await?;
        return Ok(());
    };
    let key = backup.key.clone();
    let label = backup.created_at.clone();

    reply
        .edit(
            ctx,
            poise::CreateReply::default()
                .embed(restore_confirm_embed(server, &label))
                .components(vec![confirm_buttons("restore_confirm")]),
        )
        .await?;
    let Some(confirm) = collect_component(ctx, message.id).await else {
        reply
            .edit(
                ctx,
                cleared(neutral_embed("Timed out", "Nothing was changed.")),
            )
            .await?;
        return Ok(());
    };
    acknowledge(ctx, &confirm).await?;
    if confirm.data.custom_id != "restore_confirm" {
        reply
            .edit(
                ctx,
                cleared(neutral_embed("Cancelled", "Nothing was changed.")),
            )
            .await?;
        return Ok(());
    }

    reply
        .edit(
            ctx,
            cleared(working_embed(
                &format!("Restoring {server}"),
                "Saving a safety backup, then rolling the world back…",
            )),
        )
        .await?;
    match service
        .restore_backup(&backup_ctx(ctx.data()), server, &key, &attribution(ctx))
        .await
    {
        Ok(outcome) => {
            if let Some(reason) = outcome.reason() {
                warn!(reason, server, "restore did not succeed");
            }
            reply
                .edit(ctx, cleared(restore_result_embed(&outcome, server)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, server, "failed to restore game server");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't restore the server right now. Try again in a moment.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// Recover an archived server: recreate it and restore its world. Pick from the menu.
#[poise::command(slash_command, check = "require_admin")]
pub(crate) async fn recover(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();
    let Some(service) = data.backup.clone() else {
        ctx.send(reply_with(backups_disabled_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    if !service.archives_enabled() {
        ctx.send(reply_with(archives_unavailable_embed()).ephemeral(true))
            .await?;
        return Ok(());
    }
    let Some(scope) = command_scope(ctx) else {
        ctx.send(reply_with(guild_required_embed()).ephemeral(true))
            .await?;
        return Ok(());
    };
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(working_embed("Recover a server", "Looking up archives…"))
                .ephemeral(true),
        )
        .await?;
    let archives = match service.list_archives(&scope).await {
        Ok(archives) => archives,
        Err(err) => {
            error!(error = ?err, "failed to list archives for recover");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't list archives right now. Try again in a moment.",
                    )),
                )
                .await?;
            return Ok(());
        }
    };
    if archives.is_empty() {
        reply
            .edit(
                ctx,
                cleared(neutral_embed(
                    "No archives",
                    "No servers in this Discord server have been archived yet.",
                )),
            )
            .await?;
        return Ok(());
    }

    let options: Vec<CreateSelectMenuOption> = archives
        .iter()
        .take(MAX_SELECT_OPTIONS)
        .map(|archive| {
            // The pick value carries the archive's owning guild so recover can
            // recreate it in its original tenant (a cross-guild operator may see
            // archives from several guilds at once).
            CreateSelectMenuOption::new(
                truncate_option_label(&format!("{} · {}", archive.name, archive.created_at)),
                format!("{}:{}", archive.guild, archive.name),
            )
        })
        .collect();
    let menu = CreateSelectMenu::new("recover_pick", CreateSelectMenuKind::String { options })
        .placeholder("Pick a server to recover");
    reply
        .edit(
            ctx,
            poise::CreateReply::default()
                .embed(neutral_embed(
                    "Recover a server",
                    "Pick which archived server to bring back.",
                ))
                .components(vec![CreateActionRow::SelectMenu(menu)]),
        )
        .await?;
    finish_recover(ctx, &service, &reply).await
}

/// Collect the archive pick and run the recovery. Split out of [`recover`] so each
/// stays under the line cap. The pick value is `guild:name` (see [`recover`]).
async fn finish_recover(
    ctx: Context<'_>,
    service: &BackupService,
    reply: &poise::ReplyHandle<'_>,
) -> Result<(), Error> {
    let message = reply.message().await?;
    let Some(pick) = collect_component(ctx, message.id).await else {
        reply
            .edit(
                ctx,
                cleared(neutral_embed("Timed out", "Nothing was recovered.")),
            )
            .await?;
        return Ok(());
    };
    acknowledge(ctx, &pick).await?;
    let Some((guild, name)) = string_select_value(&pick).and_then(|value| {
        value
            .split_once(':')
            .map(|(guild, name)| (guild.to_owned(), name.to_owned()))
    }) else {
        reply
            .edit(
                ctx,
                cleared(error_embed(
                    "Couldn't read your selection. Try `/recover` again.",
                )),
            )
            .await?;
        return Ok(());
    };

    reply
        .edit(
            ctx,
            cleared(working_embed(
                &format!("Recovering {name}"),
                "Recreating the server and restoring its world…",
            )),
        )
        .await?;
    match service
        .recover_archive(&backup_ctx(ctx.data()), &guild, &name, &attribution(ctx))
        .await
    {
        Ok(outcome) => {
            if let Some(reason) = outcome.reason() {
                warn!(reason, name, "recover did not succeed");
            }
            reply
                .edit(ctx, cleared(recover_result_embed(&outcome, &name)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, name, "failed to recover archived server");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't recover that server right now. Try again in a moment.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// Who ran this command, for the product event log. Every slash command is the
/// `command` surface by construction — that is what makes a friend's own action
/// separable from one Gary took on their behalf.
fn attribution(ctx: Context<'_>) -> Attribution {
    Attribution::new(Actor::Discord(ctx.author().id.get()), Surface::Command)
}

/// Record a lifecycle action against the server it acted on. Emitted after the
/// friend already has their answer, so the record is never in their way.
async fn record_action(ctx: Context<'_>, kind: EventKind, server: &str, outcome: &'static str) {
    record_boot(ctx, kind, server, outcome, None).await;
}

/// Record a lifecycle action that carries a boot payload (address, time to
/// ready) — the create and cold-start paths.
async fn record_boot(
    ctx: Context<'_>,
    kind: EventKind,
    server: &str,
    outcome: &'static str,
    details: Option<serde_json::Value>,
) {
    let data = ctx.data();
    let mut event = ProductEvent::new(kind, &attribution(ctx), outcome)
        .instance(InstanceName::new(server))
        .guild(
            ctx.guild_id()
                .map(|guild| GuildId::new(guild.get().to_string())),
        );
    if let Some(details) = details {
        event = event.details(details);
    }
    data.events
        .record_lifecycle(&data.kube_client, &data.namespace, event)
        .await;
}

/// Danger/Cancel button row for a destructive confirmation.
fn confirm_buttons(confirm_id: &str) -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(confirm_id)
            .label("Confirm")
            .style(ButtonStyle::Danger),
        CreateButton::new("action_cancel")
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ])
}

/// Collect one component interaction on `message`, scoped to the invoking user,
/// within [`COMPONENT_TIMEOUT`]. `None` means the prompt expired.
async fn collect_component(ctx: Context<'_>, message: MessageId) -> Option<ComponentInteraction> {
    serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .author_id(ctx.author().id)
        .message_id(message)
        .timeout(COMPONENT_TIMEOUT)
        .await
}

/// Acknowledge a component press so Discord doesn't mark it failed; the result is
/// written by editing the original reply.
async fn acknowledge(ctx: Context<'_>, interaction: &ComponentInteraction) -> Result<(), Error> {
    interaction
        .create_response(
            ctx.serenity_context(),
            CreateInteractionResponse::Acknowledge,
        )
        .await?;
    Ok(())
}

/// The chosen value of a string-select interaction, if that's what it was.
fn string_select_value(interaction: &ComponentInteraction) -> Option<String> {
    if let ComponentInteractionDataKind::StringSelect { values } = &interaction.data.kind {
        values.first().cloned()
    } else {
        None
    }
}

/// Send a Confirm/Cancel prompt and return the reply handle to continue editing
/// when confirmed, or `None` (already resolved to a cancel/timeout embed) otherwise.
async fn confirm_action<'a>(
    ctx: Context<'a>,
    prompt: CreateEmbed,
    confirm_id: &str,
) -> Result<Option<poise::ReplyHandle<'a>>, Error> {
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(prompt)
                .components(vec![confirm_buttons(confirm_id)])
                .ephemeral(true),
        )
        .await?;
    let message = reply.message().await?;
    let Some(interaction) = collect_component(ctx, message.id).await else {
        reply
            .edit(
                ctx,
                cleared(neutral_embed("Timed out", "Nothing was changed.")),
            )
            .await?;
        return Ok(None);
    };
    acknowledge(ctx, &interaction).await?;
    if interaction.data.custom_id != confirm_id {
        reply
            .edit(
                ctx,
                cleared(neutral_embed("Cancelled", "Nothing was changed.")),
            )
            .await?;
        return Ok(None);
    }
    Ok(Some(reply))
}

/// The set of servers this invocation may see and act on: a cross-guild operator
/// sees every guild, everyone else only the guild they ran the command in.
/// `None` for a non-operator with no guild (a DM), which has no tenant to scope
/// to — slash commands don't register in DMs, so this is defensive.
fn command_scope(ctx: Context<'_>) -> Option<ServerScope> {
    let data = ctx.data();
    visibility_scope(
        ctx.author().id.get(),
        ctx.guild_id().map(serenity::GuildId::get),
        &data.operator_ids,
    )
}

/// The owning-guild id to stamp on a server being created, as a string. Empty
/// when there's no guild (a DM) — which leaves the guild label off, matching the
/// pre-scoping convention; slash commands don't register in DMs, so in practice
/// this is always the guild the command ran in.
fn guild_id_string(ctx: Context<'_>) -> String {
    ctx.guild_id()
        .map(|guild| guild.get().to_string())
        .unwrap_or_default()
}

/// A non-ephemeral reply carrying a single embed — the shape every
/// non-interactive command response uses.
fn reply_with(embed: serenity::CreateEmbed) -> poise::CreateReply {
    poise::CreateReply::default().embed(embed)
}

/// An edit that replaces an interactive reply with a final embed and strips its
/// now-spent components (dropdown / buttons).
fn cleared(embed: serenity::CreateEmbed) -> poise::CreateReply {
    poise::CreateReply::default()
        .embed(embed)
        .components(vec![])
}

#[cfg(test)]
#[path = "tests/commands.rs"]
mod tests;
