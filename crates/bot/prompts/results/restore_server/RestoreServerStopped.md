---
id: RestoreServerStopped
type: prompt
annotations:
  sent_when: >-
    tool result when restore_server restores the world but the server stays
    paused and won't come up on its own
  used_by:
    - file: discord/gary/tools.rs
      function: format_restore_outcome
  variables:
    server:
      source: the server that was restored
      contents: the server name
  reasoning:
    - >-
      Distinguishes a paused-after-restore server from a crash: the restore
      worked, it just needs an explicit start, so the model offers to start it
      rather than diagnosing a failure.
---
restored the world onto {{server}}, but it's paused and won't come up on its own — start it when you're ready
