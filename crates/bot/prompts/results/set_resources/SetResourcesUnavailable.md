---
id: SetResourcesUnavailable
type: prompt
annotations:
  sent_when: >-
    tool result when resource overrides can't be recorded because the database
    is unavailable
  used_by:
    - file: discord/gary/tools.rs
      function: exec_set_resources
  reasoning:
    - >-
      A resize that can't be recorded would apply to the running pod and then
      vanish on the next start, which is worse than not resizing — so nothing is
      attempted and the model is told why, in the same shape run_when uses when
      its queue is unavailable.
---
can't change resources right now — the settings store is unavailable, and a change that can't be saved would be undone the next time the server restarts. nothing was changed.
