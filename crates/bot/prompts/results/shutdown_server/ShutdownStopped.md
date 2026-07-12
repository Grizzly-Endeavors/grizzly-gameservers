---
id: ShutdownStopped
type: prompt
annotations:
  sent_when: >-
    tool result when shutdown_server fully stops a server (a cold stop, distinct
    from a warm pause)
  used_by:
    - file: discord/gary/tools.rs
      function: exec_shutdown
  variables:
    server:
      source: the server being shut down
      contents: the server name
  reasoning:
    - >-
      Reassures that a full stop is non-destructive (world saved, restartable), so
      the model relays that it can be brought back rather than implying data loss.
---
stopped {{server}}; its world is saved and it can be started again
