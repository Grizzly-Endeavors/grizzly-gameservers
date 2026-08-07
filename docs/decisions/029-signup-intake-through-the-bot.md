# ADR-029 — Signup intake goes through the bot, not a second database writer

**Status:** Accepted (2026-08-07)

## Context

`gary.grizzly-endeavors.com` is a public marketing page with a beta-signup form. Something has to turn a form submission into a row an operator can approve.

The obvious shape is to let the landing service write to Postgres directly. It is a small service, the schema is small, and a role scoped to `INSERT` on one table is a narrow grant. But it puts a database credential in the one component reachable from the internet, and the narrowness stops being free the moment resubmission has to be idempotent: an `ON CONFLICT DO UPDATE` upsert needs `UPDATE` on the table, and `UPDATE` on the table means the public-facing service can flip anyone's approval status. Avoiding that means splitting into an append-only submissions table plus a bot-owned decisions table, which is real complexity bought purely to keep a grant honest.

It also drags in ordering. The bot applies its schema at startup (there is no migration tool here — see `crate::signups`), so the table does not exist until the bot has booted once, and a `GRANT` cannot precede the table. Deploying the landing service would have to wait on a bot rollout, with a sequence nobody would remember six months later.

The bot already accepts authenticated inbound HTTP from in-cluster peers: `crate::http` mounts independent sub-routers on the agent port, each gated by its own `BearerGate` — the in-game `@Gary` trigger path and the operator harness (ADR-011). A third one is a well-worn path, not a new posture.

## Decision

**The landing service holds no database credential. It POSTs each submission to a bearer-gated intake route on the bot, which is the sole writer.**

`POST /signups` on the agent endpoint (`crate::http::signup`), gated by `GAMESERVERS_SIGNUP_TOKEN` — a third credential, distinct from the in-game token (mounted into every game pod) and the operator token (which reaches Gary's full admin surface). Its entire capability is appending a signup, and that narrowness is what lets an internet-facing service hold it. The route mounts only when the token *and* a database are both present: a token with no database would accept submissions and drop them, losing people who asked for access.

`cluster/guardrails/bot-ingress.yaml` gains a fourth enumerated inbound flow, selecting the landing deployment by label. That file is default-deny, so this is required, not decorative.

**One table, with pending-ness as the absence of a status.** A single writer removes the privilege argument for splitting, so `signups` carries the address, the two independent opt-ins, the decision, and the mail result. `status IS NULL` means nobody has looked at it — the same idiom `known_guilds` uses, and one fewer value to get wrong than a `'pending'` string. Deciding is `UPDATE ... WHERE status IS NULL RETURNING email`, so the row count answers "did somebody get here first?" with no read-then-write race; two tabs cannot produce two approval emails.

**The intake route answers identically whether or not the address was already known.** A public form that responds differently for a registered address is an enumeration oracle, and this one is reachable by anyone who can reach the page.

**No signup address appears in a URL.** Not a path segment, not a query string, not a flash message. Query strings and paths land in the ingress access log and in `Referer` headers — a different trust boundary from the page body, which is behind Authentik and rendered once. The panel's decision actions therefore take the address in the POST body, which is why they use a single `/signups/{action}` route rather than `/signups/{email}/approve`.

**Product events carry the address's domain, never the address.** `events` is browsed in the panel, exported to Metabase, and read through a `SELECT`-only analytics role; solicited contact details are hard to un-spill once they reach the aggregate plane. The audit trail for who was approved and when lives on the `signups` row (`decided_by`, `decided_at`), behind Authentik.

## Consequences

The landing service becomes trivial to reason about: one outbound call, one bearer token, no schema knowledge beyond a JSON shape. Rotating the credential restarts two pods rather than re-issuing a database role. No `GRANT`, no sequence-grant trap, no deploy ordering.

Signups now depend on the bot being up. A rolling update overlaps pods so the window is small, and the landing service surfaces a `503` as "try again in a few minutes" rather than claiming success.

"Which approval failed?" is not answerable from `/events` alone — the domain is there but not the address, so it takes one click through to `/signups`. Accepted deliberately.

Adding a fourth token to the endpoint is a small ongoing tax: each is a separate 1Password item, ExternalSecret, and env var. The alternative — one shared inbound token — would mean the landing service's credential also opened the operator harness.

## Alternatives considered

**Landing service writes to Postgres directly.** Rejected above: a database credential on the internet-facing component, `UPDATE` needed for idempotent resubmission, a two-table split to compensate, and a `GRANT`-after-first-boot ordering constraint.

**Browser posts straight to the bot.** No landing-service hop at all. Rejected: it makes the most secret-dense pod in the namespace publicly reachable, and `bot-ingress.yaml` had just finished making it default-deny inbound.

**Cloudflare Pages plus a form service.** Zero infrastructure, but signup data leaves the platform and the admin panel cannot read it, which is the whole requirement.

**A `pending` status string instead of `NULL`.** One more value the writer can get wrong, and it makes "never looked at" and "decision failed to write" distinguishable in a way nothing needs.
