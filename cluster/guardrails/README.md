# cluster/guardrails

**Placeholder — no implementation yet.**

The ops agent's leash. Lives in this repo (co-located with the agent code) so the code and what it's allowed to touch move in one reviewed, gated PR. See `docs/design/00-overview.md` → "Ops-agent guardrails".

What will live here:

- The game-server `Namespace`.
- Scoped `RBAC` for the agent — only the game-server namespace, only config dirs / pod exec / restart. Never cluster-wide.
- `NetworkPolicy` isolating game-server pods so a compromised or prompt-injected agent/server can't pivot into the rest of the platform. This is the guardrail that matters most.
