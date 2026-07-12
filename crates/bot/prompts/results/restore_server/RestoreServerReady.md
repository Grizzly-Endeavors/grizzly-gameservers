---
id: RestoreServerReady
type: prompt
annotations:
  sent_when: >-
    tool result when restore_server restores a backup and the server comes back
    up healthy on the restored world
  used_by:
    - file: discord/gary/tools.rs
      function: format_restore_outcome
  variables:
    server:
      source: the server that was restored
      contents: the server name
  reasoning:
    - >-
      The clean-success outcome: the model can tell the user the restore took and
      the server is playable on the restored world.
---
restored {{server}} — it's back up on the restored world
