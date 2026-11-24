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

const ALIGNED_DOMAIN: &str = "grizzly-endeavors.com";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MailOutcome {
    Sent,
    Unavailable,
    Rejected { reason: String },
    AuthRefused,
    Failed { reason: String },
}

impl MailOutcome {
    pub(crate) fn is_sent(&self) -> bool {
        matches!(self, Self::Sent)
    }

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

#[derive(Debug, Default)]
pub(crate) struct SendGate {
    credential_rejected: AtomicBool,
}

impl SendGate {
    pub(crate) fn is_open(&self) -> bool {
        !self.credential_rejected.load(Ordering::SeqCst)
    }

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

pub(crate) struct Mailer {
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    from: Option<Mailbox>,
    gate: Arc<SendGate>,
}

impl Mailer {
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

    pub(crate) fn is_enabled(&self) -> bool {
        self.transport.is_some() && self.gate.is_open()
    }

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

    fn classify(&self, err: &lettre::transport::smtp::Error) -> MailOutcome {
        if err.is_permanent() {
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

pub(crate) const APPROVAL_SUBJECT: &str = "You're in — Gary beta access";

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
