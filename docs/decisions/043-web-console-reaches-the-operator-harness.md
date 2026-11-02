# ADR-043 — The web console reaches the operator harness through its own proxy

**Status:** Accepted (2026-11-02)

## Context

The web console (`web/`) has a live mode that drives Gary through the operator harness ([ADR-011](011-operator-harness.md)). Until now that only worked from a dev box: the Vite dev server proxied `/operator` to a `kubectl port-forward` and added the bearer token. The console is now deployed at `gameservers.grizzly-endeavors.com` so hackathon judges can use it, and they need live mode too.

The operator token reaches Gary's full admin surface, including `--confirm` on destructive tools. Putting it anywhere a browser can see it would hand that surface to anyone who opened devtools. The friends' servers were spun down for the judging window, so what is reachable from the cluster is demo state, not someone's world.

## Decision

**The console's own nginx proxies `/operator/*` to the bot's agent Service and adds the token itself, behind the console's basic auth.**

- The token and the Discord channel live in 1Password (`gameservers-operator/token`, `gameservers-web-demo/channel`), reach the `gary-web` pod through the `gary-web-live` ExternalSecret, and are rendered into the nginx config when the container starts. Neither ships in the image or the bundle; the channel reaches the browser as `/runtime-config.js` because the request body names it.
- nginx replaces the visitor's `Authorization` header (their basic-auth credential, already checked by the Ingress) with the bearer token, so the two credentials never mix.
- `cluster/guardrails/bot-ingress.yaml` admits the `gary-web` pods, by label, to port 9360. The operator routes stay bearer-gated, so that admission alone grants nothing.

## Consequences

- Anyone with the console's basic-auth password has the operator harness at admin tier, including pre-approved destructive confirmations. That is acceptable only while the cluster holds nothing a stranger shouldn't touch. Before friends' servers come back, rotate the console password or turn live mode off by deleting the `gary-web-live` ExternalSecret and its Secret, which leaves the pod unable to start, or by removing the `/operator/` location.
- Every harness action still posts its reply and any `--confirm` audit line into the configured Discord channel, so judge activity leaves the same visible trail as any operator call.
- Requests pass through Cloudflare, which ends an origin response that sends nothing for 100 seconds. A Gary turn longer than that fails at the edge even though the 900-second timeouts behind it would wait.
