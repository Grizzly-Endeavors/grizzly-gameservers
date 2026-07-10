use poise::serenity_prelude as serenity;
use serenity::{
    ButtonStyle, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow, CreateButton,
    CreateEmbed, CreateInteractionResponse, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption, MessageId,
};
use tracing::{error, warn};

use super::auth::{require_admin, require_manager, visibility_scope};
use super::render::{
    archive_confirm_embed, archive_result_embed, archives_list_embed, archives_unavailable_embed,
    backup_result_embed, backups_disabled_embed, backups_list_embed, create_result_embed,
    destroy_confirm_embed, destroy_result_embed, error_embed, guild_required_embed, neutral_embed,
    recover_result_embed, restore_confirm_embed, restore_result_embed, server_list_embed,
    shutdown_result_embed, start_result_embed, supervisor_result_embed, working_embed,
};
use super::{COMPONENT_TIMEOUT, Context, Error, backup_ctx};
use crate::agones::{
    CreateOutcome, ProvisionOutcome, RuntimeState, ServerScope, StartBegin, StartOutcome,
    begin_start, build_instance_name, destroy_instance, instance_runtime_state,
    list_active_servers, list_instance_names, now_entropy, provision_instance, shutdown_instance,
    supervisor_restart, supervisor_start, supervisor_stop, wait_for_instance_ready,
};
use crate::backup::{ArtifactSummary, BackupService};
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

    match provision_instance(
        &data.kube_client,
        &data.namespace,
        &data.domain,
        &data.provision_lock,
        entry,
        &server,
        &guild_id_string(ctx),
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
                        "I lost track of whether the server came online. \
                         Check `/servers` in a minute.",
                    )),
                )
                .await?;
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
        "config_manager_user"
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
             `/config manager-role`, or `/config manager-user`.",
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
            "\n\n⚠️ My long-term memory is offline right now, so only the owner and operator \
             are recognized until it reconnects.",
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
            "I can't save that right now — my long-term memory is offline. Try again later.",
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
        .recover_archive(&backup_ctx(ctx.data()), &guild, &name)
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
