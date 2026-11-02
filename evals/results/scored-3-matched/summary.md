| System | Harness | Model | n | Success (95% CI) | Mean turns | Mean tokens | p50 ms | p95 ms | USD / 1k | False completion | Infra errors | Staging errors |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| GLM 5.3 Flash on Fireworks in Claude Code | claude_code | accounts/fireworks/models/glm-5p3-flash | 14 | 78.6% [52.4%, 92.4%] | 11.6 | 99243 | 34415 | 115871 | 5.36 | 21.4% | 0 | 0 |
| Claude Opus 5.5 in Claude Code | claude_code | claude-opus-5-5 | 14 | 78.6% [52.4%, 92.4%] | 10.4 | 113699 | 65873 | 119585 | 97.04 | 21.4% | 0 | 0 |
| GLM 5.3 Flash on Fireworks in Gary | gary | accounts/fireworks/models/glm-5p3-flash | 14 | 71.4% [45.4%, 88.3%] | 9.4 | 111479 | 86700 | 611859 | 7.19 | 28.6% | 0 | 0 |
| Human T1/T2 support (estimate) (estimated) | human | None | n/a | n/a n/a | n/a | n/a | 86400000 | 432000000 | 48305.00 | n/a | n/a | n/a |

Compared with the baseline (ratios are system / baseline; cost and latency show 95% CI).

| System | Baseline | Paired n | Success difference | McNemar p | Cost ratio | Latency ratio |
|---|---|---|---|---|---|---|
| cc-fw | cc-opus | 14 | +0.0 pts | 1.000 | 0.06 [0.04, 0.08] | 0.62 [0.45, 0.82] |
| gary-fw | cc-opus | 14 | -7.1 pts | 1.000 | 0.07 [0.05, 0.11] | 2.11 [1.03, 3.63] |

Missing run records, counted as fails: 0.
Staging errors excluded from their system's denominator: 0.
