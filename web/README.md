# Gary web console

Two views of one product, switched in the top bar:

- **Server owner**: one conversation with Gary, who looks after all of their servers. The servers sit beside the chat, with the one Gary last touched opened up. Each reply comes with what Gary did (every tool call, readable first and raw on request), the safety loop (snapshot, apply, restart and verify, healthy or rolled back), an approval gate before anything touches the world, and a cost and latency line.
- **Hosting team**: fleet overview, servers, incidents Gary handled or escalated, and the evidence: a model arena (the same complaint run by the frontier baseline, Gary on Fireworks, and the ablation) and the benchmark results.

Built during the Fireworks Deploy hackathon (2026-11-02). The shell and design tokens are adapted from an earlier internal project of a team member; everything Gary-specific is new.

## Run

```bash
cd web
npm install
npm run dev        # http://localhost:5190
```

`?as=host` or `?as=customer` opens straight into a view.

### Replay and Live

**Recorded replay** (the default) plays bundled `GaryResponse` traces from `src/data/scenarios.ts`, so the demo runs offline. Use **Break the server** in the owner's view to stage one of the four scenarios.

**Live cluster** posts to Gary's operator harness (`POST /operator/gary`, see `crates/bot/src/http/operator.rs`) through the Vite dev proxy, which adds the bearer token server-side. Switching to Live first calls `list_servers`, so the owner's server list shows the real servers; if the harness can't be reached, the app says so and stays on Replay.

To run live, from a machine with access to the cluster:

```bash
kubectl -n game-servers port-forward svc/grizzly-gameservers-agent 19360:9360
```

Then put the operator token and a channel id in `web/.env` (see `.env.example`), or in the repo-root `.env` that `scripts/operator.sh` already uses, and run `npm run dev`. A turn can run for minutes; the UI waits up to 15, the same as the operator script.

## Deployed

The console is served at <https://gameservers.grizzly-endeavors.com> behind HTTP basic auth. The username and password are in 1Password, `grizzly-platform/gameservers-web-demo` (`username`, `password`; the `judges_htpasswd` field is what the Ingress checks, and `web/deploy/externalsecret.yaml` says how to rotate it).

Every push to `main` that touches `web/` (outside `web/deploy/`) runs `.github/workflows/web.yml`: it builds `web/Dockerfile` (Vite build served by nginx), pushes it to the in-cluster registry, and pins the digest in `web/deploy/kustomization.yaml`. A Flux Kustomization (`grizzly-gameservers-web`, registered in grizzly-platform) applies `web/deploy/` into its own `gary-web` namespace. None of this shares a trigger, image, pin file or namespace with the bot, so a web push never redeploys the bot and a bot push never redeploys the console. Watch a rollout with `gh run list --workflow web.yml`.

Live mode works on the deployed console too. The pod's nginx proxies `/operator/*` to the bot and adds the operator token itself, so the token never reaches the browser; the Discord channel arrives at pod start as `/runtime-config.js`. Both come from the `gary-web-live` ExternalSecret. Anyone with the basic-auth password has the harness at admin tier, so see [ADR-043](../docs/decisions/043-web-console-reaches-the-operator-harness.md) before friends' servers come back.

## Where the numbers come from

Every number is labelled with where it came from:

| Data | File | Status |
|---|---|---|
| Benchmark results | `src/data/eval_results.sample.json` | **Sample.** Replace with the eval's `eval_results.json` (schema in the PRD, FR-4.3). The UI drops the sample banner when `sample` is absent. |
| Model rates | `src/data/models.json` | **Placeholder.** Replace with the shared rate table; remove `_note`. |
| Demo scenario traces | `src/data/scenarios.ts` | **Illustrative** (`recorded: false`). Replace with `just operator gary` captures and set `recorded: true`. |
| Fleet and incidents | `src/host/fleet.ts` | **Demo data** for a fictional host, labelled in the UI. |
| Human T1/T2 support | sample eval file | **Estimate**, never measured. |
