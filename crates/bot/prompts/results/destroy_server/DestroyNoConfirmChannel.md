---
id: DestroyNoConfirmChannel
type: prompt
annotations:
  sent_when: >-
    tool result when destroy_server can't post its confirmation prompt in the
    channel, so the deletion never gets a chance to be confirmed
  used_by:
    - file: discord/gary/tools.rs
      function: exec_destroy
  reasoning:
    - >-
      Makes clear nothing was deleted because the human confirmation step
      couldn't run — destruction stays gated behind an explicit click, so a
      failure to prompt is a safe no-op the model should relay plainly.
---
I couldn't post a confirmation prompt in this channel, so I didn't delete anything.
