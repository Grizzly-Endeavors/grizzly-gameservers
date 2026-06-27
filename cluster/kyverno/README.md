# cluster/kyverno

**Placeholder — no implementation yet.**

Image-admission carve-out for Agones' third-party images. See `docs/design/00-overview.md` → "Image admission carve-out".

The gate/cosign/Kyverno path signs *our* images. Agones is third-party and injects an SDK sidecar (a Google image) into every GameServer pod, so even a signed game-server image shares a pod with an unsigned one. In a `grizzly.io/gated=true` namespace, Kyverno would bounce the sidecar and the operator.

What will live here: a scoped Kyverno policy exception for the Agones upstream images (`agones-system` operator + the injected SDK sidecar) — or document the alternative of keeping the operator out of a gated namespace.
