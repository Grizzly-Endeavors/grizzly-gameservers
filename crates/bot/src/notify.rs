use std::sync::Arc;

use poise::serenity_prelude as serenity;
use serenity::{CreateMessage, Http, UserId};
use tracing::{error, warn};

use crate::agent::{ChatMessage, DEFAULT_MAX_ROUNDS, EscalationReason, Role};

pub(crate) enum EscalationContext {
    Discord {
        asker: String,
        jump_link: String,
        guild: Option<u64>,
    },
    InGame {
        player: String,
        server: String,
        guild: String,
    },
    Deferred {
        server: String,
        condition: String,
        guild: Option<u64>,
    },
    Watchdog {
        server: String,
        signal: String,
        guild: Option<u64>,
    },
}

pub(crate) enum Escalation {
    AgentRequested {
        context: EscalationContext,
        request: String,
        reason: String,
        server: Option<String>,
        attempts: Vec<String>,
    },
    RoundBudgetExhausted {
        context: EscalationContext,
        request: String,
        attempts: Vec<String>,
        rounds: usize,
    },
    CrashRollback {
        server: String,
        path: String,
    },
}

impl Escalation {
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

fn render_agent_requested(
    context: &EscalationContext,
    request: &str,
    reason: &str,
    server: Option<&str>,
    attempts: &[String],
) -> String {
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

#[derive(Clone)]
pub(crate) struct OperatorNotifier {
    http: Arc<Http>,
    operator_ids: Arc<[u64]>,
}

impl OperatorNotifier {
    pub(crate) fn new(http: Arc<Http>, operator_ids: Arc<[u64]>) -> Self {
        Self { http, operator_ids }
    }

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
