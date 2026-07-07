# games — per-game catalog

The per-*game* config tier (distinct from per-*instance* config, which lives on the PVC and is mutated live by the ops agent — never here). See `docs/design/00-overview.md` → "Config: two tiers".

Each `games/<game>/` directory declares the base template for one game: container image, default env, port shape, resource sizing, persistence needs — expressed as an Agones `GameServer`/`Fleet` template plus a NodePort `Service`. These are version-controlled, gated, and Flux-deployed; they change rarely and can be authored by an AI coding agent at dev time via PR. Rendered by the `grizzly-gameservers-games` Flux Kustomization (`path: ./games`).

- `minecraft/` — the first entry and the bring-up validation server: a standalone Agones `GameServer` (composite image: grizzly-supervisor as PID 1, wrapping itzg/minecraft-server and driving the Agones SDK `/ready` + `/health`), a `PersistentVolumeClaim` for world state, and a `NodePort` Service on `7000`. Currently a single live instance; promoting to a `Fleet` + shim-managed per-instance NodePort Services is a follow-up once the shim exists.

`_template/` is the skeleton a new game copies. The catalog format is being established by `minecraft/`.
