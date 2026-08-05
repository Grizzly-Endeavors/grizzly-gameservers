# ADR-025 — A super-admin panel, on its own port behind Authentik

**Status:** Accepted (2026-08-05)

## Context

[ADR-024](024-runtime-settings.md) made the operator's knobs changeable at runtime, but left no way to change them. The settings table has no writer, and per-guild caps are reachable only through `/config limits`, which is operator-gated for good reason (ADR-022: the tier that grants everything *inside* a guild must not reach the knobs that bound the guild itself) and therefore awkward — an operator in ten guilds edits caps from whichever one they happen to be in.

The audience is one person and the questions are operational: which guilds am I serving, what is each one using against its cap, which model is Gary on, what happened in the last hour. Discord is the wrong surface for all of it. It is a chat client — a table doesn't fit, a form doesn't exist, and the commands would be visible to guild admins who must not have them.

[ADR-011](011-operator-harness.md) built the operator harness for a related need and concluded its routes "stay on a ClusterIP Service, never a NodePort or Ingress." That reasoning was about a *bearer token* surface driven by a coding agent over `kubectl port-forward`. A human wanting to look at a dashboard from a phone is a different problem, and port-forwarding is not an answer to it.

## Decision

**A server-rendered web panel at `gameservers-admin.grizzly-endeavors.com`, gated by Authentik, served by the bot process itself.**

**In the bot binary, not a separate service.** Every DB-backed setting is served from an in-process cache — `GuildLimits`, `InstanceSizing`, `RuntimeSettings` all load once and write through. A separate service writing Postgres directly would be invisible to the running bot until its next restart, so it would need an API on the bot anyway; building that API *plus* a second deployable, chart, and CI path buys isolation we don't need at this size and doubles the surface.

**Its own listener on port 9361, not a route on the agent endpoint.** This is the load-bearing decision. Port 9360 must stay reachable by game pods for the in-game `@Gary` trigger, and a game pod runs a third-party game-server binary. The panel learns who is asking from request headers, so it has to sit where only ingress-nginx can reach it — and "only ingress-nginx may reach the panel" and "game pods may reach the agent endpoint" cannot both be said about one port. Two ports let one `CiliumNetworkPolicy` say exactly that.

**Three stacked controls, not one.** This is the considered departure from ADR-011:

1. **Authentik forward-auth at the ingress** — the same four-annotation pattern Metabase, Roundcube, and grizzly-invite use, with a `grizzly-admins` policy bound to the application in the blueprint. ingress-nginx *overwrites* the `X-authentik-*` headers on every forwarded request, so the front door can't be forged.
2. **A `CiliumNetworkPolicy` admitting only ingress-nginx to 9361** — the actual in-cluster boundary, and what makes trusting those headers sound.
3. **An in-process group check** — defense in depth, and the only one of the three that can render a page telling a signed-in non-member *which group they're missing* rather than a bare 403 from a proxy they've never heard of.

**The listener does not bind unless `GAMESERVERS_ADMIN_PORT` is set.** Fail-closed by absence, the same posture as the operator token: a panel reachable but misconfigured is worse than one never served, and "unset means off" is not something a half-rolled deploy can get wrong.

**Server-rendered Rust (`maud`), no JavaScript toolchain.** Templates are Rust macros, so a page referencing a field that stopped existing fails the build rather than at render time, and the markup is embedded in the binary — which the pod's `readOnlyRootFilesystem` requires anyway. A single-operator ops page does not need a framework, and adding npm to a pure-Rust workspace would put a second build system in front of the gate.

**Mutations answer with a redirect (POST → 303 → GET).** A reload can't repeat an action and the back button lands on a view rather than a resubmission prompt. The result rides across as a query parameter, because there is no server-side session to hang a flash on and a message that survives exactly one navigation is all this needs.

**CSRF is handled by `SameSite` plus an `Origin` check, not a token.** The outpost's session cookie is `SameSite=Lax`, so a cross-site form POST arrives cookie-less and dies at the ingress' auth subrequest before reaching this process. The `Origin` comparison is the second, independent check — it keeps working if that cookie policy ever loosens, and costs one header comparison. A synchroniser token would additionally survive a browser sending neither header, which no browser in use does, at the cost of the session store this design otherwise doesn't need.

**Actions call the shared lifecycle functions, never Gary's tool dispatcher.** A tool result is model-facing prose and expects a Discord channel and tier the panel hasn't got. Going through the shared functions is also what makes ADR-022's caps apply to a panel-driven cold start for free — the panel inherits enforcement rather than working around it.

**Every mutation is recorded** with `Surface::Admin` and `Actor::Admin(username)`, deliberately distinct from `Surface::Operator`: the harness is a coding agent driving the bot, this is a human clicking a button, and telling them apart is what keeps "what did I change by hand" answerable.

## Consequences

- The knobs that used to need a redeploy — the guild allowlist above all, since a friend is waiting while it is edited — become a form. Admission changes bind immediately for Gary and sweep Discord's registered command list.
- The bot pod gains its **first ingress policy**, which flips it from allow-all to default-deny inbound. Every legitimate flow (kubelet probes, `kubectl port-forward`, game pods → 9360, ingress-nginx → 9361) is enumerated in `cluster/guardrails/bot-ingress.yaml`; a missed one is a silent Cilium drop, not an error, which is why that file lists them explicitly rather than relying on Cilium's `allow-localhost` default.
- Anyone holding cluster credentials can port-forward 9361 and present headers of their own. Accepted: that person can already read the panel's database and edit its deployment, so the boundary is drawn at the cluster edge rather than inside it.
- The bot process now serves a page on the public internet. It was already an HTTP server, but only to callers inside the cluster — a panel bug is now reachable by anyone who can get past Authentik.
- Two repos move together: the chart, the network policy, and the panel live here; the Authentik proxy provider, application, and policy binding live in grizzly-platform's blueprints. Neither half works alone.
- The panel is a second surface that must keep pace with the settings catalog. The catalog's exhaustive matches make that a compile error rather than a missing row, which is the property that keeps them from drifting.

## Alternatives rejected

- **A separate web service.** Cleaner isolation, but it can't reach the in-process caches, so it needs an API on the bot regardless — and then it is a second deployable, chart, image, and CI path for the same result.
- **Routes on the existing agent endpoint (9360).** One fewer port and one fewer Service, at the cost of the property the whole design rests on: that port is reachable by game pods, so nothing served there can trust a request header.
- **Extending the operator harness instead.** It is bearer-token, JSON, and port-forward-only — right for a coding agent, wrong for a person with a browser. Reusing it would mean putting a shared secret in a browser and inventing a session anyway.
- **Discord slash commands for the operator knobs.** Where `/config limits` already is, and the reason it's awkward: no tables, no forms, and the commands are visible in a guild whose admins must not have them.
- **A React or similar SPA.** Richer interaction, at the price of npm in a Rust workspace, a second build system in front of the gate, and a dependency tree to keep patched — for pages that are almost entirely tables and forms.
- **No network policy, relying on forward-auth alone.** Leaves any pod in the cluster — including the game pods running third-party binaries — able to reach the panel with forged headers. The headers are only trustworthy because of where the port can be reached from.
- **Basic auth or a shared link secret.** No per-person identity, nothing to revoke, and no audit trail worth the name. Authentik already exists and already knows who the admins are.
