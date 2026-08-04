use poise::serenity_prelude as serenity;
use tracing::error;

use super::render::guild_required_embed;
use super::{Context, Data, Error};
use crate::agones::{ScopeVerdict, ServerScope, verify_scope};
use crate::store::GrantSet;

/// A caller's access tier in one guild. Ordered low→high so `>=` expresses "at
/// least this tier": `Admin` implies every `Manager` privilege, which implies
/// every read-only one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AccessLevel {
    /// Look-up only — the default for anyone not granted a higher tier.
    ReadOnly,
    /// Day-to-day server operation (lifecycle + backups + Gary file edits), but
    /// not destructive or governance actions.
    Manager,
    /// Full control, including destroy, restore, `/config`, and Gary console
    /// commands.
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

/// Everything the access policy weighs for one caller in one guild. Bundled so
/// the decision is a pure function of explicit inputs, unit-testable without a
/// live interaction. `guild_owner`, `guild_admins`, and `guild_managers` are
/// guild-specific; `operators` is the cross-guild seed.
pub(crate) struct AccessCheck<'a> {
    pub(crate) user: u64,
    pub(crate) roles: &'a [u64],
    /// The guild's owner id, when known. The owner is always an admin in their
    /// own guild — this is the bootstrap path so a fresh guild is usable before
    /// any `/config` is run.
    pub(crate) guild_owner: Option<u64>,
    /// Cross-guild operator seed (env `GAMESERVERS_ADMIN_USER_IDS`): admin in
    /// every guild.
    pub(crate) operators: &'a [u64],
    /// The guild's DB-configured admin roles and users.
    pub(crate) guild_admins: &'a GrantSet,
    /// The guild's DB-configured manager roles and users.
    pub(crate) guild_managers: &'a GrantSet,
}

/// The highest tier `check.user` holds in their guild. Admins are the union of:
/// cross-guild operators, the guild owner, DB-configured admin users, and
/// members holding a DB-configured admin role; managers are the DB-configured
/// manager users and role-holders. Pure so the policy is unit-tested without a
/// live interaction. Fail-closed: with the grant sets empty (e.g. the config DB
/// is down), only operators and the owner are admitted — as `Admin` — and
/// everyone else falls to `ReadOnly`.
pub(crate) fn access_level(check: &AccessCheck<'_>) -> AccessLevel {
    if is_granted(check, check.guild_admins) {
        AccessLevel::Admin
    } else if is_granted(check, check.guild_managers) {
        AccessLevel::Manager
    } else {
        AccessLevel::ReadOnly
    }
}

/// Whether `check.user` is an operator/owner (always admin) or is named — as a
/// user or via a held role — in `grants`.
fn is_granted(check: &AccessCheck<'_>, grants: &GrantSet) -> bool {
    check.operators.contains(&check.user)
        || check.guild_owner == Some(check.user)
        || grants.users.contains(&check.user)
        || check.roles.iter().any(|role| grants.roles.contains(role))
}

/// Which servers `user` may see and act on. A cross-guild operator gets the
/// all-guilds view (`All`) even in a DM; anyone else is confined to the guild
/// they're speaking in. `None` — a non-operator with no guild (a DM) — has no
/// tenant to scope to and must be refused. Pure so the policy is unit-tested
/// without a live interaction.
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

/// poise check for the admin-only commands (`/destroy`, `/config`, `/restore`, …).
/// Denies with an ephemeral message (returning `false` alone would give the
/// friend no feedback).
///
/// # Errors
///
/// Returns an error only if sending the denial reply to Discord fails.
pub(crate) async fn require_admin(ctx: Context<'_>) -> Result<bool, Error> {
    require_at_least(
        ctx,
        AccessLevel::Admin,
        "You need to be an admin to do that.",
    )
    .await
}

/// poise check for the operator-only commands (`/config limits` mutations).
/// Deliberately above `require_admin`: guild admins are the party the caps
/// bind, so the tier that grants everything *inside* a guild still doesn't
/// reach the knobs that bound the guild itself.
///
/// # Errors
///
/// Returns an error only if sending the denial reply to Discord fails.
pub(crate) async fn require_operator(ctx: Context<'_>) -> Result<bool, Error> {
    if ctx.data().operator_ids.contains(&ctx.author().id.get()) {
        return Ok(true);
    }
    deny(ctx, "Only the bot operator can change these limits.").await?;
    Ok(false)
}

/// poise check for the manager-tier commands — the day-to-day lifecycle
/// (`/create`, `/start`, `/stop`, `/backup`, …). Admins pass too (they outrank
/// managers).
///
/// # Errors
///
/// Returns an error only if sending the denial reply to Discord fails.
pub(crate) async fn require_manager(ctx: Context<'_>) -> Result<bool, Error> {
    require_at_least(
        ctx,
        AccessLevel::Manager,
        "You need to be a manager or admin to do that. Ask an admin to grant you access with `/config manager-user add`.",
    )
    .await
}

/// Pass the check when the caller's tier is at least `needed`, else deny with
/// `message`.
///
/// # Errors
///
/// Returns an error only if sending the denial reply to Discord fails.
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

/// Who is acting, gathered by whichever surface has the facts — a slash command,
/// a chat message, or a button click. Three surfaces need the same tier answer,
/// so they collect this and share one resolver rather than each re-deriving the
/// policy.
pub(crate) struct Speaker {
    pub(crate) user: u64,
    /// `None` in a DM, where only an operator holds a tier.
    pub(crate) guild: Option<serenity::GuildId>,
    /// The guild roles they hold, empty when unknown.
    pub(crate) roles: Vec<u64>,
}

/// `speaker`'s access tier in the guild they're speaking in. Operators are
/// admins everywhere (including DMs); everyone else needs a guild and is scored
/// against the owner check plus the DB-configured admin/manager grants. A guild
/// read failure falls through as the weaker tier (fail-closed) after logging.
pub(crate) async fn access_level_for(
    ctx: &serenity::Context,
    data: &Data,
    speaker: &Speaker,
) -> AccessLevel {
    if data.operator_ids.contains(&speaker.user) {
        return AccessLevel::Admin;
    }
    let Some(guild_id) = speaker.guild else {
        // Non-operator in a DM: no guild to hold a tier in.
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

/// The guild's owner id, cache-first (the guild is cached from its `GuildCreate`),
/// falling back to an HTTP fetch. `None` on a read failure — auth then fails
/// closed to operators only for that check.
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

/// The invoking user's access tier in the guild the command ran in.
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

/// poise global `command_check` confining a server-targeting command to the
/// caller's visibility scope. Commands with no `server` option — and non-slash
/// contexts — pass straight through; a server in another guild is refused with
/// the same ephemeral wording as a missing one, so scoping never reveals another
/// guild's servers. A non-operator invoking a server command from a DM (no
/// guild) is refused with guidance.
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
