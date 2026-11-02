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

**Live cluster** posts to Gary's operator harness (`POST /operator/gary`, see `crates/bot/src/http/operator.rs`) through the Vite dev proxy, which adds the bearer token server-side. Copy `.env.example` to `.env` and fill it in.

## Where the numbers come from

Every number is labelled with where it came from:

| Data | File | Status |
|---|---|---|
| Benchmark results | `src/data/eval_results.sample.json` | **Sample.** Replace with the eval's `eval_results.json` (schema in the PRD, FR-4.3). The UI drops the sample banner when `sample` is absent. |
| Model rates | `src/data/models.json` | **Placeholder.** Replace with the shared rate table; remove `_note`. |
| Demo scenario traces | `src/data/scenarios.ts` | **Illustrative** (`recorded: false`). Replace with `just operator gary` captures and set `recorded: true`. |
| Fleet and incidents | `src/host/fleet.ts` | **Demo data** for a fictional host, labelled in the UI. |
| Human T1/T2 support | sample eval file | **Estimate**, never measured. |
