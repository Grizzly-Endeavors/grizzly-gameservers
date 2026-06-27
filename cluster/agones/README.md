# cluster/agones

**Placeholder — no implementation yet.**

Agones operator standup, gated here on purpose (the gate vets external repos before deploy, so security-sensitive standup belongs where the gate sees it — not in `grizzly-platform`). See `docs/design/00-overview.md` → "Gate + Flux integration".

What will live here:

- Agones install (Helm values / HelmRelease — packaging decision is open: dependency of the `deploy/` chart vs. separate gated release; see Open Decisions in the design doc).
- The configured dynamic port range, which must match the edge range (**7000–7010**) forwarded by `grizzly-platform` ansible.

CRD-before-CR ordering must be expressed however this gets packaged so a cold sync doesn't apply GameServer/Fleet CRs against missing CRDs.
