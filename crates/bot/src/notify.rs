//! Operator escalation notifications. When Gary spends his whole round budget
//! without converging he tells the user "I've flagged it for Bear to take a
//! look" (`agent::session::ESCALATION_REPLY`); this module is the code that
//! actually keeps that promise. It DMs the cross-guild operators
//! (`GAMESERVERS_ADMIN_USER_IDS`) with enough context to act without re-deriving
//! it: where the give-up happened, who asked, what they asked, and what Gary
//! tried before giving up.
//!
//! The message-building ([`render_escalation`], [`summarize_attempts`]) is pure
//! and unit-tested; delivery ([`OperatorNotifier::notify`]) is a thin serenity DM
//! fan-out — best-effort per operator, so one operator's closed DM inbox can't
//! swallow the notice for the rest.

use std::sync::Arc;

use poise::serenity_prelude as serenity;
use serenity::{CreateMessage, Http, UserId};
use tracing::{error, warn};

use crate::agent::{ChatMessage, Role};

/// Which surface a give-up came from, carrying the surface-specific locators the
/// operator notice needs — a Discord jump link versus a game server name.
pub(crate) enum EscalationContext {
    Discord {
        /// The asker, pre-formatted for the DM (display name plus mention).
        asker: String,
        /// Deep link to the exact message, so the operator lands in the thread.
        jump_link: String,
        /// The guild the ask happened in, or `None` for a DM with Gary.
        guild: Option<u64>,
    },
    InGame {
        player: String,
        server: String,
        guild: String,
    },
}

/// A fully-assembled escalation, ready to render into the operator DM. Two shapes
/// share one DM channel: a user ask Gary gave up on (round budget exhausted, with
/// a conversational trail to show), and a code-enforced crash/rollback recovery
/// that gave up on its own (no asker, jump link, or attempt history — the loop
/// caller only has a server and a file path).
pub(crate) enum Escalation {
    /// Gary spent his whole round budget on a user's request without resolving it.
    RoundBudgetExhausted {
        context: EscalationContext,
        /// What the user actually asked, verbatim.
        request: String,
        /// The tools Gary called this turn, in call order — his attempted approach.
        attempts: Vec<String>,
        /// The round budget that was exhausted (why he gave up).
        rounds: usize,
    },
    /// An automatic crash rollback (snapshot→apply→verify→rollback) didn't bring
    /// the server back up, or couldn't even be issued — nothing more the loop can
    /// do on its own.
    CrashRollback { server: String, path: String },
}

/// Render an escalation into the plain-text DM an operator receives. Uses Discord
/// markdown (bold labels, a jump link) because the reader is a human operator in
/// Discord, not the model.
pub(crate) fn render_escalation(escalation: &Escalation) -> String {
    match escalation {
        Escalation::RoundBudgetExhausted {
            context,
            request,
            attempts,
            rounds,
        } => render_round_budget_exhausted(context, request, attempts, *rounds),
        Escalation::CrashRollback { server, path } => format!(
            "**Gary's automatic config rollback needed a hand.**\n\n\
             An automatic config rollback on {server} didn't recover it after a crash — \
             the edit to {path} may need a manual look."
        ),
    }
}

fn render_round_budget_exhausted(
    context: &EscalationContext,
    request: &str,
    attempts: &[String],
    rounds: usize,
) -> String {
    let where_line = match context {
        EscalationContext::Discord {
            jump_link, guild, ..
        } => {
            let scope = guild.map_or_else(
                || "a direct message".to_owned(),
                |id| format!("guild `{id}`"),
            );
            format!("Discord — {scope} · [jump to the message]({jump_link})")
        }
        EscalationContext::InGame { server, guild, .. } => {
            format!("in-game chat — server `{server}` (guild `{guild}`)")
        }
    };
    let who = match context {
        EscalationContext::Discord { asker, .. } => asker.clone(),
        EscalationContext::InGame { player, .. } => format!("player {player}"),
    };
    let tried = if attempts.is_empty() {
        "nothing — he gave up before calling any tools".to_owned()
    } else {
        attempts.join(" → ")
    };
    format!(
        "**Gary escalated a request he couldn't resolve on his own.**\n\n\
         **Where:** {where_line}\n\
         **Who asked:** {who}\n\
         **They asked:** \"{request}\"\n\
         **Gary tried ({rounds} rounds, budget spent):** {tried}\n\n\
         He told them you'd take a look."
    )
}

/// The tools Gary called while working the *current* request, in call order. The
/// transcript can hold earlier turns of a continued conversation, so only the
/// calls after the last user message count as this ask's attempts.
pub(crate) fn summarize_attempts(messages: &[ChatMessage]) -> Vec<String> {
    let start = messages
        .iter()
        .rposition(|message| message.role == Role::User)
        .map_or(0, |index| index + 1);
    messages
        .get(start..)
        .unwrap_or(&[])
        .iter()
        .filter_map(|message| message.tool_calls.as_deref())
        .flatten()
        .map(|call| call.function.name.clone())
        .collect()
}

/// DMs the cross-guild operators when Gary escalates. Holds its own serenity
/// [`Http`] (a token-only client, no gateway) so both the Discord shell and the
/// in-game endpoint — which is spawned before the gateway client exists — can
/// reach it through cheap `Arc` clones.
#[derive(Clone)]
pub(crate) struct OperatorNotifier {
    http: Arc<Http>,
    operator_ids: Arc<[u64]>,
}

impl OperatorNotifier {
    pub(crate) fn new(http: Arc<Http>, operator_ids: Arc<[u64]>) -> Self {
        Self { http, operator_ids }
    }

    /// DM every operator the rendered notice. Best-effort: a failed send (an
    /// operator with DMs closed, a transient API error) is logged and the
    /// remaining operators are still tried, so the escalation isn't lost to one
    /// bad inbox. With no operators configured there is no one to tell — itself
    /// worth a warning, since the user was promised a human would look.
    pub(crate) async fn notify(&self, escalation: &Escalation) {
        if self.operator_ids.is_empty() {
            warn!(
                "gary escalated but no operators are configured to notify \
                 (GAMESERVERS_ADMIN_USER_IDS is empty)"
            );
            return;
        }
        let body = render_escalation(escalation);
        for &operator in self.operator_ids.iter() {
            if let Err(err) = UserId::new(operator)
                .direct_message(&self.http, CreateMessage::new().content(body.as_str()))
                .await
            {
                error!(error = ?err, operator, "failed to DM an operator about a Gary escalation");
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/notify.rs"]
mod tests;
