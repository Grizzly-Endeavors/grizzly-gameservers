# cluster/kyverno

**The bot is gated; the full namespace is not (yet).**

The gate/cosign/Kyverno path signs *our* images. This namespace (`game-servers`) is labelled `grizzly.io/gated=true` and the bot image is enforced at admission by a bot-scoped Kyverno policy that lives in `grizzly-platform` (`kubernetes/infrastructure/kyverno-policies/enforce-gameservers-bot-signature.yaml`). That policy targets exactly `registry.registry.svc.cluster.local:5000/grizzly-gameservers@*`, so:

- **Bot pod** — its gate-signed, digest-pinned image must carry a valid signature or it is refused. This is the enforced deploy boundary (see ADR-003).
- **Agones SDK sidecar** — a `gcr.io` Google image. The policy's `imageReferences` glob never matches it, so it is admitted untouched. No carve-out needed.
- **Game-server (supervisor) images** — `…/grizzly-gameservers-minecraft@…`. Excluded by the glob (the char after `grizzly-gameservers` is `-`, not `@`), so on-demand game allocation keeps working. The platform's broad `verify-gate-signature` policy stays in **Audit**, so these only produce report-only entries here.

## Remaining work to gate the whole namespace

To flip the *entire* `game-servers` namespace to enforced signatures (not just the bot):

1. Gate-sign game/supervisor images in CI (extend the gated build to `scripts/push-game-image.sh`'s output).
2. Add a real Kyverno policy **exception** for the Agones upstream images (`agones-system` operator + the injected SDK sidecar) — or keep the operator out of a gated namespace.

Until both exist, full-namespace enforcement would block game allocation, so it stays a documented follow-up. See `docs/design/00-overview.md` → "Image admission carve-out".
