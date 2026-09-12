# ADR-041 — The Valheim patcher ships as a committed binary, because one `.cs` file blocks every deploy

**Status:** Accepted (2026-09-12)

## Context

[ADR-038](038-valheim-null-player-info-guard.md) added a BepInEx preloader patcher in C# — 60 lines of Mono.Cecil that rewrite `ZPackage.Write(string)` so a null string can't abort a Steam-only server's peer handshake. It was built in the Valheim image by a `dotnet publish` stage.

That merge turned `main` red and it stayed red. The gate's honest map fails closed on any file in a language it has no adapter for:

```
Error: honest-map verification failed (fail closed):
  Unsupported languages (gate has no adapter — cannot be gated):
    [csharp] games/valheim/patchers/GrizzlyPlayerInfoGuard/Patcher.cs
```

The failure is not scoped to the image that file belongs to. The gate runs once per push over the whole tree, and the `deploy` job needs it — so a single unsupported source file stopped the **bot** from deploying, and the bot is the product. Two merges' worth of work sat undeployed: the handshake guard itself, and the mod-list maintenance in [ADR-039](039-sweep-orphaned-mod-settings.md)/[ADR-040](040-a-mod-list-change-is-legible.md).

The gate is centrally owned in `grizzly-platform` and its reusable workflow takes three inputs — `image`, `gate_version`, `runs_on`. There is no per-repo escape hatch, and it signs the digest **only** on a clean pass. That signature is load-bearing: `enforce-gameservers-bot-signature` verifies it at admission with `failureAction: Enforce` and `required: true`, so simply removing the gate job would not have unblocked anything — it would have made the bot unadmittable, which is worse than red CI.

## Decision

**Commit the built assembly and keep its source out of this repo.**

`games/valheim/patchers/GrizzlyPlayerInfoGuard.dll` is checked in; the Dockerfile copies it instead of compiling it; the `patcherbuild` stage is gone. `games/valheim/patchers/README.md` carries the source's location (the commit it was removed in), the exact pinned-SDK build command, and how to verify a build is the patcher it claims to be.

The alternative shapes were weighed and this one was chosen for a specific reason: **it is the only option that changes nothing about what the gate means.** Every other way out either narrows what gets checked or moves the problem into the shared gate.

- Making unsupported languages audit-only, or teaching the gate C#, are changes to a component every repo in the org depends on. Worth doing on their own merits, on their own timeline, not as a side effect of unblocking one deploy.
- Turning gating off for this repo would have meant an unsigned, unscanned bot image plus a matching relaxation of the Kyverno policy — in the one repo where an LLM operates live production systems unattended. The gate's whole claim is that a green gate is what lets an agent ship without manual review; this is the last repo to weaken it in.

**What this costs, stated plainly:** the patcher's source is no longer in the tree that ships it. Changing the patcher means restoring the source from git, editing, rebuilding, and deleting it again — and forgetting that last step turns every deploy red, not just this image's. The README says so at the point where someone would make that mistake. A vendored binary is also a thing the gate's own image scan sees but cannot attribute to reviewable source, which is exactly the property the honest map exists to prevent; it is accepted here only because the artifact is 6KB, does one documented thing, and is reproducible from a pinned SDK image.

This is a workaround with a shelf life, not a design. The durable fix is one of the two gate changes above.

## Consequences

`main` goes green and both queued merges deploy.

The Valheim image builds faster and pulls one less base image — the .NET SDK stage was the largest thing in the build that had nothing to do with the game.

A change to the patcher is now a three-step dance instead of an edit, and the failure mode of getting it wrong is a red pipeline for the whole repo. This is the main reason the decision is worth revisiting rather than settling: the moment a second language splash appears — a Lua mod hook, a shell utility the gate can't read — the same cost recurs, and the right answer is to fix the gate rather than vendor a second binary.

ADR-038's account of *what* the patcher does and why it is a patcher rather than a plugin still stands. Only its build mechanism is superseded here.

## Alternatives considered

**Make the gate treat unsupported languages as audit-only.** The recommended durable fix, and still the right one: the honest map keeps reporting what it can't check, the Rust that is the product keeps its full scan and its signature, and an incidental file in another language stops being a hard stop for every repo at once. Deferred, not rejected — it is a change to shared infrastructure and deserves its own decision rather than being made under deploy pressure.

**Add a C# adapter to the gate.** Keeps everything honest and unblocks this file, but buys only C#; the next splash blocks again. Also shared-infrastructure work.

**Turn gating off for this repo.** Rejected as described above: it requires relaxing the admission policy too, and it removes the property that makes unattended agent deploys defensible here.

**Move the patcher source to its own ungated repo and fetch the built DLL by digest.** The most correct version of vendoring — it is the pattern this same Dockerfile already uses for `ValheimRcon.dll` (download plus `sha256sum -c`). Rejected for now only on scope: it needs a new repo and a release flow for a 60-line file. If a second patcher ever appears, this is the shape to move to.
