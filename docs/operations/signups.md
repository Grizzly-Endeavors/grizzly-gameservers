# Operating the beta signup queue

Where beta applicants come from, how to let them in, and the contract this repo owes the landing service.

Design: [ADR-029](../decisions/029-signup-intake-through-the-bot.md) (intake and data model), [ADR-030](../decisions/030-transactional-mail-fails-closed.md) (mail).

## Before the page goes public

**Populate the guild allowlist first.** `guild_is_admitted` treats an empty allowlist as admitting *every* guild — the safe default for an unadvertised deploy, and an open door the moment a public page starts handing out bot-invite links. Anyone who reads the client id off an invite link could add the bot and get full service without ever touching the queue.

1. In the panel's **Guilds** page, click *Serve* on each guild that should be served. The first click bounces for confirmation, because it flips the service from "serves everyone" to explicit admission — that confirmation is the moment to do this deliberately.
2. Also set `discord-bot/guild_allowlist` in 1Password, so a boot with no database falls back to the explicit list rather than to open.

The **Signups** page warns whenever the allowlist is empty, and approving in that state takes an extra checkbox. Both exist so this can't be forgotten quietly.

## The flow

1. Someone fills in the form at `gary.grizzly-endeavors.com`. They can tick **beta**, **updates**, or both — the two are independent.
2. The landing service POSTs it to the bot, which appends a row.
3. Beta requests appear under **Waiting on you** on the panel's Signups page, oldest first.
4. **Approve** records the decision and mails them the bot-invite link. **Deny** takes the address typed back as confirmation, because it is the irreversible half.
5. They add the bot. The guild shows up on the **Guilds** page as *not admitted*.
6. You click **Serve**. That is the moment they actually get access.

Step 6 is the real gate. The approval email is a convenience, not a capability — Discord offers no way to restrict who installs a public bot, and the client id is readable off any invite link.

## Settings

Under **Settings → Beta signups**:

- `signups.invite_url` — where an approved applicant is sent to add the bot. Carries the client id and permissions bitmask, so it changes whenever the bot needs a new scope. **Approving is refused while this is empty**, because an email pointing nowhere cannot be unsent.
- `signups.community_url` — optional. Unset omits that paragraph from the email entirely.

Both are `https`-only and stored exactly as typed.

## When mail fails

The Signups page grows an **Approved, not emailed** section: people who were let in but never told, with the reason and a **Resend** button. Work it whenever it isn't empty — Resend only sends, so it is safe to click twice.

If the reason says the credential was refused, sending is off for the life of the process by design ([ADR-030](../decisions/030-transactional-mail-fails-closed.md)). Fix `gary_password` on the `platform-stalwart` 1Password item and redeploy; a restart is required to clear the latch.

If sends fail with a connection reset (`errno 104`), the bot dialed a LAN address. Check that `mail.grizzly-endeavors.com` still resolves publicly — a split-horizon DNS record pointing it at a LAN IP is the cause, not the guardrails.

## The contract the landing service depends on

`POST /signups` on the bot's agent endpoint (port 9360), in-cluster only, `Authorization: Bearer <GAMESERVERS_SIGNUP_TOKEN>`:

```json
{ "email": "friend@example.com", "wants_beta": true, "wants_updates": false, "note": "optional" }
```

Answers `200 {"recorded": true, ...}` on success, `400` for a bad address or a submission that opts into nothing, `503` when there is nowhere to record it (retry is genuine).

Properties the landing service can rely on, and must not break:

- **Resubmission is safe.** The same address upserts; opt-ins only ever turn *on*, and a resubmission can never disturb a decision an operator already made.
- **The response never reveals whether the address was already known.** Keep it that way — a form that answers differently is an enumeration oracle.
- **Email is stored as typed and matched lowercased.** Don't pre-normalize; send what the person entered.
- **`note` is bounded on the way in.** Anything longer is truncated, not rejected.
- **The bot applies the schema at startup**, so the table exists after its first boot on a release carrying it. Nothing else has to be provisioned — no database role, no grant.

Changing the row shape means changing `crate::signups`'s schema constant in this repo first.

## Reading the numbers

`/events` carries `beta_approved`, `beta_denied`, and `signup_emailed`, each with the applicant's email **domain** and never the address. `signup_emailed` is separate from `beta_approved` on purpose: a Resend produces its own event, and "approved but never told" stays visible as a gap between the two counts.

`just db-query signup-funnel` walks submitted → beta-requested → approved → emailed → guild joined. The updates list is downloadable as CSV from the Signups page; it deliberately excludes the free-text note, because a CSV field beginning `=` is a formula in Excel.
