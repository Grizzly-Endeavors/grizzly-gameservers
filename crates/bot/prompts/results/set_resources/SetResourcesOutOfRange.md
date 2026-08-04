---
id: SetResourcesOutOfRange
type: prompt
annotations:
  sent_when: >-
    tool result when a requested figure fails validation — outside the allowed
    range, or a request above its own limit
  used_by:
    - file: discord/gary/tools.rs
      function: exec_set_resources
  variables:
    detail:
      source: the specific rule that was broken, from SizingRejection
      contents: one sentence naming the figure at fault and the bound it broke
  reasoning:
    - >-
      Refused rather than clamped, so the wording must not suggest anything was
      applied. A friend told "done" who quietly got a different number is worse
      served than one told the value was out of range.
---
nothing changed — {{detail}}
