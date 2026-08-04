# ADR-018: Terraria runs the TShock flavor

## Status

Accepted.

## Context

Terraria was the one game in the catalog that neither auto-updated nor reported occupancy.

Every other game restarts into a fresh build: the SteamCMD games (Palworld, Valheim, Satisfactory) re-fetch on launch, and Minecraft and Factorio resolve their version at boot. Terraria's vanilla image bakes the server binary at build time — no version argument, no latest-on-boot — and Terraria isn't distributed through SteamCMD, so there is no "restart means update" path to lean on. Bumping it meant a catalog PR.

It also has no RCON. Update-on-empty only engages for a game whose player count the supervisor can read, so Terraria could never join it, and Gary's check-before-restart reported `unknown` — which the guidance correctly treats as "possibly occupied", so a Terraria restart always had to ask a human first.

Two options were live: hand-roll a version fetch in the start wrapper (scrape terraria.org, cache the zip on the PVC, exec the fetched binary), or move to the TShock flavor of the same upstream image.

Investigating the second turned up something that reframed the choice: **the digest already pinned in `games/terraria/Dockerfile` was the TShock image.** Upstream points `ryshe/terraria:latest` at TShock, the pin was taken from `latest`, and the comment beside it said "vanilla". The wrapper was driving it as vanilla — `cd /terraria-server && exec ./TerrariaServer`, a path that does not exist in that image. Terraria therefore had two independent reasons it could not boot: this, and the glibc mismatch in #68.

## Decision

Terraria runs TShock, deliberately and by an explicit `tshock-` tag rather than by whatever `latest` happens to be.

TShock buys both missing capabilities at once. Its builds ship as GitHub releases, so a restart re-pulls a predictable latest instead of running a binary frozen at image-build time. And its REST API reports a live player count, so Terraria joins update-on-empty and Gary can see who is on before restarting.

Occupancy reads `GET /v2/server/status`, which is one of the few TShock endpoints that takes no token — so the supervisor mints, stores, and rotates no credential to answer "how many players". The REST API ships disabled, so `terraria-start.sh` asserts `RestApiEnabled` and `RestApiPort` in `config.json` on **every** boot rather than seeding once: a friend or Gary editing that file must not be able to quietly switch off the signal the auto-updater and check-before-restart depend on.

This generalises the occupancy plumbing rather than special-casing Terraria. `OccupancySource` is now RCON, the Satisfactory API, or the TShock API, selected per image by `SUPERVISOR_OCCUPANCY_API` — and an unrecognised value is a startup error, because the dialects are different protocols and a typo that silently picked the wrong one would report unknown occupancy forever with nothing naming the cause.

## Consequences

- **Terraria is a modded server now.** TShock brings plugins, permission groups, user accounts, and its own config surface. Players get `/setup`-style admin flows and TShock's login behaviour rather than vanilla's. This is a gameplay change, not just plumbing, and it is the price of the two capabilities above.
- Restart means update for every game in the catalog. Terraria is no longer an exception, and no longer needs a catalog PR to move builds.
- Terraria reports a real player count, so it joins update-on-empty and Gary stops asking a human before every restart.
- The REST port (7878) is pod-internal: never added to the NodePort Service, and the `game-servers` NetworkPolicy blocks pod-to-pod traffic, so only the supervisor beside it in the pod can reach it. The image exposes it, which is why the manifest must not.
- **An existing vanilla world is not migrated.** TShock reads vanilla `.wld` files, so a world carries over, but the config does not and player state (TShock accounts, groups) starts empty. No live Terraria server existed to migrate — it could not boot — so this cost was not paid in practice.
- The `latest`-tag trap is closed by naming the flavor in the tag on every bump. It is worth noting that pinning by digest did not prevent it: the digest was correct and immutable, and still meant something other than the comment claimed.
