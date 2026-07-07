use poise::serenity_prelude as serenity;
use tracing::error;

use super::{Context, Error};
use crate::agones::{ScopeVerdict, ServerScope, verify_scope};

/// Whether `user` may run the mutating commands: either they are on the explicit
/// allowlist, or they hold the configured admin role. Pure so the policy is
/// unit-tested without a live interaction.
pub(crate) fn is_authorized(
    user: u64,
    roles: &[u64],
    admin_role: Option<u64>,
    allowlist: &[u64],
) -> bool {
    allowlist.contains(&user) || admin_role.is_some_and(|role| roles.contains(&role))
}

/// Which servers `user` may see and act on. Only the explicit user-id allowlist
/// grants the cross-channel super-admin view — deliberately *not* the admin
/// role, so a friend-group admin can't reach another group's servers. Everyone
/// else is confined to the channel they're speaking in (a DM being its own
/// channel). Pure so the policy is unit-tested without a live interaction.
pub(crate) fn visibility_scope(user: u64, channel: u64, allowlist: &[u64]) -> ServerScope {
    if allowlist.contains(&user) {
        ServerScope::All
    } else {
        ServerScope::Channel(channel.to_string())
    }
}

/// poise check for `/create`, `/stop`, `/start`, `/destroy`. Denies with an
/// ephemeral message (returning `false` alone would give the friend no feedback).
///
/// # Errors
///
/// Returns an error only if sending the denial reply to Discord fails.
pub(crate) async fn require_admin(ctx: Context<'_>) -> Result<bool, Error> {
    let data = ctx.data();
    let roles: Vec<u64> = match ctx.author_member().await {
        Some(member) => member.roles.iter().map(|role| role.get()).collect(),
        None => Vec::new(),
    };
    if is_authorized(
        ctx.author().id.get(),
        &roles,
        data.admin_role_id,
        &data.admin_user_ids,
    ) {
        return Ok(true);
    }
    deny(ctx, "You're not allowed to do that.").await?;
    Ok(false)
}

/// poise global check that confines a server-targeting command to the caller's
/// visibility scope. Commands with no `server` option — and non-slash contexts —
/// pass straight through; a server in another channel is refused with the same
/// ephemeral wording as a missing one, so scoping never reveals another
/// channel's servers. Registered as `command_check`, so it runs before the
/// per-command `require_admin` and any future `server`-targeting command inherits
/// the gate without extra wiring.
///
/// # Errors
///
/// Returns an error only if reading the cluster or sending the denial to Discord
/// fails.
pub(crate) async fn require_scope(ctx: Context<'_>) -> Result<bool, Error> {
    let poise::Context::Application(app) = ctx else {
        return Ok(true);
    };
    let Some(server) = server_option(app.args) else {
        return Ok(true);
    };
    let data = ctx.data();
    let scope = visibility_scope(
        ctx.author().id.get(),
        ctx.channel_id().get(),
        &data.admin_user_ids,
    );
    match verify_scope(&data.kube_client, &data.namespace, server, &scope).await {
        Ok(ScopeVerdict::InScope) => Ok(true),
        Ok(ScopeVerdict::Foreign | ScopeVerdict::Absent) => {
            deny(ctx, &format!(
                "There's no server called **{server}** here. Run `/servers` to see this channel's servers."
            ))
            .await?;
            Ok(false)
        }
        Err(err) => {
            error!(error = ?err, server, "failed to check server scope");
            deny(
                ctx,
                "Couldn't reach the cluster right now. Try again in a moment.",
            )
            .await?;
            Ok(false)
        }
    }
}

/// The `server` slash-command option's value, if this command has one.
fn server_option<'a>(args: &'a [serenity::ResolvedOption<'a>]) -> Option<&'a str> {
    args.iter().find_map(|option| {
        if option.name != "server" {
            return None;
        }
        if let serenity::ResolvedValue::String(value) = &option.value {
            Some(*value)
        } else {
            None
        }
    })
}

/// Send an ephemeral denial for a failed check, so the friend gets feedback
/// rather than a silently swallowed command.
async fn deny(ctx: Context<'_>, message: &str) -> Result<(), Error> {
    ctx.send(
        poise::CreateReply::default()
            .content(message)
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
