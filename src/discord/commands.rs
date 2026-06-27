use std::time::{Duration, SystemTime, UNIX_EPOCH};

use poise::serenity_prelude as serenity;
use serenity::{
    ButtonStyle, ComponentInteractionDataKind, CreateActionRow, CreateButton,
    CreateInteractionResponse, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption,
};
use tracing::error;

use super::auth::require_admin;
use super::render::{
    create_result_embed, error_embed, neutral_embed, remove_confirm_embed, remove_result_embed,
    server_list_embed, start_result_embed, stop_result_embed,
};
use super::{Context, Error};
use crate::agones::{
    build_instance_name, create_instance, list_active_servers, list_instance_names,
    remove_instance, start_instance, stop_instance,
};

/// How long the dropdown / confirm components stay live before we give up
/// waiting for the user and clear them.
const COMPONENT_TIMEOUT: Duration = Duration::from_secs(120);

/// List the game servers currently running and how to connect to them.
#[poise::command(slash_command)]
pub(crate) async fn servers(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();

    match list_active_servers(data.kube_client.clone(), &data.namespace, &data.domain).await {
        Ok(summaries) => {
            ctx.send(reply_with(server_list_embed(&summaries))).await?;
        }
        Err(err) => {
            error!(error = ?err, namespace = %data.namespace, "failed to list game servers");
            ctx.send(reply_with(error_embed(
                "Couldn't reach the cluster right now. Try again in a moment.",
            )))
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

    let instance = match build_instance_name(game, name, now_entropy()) {
        Ok(instance) => instance,
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

    match create_instance(
        &data.kube_client,
        &data.namespace,
        &data.domain,
        &data.provision_lock,
        entry,
        &instance,
    )
    .await
    {
        Ok(outcome) => {
            reply
                .edit(ctx, cleared(create_result_embed(&outcome, &instance)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, game = %game, instance = %instance, "failed to create game server");
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

/// Stop a running server, keeping its world so it can be started again later.
#[poise::command(slash_command, check = "require_admin")]
pub(crate) async fn stop(
    ctx: Context<'_>,
    #[description = "Which server to stop"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    ctx.defer().await?;
    match stop_instance(&data.kube_client, &data.namespace, &server).await {
        Ok(outcome) => {
            ctx.send(reply_with(stop_result_embed(&outcome, &server)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to stop game server");
            ctx.send(reply_with(error_embed(
                "Couldn't stop the server right now. Try again in a moment.",
            )))
            .await?;
        }
    }
    Ok(())
}

/// Start a previously stopped server, resuming its saved world.
#[poise::command(slash_command, check = "require_admin")]
pub(crate) async fn start(
    ctx: Context<'_>,
    #[description = "Which server to start"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();
    ctx.defer().await?;
    match start_instance(
        &data.kube_client,
        &data.namespace,
        &data.domain,
        &data.catalog,
        &server,
    )
    .await
    {
        Ok(outcome) => {
            ctx.send(reply_with(start_result_embed(&outcome, &server)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to start game server");
            ctx.send(reply_with(error_embed(
                "Couldn't start the server right now. Try again in a moment.",
            )))
            .await?;
        }
    }
    Ok(())
}

/// Permanently remove a server and delete its world.
#[poise::command(slash_command, check = "require_admin")]
pub(crate) async fn remove(
    ctx: Context<'_>,
    #[description = "Which server to remove (this deletes its world)"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let data = ctx.data();

    let buttons = CreateActionRow::Buttons(vec![
        CreateButton::new("remove_confirm")
            .label("Delete it")
            .style(ButtonStyle::Danger),
        CreateButton::new("remove_cancel")
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ]);
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(remove_confirm_embed(&server))
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
                cleared(neutral_embed(
                    "Cancelled",
                    "Timed out — nothing was deleted.",
                )),
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

    if interaction.data.custom_id != "remove_confirm" {
        reply
            .edit(
                ctx,
                cleared(neutral_embed("Cancelled", "Nothing was deleted.")),
            )
            .await?;
        return Ok(());
    }

    match remove_instance(&data.kube_client, &data.namespace, &server).await {
        Ok(outcome) => {
            reply
                .edit(ctx, cleared(remove_result_embed(&outcome, &server)))
                .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to remove game server");
            reply
                .edit(
                    ctx,
                    cleared(error_embed(
                        "Couldn't remove the server right now. Try again in a moment.",
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
    let names = match list_instance_names(&data.kube_client, &data.namespace).await {
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

/// Clock-derived entropy for generated instance ids. Not security-sensitive —
/// uniqueness is ultimately enforced by the API rejecting a duplicate name.
fn now_entropy() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
