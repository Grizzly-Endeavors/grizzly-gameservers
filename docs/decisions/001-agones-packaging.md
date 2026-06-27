# ADR-001 — Agones as a standalone gated HelmRelease

**Status:** Accepted (2026-06-27)

## Context

Agones standup is gated in this repo (the gate vets it before deploy). An open question was *how* to package it: as a Helm dependency of the app's `deploy/` chart, or as a separate Flux `HelmRelease`. This affects CRD-before-CR ordering and whether app redeploys disturb the operator.

## Decision

Install Agones as its own Flux `HelmRelease` under `cluster/agones/`, rendered by the `grizzly-gameservers-cluster` Flux Kustomization (`path: ./cluster`) in `grizzly-platform` — independent of the app `deploy/` chart. The `grizzly-gameservers-games` Kustomization `dependsOn` it (`wait: true`), so Agones CRDs exist before any GameServer CR applies.

## Consequences

- App redeploys never churn the operator; the operator's lifecycle is decoupled from the bot/agent image.
- CRD ordering is explicit at the Flux-Kustomization level rather than buried in Helm hook weights.
- The Agones chart manages its own CRDs (`crds: CreateReplace`); a cold sync applies them before CRs.
- Agones' hostPort range is pinned to `8000–8100`, away from the `7000–7010` NodePort band (we expose via NodePort, not hostPort — see ADR-002).
