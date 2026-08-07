# ADR-030 — Transactional mail fails closed on a refused credential, and never retries

**Status:** Accepted (2026-08-07)

## Context

Approving a beta signup has to tell the applicant, and the only address we have is an email address. Until now the bot had no way to send mail at all.

The platform runs Stalwart ([grizzly-platform ADR-050](https://github.com/Grizzly-Endeavors/grizzly-platform)), relaying outbound through SMTP2GO with aligned SPF/DKIM/DMARC. Its integration doc carries two constraints that shape everything here, and one warning that shapes it more.

The constraints: submission must dial the **public** host even from inside the cluster, because the listeners trust PROXY-protocol from `10.0.0.0/8` — which covers the pod network — so a direct in-cluster connection is parsed as a PROXY header and reset with `errno 104`. And the `From:` must be on `grizzly-endeavors.com` or DMARC alignment fails and the message is filtered rather than bounced, which is a silent failure.

The warning: *"Do not let your app retry into this. A mailer that retries a bad credential will ban mail for the whole house."* Submission hairpins out to the VPS, so Stalwart's per-IP auto-ban sees the household's public WAN address, not a pod IP — the `10.0.0.0/8` allowlist does not cover it.

## Decision

**lettre 0.11.23**, `default-features = false` with the tokio transport, `tokio1-native-tls`, and no connection pool. native-tls rather than rustls to unify with the backend serenity is pinned to and reqwest already pulls; a fourth TLS configuration would risk `deny.toml`'s `multiple-versions` for nothing. No pool because the volume is a handful of human-clicked sends, and a warm connection held against a per-IP-auto-banning server is a liability rather than an optimization.

**No retry at any layer, and the first authentication refusal latches sending off for the life of the process.** `SendGate` is a one-way `AtomicBool`; once closed, every send returns `Unavailable` without opening a socket, and it logs the remedy once. A plain no-retry rule would still let three operators clicking Approve produce three authentication attempts — which is exactly the pattern the auto-ban watches for. Recovery is deliberately manual: fix the credential in 1Password and restart the pod. That is honest, because the credential arrives via ESO at startup anyway.

**The decision is written before the mail is sent.** An approval that isn't recorded returns to the queue and gets approved twice, which means two emails to a stranger; a recorded approval whose mail failed is a visible, fixable state. The panel grows an **"Approved, not emailed"** section listing `status = 'approved' AND emailed_at IS NULL` with the failure reason and a **Resend** button. That section is why a failed send cannot be silently lost, and it is the only recovery path.

**Mail is `Option`-shaped like every other credentialed subsystem.** No `GAMESERVERS_SMTP_PASSWORD` means approvals are still recorded and simply not mailed, with a warning on the signups page and a row on the dashboard's subsystem table. The bot never crash-loops over mail.

**There is deliberately no egress carve-out.** `mail.grizzly-endeavors.com` resolves to the VPS's public address, and the namespace leash already permits the public internet — unlike Postgres, Valkey, S3, and Langfuse, all of which needed carve-outs precisely because they sit inside RFC1918. `cluster/guardrails/README.md` says so explicitly, so the absence reads as a decision rather than an oversight.

**Sender is `gary@grizzly-endeavors.com`**, its own submission account per the platform's one-account-per-sender rule so it can be revoked independently. Named for the product rather than the repo, because a `From:` header is prose a person reads (ADR-028).

## Consequences

A wrong credential costs every approval email until someone notices and redeploys — strictly worse for this feature than retrying, and strictly better for every other thing on the domain that sends mail. That trade is the decision.

The mailer is reusable beyond signups: `Mailer::send` takes a recipient, subject, and body, and knows nothing about the queue.

The latch is process-scoped, so a restart clears it whether or not the credential was actually fixed. A pod that keeps restarting could therefore keep retrying at roughly its crash cadence. Acceptable — that cadence is minutes, not the tight loop the ban targets — but worth remembering if the bot ever gets a crash-loop.

Startup warns when the `From:` domain isn't `grizzly-endeavors.com`. It cannot be an error, because the value is operator-settable and refusing to boot over a marketing address would be worse than sending mail that lands in spam.

## Alternatives considered

**Retry with backoff.** Structurally forbidden by the auto-ban hazard. Backoff does not help: the ban counts attempts, and a bad credential fails identically every time.

**A background send queue.** The operator would click Approve and learn nothing about whether the mail went, which is the opposite of what this UI is for. The Resend button covers the same recovery with the failure visible.

**Latch on any permanent error, not just authentication.** Over-broad: a single undeliverable address would stop mail for everyone else in the queue.

**`mail-send` (Stalwart Labs' own crate).** A neat fit on paper given the server, but lettre has an order of magnitude more use and a release three days before this was written. Reconsider if lettre's maintenance changes.
