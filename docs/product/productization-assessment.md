# Productization assessment — Gary

**Status:** the market and economics analysis standing behind [ADR-028](../decisions/028-gary-is-the-product.md). Market figures are as of August 2026 and go stale; the structural reads do not.

---

## Verdict

**Pursue it as an instrumented experiment with defined kill criteria, not as a decision to become a hosting company.**

The differentiator is real and unclaimed: nobody is shipping an agent that *operates* game servers. The market it sits in is structurally unattractive — commodity, price-driven, low ARPU — so even a strong outcome is a good small company rather than a large one. The single most likely failure mode is customer acquisition, which is substantially de-risked by the founder's 18 months of marketing inside a game server host.

The reason to run it isn't the hosting business. It's that the thing being proven — **an LLM agent that safely mutates production state** — is worth considerably more than the market it's being proven in, and this is the cheapest available way to prove it. That is the reasoning the product framing rests on: the agent is the asset, the hosting is the proving ground.

---

## 1. What exists today

A live, deployed LLM ops agent running real game servers for real friend groups across seven games, reached through Discord and from inside the games themselves.

| Component | Productization read |
|---|---|
| Discord bot — multi-guild, three-tier access control | Replicable. Not a moat. |
| Kubernetes + Agones provisioning, dynamic ports | Undifferentiated, and the most environment-specific piece. |
| **Per-pod supervisor** — in-place restart, file access, console bridge, occupancy | **The key technical asset.** |
| **Gary** — ~24 tools, snapshot→apply→verify→auto-rollback, escalation exit | **The differentiator.** |
| **Cross-tenant memory** — self-authored operational facts, shared across customers | **The compounding asset.** |
| Deferred task queue — "do this when everyone logs off" | Novel UX. Nobody else has it. |
| In-game chat triggers | Strong demo, carefully scoped, read-only. |
| Gate-signed CI, ADR trail, pervasive graceful degradation | Rare discipline. Sells well in a vendor conversation. |

~31k lines of Rust. Every external dependency degrades gracefully rather than failing — no database,
no queue, no API key each produce a reduced service rather than an outage. That discipline is a
prerequisite for shipping this as software other people run.

---

## 2. The market

**Size.** Game server hosting platforms: ~$2.31B (2025) → $6.83B (2035) at 11.5% CAGR on the narrow
definition; broader definitions put 2025 at ~$5.8B. Minecraft alone has 350M+ copies sold and an
estimated 2.6M+ active servers including private ones. The relevant slice — private friend-group
servers — is large in units, small in dollars per unit.

**Who's there.**

- *Panel software:* Pterodactyl (legacy, community migrating off it through 2026), Pelican (the
  successor), Catalyst, PufferPanel, AMP. This layer is mid-churn, which is a good moment to enter
  with something that isn't another panel.
- *Hosts:* Shockbyte, Apex, BisectHosting, Nitrado, GPortal, Hostinger. Compete on price and
  one-click modpack installers.
- *On-demand:* Aternos (free, ad-supported), exaroton (pay-per-use, same company).

**What AI looks like here today.** Two things, neither of them this:

1. Support chatbots bolted onto hosting panels. Answer questions; never touch the server.
2. AI *playing* the game — mineflayer bots, Minecraft MCP servers, LLM-driven mods. Adjacent hype,
   unrelated market.

**Nobody operates the server.** No competitor found that reads live config, edits it, restarts,
verifies, and rolls back. The nearest Discord integration (PteroControl) is a start/stop wrapper over
a panel API.

**Why the gap is worth money.** Price is the #1 churn driver (56% of providers cite it), and support
runs $18–35/ticket at SaaS benchmarks — with a large share of game-hosting tickets being non-technical
customers stuck on modpack conflicts, config syntax, and crash loops. The product aims at the
industry's biggest cost center rather than a feature checkbox.

---

## 3. What's defensible

**Cross-tenant memory is the only compounding asset.** A quirk learned on one customer's server makes
every other customer's agent better. Every tenant improves the product for all tenants. A self-hosted
competitor structurally cannot have this, and a new entrant starts from zero regardless of funding or
model quality. It is already implemented.

**The supervisor's typed control API instead of `exec`.** Handing an LLM shell access is the tempting
shortcut and produces an unbounded, unverifiable surface. Instead the agent gets a small typed
contract, file access scoped to a data root with traversal refused, and — because the supervisor is
PID 1 wrapping the game — restart-in-place in seconds with state intact. A competitor building on
`wings` or raw exec finds this out in production.

**Snapshot → apply → verify → auto-rollback.** The line between "AI feature" and "AI you can leave
unattended." It is also the demo.

**Not defensible:** the Discord bot, the orchestration wiring, the game catalog, the prompt tooling.
All useful; none of them stop anyone.

**The window.** A funded host could ship a worse version capturing most of the value in ~6 months
once they've seen it work. What they can't skip is time under real load and an accumulated memory
corpus. That argues for getting non-friends onto it soon rather than polishing further.

---

## 4. Three business shapes

**(a) Become a host.** Compete on the incumbents' axis. Commodity market, price-driven churn,
hardware financing, and it converts currently-free capacity into a real cost line. Avoid.

**(b) Sell the AI ops layer to hosts and panels (B2B).** Priced against ticket deflection rather than
against RAM. This is where the money and the durable moat are.

The integration is cheaper than it first appears: `wings` already exposes file read/write/list, power
control, console commands, and a websocket log stream — essentially the agent's entire tool surface.
No access to their images is required; it's a connector plus a second backend behind the existing
tools.

The honest cost: a panel-backed agent is a *degraded* agent. It loses second-scale in-place restarts,
readiness semantics, occupancy polling, and the ability to guarantee the rollback window. Still far
past anything on the market, but not the full product.

Buyer arithmetic: at $18–35/ticket and ~3 tickets/customer/year, there's $50–100/customer/year of
deflectable cost, implying **$0.50–2 per managed server per month**. That needs volume, so the buyer
is a mid-size host — small ones have no budget, large ones build it themselves once convinced.

The real barrier is trust, not technology: letting a third-party AI write to customers' files is a
serious security conversation. Gate-signed CI, scoped permissions, containment, tiered access where
the agent structurally cannot destroy, and a full audit trail are the answer — and they already exist.
That body of work *is* the sales collateral.

**(c) Self-hostable open core.** Lowest ceiling, best distribution, cheapest to run, and it feeds (b)
by generating the memory corpus and the reliability numbers.

**Recommended sequence:** consumer service first (full-fidelity stack, own economics, generates the
corpus), B2B as a channel opened later — that sale is far easier from "we run 2,000 servers, our
escalation rate is 6%, here's the audit log" than from a demo.

---

## 5. Pricing: flat rate instead of $/GB

**The instinct is right, and for a better reason than avoiding comparison.**

$/GB is the incumbents' axis; pricing there means being compared there, with an extra cost line and
no scale advantage. But the deeper point is that non-technical buyers *don't know how much RAM they
need* — it's the most confusing part of the purchase, and it exists only because hosts can't
right-size for them.

**Why nobody does it.** Four levers control peak cost. Three are taken:

| Lever | Who | Cost |
|---|---|---|
| Overprovision hardware | Premium hosts, implicitly | Kills margin |
| Queue at peak | Aternos | Kills UX |
| Meter usage | exaroton, everyone else | The thing being avoided |
| **Make each server cheaper to run** | **nobody** | ← the opening |

Aternos is the existence proof that flat rate works — free servers, sleep-when-empty, ad-supported.
And its pitfall is visible in the mechanism: sleeping fixes the *average* problem and does nothing for
the *peak* problem, so at 7pm players wait in a queue. They didn't avoid the cost; they made the
customer absorb it.

**The real argument.** $/GB isn't only pricing — it's support triage and the upsell path in one
("you're on 2GB, that's why it crashes, upgrade"). Flat rate destroys both, converting every
performance problem into the provider's cost. **Which is exactly why it's viable here and nowhere
else: flat rate requires support marginal cost near zero, and that is what the agent is.**

**Tier on the cost-shape seam.** Vanilla runs 2–4GB and scales with player count. A ~500-mod pack
needs 10–12GB *for a single player* — driven by the pack, not the players. Two genuinely different
cost shapes, which is where a tier boundary belongs. The stronger argument is variance, not level:
mods are unpredictable, and unpredictability is what kills flat rate. Walling them off leaves the base
tier low-variance and safely flat.

**Sell tiers, provision on usage.** Slot-based pricing already exists (GTXGaming, GPortal, Nitrado)
and is mildly resented — Apex and BisectHosting compete on *including* unlimited slots. So sell "you
and about ten friends," and use observed player count and actual usage as the provisioning input.

**"More servers" is the best tier feature** — idle servers cost nearly nothing, so it's high perceived
value at near-zero marginal cost. Cap *concurrently running* rather than total created, and lean on
the existing archive machinery so "unlimited saved worlds, N running at once" is genuinely cheap.

**Two cautions.** Flat rate adversely selects for heavy users — needs a fair-use ceiling and an agent
empowered to require a tier change. And it caps ARPU by removing the upgrade ladder, so the tiers must
give the agent an honest upsell to conduct.

---

## 6. Unit economics

### Infrastructure

Three things are called "cloud" and they price an order of magnitude apart:

| Option | Cost per GB RAM/mo | Verdict |
|---|---|---|
| Hyperscaler (AWS/GCP/Azure) | ~$5–10+ | Dead on arrival; retail floor is $1/GB |
| Elastic cloud VMs (Hetzner CPX/CCX) | Up 113–176% in June 2026 | Timing is bad |
| **Rented dedicated** (Hetzner AX, OVH) | **~€1.00/GB** | Viable; what competitors actually run on |

Retail comparison: ~$1/GB at the budget floor, ~$3 mid-market, ~$4 premium. Against ~€1/GB cost that's
**no margin at the budget end** and ~2.75× gross mid-tier before bandwidth, DDoS, payments, and CAC.

**Consequence: don't compete on price.** The agent is an extra cost line no competitor carries.
Price at or above the premium tier and let "it fixes itself" be the reason.

**Planning number is peak concurrency, not average utilization.** Oversubscription works in web
hosting because traffic is uncorrelated; game load is correlated — everyone plays evenings and
weekends. At peak the oversubscription ratio collapses toward 1:1. RAM is also the one resource that
genuinely can't be oversold (~300–400MB per active player, resident).

**Sleep-on-idle is weaker than it looks, and the correlation runs the wrong way:** the tier that must
run 24/7 (modded, chunk loaders, automation) is also the expensive tier and the CPU-bound one. Two
mitigations: for vanilla, running while empty does very little anyway (chunks unload; farms need a
player), so the base tier can sleep honestly; and for the tier that must run, "hibernate in place"
— JVM heap uncommit plus Kubernetes in-place pod resize, both now stable — frees capacity without
stopping the server. Nobody in game hosting does this.

Indicative tiers at ~€1/GB with 50% peak packing:

| Tier | Peak | Cost | Price | Market comparable |
|---|---|---|---|---|
| Base (vanilla, right-sized) | ~4GB | ~€2 | $8–12 | 2GB budget server ≈ $6–8 |
| Modded | ~12GB | ~€6 | $25–30 | 12GB at ~$3/GB ≈ $36 |

~75–80% gross both, with the modded tier undercutting its per-GB comparable. **Every number rests on
the packing ratio, which is unmeasured.**

**A constraint the RAM model misses:** heavy modpacks need high single-thread clock speed. The modded
tier may be CPU-bound before RAM-bound, which breaks packing precisely in the tier that earns the
money — and makes hardware selection (high-clock Ryzen over older Xeons) a business decision, not a
preference.

### Inference

| Model | Shape | $/M in | cached in | $/M out |
|---|---|---|---|---|
| DeepSeek V4-Flash | 284B MoE / 13B active | $0.14 | **$0.0028** | **$0.28** |
| DeepSeek V4-Pro | 1.6T MoE / 49B active | $0.435 | $0.0036 | $0.87 |
| GLM-5.2 *(currently used)* | 753B MoE / 40B active | $1.40 | $0.26 | **$4.40** |

**The highest-leverage cost move is model routing, not infrastructure.** The current model is ~16×
the cheapest capable option on output. Route a cheap model for the ~90% of turns that are tool
routing, status lookups, and file reads; escalate only for genuine open-ended diagnosis.

**Self-hosting does not win on cost.** A 31B dense model on a rented H100 ($1,095/mo, ~1,260 tok/s
measured) costs $0.34/M output at *100%* utilization — unachievable — and ~$1.12/M at a realistic
30%. Commodity APIs are $0.28/M with no floor. Two structural reasons: dense models are the least
efficient thing to serve, and cache-hit pricing (a 50× discount on the dominant token category, and
this workload is a large fixed prefix with a small varying tail) has no self-hosted price equivalent.

**Serverless GPU doesn't rescue it.** Best case ~$1.05/M fully saturated, ~$8–13/M at the low
concurrency a serverless worker actually sees, plus 20–60s cold starts. It trades away cross-tenant
batching, which is where token APIs get their price.

**Own inference for two reasons, neither of them unit cost:**

1. **Fine-tuning on the memory corpus.** Operational facts plus traces of successful fixes form a
   training set nobody else can assemble. This is the one thing an API structurally cannot sell, and
   it compounds with the moat. Cheap to try — QLoRA on a 31B fits in ~16GB VRAM — and the right
   serving vehicle is a LoRA on managed per-token infrastructure, not a self-managed GPU.
2. **Data residency unblocks B2B.** A host will not send customer file contents to a third-party API.
   That's a hard blocker, and removing it may be worth more than any cost difference.

**One architectural fit:** batch inference is billed at ~50% by several providers, and the deferred
task queue is by construction batched, non-latency-sensitive work. Half the agent's traffic can take
batch pricing while only the interactive path pays for latency.

---

## 7. Risks

**Market structure.** Commodity, price-driven, low ARPU, consolidating. Even winning is modest —
10,000 customers at $15/mo is $1.8M ARR, reached over years, and would make this one of the larger
independent hosts.

**Customer acquisition is the actual business, and it's pay-to-play.** Affiliate programs in this
space pay **up to 80% commission, and CPA models up to 90% of the first payment** — the entire first
payment is the acquisition cost. Combined with price-driven churn, payback is long and fragile. A
better product does not get discovered on merit here. *This is materially de-risked by the founder's
industry background, but the channel economics are unchanged.*

**The counter, and the go-to-market thesis:** the agent lives inside the customer's own Discord
community. A friend group is 5–30 people who all watch it work. The incumbent product is a web panel
exactly one person ever logs into — structurally invisible to everyone else. That's built-in
demonstration and referral, and it can't be copied without rebuilding around Discord.

*Caution:* the founder's expertise is the incumbent playbook (SEO, affiliate, sponsorship); the
asymmetry above is a non-incumbent motion. The failure mode is defaulting to the familiar expensive
channel early. Use industry knowledge for pricing, positioning, and channel evaluation; let the
Discord motion carry acquisition until proven otherwise.

**Liability shifts with scale.** Today a mistake is an apology to a friend; at scale it's someone's
400-hour world. Backup and restore exist, but need per-instance guarantees and *tested* restores. The
existing tier design — where the agent structurally cannot destroy, overwrite, or change access — is
an unusually well-formed safety story to walk into due diligence with.

**Prompt injection widens.** Multi-tenant means anyone who can write a file the agent later reads is
writing into its context. The containment shape is right; it was sized for one trusted group.

**Per-game licensing.** Minecraft's commercial guidelines are navigable — hosts operate under them
daily — but charging money puts you under them. Some games are genuinely ambiguous about monetized
hosting. Review cost scales with catalog size, and favors the B2B path where the host owns the
licensing relationship.

**Operational and capital burden on a solo founder.** Prepaid capacity, churn-plus-prepay working
capital risk, 24/7 uptime, customers awake at 2am. This is an ops business, not only a software one.

---

## 8. The experiment

Run it as a bounded test, not a commitment. Three months, near-zero cost, four numbers:

1. **Escalation rate on non-friend users** — is the agent actually autonomous?
2. **RAM reclaimed by right-sizing** — does the cost lever exist? Measurable now, on existing
   servers, with no product changes: take them as configured, let the agent tune them, record the
   delta.
3. **Cost per agent turn** — is COGS controllable? The operator harness makes this nearly free.
4. **Do 20–50 strangers stick?**

None of this requires hardware, incorporation, or quitting anything.

**Two design points so current hardware doesn't produce a false negative:** test the *base* tier, not
the modded one (older Xeons are weak on single-thread and modded is main-thread bound, so a hardware
problem would read as a product problem); and widen the port range first, since a capacity ceiling
looks exactly like churn in the data.

**Also free, and worth doing first:** write down, from memory, what fraction of tickets at a real host
were config/modpack/crash-loop issues. It's both a prior and an honesty check against the escalation
rate the experiment produces.

**Stop signals.** Escalation >30% after tuning; strangers churn fast or never convert; no organic
referral inside Discord communities.

**Go-hard signals.** Escalation <10%; servers visibly acquiring servers through Discord; RAM reclaim
>25%.

**Start capturing traces now.** Log the full record of successful fixes — symptom, files read, edit
made, outcome — not just the facts learned. That single artifact is simultaneously the memory corpus,
the fine-tuning dataset, and the evidence base for the B2B sale. It costs almost nothing today and
cannot be reconstructed later.

---

## Why to run it regardless

The downside is protected. If the hosting business doesn't work, what survives is a production-tested
agent that safely mutates live state, a documented decision trail, and a portfolio piece that sells
the agent work directly.

Nearly every "AI for ops" product on the market is read-only, **because nobody has solved safe
mutation.** This solves it — in a narrow domain, in production, with real users and real consequences,
where the blast radius is a game world rather than a payments ledger.

That's the real reason to do it. Not because game hosting is a good market — it isn't — but because
it's the cheapest way to prove something that matters well beyond it.

---

## Sources

**Market and competition**
- [Game Server Hosting Platform Market to 2035 — SNS Insider](https://finance.yahoo.com/news/game-server-hosting-platform-market-040000617.html)
- [Game Server Hosting Market Report 2034 — Market Intelo](https://marketintelo.com/report/game-server-hosting-market)
- [Minecraft User and Revenue Statistics 2026 — Host Havoc](https://hosthavoc.com/blog/minecraft-statistics)
- [Best Pterodactyl Alternatives in 2026 — Iceline](https://iceline-hosting.com/blog/best-pterodactyl-alternatives-in-2026-ranked-game-server-management-software-for-gamers-and-businesses)
- [Pelican vs Pterodactyl: The 2026 Paradigm Shift](https://space-node.net/blog/pelican-panel-vs-pterodactyl-migration-guide)
- [PteroControl Discord Bot](https://top.gg/bot/816219634390663230)
- [Pterodactyl Wings API Overview](https://pterodactyl-wings.mintlify.app/api/overview)

**Pricing and economics**
- [Pricing — exaroton](https://support.exaroton.com/hc/en-us/articles/360019687657-Pricing)
- [What is the revenue model of Aternos?](https://stepofweb.com/how-aternos-earn-money/)
- [Minecraft Hosting Compared: Apex vs Shockbyte vs BisectHosting 2026](https://mc-servers.io/blog/minecraft-server-hosting-compared)
- [Hetzner price increase June 2026: CPX and CCX up to +176%](https://wz-it.com/en/blog/hetzner-price-increase-june-2026-cpx-ccx-alternatives/)
- [Hetzner cloud price increases 2026 — Northflank](https://northflank.com/blog/hetzner-cloud-server-price-increases)
- [Cost Per Support Ticket: Benchmarks — Lorikeet](https://www.lorikeetcx.ai/articles/customer-service-cost-per-ticket)
- [Scaling Hosting in 2026 — CloudLinux](https://blog.cloudlinux.com/scaling-hosting-in-2026-where-growth-meets-its-limits-and-how-hosting-providers-respond)
- [Game Server Sponsorship & Affiliate Program — Godlike](https://godlike.host/partnerships/)
- [How to Start a Game Server Hosting Business — Cybernex](https://cybernex.lk/knowledge-base/how-to-start-game-server-business)

**Technical**
- [ATM10 Server Requirements 2026](https://space-node.net/blog/all-the-mods-10-server-requirements-guide-2026)
- [How Much RAM for a Minecraft Server — WinterNode](https://winternode.com/blog/minecraft/how-much-ram-minecraft-server)
- [DeepSeek V4: Pro and Flash — Complete Guide](https://deepseek.ai/deepseek-v4)
- [DeepSeek API Pricing 2026](https://deepseek.ai/pricing)
- [GLM 5.2 — OpenRouter](https://openrouter.ai/z-ai/glm-5.2)
- [Gemma 4 31B on H100: Inference Benchmark](https://inferencebench.io/blog/gemma-4-31b-h100-complete-inference-benchmark/)
- [Fireworks AI Pricing 2026 — Morph](https://www.morphllm.com/fireworks-ai-pricing)
- [RunPod H100 Pricing 2026 — Spheron](https://www.spheron.network/blog/runpod-h100-pricing-2026/)
- [JEP 346: Promptly Return Unused Committed Memory from G1](https://openjdk.org/jeps/346)
- [Kubernetes In-Place Pod Resize](https://kubernetes.io/blog/2025/05/16/kubernetes-v1-33-in-place-pod-resize-beta/)
- [Minecraft Usage Guidelines](https://www.minecraft.net/en-us/usage-guidelines)
