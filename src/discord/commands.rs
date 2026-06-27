use std::time::{SystemTime, UNIX_EPOCH};

use tracing::error;

use super::auth::require_admin;
use super::render::format_server_list;
use super::{Context, Error};
use crate::agones::{
    CreateOutcome, RemoveOutcome, StartOutcome, StopOutcome, build_instance_name, create_instance,
    list_active_servers, list_instance_names, remove_instance, start_instance, stop_instance,
};

/// List the game servers currently running and how to connect to them.
#[poise::command(slash_command)]
pub(crate) async fn servers(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();

    match list_active_servers(data.kube_client.clone(), &data.namespace, &data.domain).await {
        Ok(summaries) => {
            ctx.say(format_server_list(&summaries)).await?;
        }
        Err(err) => {
            error!(error = ?err, namespace = %data.namespace, "failed to list game servers");
            ctx.say("Couldn't reach the cluster right now. Try again in a moment.")
                .await?;
        }
    }

    Ok(())
}

/// Spin up a new game server.
#[poise::command(slash_command, check = "require_admin")]
pub(crate) async fn create(
    ctx: Context<'_>,
    #[description = "Which game to run"]
    #[autocomplete = "autocomplete_game"]
    game: String,
    #[description = "Optional name for this world"] name: Option<String>,
) -> Result<(), Error> {
    let data = ctx.data();
    let Some(entry) = data.catalog.get(&game) else {
        ctx.send(ephemeral(format!(
            "Unknown game '{game}'. Pick one from the suggestions."
        )))
        .await?;
        return Ok(());
    };
    let instance = match build_instance_name(&game, name.as_deref(), now_entropy()) {
        Ok(instance) => instance,
        Err(err) => {
            ctx.send(ephemeral(format!("That name won't work: {err}")))
                .await?;
            return Ok(());
        }
    };

    ctx.defer().await?;
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
        Ok(CreateOutcome::Created { address, ready }) => {
            let message = if ready {
                format!("**{instance}** is up — connect at `{address}`")
            } else {
                format!(
                    "**{instance}** is starting — connect at `{address}` in a couple of minutes."
                )
            };
            ctx.say(message).await?;
        }
        Ok(CreateOutcome::AlreadyExists) => {
            ctx.say(format!("A server named **{instance}** already exists."))
                .await?;
        }
        Ok(CreateOutcome::PortsExhausted) => {
            ctx.say("All server slots are in use right now. Remove one first, then try again.")
                .await?;
        }
        Err(err) => {
            error!(error = ?err, game = %game, instance = %instance, "failed to create game server");
            ctx.say("Couldn't create the server right now. Try again in a moment.")
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
        Ok(StopOutcome::Stopped) => {
            ctx.say(format!(
                "Stopped **{server}** — its world is saved. Use `/start {server}` to bring it back."
            ))
            .await?;
        }
        Ok(StopOutcome::NotFound) => {
            ctx.say(format!("There's no server named **{server}**."))
                .await?;
        }
        Ok(StopOutcome::NotManaged) => {
            ctx.say(format!(
                "**{server}** is managed by the platform and can't be controlled from here."
            ))
            .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to stop game server");
            ctx.say("Couldn't stop the server right now. Try again in a moment.")
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
        Ok(StartOutcome::Started { address, ready }) => {
            let message = if ready {
                format!("**{server}** is back up — connect at `{address}`")
            } else {
                format!("**{server}** is starting — connect at `{address}` in a couple of minutes.")
            };
            ctx.say(message).await?;
        }
        Ok(StartOutcome::AlreadyRunning) => {
            ctx.say(format!("**{server}** is already running.")).await?;
        }
        Ok(StartOutcome::NotFound) => {
            ctx.say(format!("There's no server named **{server}**."))
                .await?;
        }
        Ok(StartOutcome::NotManaged) => {
            ctx.say(format!(
                "**{server}** is managed by the platform and can't be controlled from here."
            ))
            .await?;
        }
        Ok(StartOutcome::UnknownGame(game)) => {
            ctx.say(format!(
                "**{server}** runs '{game}', which is no longer in the catalog."
            ))
            .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to start game server");
            ctx.say("Couldn't start the server right now. Try again in a moment.")
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
    ctx.defer().await?;
    match remove_instance(&data.kube_client, &data.namespace, &server).await {
        Ok(RemoveOutcome::Removed) => {
            ctx.say(format!("Removed **{server}** and deleted its world."))
                .await?;
        }
        Ok(RemoveOutcome::NotFound) => {
            ctx.say(format!("There's no server named **{server}**."))
                .await?;
        }
        Ok(RemoveOutcome::NotManaged) => {
            ctx.say(format!(
                "**{server}** is managed by the platform and can't be controlled from here."
            ))
            .await?;
        }
        Err(err) => {
            error!(error = ?err, server = %server, "failed to remove game server");
            ctx.say("Couldn't remove the server right now. Try again in a moment.")
                .await?;
        }
    }
    Ok(())
}

#[expect(
    clippy::unused_async,
    reason = "poise autocomplete callbacks must be async"
)]
async fn autocomplete_game(ctx: Context<'_>, partial: &str) -> impl Iterator<Item = String> {
    let needle = partial.to_owned();
    let games: Vec<String> = ctx.data().catalog.game_ids().map(str::to_owned).collect();
    games
        .into_iter()
        .filter(move |game| game.starts_with(&needle))
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

fn ephemeral(content: impl Into<String>) -> poise::CreateReply {
    poise::CreateReply::default()
        .content(content)
        .ephemeral(true)
}

/// Clock-derived entropy for generated instance ids. Not security-sensitive —
/// uniqueness is ultimately enforced by the API rejecting a duplicate name.
fn now_entropy() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
