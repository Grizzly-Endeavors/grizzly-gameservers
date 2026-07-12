---
id: LifecycleAlreadyPaused
type: prompt
annotations:
  sent_when: >-
    tool result when a stop/pause op targets a server that is already paused —
    a no-op the model should relay rather than retry
  used_by:
    - file: discord/gary/tools.rs
      function: format_supervisor
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      States the no-op plainly so Gary doesn't loop trying to pause an already-
      paused server or report a failure where there was none.
---
{{server}} is already paused
