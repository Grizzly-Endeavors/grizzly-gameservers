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

use crate::agent::{ChatMessage, DEFAULT_MAX_ROUNDS, EscalationReason, Role};

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
    /// A deferred-task batch (a `run_when` queue firing) gave up. There's no single
    /// asker or jump link — the batch ran unattended once its condition was met —
    /// so it carries the server and condition instead.
    Deferred {
        server: String,
        condition: String,
        guild: Option<u64>,
    },
    /// A proactive watchdog turn gave up. Nobody asked at all — Gary's own
    /// monitoring started it — so it carries the server and the signal that
    /// fired (`crash` | `wedged` | `memory`).
    Watchdog {
        server: String,
        signal: String,
        guild: Option<u64>,
    },
}

/// A fully-assembled escalation, ready to render into the operator DM. Three
/// shapes share one DM channel: a request Gary decided he couldn't finish and
/// handed off deliberately, a request he ran out of budget on, and a
/// code-enforced crash/rollback recovery that gave up on its own (no asker, jump
/// link, or attempt history — the loop caller only has a server and a file path).
pub(crate) enum Escalation {
    /// Gary called the escalation tool: he judged that he couldn't finish and
    /// asked for a human, in his own words. The most informative of the three,
    /// because the diagnosis comes from the agent rather than being inferred
    /// from where the loop stopped.
    AgentRequested {
        context: EscalationContext,
        /// What the user actually asked, verbatim.
        request: String,
        /// Gary's own account of what's wrong and what he needs from a human.
        reason: String,
        /// The instance he named, when the handoff is about one. Absent on a
        /// question that isn't tied to a server.
        server: Option<String>,
        /// The tools Gary called this turn, in call order — his attempted approach.
        attempts: Vec<String>,
    },
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

impl Escalation {
    /// Build the notice for a turn that ended by escalating, pairing the loop's
    /// reason with the locators only the surface knows. Every shell that runs a
    /// turn goes through here, so the two exits can't drift into being reported
    /// differently on one surface and not another.
    pub(crate) fn from_turn(
        reason: &EscalationReason,
        context: EscalationContext,
        request: String,
        attempts: Vec<String>,
    ) -> Self {
        match reason {
            EscalationReason::AgentRequested { reason, server } => Self::AgentRequested {
                context,
                request,
                reason: reason.clone(),
                server: server.clone(),
                attempts,
            },
            EscalationReason::RoundBudget => Self::RoundBudgetExhausted {
                context,
                request,
                attempts,
                rounds: DEFAULT_MAX_ROUNDS,
            },
        }
    }
}

/// Render an escalation into the plain-text DM an operator receives. Uses Discord
/// markdown (bold labels, a jump link) because the reader is a human operator in
/// Discord, not the model.
pub(crate) fn render_escalation(escalation: &Escalation) -> String {
    match escalation {
        Escalation::AgentRequested {
            context,
            request,
            reason,
            server,
            attempts,
        } => render_agent_requested(context, request, reason, server.as_deref(), attempts),
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

/// Gary asked for a human on purpose. His own reason leads, because it is the
/// one thing this shape has that the budget-exhaustion shape doesn't — and it is
/// what decides whether an operator needs to drop what they're doing.
fn render_agent_requested(
    context: &EscalationContext,
    request: &str,
    reason: &str,
    server: Option<&str>,
    attempts: &[String],
) -> String {
    // Only rendered when he named one: a blank "Server:" line on a question that
    // wasn't about a server reads as a lookup that failed.
    let server_line = server.map_or_else(String::new, |name| format!("**Server:** `{name}`\n"));
    format!(
        "**Gary asked for a hand — he stopped rather than guessing.**\n\n\
         **Why:** {reason}\n\
         {server_line}\
         **Where:** {}\n\
         **Who asked:** {}\n\
         **They asked:** \"{request}\"\n\
         **Gary tried:** {}\n\n\
         He told them you'd take a look.",
        where_line(context),
        who_asked(context),
        tried_line(attempts)
    )
}

/// Where the give-up happened, with the locator that surface can offer.
fn where_line(context: &EscalationContext) -> String {
    match context {
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
        EscalationContext::Deferred {
            server,
            condition,
            guild,
        } => {
            let scope = guild.map_or_else(|| "no guild".to_owned(), |id| format!("guild `{id}`"));
            format!("a deferred task — server `{server}`, condition `{condition}` ({scope})")
        }
        EscalationContext::Watchdog {
            server,
            signal,
            guild,
        } => {
            let scope = guild.map_or_else(|| "no guild".to_owned(), |id| format!("guild `{id}`"));
            format!("a proactive incident — server `{server}`, signal `{signal}` ({scope})")
        }
    }
}

/// Who set the turn going — a person on two surfaces, and nobody on the other two.
fn who_asked(context: &EscalationContext) -> String {
    match context {
        EscalationContext::Discord { asker, .. } => asker.clone(),
        EscalationContext::InGame { player, .. } => format!("player {player}"),
        EscalationContext::Deferred { .. } => "a deferred batch (no live asker)".to_owned(),
        EscalationContext::Watchdog { .. } => {
            "nobody — Gary's own monitoring flagged it".to_owned()
        }
    }
}

/// The approach Gary took, in call order. An empty list is stated rather than
/// left blank, because "he called nothing" is itself the diagnosis.
fn tried_line(attempts: &[String]) -> String {
    if attempts.is_empty() {
        "nothing — he called no tools".to_owned()
    } else {
        attempts.join(" → ")
    }
}

fn render_round_budget_exhausted(
    context: &EscalationContext,
    request: &str,
    attempts: &[String],
    rounds: usize,
) -> String {
    let where_line = where_line(context);
    let who = who_asked(context);
    let tried = tried_line(attempts);
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
        self.fan_out(&render_escalation(escalation)).await;
    }

    /// DM every operator a pre-rendered notice that isn't an escalation — the
    /// watchdog's storm-guard and no-gateway fallbacks. Same best-effort
    /// fan-out; with no operators configured the notice is logged and dropped.
    pub(crate) async fn notify_plain(&self, body: &str) {
        if self.operator_ids.is_empty() {
            warn!(
                body,
                "an operator notice had no one to go to (GAMESERVERS_ADMIN_USER_IDS is empty)"
            );
            return;
        }
        self.fan_out(body).await;
    }

    async fn fan_out(&self, body: &str) {
        for &operator in self.operator_ids.iter() {
            if let Err(err) = UserId::new(operator)
                .direct_message(&self.http, CreateMessage::new().content(body))
                .await
            {
                error!(error = ?err, operator, "failed to DM an operator a notice");
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/notify.rs"]
mod tests;
