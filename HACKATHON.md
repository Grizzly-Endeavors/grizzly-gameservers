# Hackathon: Fireworks Deploy (with Microsoft & NVIDIA)

Working reference for the team and for any agent working in this repo during the event.

## How we work today

**Fast and good enough beats checking every box.**

This deliberately inverts the usual standard in this repo. The ADRs, gate-signed CI, exhaustive tests,
and polish described in `CLAUDE.md` and elsewhere are the right approach for the product — not for a
six-hour build. Today:

- Ship the working version, then improve it only if the eval needs it.
- Prefer the simple hack over the clean abstraction.
- Don't stop to write docs, ADRs, or tests beyond what the eval and the submission require.
- If something is good enough to produce a trustworthy number, it's done.

The one place not to cut corners is **evaluation integrity** (see below). That's 40% of the score,
and a shaky number poisons everything else.

## Event

- **When:** Monday, Nov 2, 2026. Build 10:30 AM – 4:30 PM **Pacific**.
- **Where:** TERRA, 511 Harrison St, San Francisco. In person only.
- **Submission:** start by 3:30 PM PT, finish by 4:00 PM PT (video included). Hard close 4:30 PM PT —
  no code or result changes after that. Submit the Git commit hash of the judged version.
- **Finals:** six teams present live. At least one team member must stay for awards (7:00 PM).

## The challenge

Build an open-model system for a meaningful task and show **measurable improvement over a
frontier-model baseline** — in quality, cost, or latency. Cost or latency wins must keep useful task
quality, and all material tradeoffs must be disclosed.

- **Required:** name the frontier baseline model; compare both systems on the same eval cases.
- **Optional:** fine-tuning. Harnesses, routing across open models, or combinations all count.
- **Not required:** a deployed app. A notebook or API demo can earn full marks.
- **Prizes require Fireworks.** Serve the open models on Fireworks.

## Judging

| Criterion | Weight | What judges look for |
|---|---|---|
| Benchmark evaluation | 40% | Meaningful, reliable advantage over the frontier baseline, backed by evidence |
| Business value | 25% | Clear user, valuable problem, compelling reason to adopt |
| Technical execution | 20% | Thoughtful engineering, working implementation, meaningful experimentation |
| Presentation & demo | 10% | Clear explanation, convincing demo of results |
| Documentation | 5% | Another builder can understand and reproduce it |

Prizes: 1st $10k, 2nd $5k, 3rd $3k. NVIDIA Best-Use Award: a DGX Spark.

## Evaluation integrity (don't cut these)

- Baseline and our system run on the **same held-out cases** with the **same scoring**.
- Keep eval data separate from anything we build or tune against — including per-game knowledge
  notes. Use a small dev set for building; never look at the scored set while tuning.
- **No weakened baseline.** The frontier model gets the same containers, task prompts, and access.
- Target **100+ varied cases** where feasible.
- Report **uncertainty** (repeated runs, confidence intervals).
- Separate **measured results** from **estimates** (e.g. the human-support comparison is an estimate).
- Disclose quality, cost, and latency tradeoffs. No cherry-picking, no hidden failures.

## Required submission materials

- **Project overview:** task, intended user, approach, business value.
- **Public GitHub repo:** code, setup, and eval instructions. Must be public and reviewable without
  permission — test the link signed-out. Private/broken repo = unscored.
- **Eval evidence:** baseline and model details, dataset source and size, metrics, method,
  before/after results, limitations.
- **Demo video under 120 seconds** (aim for ~60): problem → approach → measured result.
- **Team info:** all contributing members.
- **Disclosures:** pre-existing code and prior work; what was built or changed during the event;
  material use of AI coding assistants (we must be able to explain the implementation).

Never commit API keys, credentials, or personal data — including anything from real friend-group
servers.

## Our approach

**Thesis:** a specialized operator harness on open models does the job of game-server T1/T2 support
at comparable quality to a frontier generalist, for a fraction of the cost and time.

The value is knowledge and efficiency, not guardrails. A generic agent spends turns working out where
each game keeps its config and how its quirks behave; a specialized harness starts out knowing. That
should show up as fewer turns, fewer tokens, faster fixes, and better results on game-specific quirks.

**Comparisons:**

1. **Frontier model in Claude Code** — the required baseline.
2. **Our system** — open model(s) on Fireworks in our harness.
3. **Same open model in Claude Code** — ablation isolating what the harness buys (technical execution).
4. **Human T1/T2 support** (Shockbyte-style hosts) — estimated, not measured; business-value framing.

**Task areas:**

1. Config changes and basic user support.
2. Active troubleshooting of a broken server.

**Games:** multi-game is the point — handling per-game quirks is why the operator is an AI. Choose
games by boot time and image size; breadth is a dial, credibility isn't. If time bites, cut games
before cutting eval quality.

**Success** is verified, not claimed: the server comes back healthy and the intended change happened.
Unintended changes elsewhere are reported as a column, not a headline.

## Fixed vs. dials

- **Fixed:** frontier baseline comparison, held-out eval, tasks grounded in real support tickets,
  honest reporting.
- **Dials:** number of games, number of cases, which open models, routing, fine-tuning.

## Pre-existing work

This repo is pre-existing (Grizzly Gameservers / Gary). Existing code is allowed by the organizers but
must be disclosed, and the submission must clearly identify what was built during the event. Keep
hackathon work in clearly separable commits/paths so that's easy to show.

## References

- Event page: <https://luma.com/fireworks-8kmc>
- Rules: <https://hackersquad.io/events/fireworks-deploy-with-microsoft-nvidia/resources?tab=rules>
- Submission & judging: <https://hackersquad.io/events/fireworks-deploy-with-microsoft-nvidia/resources?tab=submission>
- Product description: [`docs/product/what-this-is.md`](docs/product/what-this-is.md)
- Architecture: [`docs/design/00-overview.md`](docs/design/00-overview.md)
