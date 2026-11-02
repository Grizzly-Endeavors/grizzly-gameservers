| System | Harness | Model | n | Success (95% CI) | Mean turns | Mean tokens | p50 ms | p95 ms | USD / 1k | False completion | Infra errors | Staging errors |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| GLM 5.3 Flash on Fireworks in Claude Code | claude_code | accounts/fireworks/models/glm-5p3-flash | 92 | 14.1% [8.4%, 22.7%] | 11.7 | 97877 | 37876 | 115871 | 5.29 | 18.8% | 0 | 10 |
| Claude Opus 5.5 in Claude Code | claude_code | claude-opus-5-5 | 95 | 21.1% [14.1%, 30.3%] | 9.8 | 104564 | 53580 | 135293 | 90.07 | 27.6% | 1 | 7 |
| GLM 5.3 Flash on Fireworks in Gary | gary | accounts/fireworks/models/glm-5p3-flash | 92 | 13.0% [7.6%, 21.4%] | 9.0 | 106561 | 80534 | 611859 | 6.81 | 29.4% | 0 | 10 |
| Human T1/T2 support (estimate) (estimated) | human | None | n/a | n/a n/a | n/a | n/a | 86400000 | 432000000 | 48305.00 | n/a | n/a | n/a |

Compared with the baseline (ratios are system / baseline; cost and latency show 95% CI).

| System | Baseline | Paired n | Success difference | McNemar p | Cost ratio | Latency ratio |
|---|---|---|---|---|---|---|
| cc-fw | cc-opus | 90 | -5.6 pts | 0.062 | 0.06 [0.04, 0.08] | 0.63 [0.46, 0.82] |
| gary-fw | cc-opus | 92 | -6.5 pts | 0.070 | 0.07 [0.05, 0.09] | 1.67 [0.81, 2.94] |

Missing run records, counted as fails: 217.
Staging errors excluded from their system's denominator: 27.
