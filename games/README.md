# games — per-game catalog

**Placeholder — no implementation yet.**

The per-*game* config tier (distinct from per-*instance* config, which lives on the PVC and is mutated live by the ops agent — never here). See `docs/design/00-overview.md` → "Config: two tiers".

Each `games/<game>/` directory declares the base template for one game: container image, default env, port shape, resource sizing, persistence needs — expressed as an Agones `GameServer`/`Fleet` template. These are version-controlled, gated, and Flux-deployed; they change rarely and can be authored by an AI coding agent at dev time via PR.

`_template/` is the skeleton a new game copies. The exact catalog format is an open decision (see the design doc).
