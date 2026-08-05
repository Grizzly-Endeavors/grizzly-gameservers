# ADR-024 — Runtime settings: database override > environment > compiled default

**Status:** Accepted (2026-08-05)

## Context

Every operator-tunable value in the bot reached it the same way: an environment variable with a compiled default behind it, parsed once in `BotConfig::from_env` and never consulted again. Changing any of them means editing a 1Password item, forcing an ESO re-sync, and restarting the pod — or, for the ones that were never wired to the environment at all (the per-instance sizing clamps, Gary's round budget), editing a constant and shipping an image.

That was the right shape while the only person changing a setting was also the person rolling the deploy. The beta broke the assumption in both directions. Admitting a guild — the single most time-sensitive knob, since a friend is waiting — is a 1Password edit plus a restart. Retuning the throttle, trying a different model, or raising a guild's caps all cost a deploy cycle each, which is long enough that the honest response is to guess generously up front rather than tune against real traffic. And several values that clearly *want* tuning during a beta (the sizing clamps, the round budget) had no override path at all.

The three `GAMESERVERS_TURN_*` throttle variables were not even in the Helm chart, so production ran them on their compiled defaults with no way to change them short of a code change.

## Decision

**A third precedence layer in front of the existing two**, in a new `crate::settings`:

```
database override  >  environment variable  >  compiled default
```

Environment is demoted from "the last word" to "the deploy-time default". Nothing about the environment path changes: a deploy with no stored overrides behaves exactly as it did before, and clearing an override returns a setting to whatever the environment says. This matters for rollback — the override table is additive, so reverting the image reverts the behaviour of any key that release introduced, while keys it didn't touch keep working.

**One key-value table, one closed catalog enum.** `runtime_settings(key, value, updated_at, updated_by)` with the same audit columns as `guild_limits`. Key-value rather than typed columns because the catalog will keep growing through the beta and a new knob should be a code change, not a schema change; the type safety that loses at the column level is recovered in `SettingKey`, whose every method matches exhaustively — under the workspace's `wildcard_enum_match_arm` deny, adding a variant without deciding its grammar is a compile error rather than a half-wired setting.

**Reads go through a whole-snapshot swap.** `RuntimeSettings::snapshot()` returns an `Arc<SettingsSnapshot>` replaced as a unit (`arc_swap::ArcSwap`), never mutated field by field. A reader therefore cannot observe half of a multi-field change, and the read is cheap and synchronous — which it has to be, because `guild_is_admitted` sits on the per-message path and is not async.

**Consumers read the snapshot at the point of use, never a copy.** A stored copy is precisely how a hot setting quietly stops being hot, and this codebase already had three independent copies of the model connection. So `guild_is_admitted` reads the allowlist per call, `GuildLimits` holds the settings façade instead of a `GuildLimitDefaults` value, `Sizing::validate` takes its bounds as a parameter, and the watchdog responder reads its mode per incident.

**A turn captures its model connection and round budget once, at the start.** The exception that proves the rule: within one turn the model must not change between rounds, or a conversation would switch models partway through — which reads to the person as Gary changing his mind about what he is. `Data::ollama_now()` is the single resolution point, and both surfaces call it once.

**Secrets are not settings.** `OLLAMA_API_KEY`, the database password, the bearer tokens stay in 1Password behind ESO. A `TEXT` column an admin page can echo is the wrong home for a credential, so *whether Gary is configured at all* remains a startup fact and only *which model he drives* is tunable. The same reasoning keeps the endpoints and connection settings out.

**Restart-class keys are recorded, not refused.** The background cycles bake their cadence into a ticker when their task spawns, and re-reading a 24-hour backup interval per tick would buy nothing. Those six keys are still overridable — the override is stored and takes effect on the next boot — rather than being unavailable, because "recorded, in force after a restart" is a fine operator story and "not settable" is not.

**Degradation is toward the deployed values, always.** Connecting never fails; an unreachable database, an unreadable table, an unparseable row, and an internally inconsistent set all log loudly and fall back to the baseline. A settings store that can't be read must not be able to un-cap or un-throttle anything, which is the same stance `GuildLimits` takes and the opposite of `InstanceSizing`'s — a cap override only *reads* policy, so the generous baseline is a safe answer, whereas a sizing override *records* state that would evaporate.

**Cross-field rules are checked on the whole snapshot**, not per key: a sizing floor above its ceiling would refuse every resize, and each half parses fine alone. `coherence()` runs on every compose, and a change that would break it is rejected before it is written.

## Consequences

- The allowlist, the model, the throttle, the per-guild cap defaults, the sizing clamps, the round budget, and the watchdog mode all become changeable without a deploy — fifteen hot keys and six restart-class ones.
- The sizing clamps and Gary's round budget gain an override path they never had; the three throttle variables gain one that reaches production, which the chart never gave them.
- `GAMESERVERS_WATCHDOG=off` no longer means "no watchdog this process": the sampler→responder channel is now built unconditionally and both ends read the mode live, so `off → shadow → on` is a live transition. The detector is dropped while off and rebuilt from scratch when it resumes, because carrying its state across the gap would difference the whole quiet period into a crash burst the moment detection came back.
- Startup ordering is now load-bearing: settings connect before `DurableState`, because `GuildLimits` takes the façade, and before `CycleSchedule`, because the cadences come from the resolved snapshot rather than from `BotConfig`.
- An override row outlives the release that introduced its key. Renaming a key orphans its override, which reads as the setting silently reverting — so keys are treated as a published vocabulary, and an unrecognized row is logged and left in place rather than deleted, so a rollback doesn't lose it.
- One more Postgres pool (2 connections), and one more thing that is wrong if the database is down — though wrong in the direction of the deployed configuration, which is the configuration the deploy intended.

## Alternatives rejected

- **Typed columns instead of key-value.** Better type safety at the storage layer, at the cost of a schema change per knob during exactly the period when knobs are being added fastest. The closed enum recovers the safety where it matters — at the parse boundary — and the exhaustive matches make an unwired key impossible.
- **Per-field locks instead of a whole-snapshot swap.** Cheaper writes, but a reader can then observe a half-applied multi-field change, and the sizing floor/ceiling pair is exactly a multi-field change that must not be seen torn.
- **Push the new values into each consumer on change.** Would let stateful consumers (the throttle's semaphore) update precisely, but every consumer becomes something the writer must remember, and a forgotten one is a setting that silently isn't hot. Pull-at-point-of-use has no such failure mode. The one genuinely stateful consumer is handled where its writer lands.
- **Re-read the cycle cadences per tick so nothing needs a restart.** Rewrites three ticker loops to buy a few minutes' latency on knobs that get turned by hand, and introduces a class of bug (a ticker rebuilt mid-cycle) that the restart path doesn't have.
- **Put the secrets in the table too, so everything is in one place.** A credential in a `TEXT` column an admin page renders is a worse failure than a redeploy to rotate a key. ESO already solves secret delivery.
- **A config file on a mounted ConfigMap, reloaded on change.** Kubernetes-native and needs no database, but the pod runs `readOnlyRootFilesystem`, a ConfigMap edit is still a git round trip under Flux, and the panel would then be writing YAML into a repo rather than a row into a table it already owns.
