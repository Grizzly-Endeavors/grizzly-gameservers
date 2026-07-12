---
id: RestoreServerCrashed
type: prompt
annotations:
  sent_when: >-
    tool result when restore_server restores the world but the server crashes
    coming back up
  used_by:
    - file: discord/gary/tools.rs
      function: format_restore_outcome
  variables:
    server:
      source: the server that was restored
      contents: the server name
  reasoning:
    - >-
      Names the likely cause (the restored data) and the next steps (read logs,
      or escalate) so the model investigates rather than blindly retrying — the
      restored world itself may be the problem.
---
restored the world onto {{server}}, but it crashed coming back up — read its logs (the restored data may be the cause), or ping an operator
