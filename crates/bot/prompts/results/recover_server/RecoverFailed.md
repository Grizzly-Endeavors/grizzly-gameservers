---
id: RecoverFailed
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server couldn't bring an archived server back
    cleanly
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  variables:
    name:
      source: the archive name that was attempted
      contents: the server name
  reasoning:
    - >-
      A retryable failure with no partial-state claim, so the model can try again
      without implying the archive was consumed or damaged.
---
I couldn't bring {{name}} back cleanly — worth trying again in a moment
