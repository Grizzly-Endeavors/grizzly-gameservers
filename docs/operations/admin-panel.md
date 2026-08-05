# Operating the super-admin panel

**<https://gameservers-admin.grizzly-endeavors.com>**

The operator's view of the whole service: what it's running, who it's serving, and every knob that used to need a redeploy. Separate from Discord's admin tier — a guild admin runs their own Discord server, this reaches the settings that bound every guild at once. The "why" is [ADR-025](../decisions/025-super-admin-panel.md); the settings model underneath is [ADR-024](../decisions/024-runtime-settings.md).

## Getting in

Sign in with Authentik. Access needs membership of **`grizzly-admins`**; a signed-in non-member gets a page naming the group rather than a bare 403, so "I can see a page that says I'm not an admin" means SSO worked and the group is the problem. Add someone in the Authentik admin UI, or in `blueprints/groups.yaml` in grizzly-platform.

Three things have to be true for the panel to answer at all, and they fail in distinguishable ways:

| Symptom | Cause |
| --- | --- |
| Authentik login loop, or a 404 from the outpost | The proxy provider isn't in the embedded outpost's list (`blueprints/grizzly-webmail.yaml` owns it) |
| "Not signed in" page despite being signed in | The ingress lost its `auth-response-headers` annotation, or something reached the panel bypassing ingress-nginx |
| Connection refused / 503 | `bot.adminPanel.enabled` is false, or `GAMESERVERS_ADMIN_PORT` isn't reaching the pod |

## What each page is for

**Overview** — whether the gateway is connected, which optional subsystems are wired, and what the live model, watchdog mode, and throttle figures are. The "what it costs when off" column is there so a missing subsystem reads as a known trade rather than an outage.

**Settings** — the whole catalog, grouped. Each row shows the value in force, whether it came from an override or the deployment, and whether a change is live or waits for a restart. The field starts as exactly what is in force, so saving without editing changes nothing.

**Guilds** — every Discord server the bot is in, whether it's served, and its usage against its caps. Admission and per-guild caps are edited here.

**Servers** — the game servers, with the lifecycle actions. Click one for its detail page, its recent events, and the destructive actions.

**Events** — the product event log, filterable. Paging is by id rather than offset, so a page doesn't shift under you as new events land.

## Live vs. waits-for-a-restart

Most settings bind on the next read — a new model tag applies to the next Gary turn, a changed cap to the next create. Six do not, and the panel badges them **on restart**:

`backup.interval_hours` · `backup.retention` · `archive.retention_days` · `reconcile.interval_minutes` · `reconcile.stranded_dwell_minutes` · `sampling.interval_minutes`

Each is baked into a ticker when its background task spawns. The override is recorded and shown immediately, and a banner names any that the running process hasn't picked up — so a value on this page is never quietly different from what the bot is using.

They take effect on the next deploy. If you don't want to wait for one, the pod has to restart, and **that is the one case where restarting the bot by hand is right** — everywhere else, CI and Flux own the rollout (see the deploy notes in `CLAUDE.md`).

## Two changes that affect more than they look like

The allowlist is the one place where editing *one* guild changes what *every* guild gets, so both of these ask twice:

- **Admitting the first guild** closes the door on all the others. An empty allowlist serves everyone; adding one entry makes it the only server served, and every other guild loses its commands.
- **Removing the last guild** re-opens the service to everyone — the opposite of what "stop serving" sounds like.

An empty allowlist is a legitimate, deliberate posture (it's what an un-configured deploy runs), which is why it isn't simply forbidden. The Guilds page carries a standing notice whenever it's in force.

## Destroying a server

Destroy asks for the server's name typed back. That is not decoration — the handler compares it, so a mis-click can't get through. Archives the server already has survive a destroy; anything not archived does not.

## When the database is down

The panel goes read-only and says so. The bot keeps running on its deployed configuration — every setting falls back to the environment variable or compiled default, which is the configuration the deploy intended. Nothing un-caps or un-throttles itself.

## Break-glass

There is deliberately no way to bypass the group check, so if Authentik is down the panel is unreachable. The settings underneath are still editable:

```sh
# The bot's own Postgres role, on the R730xd.
psql -h 10.0.0.200 -U grizzly_gameservers -d grizzly_gameservers

SELECT key, value, updated_by, updated_at FROM runtime_settings ORDER BY key;
UPDATE runtime_settings SET value = '...', updated_by = 'break-glass' WHERE key = '...';
DELETE FROM runtime_settings WHERE key = '...';   -- back to the deployed value
```

**A hand-edited row is not picked up until the process restarts.** The panel writes through the in-memory cache; `psql` doesn't. Restart the bot after editing, and prefer the panel whenever it is reachable — it validates, and a value that doesn't parse is logged and ignored at the next boot rather than applied.

The environment variables still work too, and outrank nothing: a stored override beats them. If a hand-edited row is the problem, `DELETE` it and the environment value returns.

## The network policy

`cluster/guardrails/bot-ingress.yaml` is what makes the panel's trust in Authentik's headers sound: it admits only ingress-nginx to port 9361. It is also the **first ingress policy on the bot pod**, so it flips that pod to default-deny inbound.

After any change to it, check all four flows — Cilium drops silently rather than erroring:

```sh
kubectl get pods -n game-servers -l app.kubernetes.io/component=bot   # probes: still Ready?
just operator tool list_servers                                        # port-forward on 9360
# in-game: @Gary in any running server's chat should still answer      # game pods → 9360
curl -sS -o /dev/null -w '%{http_code}\n' https://gameservers-admin.grizzly-endeavors.com  # ingress → 9361
```

A game pod must *not* reach 9361. From inside one:

```sh
kubectl exec -n game-servers <game-pod> -- timeout 5 sh -c '</dev/tcp/grizzly-gameservers-admin/80' \
  && echo "REACHABLE — the policy is not doing its job" || echo "blocked, as intended"
```
