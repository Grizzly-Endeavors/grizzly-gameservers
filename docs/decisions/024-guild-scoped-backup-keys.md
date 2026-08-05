# ADR-024: Backups are keyed by guild, and the existing ones were migrated

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

Backups are keyed `backups/{guild}/{instance}/`, and the backups already in the bucket were migrated into it rather than orphaned or deleted.

`/destroy` still leaves the prefix alone. The safety net survives a mistaken teardown, which was the reason not to purge; namespacing means the surviving keys are no longer reachable by whoever next claims the name.

**The migration reads each artifact's own manifest, not the cluster.** Every artifact already carries a `manifest.json` sidecar naming its owning guild (ADR-004's self-describing bucket, which is what made this cheap). Asking Kubernetes instead would have been wrong twice over: a destroyed instance has no Service left to ask, and a *reused* name would answer with the new owner — re-filing A's worlds under B, the exact crossing being undone. `scripts/migrate-backup-keys.sh` is a one-shot, re-runnable job, not startup code in the bot; copies are server-side and each destination is verified before its source is deleted, so an interrupt leaves a duplicate rather than a hole.

**Listing resolves the owner from the Service, in the instance's current lifecycle state.** `list_backups` reads the owning guild off the instance's Service — the one object of the trio that outlives both `/stop` and `/shutdown` — so a paused or shut-down server still lists its backups. An instance with no Service has been destroyed and has no owner to answer for, so it lists nothing. `restore_backup` re-derives the same prefix and refuses a key outside it; every caller already picks keys out of a listing, so that check only fires if one arrives another way, but it is the step that writes one world over another.

**A pre-scoping instance carries no guild label**, and an empty path segment would render `backups//{instance}/` — straight back into the shared namespace this exists to avoid. Those go under a reserved `_unscoped` segment. A Discord snowflake is all digits, so it can never collide with a real guild.

**The name-collision report was split.** `/create` refusing a name now distinguishes "you already have one" from "that name isn't available", because the old shared wording confirmed the existence of another guild's server one guess at a time — an oracle that `scope.rs` deliberately closes everywhere else by masking `Foreign` as `Absent`.

## Consequences

- Backups taken before the migration are reachable again, under their original owner. Nothing was deleted and nothing was re-attributed.
- The migration must run *after* the code deploys, not before: the old code writes flat keys, so an earlier pass would leave a fresh crop behind it. Between deploy and migration, existing servers list no backups — visibility, not data, and it closes as soon as the script runs.
- `/create` no longer confirms that another guild holds a name, but it still refuses it, so unavailability is itself a weak signal. Closing that entirely means namespacing the Kubernetes object names, which would rename every live instance — deliberately out of scope.
- Backups still survive `/destroy`, so a bucket accumulates prefixes for instances that no longer exist. They are inert (nothing lists them once the Service is gone) but they are not free, and reclaiming them is a separate decision from this one.
- The `_unscoped` segment is a permanent branch in the key scheme for a case that may have no live instances. It costs one reserved word and removes a malformed-key class outright.
