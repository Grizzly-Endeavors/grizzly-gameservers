# ADR-038 — Valheim's peer handshake is guarded against a null string in the image, not worked around in config

**Status:** Accepted (2026-09-10)

## Context

Valheim shipped 1.0 on 2026-09-09 with the largest networking change in the game's history. Every Valheim instance this platform created that day was unjoinable: a friend connecting reached the loading screen and stayed there. Nothing in the catalog, the supervisor, or the mod list had changed.

The server-side account is unambiguous. On each connect the game throws inside the handshake:

```
ArgumentNullException: Value cannot be null. (Parameter name: value)
  at System.IO.BinaryWriter.Write (System.String value)
  at ZPackage.Write (System.String data)
  at ZNet.WritePlayerInfo (System.Collections.Generic.List`1[T] playerInfoList)
  at ZNet.SendPlayerList ()
  at ZNet.RPC_PeerInfo (ZRpc rpc, ZPackage pkg)
```

`RPC_PeerInfo` is the peer handshake. It dies partway through — after the peer is registered enough to appear in the player list, before the step that streams the world to it. The client is therefore online, at position `(0 0 0)`, with no character, forever. The same null then re-throws every two seconds from `SendPeriodicData`, so the player list never syncs either.

The null is `PlayerInfo.m_serverAssignedDisplayName`. It is the one string in the struct that the baked RCON plugin's `players` command does not print (it prints name, host and platform display name, all populated), and `m_gamerTag`/`m_networkUserId` — the fields older Valheim builds carried — are gone from `assembly_valheim.dll` in this build. It is assigned from the server's PlayFab session, and on a Steam-only server that session is never established: the login stalls at `Login postponed until ID has been set.` and never sets the custom ID.

Two config-level answers were tested against the live server and both failed:

- **`SERVER_PUBLIC=0`.** The stall is not caused by the community listing. PlayFab still logged `login request (attempt 1)`, still never completed, and the next join threw the same null.
- **`CROSSPLAY=true`.** This *does* fix the underlying cause — the server sets its custom ID, logs in, opens a PlayFab server and registers a session. But it moves connections onto the PlayFab Party relay and registers the server at its NAT address and container port (`<home NAT address>:2456`), not the NodePort this platform leases. The joining client could not reach the server at all: no `New connection`, no handshake, nothing server-side. Crossplay is incompatible with the direct IP:port joining the Discord shim hands out, which is the platform's entire delivery model.

Pinning Valheim to a pre-1.0 build was ruled out: clients auto-updated to network version 39 the same day, so an older server refuses all of them. Two Thunderstore mods touch this area (`A2-No_PlayFab`, `Zeitsurfer-SteamworksDirectConnect`); both predate 1.0 and pin an older BepInEx pack than the one the image runs, so adopting either would be pinning a stale artifact against a moving target.

No public report of this trace exists — not on the bug tracker, the Steam forums, the server-image repositories, or the Jotunn and BepInEx issue trackers. Waiting for an upstream fix means Valheim stays down for an unknown number of days, on a platform whose users are friends with real worlds in it.

## Decision

**A BepInEx preloader patcher, baked into the Valheim image, rewrites `ZPackage.Write(string)` to substitute an empty string for null.**

### The guard goes on the primitive, not on the field

`WritePlayerInfo` is not special. Every null string that reaches `ZPackage.Write(string)` fails the same way, and Valheim 1.0 writes many strings the server may not have populated. Guarding the one field known to be null today fixes one instance of a class the release has clearly widened; guarding the primitive fixes the class.

The substitution cannot regress anything. A null argument currently throws — there is no caller relying on that behaviour, because any caller reaching it takes down whatever RPC it sits inside. Turning the throw into an empty string strictly converts a crash into a benign wire value.

### A patcher, not a plugin

The game's assemblies land at runtime — steamcmd downloads them on first boot — so at image build time there is nothing to compile a plugin against, and a BepInEx plugin must derive from `BaseUnityPlugin` and therefore resolve UnityEngine. A preloader patcher is discovered by convention (`TargetDLLs`, `Patch`) and needs only Mono.Cecil, which comes from NuGet. That keeps the build hermetic and free of the game's own assemblies.

It also lands at the right moment. The patcher rewrites `assembly_valheim.dll` as the preloader loads it, before any of the game's static state exists, so there is no ordering question about whether the target type has been touched yet.

### Baked into the image, not installed as a mod

The patcher is placed by `valheim-pre-server.sh` alongside the baked RCON plugin, and excluded from the staged-mod mirror's `--delete` for the same reason: `BepInEx/patchers/` lives on the image rather than the PVC, and the mirror treats the staged tree as complete desired state. Baking it means every instance is fixed on cold start with no per-instance mod list to maintain, and a friend clearing their mod list cannot remove the thing that makes their server joinable.

## Consequences

Valheim servers join over Steam at IP:port as before. Crossplay stays off, so the platform does not acquire a dependency on a Microsoft backend to complete a Steam handshake.

We now carry a patch against a third party's game. It is pinned to a method signature (`ZPackage.Write(System.String)`) that has been stable across Valheim's history, and the patcher returns without touching anything if the type or the overload is not found — so a future Valheim that renames or removes it degrades to the unpatched game rather than failing to boot. When Iron Gate fixes the null upstream, this becomes a no-op rather than a conflict, and can be removed on its own schedule.

The empty string reaches clients in the player list where a real display name would otherwise appear. On a Steam-only server that name was never populated in the first place, so nothing is lost.
