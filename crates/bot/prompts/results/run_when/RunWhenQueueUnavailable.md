---
id: RunWhenQueueUnavailable
type: prompt
annotations:
  sent_when: >-
    tool result when run_when is called but the deferred-task queue isn't
    configured/available, so nothing can be scheduled
  used_by:
    - file: discord/gary/tools.rs
      function: exec_run_when
  reasoning:
    - >-
      Tells the model scheduling is off and explicitly steers it to offer doing
      the thing now instead — so a missing queue degrades to an immediate action
      rather than a dead end.
---
I can't schedule things right now — my task queue isn't available. Offer to do it now instead.
