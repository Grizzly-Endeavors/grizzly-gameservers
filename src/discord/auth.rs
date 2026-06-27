use super::{Context, Error};

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

/// poise check for `/create`, `/stop`, `/start`, `/remove`. Denies with an
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
    ctx.send(
        poise::CreateReply::default()
            .content("You're not allowed to do that.")
            .ephemeral(true),
    )
    .await?;
    Ok(false)
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
