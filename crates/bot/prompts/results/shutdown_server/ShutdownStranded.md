---
id: ShutdownStranded
type: prompt
annotations:
  sent_when: >-
    tool result when shutdown_server issued the stop and the cluster accepted it,
    but the server object was still present when the wait ran out — the machine
    it was running on stopped answering mid-shutdown
  used_by:
    - file: discord/gary/tools.rs
      function: exec_shutdown
  variables:
    server:
      source: the server being shut down
      contents: the server name
  reasoning:
    - >-
      Says the stop is unconfirmed rather than done, because reporting a
      completed shutdown that hasn't happened is what leaves a friend with no way
      to tell a stopped server from an unreachable one.
    - >-
      States that recovery is automatic and the world is safe, so the model
      reassures instead of proposing fixes the friend can't carry out.
---
couldn't confirm {{server}} stopped — the machine it was running on stopped
responding mid-shutdown; its world is saved and it will be moved to another
machine automatically, so no action is needed
