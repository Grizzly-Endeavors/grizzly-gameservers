# cluster/guardrails

The ops agent's leash. Lives in this repo (co-located with the agent code) so the code and what it's allowed to touch move in one reviewed, gated PR. See `docs/design/00-overview.md` → "Ops-agent guardrails".

- `namespace.yaml` — the `game-servers` `Namespace`. Defined here (not in `grizzly-platform`) so it exists under the same `./cluster` Flux Kustomization that installs Agones, which provisions per-namespace SDK RBAC into it. Ships **without** `grizzly.io/gated=true` until the Kyverno carve-out for Agones' sidecar exists.
- `networkpolicy.yaml` — `CiliumNetworkPolicy` egress leash (the guardrail that matters most). Restricts egress to DNS + the Kubernetes API server + the public internet, denying private RFC1918 space so a compromised or prompt-injected game server / agent can't pivot into OpenBao or other platform services. Ingress is left open (internet-facing game servers behind the NodePort edge).
- `rbac.yaml` — the `ops-agent` `ServiceAccount` + namespace-scoped `Role`/`RoleBinding`: read pods/logs, exec, and get/patch Agones `gameservers` + create `gameserverallocations`. Never cluster-wide. Minimal for now; widened when the ops-agent code lands.
