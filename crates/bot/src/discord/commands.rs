use poise::serenity_prelude as serenity;
use serenity::{
    ButtonStyle, ComponentInteractionDataKind, CreateActionRow, CreateButton,
    CreateInteractionResponse, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption,
};
use tracing::error;

use super::auth::{require_admin, visibility_scope};
use super::render::{
    create_result_embed, destroy_confirm_embed, destroy_result_embed, error_embed, neutral_embed,
    server_list_embed, shutdown_result_embed, start_result_embed, supervisor_result_embed,
    working_embed,
};
use super::{COMPONENT_TIMEOUT, Context, Error};
use crate::agones::{
    CreateOutcome, ProvisionOutcome, RuntimeState, ServerScope, StartBegin, StartOutcome,
    begin_start, build_instance_name, destroy_instance, instance_runtime_state,
    list_active_servers, list_instance_names, now_entropy, provision_instance, shutdown_instance,
    supervisor_restart, supervisor_start, supervisor_stop, wait_for_instance_ready,
};
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
