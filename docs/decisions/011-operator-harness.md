# ADR-011 — An operator harness for driving the deployed bot

**Status:** Accepted (2026-08-01)

## Context

`CLAUDE.md` is explicit that the deployed instance is the one that counts: "a bot change is verified once it's live on the cluster, not off-cluster." That is the right standard, and it had one consequence nobody chose — the only way to *reach* the deployed bot was Discord, so verifying any change meant a human sitting at a Discord client typing on the agent's behalf. Development was gated on someone else's attention, and the local-run escape hatch (`just bot-start`) doesn't help, because it exercises a different process against a different config.

Two pieces already existed that made a better answer cheap. The bot serves an axum endpoint on `:9360` for the in-game reverse loop ([ADR-005](005-ingame-agent-triggers.md)), with a bearer gate and a ClusterIP Service. And `tools::dispatch` is a single seam that every one of Gary's tools flows through, already enforcing the access tier and the guild-scope gate.

## Decision

**An authenticated HTTP surface on the existing endpoint, not a second Discord identity.** A harness Discord account was the obvious alternative and is structurally incapable of the job: a bot account cannot invoke a slash command, and it cannot click a message component — the two things a destructive confirmation and half the command surface depend on. `kubectl exec` was the other candidate; the container has a read-only root filesystem and, more fundamentally, exec can't reach the *running process's* state (the live gateway, the session store), which is exactly what needs driving.

**Direct tool invocation is the primary route; a model turn is the exception.** `POST /operator/tool` runs one tool by name with JSON arguments and returns the dispatcher's text. Driving an LLM to perform a deterministic action is slow, nondeterministic, and costs tokens to answer a question that has one right answer. `POST /operator/gary` runs a full turn and exists for when Gary's *reasoning* is what's under test; it returns the reply plus a trace of every tool the model called and what each returned — strictly more observable than reading a Discord channel. `GET /operator/tools` returns a tier's schemas, so the caller never guesses an argument name.

**The access tier is a request parameter.** The caller has already proven it holds the operator token, so the tier isn't a security boundary here — it's an instrument. Being able to ask "what does a read-only friend see?" without standing up a second Discord account is the point. The tier still flows through the same `dispatch` gates, so a refusal is the real refusal.

**A separate bearer token, failing closed.** `GAMESERVERS_OPERATOR_TOKEN` is its own OpenBao secret (`grizzly-platform/gameservers/operator → token`), not the in-game token. The in-game token is mounted into every game pod in this namespace by design, so reusing it would mean a compromised game pod inherits Gary's full admin surface — a straight downgrade of ADR-005's posture. And where the in-game route degrades *open* when unprovisioned (NetworkPolicy-only, so the reverse loop doesn't silently break), these routes degrade to **not mounted**: they reach the destructive tools, so running them open is never the safer failure. That is enforced by type — `operator::router` takes a non-optional token.

**Replies land in a real Discord channel.** The turn posts exactly as a real turn does, so chunking, embeds and confirmation cards are genuinely exercised rather than bypassed, and every action the harness takes leaves a trail a human can read after the fact. The HTTP response carries the same content, so the caller doesn't have to read Discord to know what happened.

**Destructive confirmations can be pre-approved, per request.** `destroy`/`archive`/`restore` block on a Discord button click, and no bot can click one — a headless caller would only ever hit the 2-minute timeout, leaving the destructive paths permanently unverifiable. `ConfirmMode::Preapproved` skips the button. It is admin-only, opt-in per request (never a mode the deployment sits in), logged at `warn`, and posts a visible audit line into the channel in place of the button card. If that line can't be posted, **the action is refused** — the visible trail is the entire reason a bot may skip a human's click, so without it there is no justification left. `ConfirmMode::Interactive` is the `Default` and is spelled out explicitly at every other construction site, so a future surface has to *ask* for pre-approval rather than inherit it.

## Consequences

- A change can be shipped and verified without anyone opening Discord, which is what this is for.
- The bot gains three routes on an endpoint that already existed — no new port, Service, or Cilium rule. Ingress to the pod was already unrestricted (the leash is egress-only), and `kubectl port-forward` runs through the apiserver regardless, so no network change was warranted and none was made.
- The token grants Gary's full admin tool surface across every guild. It is a genuinely powerful credential — it belongs only in OpenBao and the bot's own Secret, and the routes stay on a ClusterIP Service, never a NodePort or Ingress.
- The guardrails are unchanged in substance: the namespace RBAC/NetworkPolicy box still contains Gary regardless of entrypoint, and `dispatch` still applies the tier and guild-scope gates. What changed is who can *reach* the entrypoint, not what it may do.
- `destroy` can now run without a human clicking. That is the deliberate trade, bounded by: admin-only, per-request, and refused outright if it can't be recorded in-channel.
- Governance stays in Discord. `/config` and `/gary-home` are poise commands with no tool equivalent, so per-guild grants and home channels are still set from a Discord client. They're one-time-per-guild actions, so the friction is small; a tool surface for them can follow if that stops being true.
- Splitting the HTTP transport out of `ingame/` was a prerequisite, not a side quest: that module owned both the server and its own route, and hanging a serenity-carrying router off it would have made the module name lie.
