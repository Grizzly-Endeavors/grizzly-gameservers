use poise::serenity_prelude as serenity;
use tracing::error;

use super::render::guild_required_embed;
use super::{Context, Data, Error};
use crate::agones::{ScopeVerdict, ServerScope, verify_scope};
use crate::store::GrantSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AccessLevel {
    ReadOnly,
    Manager,
    Admin,
}

impl AccessLevel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Manager => "manager",
            Self::Admin => "admin",
        }
    }
}

pub(crate) struct AccessCheck<'a> {
    pub(crate) user: u64,
    pub(crate) roles: &'a [u64],
    pub(crate) guild_owner: Option<u64>,
    pub(crate) operators: &'a [u64],
    pub(crate) guild_admins: &'a GrantSet,
    pub(crate) guild_managers: &'a GrantSet,
}

pub(crate) fn access_level(check: &AccessCheck<'_>) -> AccessLevel {
    if is_granted(check, check.guild_admins) {
        AccessLevel::Admin
    } else if is_granted(check, check.guild_managers) {
        AccessLevel::Manager
    } else {
        AccessLevel::ReadOnly
    }
}

fn is_granted(check: &AccessCheck<'_>, grants: &GrantSet) -> bool {
    check.operators.contains(&check.user)
        || check.guild_owner == Some(check.user)
        || grants.users.contains(&check.user)
        || check.roles.iter().any(|role| grants.roles.contains(role))
}

pub(crate) fn visibility_scope(
    user: u64,
    guild: Option<u64>,
    operators: &[u64],
) -> Option<ServerScope> {
    if operators.contains(&user) {
        Some(ServerScope::All)
    } else {
        guild.map(|id| ServerScope::Guild(id.to_string()))
    }
}

pub(crate) async fn require_admin(ctx: Context<'_>) -> Result<bool, Error> {
    require_at_least(
        ctx,
        AccessLevel::Admin,
        "You need to be an admin to do that.",
    )
    .await
}

pub(crate) async fn require_operator(ctx: Context<'_>) -> Result<bool, Error> {
    if ctx.data().operator_ids.contains(&ctx.author().id.get()) {
        return Ok(true);
    }
    deny(ctx, "Only the bot operator can change these limits.").await?;
    Ok(false)
}

pub(crate) async fn require_manager(ctx: Context<'_>) -> Result<bool, Error> {
    require_at_least(
        ctx,
        AccessLevel::Manager,
        "You need to be a manager or admin to do that. Ask an admin to grant you access with `/config manager-user add`.",
    )
    .await
}

async fn require_at_least(
    ctx: Context<'_>,
    needed: AccessLevel,
    message: &str,
) -> Result<bool, Error> {
    if access_level_of(ctx).await >= needed {
        return Ok(true);
    }
    deny(ctx, message).await?;
    Ok(false)
}

pub(crate) struct Speaker {
    pub(crate) user: u64,
    pub(crate) guild: Option<serenity::GuildId>,
    pub(crate) roles: Vec<u64>,
}

pub(crate) async fn access_level_for(
    ctx: &serenity::Context,
    data: &Data,
    speaker: &Speaker,
) -> AccessLevel {
    if data.operator_ids.contains(&speaker.user) {
        return AccessLevel::Admin;
    }
    let Some(guild_id) = speaker.guild else {
        return AccessLevel::ReadOnly;
    };
    let guild_admins = data.guild_config.admins(guild_id.get()).await;
    let guild_managers = data.guild_config.managers(guild_id.get()).await;
    access_level(&AccessCheck {
        user: speaker.user,
        roles: &speaker.roles,
        guild_owner: guild_owner_id(ctx, guild_id).await,
        operators: &data.operator_ids,
        guild_admins: &guild_admins,
        guild_managers: &guild_managers,
    })
}

pub(crate) async fn guild_owner_id(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
) -> Option<u64> {
    if let Some(guild) = guild_id.to_guild_cached(&ctx.cache) {
        return Some(guild.owner_id.get());
    }
    match guild_id.to_partial_guild(&ctx.http).await {
        Ok(guild) => Some(guild.owner_id.get()),
        Err(err) => {
            error!(error = ?err, guild = guild_id.get(), "failed to read guild for owner check");
            None
        }
    }
}

pub(crate) async fn access_level_of(ctx: Context<'_>) -> AccessLevel {
    let roles: Vec<u64> = match ctx.author_member().await {
        Some(member) => member.roles.iter().map(|role| role.get()).collect(),
        None => Vec::new(),
    };
    access_level_for(
        ctx.serenity_context(),
        ctx.data(),
        &Speaker {
            user: ctx.author().id.get(),
            guild: ctx.guild_id(),
            roles,
        },
    )
    .await
}

pub(crate) async fn require_scope(ctx: Context<'_>) -> Result<bool, Error> {
    let poise::Context::Application(app) = ctx else {
        return Ok(true);
    };
    let Some(server) = server_option(app.args) else {
        return Ok(true);
    };
    let data = ctx.data();
    let Some(scope) = visibility_scope(
        ctx.author().id.get(),
        ctx.guild_id().map(serenity::GuildId::get),
        &data.operator_ids,
    ) else {
        ctx.send(
            poise::CreateReply::default()
                .embed(guild_required_embed())
                .ephemeral(true),
        )
        .await?;
        return Ok(false);
    };
    match verify_scope(&data.kube_client, &data.namespace, server, &scope).await {
        Ok(ScopeVerdict::InScope) => Ok(true),
        Ok(ScopeVerdict::Foreign | ScopeVerdict::Absent) => {
            deny(ctx, &format!(
                "There's no game server called **{server}** in this Discord server. Run `/servers` to see the game servers shared here."
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
