# ADR-025: Backups are keyed by guild; the old ones were replaced, not moved

## Status

Accepted.

## Context

Archives were namespaced by guild end to end (`archives/{guild}/{name}/`). Plain backups were not: the prefix was `backups/{instance}/` and `list_backups` took no scope at all.

That is only safe while an instance name means one thing forever, and it doesn't. `/destroy` deletes the GameServer, Service and PVC but leaves the S3 prefix, and instance names are friend-chosen with no entropy suffix. So:

1. Guild A creates `smp`, uses it, destroys it. `backups/smp/*` survives.
2. The Kubernetes name is free again. Guild B creates `smp`. The Service carries B's guild label, so every scope check legitimately passes for B — the tenancy checks are working correctly and still admit this.
3. A B admin runs `/restore smp` and gets guild A's world unpacked onto B's PVC. Any B member running `/backups smp` sees A's backup timestamps first.

Names are friendly and guessable (`smp`, `survival`, `creative`), which makes step 2 something that happens by coincidence, not only by malice.

Two fixes were plausible, and the choice was real:

- **Namespace the keys.** Correct, and matches archives — but every existing backup is orphaned unless it is migrated, and there was live user data in the bucket.
- **Purge `backups/{instance}/` on destroy.** Surgical and closes the crossing — but it deletes the safety net that currently survives an accidental `/destroy`, which is plausibly load-bearing.

## Decision

Backups are keyed `backups/{guild}/{instance}/`. The backups already in the bucket were **not** moved into it: each live server was given a fresh backup under the new layout, and the old keys were then deleted.

`/destroy` still leaves the prefix alone. The safety net survives a mistaken teardown, which was the reason not to purge; namespacing means the surviving keys are no longer reachable by whoever next claims the name.

**Re-keying existing objects was not worth what it cost.** S3 has no rename: a key *is* the object's identity, so changing the prefix of an existing backup means copying every byte and deleting the original. For one Palworld world that is ~5 GB per generation, and the gateway copies at roughly 9 MB/s.

What made the copying avoidable is that the *fresh* backup is as good a safety net as the old one. Each live server was backed up once under the new layout, at which point the old generations were history rather than protection, and history was not worth ~29 GB of copying to preserve. The old keys — flat, and misfiled ones from a half-finished migration attempt — were deleted.

This is a decision about *existing* data only. New backups are written guild-scoped from the moment the code deployed, which is what closes the crossing going forward.

Two things ruled out along the way. Filtering reads by the manifest's `guild` field instead of separating the prefixes would have been free — `summaries` already fetches every manifest, and `ArtifactSummary` already carries the owner — but it does not fix retention: `prune_backups` keeps the newest N under a prefix, so in a shared flat prefix one guild's new backups compete with another guild's stale ones for the same window. Separation is what fixes that. And resolving each artifact's owner *as it is moved* is unsafe: `…manifest.json` sorts ahead of `…tar.zst`, so a per-key migration relocates the manifest first and the tarball then finds no owner. A migration that must run has to resolve every owner before it moves anything.

**Listing resolves the owner from the Service, in the instance's current lifecycle state.** `list_backups` reads the owning guild off the instance's Service — the one object of the trio that outlives both `/stop` and `/shutdown` — so a paused or shut-down server still lists its backups. An instance with no Service has been destroyed and has no owner to answer for, so it lists nothing. `restore_backup` re-derives the same prefix and refuses a key outside it; every caller already picks keys out of a listing, so that check only fires if one arrives another way, but it is the step that writes one world over another.

**A pre-scoping instance carries no guild label**, and an empty path segment would render `backups//{instance}/` — straight back into the shared namespace this exists to avoid. Those go under a reserved `_unscoped` segment. A Discord snowflake is all digits, so it can never collide with a real guild.

**The name-collision report was split.** `/create` refusing a name now distinguishes "you already have one" from "that name isn't available", because the old shared wording confirmed the existence of another guild's server one guess at a time — an oracle that `scope.rs` deliberately closes everywhere else by masking `Foreign` as `Absent`.

## Consequences

- **Backup history before this change is gone.** Each live server has exactly one backup — the fresh one — and rebuilds its retention window from there on the normal cycle. That is the price of not copying, and it was only acceptable because a current backup, not an old one, is what a safety net is.
- Backups for instances that no longer exist (destroyed test servers) were left alone rather than swept up: they are unreachable under the new layout, and deleting somebody else's history is a separate decision from this one.
- `/create` no longer confirms that another guild holds a name, but it still refuses it, so unavailability is itself a weak signal. Closing that entirely means namespacing the Kubernetes object names, which would rename every live instance — deliberately out of scope.
- Backups still survive `/destroy`, so a bucket accumulates prefixes for instances that no longer exist. They are inert (nothing lists them once the Service is gone) but they are not free, and reclaiming them is a separate decision from this one.
- The `_unscoped` segment is a permanent branch in the key scheme for a case that may have no live instances. It costs one reserved word and removes a malformed-key class outright.
