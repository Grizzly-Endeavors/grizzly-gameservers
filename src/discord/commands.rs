use tracing::error;

use super::render::format_server_list;
use super::{Context, Error};
use crate::agones::list_active_servers;

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
