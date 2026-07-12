---
id: RestoreServerFailed
type: prompt
annotations:
  sent_when: >-
    tool result when restore_server couldn't complete the restore cleanly
  used_by:
    - file: discord/gary/tools.rs
      function: format_restore_outcome
  variables:
    server:
      source: the server that was targeted
      contents: the server name
  reasoning:
    - >-
      A retryable failure with no partial-state claim, so the model can try again
      without implying the world was damaged mid-restore.
---
I couldn't restore {{server}} cleanly — worth trying again in a moment
