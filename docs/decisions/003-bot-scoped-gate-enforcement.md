# ADR-003 — Bot-scoped gate-signature enforcement

**Status:** Accepted (2026-06-28)

## Context

grizzly-gameservers is the first first-party app to be enforced by grizzly-gate at the deploy boundary. The ADR-020 delivery model is now wired: on push to `main`, CI builds the bot image, the centrally-owned gate cosign-signs the digest on a clean pass, the signed digest is pinned into `deploy/values.yaml`, and Flux reconciles it. The remaining question was *how much* of the `game-servers` namespace to enforce.

The namespace is not homogeneous. Alongside the bot Deployment (a first-party image we build and sign), it hosts on-demand Agones GameServer pods. Each game pod runs a composite supervisor image (`…/grizzly-gameservers-minecraft@…`) that is **not** gate-signed today, and Agones injects an unsigned third-party SDK sidecar (a `gcr.io` Google image) into every game pod. Kyverno's `verifyImages` matches on image references; a namespace-wide enforce rule scoped to `registry.registry.svc.cluster.local:5000/*` would refuse unsigned game-supervisor images and break game allocation — the core product feature.

## Decision

Enforce the gate signature on the **bot image only**, via a dedicated Kyverno `ClusterPolicy` in grizzly-platform (`enforce-gameservers-bot-signature`) whose `imageReferences` is exactly `registry.registry.svc.cluster.local:5000/grizzly-gameservers@*`. Label `game-servers` with `grizzly.io/gated=true` to opt it in.

The glob is deliberately precise:

- It matches the bot's digest ref (`…/grizzly-gameservers@sha256:…`).
- It does **not** match game-supervisor images (`…/grizzly-gameservers-minecraft@…` — the next char is `-`, not `@`).
- It does **not** match the Agones SDK sidecar (`gcr.io/…`).

So no Agones carve-out is required for this phase. The platform's broad `verify-gate-signature` policy stays in **Audit** (report-only), so unsigned game images in this namespace generate policy reports but are still admitted.

Enforcement is enabled only after the first gate-signed bot image is live, so an already-running, now-signed bot is never blocked mid-rollout.

## Consequences

- The bot ships through a genuinely enforced gate: an unsigned bot image is refused at admission, not merely reported. This is the "first project gated by grizzly-gate" milestone.
- Game allocation is unaffected — game-supervisor and Agones images are outside the enforced glob.
- The signature check is digest-based, so the chart must pin the bot by digest (`repository@digest`) rather than a mutable tag; `deploy/values.yaml` gained an `image.digest` field that CI overwrites, and `deploy/templates/deployment.yaml` renders the digest ref when set (falling back to the `:dev` tag for the local `just bot-push` loop).
- Full-namespace enforcement is deferred: it requires gate-signing game/supervisor images in CI **and** a real Kyverno exception for the Agones upstream images. Tracked in `cluster/kyverno/README.md`.
- Reversible: deleting the `enforce-gameservers-bot-signature` policy (or removing the namespace label) drops back to Audit.
