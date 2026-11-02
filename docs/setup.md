# Setup

How to stand up the whole system from an empty Kubernetes cluster, and what you can run without one. For what the system is and why it is shaped this way, read [`design/00-overview.md`](design/00-overview.md) first. To reproduce the benchmark, see [`../evals/README.md`](../evals/README.md).

## What runs where

| Piece | Where it runs | Source | Built by |
|---|---|---|---|
| Gary + Discord shim + operator harness (`grizzly-gameservers`) | one Deployment in the `game-servers` namespace | `crates/bot` | `Dockerfile` at the repo root; `.github/workflows/deploy.yml` on push to `main`, or `just bot-push` by hand |
| Per-pod supervisor (`grizzly-supervisor`) | PID 1 inside every game pod | `crates/supervisor` | baked into each game image by `games/<game>/Dockerfile`; `just game-push <game> <tag>` |
| Wire contract shared by the two | library | `crates/control-api` | — |
| Game catalog (Agones `GameServer` + NodePort `Service` + PVC per game) | read by the bot at runtime, rendered per server | `games/` | baked into the bot image |
| Agones, namespace, RBAC, network policies, quota | cluster-level | `cluster/` | applied by Flux |
| Helm chart for the bot | `game-servers` namespace | `deploy/` | rendered by Flux |
| Web console (demo UI) | `gary-web` namespace, or `npm run dev` locally | `web/` | `.github/workflows/web.yml`, or by hand |
| Eval harness | your workstation | `evals/` | Python 3, no build |

The bot reaches each game pod's supervisor over a typed HTTP control API (port `9359`). Gary never uses `kubectl exec`. The bot's own ports are `9360` (in-game `@Gary` triggers and the bearer-gated `/operator/*` harness) and `9361` (the optional super-admin panel).

## Run it without a cluster

Everything below works on a plain Linux or macOS machine with the Rust toolchain pinned in `rust-toolchain.toml` and Python 3 with `pyyaml`.

```bash
cargo build --workspace
cargo test --quiet --all-features                  # bot, supervisor, control-api
python3 -m pytest -q evals/harness/tests           # eval runner and scorer
python3 evals/harness/run.py --cases evals/cases/dev --systems gary-fw,cc-opus --dry-run --out /tmp/dry
python3 evals/harness/score.py evals/results/<run> --cases evals/cases/scored   # recompute tables from committed records
```

The `--dry-run` runner fakes the cluster and the models, so it checks the case files and the run plumbing without spending anything. The web console runs offline in replay mode: `cd web && npm install && npm run dev`.

## Full deployment

This is the setup the live instance runs on: a self-hosted kubeadm cluster managed with Flux, plus a small VPS that forwards game ports. The platform-level pieces (Flux, ESO, the secret store, Postgres, Valkey, Langfuse, ingress, the edge VPS) live in a separate infrastructure repo. They are listed here so you can replace each one with your own equivalent.

### 1. Cluster prerequisites

| Requirement | Why | Required? |
|---|---|---|
| Kubernetes ≥ 1.33 (the live cluster runs 1.36) | `pods/resize` (live CPU/memory resize, [ADR-021](decisions/021-in-place-resource-resize.md)) | yes |
| [Flux](https://fluxcd.io) | `cluster/agones` is a Flux `HelmRelease`; the chart in `deploy/` is rendered by a Flux `HelmRelease` | yes, or apply the equivalent Helm charts by hand |
| Cilium | `cluster/guardrails/` uses `CiliumNetworkPolicy` for Gary's egress leash and the bot's ingress policy | yes, for the guardrails as written |
| API server `--service-node-port-range` that includes `7000-7099` | every game server gets a NodePort in that band ([ADR-002](decisions/002-nodeport-no-node-pin.md)) | yes |
| A container registry the cluster can pull from | the bot and game images (`registry.registry.svc.cluster.local:5000` here) | yes |
| [External Secrets Operator](https://external-secrets.io) and a `ClusterSecretStore` | the chart renders one `ExternalSecret` per credential (`deploy/templates/externalsecrets.yaml`); the store name is `externalSecrets.secretStore` in `deploy/values.yaml` | no; set `externalSecrets.enabled: false` and create the Secrets yourself |
| metrics-server | resource sampling for the cost plane | no |
| ingress-nginx (+ a forward-auth provider such as Authentik) | the super-admin panel and the web console | no; set `bot.adminPanel.enabled: false` |
| Kyverno | image signature enforcement ([ADR-003](decisions/003-bot-scoped-gate-enforcement.md)) | no |
| An edge host that forwards `7000-7099` TCP+UDP to the cluster | public reachability for players; here a VPS over WireGuard | no, if players are on your LAN |

### 2. External services

| Service | Env var(s) the bot reads | Without it |
|---|---|---|
| Discord bot application | `DISCORD_BOT_TOKEN`, `GAMESERVERS_ADMIN_USER_IDS` | the Discord surface is off; the operator harness and web console still work |
| Model provider: Ollama Cloud | `OLLAMA_API_KEY` (`OLLAMA_BASE_URL`, `OLLAMA_MODEL` optional) | Gary replies that he is not configured |
| Model provider: Fireworks | `FIREWORKS_API_KEY` | operator-harness turns cannot pick `provider: fireworks` (what the benchmark uses) |
| PostgreSQL | `DB_PASSWORD` (`DB_HOST`, `DB_PORT`, `DB_NAME`, `DB_USER`) | no home channels, per-guild config falls back to operators + guild owner, no event log, no watchdog |
| Valkey / Redis | `REDIS_PASSWORD` (`REDIS_HOST`, `REDIS_PORT`, `REDIS_DB`) | `run_when` deferred tasks are off |
| S3-compatible object storage | the `grizzly-gameservers-s3` Secret | world backups and archive/restore are off |
| Langfuse | `LANGFUSE_PUBLIC_KEY`, `LANGFUSE_SECRET_KEY` (`LANGFUSE_HOST`) | no turn telemetry |
| SMTP submission account | `GAMESERVERS_SMTP_PASSWORD` | beta-signup approvals are recorded but not mailed |

Every one of these is optional except a model provider key, and the bot logs one line at startup for each subsystem it turns off. The full list of settings, their defaults, and which ones can change at runtime is in the "Running & configuring the bot" section of [`../CLAUDE.md`](../CLAUDE.md).

### 3. Shared tokens

Three bearer tokens are generated by you (any long random string) and stored as Secrets. Each one only opens its own route:

| Secret | Purpose |
|---|---|
| `grizzly-gameservers-control` | bot → supervisor control API ([ADR-017](decisions/017-supervisor-control-auth.md)) |
| `grizzly-gameservers-ingame` | game pod → bot, for in-game `@Gary` chat ([ADR-005](decisions/005-ingame-agent-triggers.md)) |
| `grizzly-gameservers-operator` | the `/operator/*` harness the eval and web console drive ([ADR-011](decisions/011-operator-harness.md)); without it those routes are not mounted |

### 4. Apply the cluster layer

Point a Flux `GitRepository` at this repo and register two Flux `Kustomization`s:

- `./cluster`: Agones (pinned in `cluster/agones/helmrelease.yaml`), the `game-servers` namespace, the bot's ServiceAccount and Role, the network policies, and the namespace `ResourceQuota`. See `cluster/agones/README.md` and `cluster/guardrails/README.md`.
- `./games`, depending on the first. It is intentionally empty (every server is created on demand), so it only matters if you want an always-on server.

Then a Flux `HelmRelease` for `./deploy` into the `game-servers` namespace. Edit `deploy/values.yaml` for your registry (`image.repository`), your domains (`bot.domain`, `bot.adminPanel.host`), and your secret store.

### 5. Build and push images

```bash
just bot-push                  # bot image → in-cluster registry, tag :dev
just game-push minecraft dev   # one per game you want: minecraft, factorio, terraria, valheim, palworld, enshrouded, satisfactory
```

Both scripts port-forward the in-cluster registry and push through `localhost:5000`. On this repo's own `main`, CI builds the bot image and commits its digest into `deploy/values.yaml`, and Flux rolls it out. Game images are always pushed by hand. Each catalog entry pulls `:dev` with `imagePullPolicy: Always`, so a newly created server picks up the newest image.

### 6. Check it works

Put `GAMESERVERS_OPERATOR_CHANNEL` (any Discord channel id the bot can post to) in a repo-root `.env`, then:

```bash
just operator tools                                       # the tools Gary has, with JSON schemas
just operator tool list_servers                           # no model in the loop
just operator tool create_server '{"game":"minecraft","name":"smoke"}'   # then poll list_servers until Ready
just operator gary "what servers are running?"            # one full Gary turn; prints the tool trace and token cost
```

`scripts/operator.sh` port-forwards the bot's ClusterIP Service and reads the operator token from the cluster Secret, so the harness is never exposed through an Ingress. In Discord, invite the bot to a server you own; the guild owner is an implicit admin, and `/config` sets up roles from there.

### 7. Web console (optional)

See [`../web/README.md`](../web/README.md). Replay mode needs nothing. Live mode needs the same port-forward plus the operator token in `web/.env`.

## Where to look next

- [`design/00-overview.md`](design/00-overview.md): the architecture.
- [`decisions/`](decisions/): one ADR for each decision that shaped the code.
- [`operations/admin-panel.md`](operations/admin-panel.md), [`operations/signups.md`](operations/signups.md): running the shipped subsystems.
- `crates/bot/prompts/`: every piece of text the model sees, as Markdown compiled into typed Rust.
