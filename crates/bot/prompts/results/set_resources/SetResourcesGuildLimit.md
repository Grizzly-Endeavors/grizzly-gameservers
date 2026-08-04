---
id: SetResourcesGuildLimit
type: prompt
annotations:
  sent_when: >-
    tool result when a resize passes the per-server range check but would push
    the community's live CPU/memory total over its allowance
  used_by:
    - file: discord/gary/tools.rs
      function: exec_set_resources
  variables:
    detail:
      source: describe_limit_refusal over the carried LimitRefusal figures
      contents: >-
        one sentence naming what the resize needs, what the community's other
        live servers hold, and the allowance it would break
  reasoning:
    - >-
      Refused rather than clamped, and worded to make clear nothing was applied
      — the same contract as the per-server range refusal. The community
      allowance is a different bound from the per-server range, so the way out
      is different too: shrink or stop another server, or escalate to the
      operator.
---
nothing changed — {{detail}}. shrinking or stopping one of the community's other servers frees room, or suggest asking the operator for a bigger allowance
