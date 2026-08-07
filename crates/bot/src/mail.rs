//! Transactional mail: the message that tells someone their beta request was
//! approved and how to get started.
//!
//! Deliberately not folded into [`crate::notify`]. That module fans Discord DMs
//! out to operators over a serenity `Http` — people who already opted in, on a
//! transport where a failure costs a notification. This one writes to strangers
//! over SMTP, where a failure costs the only thing that was going to tell them
//! they were let in, and where the failure modes are a mail server's rather than
//! Discord's.
//!
//! ## Never retry
//!
//! Submission hairpins out to the VPS, so Stalwart's per-IP auto-ban sees the
//! household's public WAN address rather than a pod IP — the `10.0.0.0/8`
//! allowlist does not cover it. The platform's mail doc is explicit that an app
//! which retries a bad credential bans mail for the whole house.
//!
//! So there is no retry at any layer, and [`SendGate`] goes further: the first
//! authentication refusal latches the mailer off for the life of the process. A
//! plain no-retry rule would still let three operators clicking Approve produce
//! three authentication attempts, which is exactly the pattern the ban watches
//! for. Recovery is a human fixing the credential and restarting the pod, which
//! is honest — the credential arrives via ESO at startup anyway.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use tracing::{error, info, warn};

use crate::config::MailConfig;

/// The domain DMARC alignment is anchored on. A `From:` outside it is filtered
/// rather than bounced, so the mail simply never arrives and nothing says so.
const ALIGNED_DOMAIN: &str = "grizzly-endeavors.com";

/// What became of one send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MailOutcome {
    Sent,
    /// Not configured, or the credential latch has tripped. Nothing was dialed.
    Unavailable,
    /// The server refused the message permanently — a bad address, or a 5xx.
    /// Retrying would fail identically.
    Rejected {
        reason: String,
    },
    /// The credential was refused. Latches the mailer off; see the module docs.
    AuthRefused,
    /// Anything else: a timeout, a TLS failure, a transient 4xx.
    Failed {
        reason: String,
    },
}

impl MailOutcome {
    /// Whether the message actually went.
    pub(crate) fn is_sent(&self) -> bool {
        matches!(self, Self::Sent)
    }

    /// A sentence for the operator, naming what to do where that isn't obvious.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Sent => "The invite email is on its way.".to_owned(),
            Self::Unavailable => {
                "Mail isn't configured, so nothing was sent. Use Resend once it is.".to_owned()
            }
            Self::Rejected { reason } => {
                format!("The mail server rejected that address: {reason}")
            }
            Self::AuthRefused => "The mail credential was refused, so sending is now off until \
                                  the bot restarts. Fix the password in 1Password and redeploy."
                .to_owned(),
            Self::Failed { reason } => format!("The email didn't send: {reason}"),
        }
    }
}

/// One-way latch that turns sending off after an authentication refusal.
///
/// Separated from [`Mailer`] so the interesting half is testable without a
/// socket: the rule that matters is "once refused, never dial again", and that
/// is pure state.
#[derive(Debug, Default)]
pub(crate) struct SendGate {
    credential_rejected: AtomicBool,
}

impl SendGate {
    /// Whether a send may be attempted.
    pub(crate) fn is_open(&self) -> bool {
        !self.credential_rejected.load(Ordering::SeqCst)
    }

    /// Latch closed. Idempotent — repeated refusals report the remedy once.
    pub(crate) fn close(&self) {
        if !self.credential_rejected.swap(true, Ordering::SeqCst) {
            error!(
                "the mail credential was refused; sending is disabled for the life of this \
                 process. Retrying a bad credential gets the household's address banned by the \
                 submission server, so recovery is deliberately manual: fix gary_password on the \
                 platform-stalwart 1Password item and restart the bot."
            );
        }
    }
}

/// Sends the queue's outbound mail.
pub(crate) struct Mailer {
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    from: Option<Mailbox>,
    gate: Arc<SendGate>,
}

impl Mailer {
    /// Build the transport, or a disabled mailer. Never fails: a mail problem
    /// costs the approval email, never the boot.
    pub(crate) fn connect(config: Option<&MailConfig>) -> Self {
        let Some(config) = config else {
            warn!(
                "GAMESERVERS_SMTP_PASSWORD not set; approval mail is disabled (approvals are \
                 still recorded, and can be mailed later with Resend)"
            );
            return Self::disabled();
        };

        let from = match config.from.parse::<Mailbox>() {
            Ok(from) => from,
            Err(err) => {
                error!(
                    error = ?err,
                    from = %config.from,
                    "GAMESERVERS_MAIL_FROM is not a valid mailbox; approval mail is disabled"
                );
                return Self::disabled();
            }
        };
        // A misaligned sender fails DMARC at the recipient, which looks like
        // nothing happening rather than an error — worth a log line at startup
        // rather than a comment nobody reads.
        if !from.email.domain().eq_ignore_ascii_case(ALIGNED_DOMAIN) {
            warn!(
                from = %config.from,
                "the mail From: is outside {ALIGNED_DOMAIN}, so DMARC alignment will fail and \
                 messages will be filtered rather than delivered"
            );
        }

        let builder = match AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host) {
            Ok(builder) => builder,
            Err(err) => {
                error!(
                    error = ?err,
                    host = %config.host,
                    "could not build the SMTP transport; approval mail is disabled"
                );
                return Self::disabled();
            }
        };
        let transport = builder
            .port(config.port)
            .credentials(Credentials::new(
                config.user.clone(),
                config.password.clone(),
            ))
            .build();
        info!(host = %config.host, port = config.port, from = %config.from, "approval mail enabled");
        Self {
            transport: Some(transport),
            from: Some(from),
            gate: Arc::new(SendGate::default()),
        }
    }

    fn disabled() -> Self {
        Self {
            transport: None,
            from: None,
            gate: Arc::new(SendGate::default()),
        }
    }

    /// Whether mail can be sent right now — configured, and not latched off.
    pub(crate) fn is_enabled(&self) -> bool {
        self.transport.is_some() && self.gate.is_open()
    }

    /// Send one plain-text message.
    ///
    /// Never returns an error: every failure is a [`MailOutcome`] the caller
    /// shows the operator and records, because "the decision stuck but the mail
    /// didn't" is a real state that needs a Resend rather than an error page.
    pub(crate) async fn send(&self, to: &str, subject: &str, body: &str) -> MailOutcome {
        let (Some(transport), Some(from)) = (self.transport.as_ref(), self.from.as_ref()) else {
            return MailOutcome::Unavailable;
        };
        if !self.gate.is_open() {
            return MailOutcome::Unavailable;
        }
        let Ok(recipient) = to.parse::<Mailbox>() else {
            return MailOutcome::Rejected {
                reason: "that isn't an address the mail server can parse".to_owned(),
            };
        };
        let message = match Message::builder()
            .from(from.clone())
            .to(recipient)
            .subject(subject)
            // Explicit charset. Without it the body goes out as bare `text/plain`,
            // which readers decode as ASCII/latin-1 — the em dashes in the copy
            // then arrive as mojibake in the first message an applicant ever
            // gets. The subject header is encoded separately and looks fine, so
            // this only shows up in the body.
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_owned())
        {
            Ok(message) => message,
            Err(err) => {
                error!(error = ?err, "could not build the approval email");
                return MailOutcome::Rejected {
                    reason: format!("{err}"),
                };
            }
        };

        match transport.send(message).await {
            Ok(_) => MailOutcome::Sent,
            Err(err) => self.classify(&err),
        }
    }

    /// Turn a transport error into an outcome, latching the gate on an
    /// authentication refusal.
    fn classify(&self, err: &lettre::transport::smtp::Error) -> MailOutcome {
        if err.is_permanent() {
            // A 535 arrives as a permanent error carrying an auth status. Treat
            // any permanent failure that mentions authentication as the
            // credential being wrong: over-latching costs a restart, while
            // under-latching costs the household's outbound mail.
            let text = err.to_string();
            if text.contains("authentication") || text.contains("Authentication") {
                self.gate.close();
                return MailOutcome::AuthRefused;
            }
            error!(error = ?err, "the mail server permanently rejected an approval email");
            return MailOutcome::Rejected { reason: text };
        }
        error!(error = ?err, "an approval email failed to send");
        MailOutcome::Failed {
            reason: err.to_string(),
        }
    }
}

/// The subject line of the approval email.
pub(crate) const APPROVAL_SUBJECT: &str = "You're in — Gary beta access";

/// The approval email's body.
///
/// Plain text on purpose: it is four sentences and a link, and an HTML part
/// would only add a way for it to render badly. The community link is omitted
/// entirely when unset rather than rendered as an empty sentence.
pub(crate) fn approval_body(invite_url: &str, community_url: Option<&str>) -> String {
    let mut body = String::new();
    body.push_str(
        "You asked for beta access to Gary, and you're in.\n\n\
         Gary runs game servers for your Discord server — you ask him for what you want in \
         plain English, and he does it. To get started, add him to your Discord server:\n\n",
    );
    body.push_str(invite_url);
    body.push_str("\n\n");
    if let Some(community) = community_url.filter(|url| !url.trim().is_empty()) {
        body.push_str(
            "Come say hello in the community server — it's the best place to ask \
                       questions and tell us what's broken:\n\n",
        );
        body.push_str(community);
        body.push_str("\n\n");
    }
    body.push_str(
        "One thing worth knowing: adding Gary is the first half. Someone still has to switch \
         your server on at our end, so give it a little while before he starts answering.\n\n\
         — Gary\n",
    );
    body
}

#[cfg(test)]
#[path = "tests/mail.rs"]
mod tests;
