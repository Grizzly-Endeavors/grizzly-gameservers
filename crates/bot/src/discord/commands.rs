use poise::serenity_prelude as serenity;
use serenity::{
    ButtonStyle, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow, CreateButton,
    CreateEmbed, CreateInteractionResponse, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption, MessageId,
};
use tracing::{error, warn};

use super::auth::{require_admin, visibility_scope};
use super::render::{
    archive_confirm_embed, archive_result_embed, archives_list_embed, archives_unavailable_embed,
    backup_result_embed, backups_disabled_embed, backups_list_embed, create_result_embed,
    destroy_confirm_embed, destroy_result_embed, error_embed, neutral_embed, recover_result_embed,
    restore_confirm_embed, restore_result_embed, server_list_embed, shutdown_result_embed,
    start_result_embed, supervisor_result_embed, working_embed,
};
use super::{COMPONENT_TIMEOUT, Context, Error, backup_ctx};
use crate::agones::{
    CreateOutcome, ProvisionOutcome, RuntimeState, ServerScope, StartBegin, StartOutcome,
    begin_start, build_instance_name, destroy_instance, instance_runtime_state,
    list_active_servers, list_instance_names, now_entropy, provision_instance, shutdown_instance,
    supervisor_restart, supervisor_start, supervisor_stop, wait_for_instance_ready,
};
use crate::backup::{ArtifactSummary, BackupService};
use crate::store::HomeToggle;

/// List the game servers currently running and how to connect to them.
#[poise::command(slash_command)]
pub(crate) async fn servers(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();
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
        &command_scope(ctx),
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
#[poise::command(slash_command, check = "require_admin")]
pub(crate) async fn create(
    ctx: Context<'_>,
    #[description = "Optional name for this world"] name: Option<String>,
) -> Result<(), Error> {
    let data = ctx.data();

    let options: Vec<CreateSelectMenuOption> = data
        .catalog
        .game_ids()
        .map(|id| CreateSelectMenuOption::new(id, id))
        .collect();
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
    let Some(interaction) = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .author_id(ctx.author().id)
        .message_id(message.id)
        .timeout(COMPONENT_TIMEOUT)
        .await
    else {
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

    // Acknowledge the selection so Discord doesn't mark it failed; the actual
    // result is written back by editing the original ephemeral reply.
    interaction
        .create_response(
            ctx.serenity_context(),
            CreateInteractionResponse::Acknowledge,
        )
        .await?;

    let game = if let ComponentInteractionDataKind::StringSelect { values } = &interaction.data.kind
    {
        values.first().cloned()
    } else {
        None
    };
    let Some(game) = game else {
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

    match provision_instance(
        &data.kube_client,
        &data.namespace,
        &data.domain,
        &data.provision_lock,
        entry,
        &server,
        &ctx.channel_id().get().to_string(),
    )
    .await
    {
        Ok(ProvisionOutcome::Provisioned { address }) => {
            await_ready(ctx, reply, &server, address, |address, ready| {
                create_result_embed(&CreateOutcome::Created { address, ready }, &server)
            })
            .await?;
        }
        Ok(ProvisionOutcome::AlreadyExists) => {
            reply
                .edit(
                    ctx,
                    cleared(create_result_embed(&CreateOutcome::AlreadyExists, &server)),
                )
                .await?;
        }
        Ok(ProvisionOutcome::PortsExhausted) => {
            reply
                .edit(
                    ctx,
                    cleared(create_result_embed(&CreateOutcome::PortsExhausted, &server)),
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

/// Show the connection address immediately, then poll until the server reports
/// Ready and write the final embed. Shared by `/create` and `/start`, whose
/// readiness wait can run for minutes — `finalize` builds the done-embed from the
/// resolved address and readiness so each caller keeps its own wording.
async fn await_ready(
    ctx: Context<'_>,
    reply: &poise::ReplyHandle<'_>,
    server: &str,
    address: String,
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

    match wait_for_instance_ready(&data.kube_client, &data.namespace, server).await {
        Ok(ready) => {
            reply.edit(ctx, cleared(finalize(address, ready))).await?;
        }
        Err(err) => {
            error!(error = ?err, %server, "failed to wait for readiness");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "The server was created, but I lost track of whether it came online. \
                         Check `/servers` in a minute.",
                    )),
                )
                .await?;
        }
    }
    Ok(())
}

/// Pause a server — keep it warm so /start resumes in seconds. World is saved.
#[poise::command(slash_command, check = "require_admin")]
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
        &data.http,
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
        }
    }
    Ok(())
}

/// Fully shut a server down to free the slot, keeping the world so /start can bring it back.
#[poise::command(slash_command, check = "require_admin")]
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
        }
    }
    Ok(())
}

/// Restart a running server in place — a quick reboot that keeps the world and address.
#[poise::command(slash_command, check = "require_admin")]
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
        &data.http,
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
        }
    }
    Ok(())
}

/// Start a server: a paused one resumes in seconds, a shut-down one is rescheduled (slower).
#[poise::command(slash_command, check = "require_admin")]
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
                &data.http,
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
                }
            }
        }
        Ok(RuntimeState::Down) => start_cold(ctx, &reply, &server).await?,
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
    let begin = match begin_start(
        &data.kube_client,
        &data.namespace,
        &data.domain,
        &data.catalog,
        server,
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
            return Ok(());
        }
    };

    if let StartBegin::Starting { address } = begin {
        await_ready(ctx, reply, server, address, |address, ready| {
            start_result_embed(&StartOutcome::Started { address, ready }, server)
        })
        .await?;
    } else {
        reply
            .edit(
                ctx,
                cleared(start_result_embed(&begin_to_outcome(begin), server)),
            )
            .await?;
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

    let buttons = CreateActionRow::Buttons(vec![
        CreateButton::new("destroy_confirm")
            .label("Delete it")
            .style(ButtonStyle::Danger),
        CreateButton::new("destroy_cancel")
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ]);
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(destroy_confirm_embed(&server))
                .components(vec![buttons])
                .ephemeral(true),
        )
        .await?;

    let message = reply.message().await?;
    let Some(interaction) = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .author_id(ctx.author().id)
        .message_id(message.id)
        .timeout(COMPONENT_TIMEOUT)
        .await
    else {
        reply
            .edit(
                ctx,
                cleared(neutral_embed("Timed out", "Nothing was deleted.")),
            )
            .await?;
        return Ok(());
    };

    interaction
        .create_response(
            ctx.serenity_context(),
            CreateInteractionResponse::Acknowledge,
        )
        .await?;

    if interaction.data.custom_id != "destroy_confirm" {
        reply
            .edit(
                ctx,
                cleared(neutral_embed("Cancelled", "Nothing was deleted.")),
            )
            .await?;
        return Ok(());
    }

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
            reply
                .edit(ctx, cleared(destroy_result_embed(&outcome, &server)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to destroy game server");
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

async fn autocomplete_server(ctx: Context<'_>, partial: &str) -> impl Iterator<Item = String> {
    let data = ctx.data();
    let needle = partial.to_owned();
    let names =
        match list_instance_names(&data.kube_client, &data.namespace, &command_scope(ctx)).await {
            Ok(names) => names,
            Err(err) => {
                error!(error = ?err, "failed to list instances for autocomplete");
                Vec::new()
            }
        };
    names
        .into_iter()
        .filter(move |name| name.starts_with(&needle))
}

/// Toggle whether Gary answers in this channel without being @mentioned.
#[poise::command(slash_command, rename = "gary-home", check = "require_admin")]
pub(crate) async fn gary_home(ctx: Context<'_>) -> Result<(), Error> {
    // DMs are always no-mention, so there's nothing to toggle there.
    if ctx.guild_id().is_none() {
        ctx.send(
            reply_with(neutral_embed(
                "Already listening",
                "This is a DM — I already answer here without being @mentioned.",
            ))
            .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let embed = match ctx
        .data()
        .home_channels
        .toggle(ctx.channel_id().get())
        .await
    {
        Ok(HomeToggle::Added) => neutral_embed(
            "This is now Gary's channel",
            "I'll answer here without being @mentioned. Run `/gary-home` again to turn that off.",
        ),
        Ok(HomeToggle::Removed) => neutral_embed(
            "Back to mentions only",
            "I'll only answer here when you @mention me now.",
        ),
        Ok(HomeToggle::Unavailable) => error_embed(
            "I can't remember that right now — my long-term memory is offline. \
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
    ctx.data()
        .sessions
        .clear((ctx.channel_id().get(), ctx.author().id.get()));
    ctx.send(
        reply_with(neutral_embed(
            "Fresh start",
            "Okay, starting fresh — I've cleared what we were just talking about.",
        ))
        .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Take a backup of a running server's world to durable storage right now.
#[poise::command(slash_command, check = "require_admin")]
pub(crate) async fn backup(
    ctx: Context<'_>,
    #[description = "Which server to back up"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    let Some(service) = data.backup.clone() else {
        ctx.send(reply_with(backups_disabled_embed())).await?;
        return Ok(());
    };
    let reply = ctx
        .send(reply_with(working_embed(
            &format!("Backing up {server}"),
            "Saving a snapshot of the world…",
        )))
        .await?;
    match service
        .backup_instance(&backup_ctx(data), &server, &actor_id(ctx))
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
        ctx.send(reply_with(backups_disabled_embed())).await?;
        return Ok(());
    };
    let reply = ctx
        .send(reply_with(working_embed(
            &format!("Backups of {server}"),
            "Looking them up…",
        )))
        .await?;
    match service.list_backups(&server).await {
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

/// List the servers archived in this channel (in cold storage, recoverable).
#[poise::command(slash_command)]
pub(crate) async fn archives(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();
    let Some(service) = data.backup.clone() else {
        ctx.send(reply_with(backups_disabled_embed())).await?;
        return Ok(());
    };
    if !service.archives_enabled() {
        ctx.send(reply_with(archives_unavailable_embed())).await?;
        return Ok(());
    }
    let reply = ctx
        .send(reply_with(working_embed(
            "Archived servers",
            "Looking them up…",
        )))
        .await?;
    match service
        .list_archives(&ctx.channel_id().get().to_string())
        .await
    {
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
        ctx.send(reply_with(backups_disabled_embed())).await?;
        return Ok(());
    };
    if !service.archives_enabled() {
        ctx.send(reply_with(archives_unavailable_embed())).await?;
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
        .archive_instance(&backup_ctx(data), &server, &actor_id(ctx))
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
        ctx.send(reply_with(backups_disabled_embed())).await?;
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
    let backups = match service.list_backups(&server).await {
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
        .take(25)
        .map(|backup| CreateSelectMenuOption::new(backup.created_at.clone(), backup.key.clone()))
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
    let Some(key) = string_select_value(&pick) else {
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
    let label = backups.iter().find(|backup| backup.key == key).map_or_else(
        || "the selected backup".to_owned(),
        |b| b.created_at.clone(),
    );

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
        .restore_backup(&backup_ctx(ctx.data()), server, &key)
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
        ctx.send(reply_with(backups_disabled_embed())).await?;
        return Ok(());
    };
    if !service.archives_enabled() {
        ctx.send(reply_with(archives_unavailable_embed())).await?;
        return Ok(());
    }
    let channel = ctx.channel_id().get().to_string();
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(working_embed("Recover a server", "Looking up archives…"))
                .ephemeral(true),
        )
        .await?;
    let archives = match service.list_archives(&channel).await {
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
                    "There's nothing archived in this channel to recover.",
                )),
            )
            .await?;
        return Ok(());
    }

    let options: Vec<CreateSelectMenuOption> = archives
        .iter()
        .take(25)
        .map(|archive| {
            CreateSelectMenuOption::new(
                format!("{} · {}", archive.name, archive.created_at),
                archive.name.clone(),
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
    finish_recover(ctx, &service, &reply, &channel).await
}

/// Collect the archive pick and run the recovery. Split out of [`recover`] so each
/// stays under the line cap.
async fn finish_recover(
    ctx: Context<'_>,
    service: &BackupService,
    reply: &poise::ReplyHandle<'_>,
    channel: &str,
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
    let Some(name) = string_select_value(&pick) else {
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
        .recover_archive(&backup_ctx(ctx.data()), channel, &name)
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

/// The invoking user's id, recorded as `created_by` on a manual backup/archive.
fn actor_id(ctx: Context<'_>) -> String {
    ctx.author().id.get().to_string()
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

/// The set of servers this invocation may see and act on: the allowlisted
/// super-admin sees every channel, everyone else only the channel they ran the
/// command in (a DM being its own channel).
fn command_scope(ctx: Context<'_>) -> ServerScope {
    let data = ctx.data();
    visibility_scope(
        ctx.author().id.get(),
        ctx.channel_id().get(),
        &data.admin_user_ids,
    )
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
