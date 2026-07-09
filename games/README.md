# games — per-game catalog

The per-*game* config tier (distinct from per-*instance* config, which lives on the PVC and is mutated live by the ops agent — never here). See `docs/design/00-overview.md` → "Config: two tiers".

Each `games/<game>/` directory declares the base template for one game: container image, default env, port shape, resource sizing, persistence needs — expressed as an Agones `GameServer` template plus a NodePort `Service` and a `PersistentVolumeClaim`. These are version-controlled, gated, and change rarely; they can be authored by an AI coding agent at dev time via PR.

**Everything is provisioned on demand.** The bot bakes this whole directory into its image and reads every `games/<id>/` at runtime as its catalog. When a friend issues a Discord command, the bot renders that game's three templates into a uniquely-named instance on a leased NodePort (`7000–7010`), then tears it down when they're done — the template's literal name and nodePort are placeholders the renderer overrides per instance. So a game is provisionable as soon as its directory exists (and its image is pushed); it does **not** need to be listed anywhere else.

`games/kustomization.yaml` is a separate, deliberately-empty concern: it's the *always-on* set that the `grizzly-gameservers-games` Flux Kustomization stands up 24/7. Nothing lives there — the model is all-dynamic. It once held a standalone `minecraft` server used to prove bring-up; that was decommissioned once dynamic provisioning worked. Add an entry there only to resurrect a genuinely always-on server.

Catalog entries:

- `minecraft/` — the reference entry (composite image: grizzly-supervisor as PID 1 wrapping itzg/minecraft-server, driving the Agones SDK `/ready` + `/health`), with native RCON wired for Gary's `send_command` and the in-game `@Gary` chat loop.
- `factorio/`, `palworld/` — native Source RCON, so Gary can issue live console commands. Palworld caps its admin/RCON password at 30 chars, handled by the supervisor's `SUPERVISOR_RCON_PASSWORD_MAX_LEN`; Factorio takes its password from a file, seeded by a small wrapper.
- `terraria/`, `valheim/`, `satisfactory/` — no console protocol the supervisor speaks, so they run **without** live commands (Gary still reads/writes config and restarts). Terraria is vanilla (no RCON); Valheim is UDP-only and signals readiness off a log marker (`SUPERVISOR_READY_LOG_PATTERN`); Satisfactory comes up and is manageable but full internet multiplayer needs the two-port provisioner follow-up (issue #29).

Readiness: the supervisor calls Agones `/ready` only once the game is actually up. Most games prove that with a TCP connect to a port that opens on boot (the game's own TCP port, or its RCON/API port); a UDP-only game with no TCP port at all (Valheim) uses a log-line marker instead. Games with a long first-boot SteamCMD download raise the probe's give-up via `SUPERVISOR_READINESS_TIMEOUT_SECS`.

`_template/` is the skeleton a new game copies.
