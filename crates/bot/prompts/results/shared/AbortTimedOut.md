---
id: AbortTimedOut
type: prompt
annotations:
  sent_when: >-
    the reason opener of a ConfirmAborted result when the destructive
    confirmation prompt timed out with no response
  used_by:
    - file: discord/gary/tools.rs
      function: finish_destroy
  reasoning:
    - >-
      Names the timeout (nobody responded) so the model can offer to try again,
      distinct from a deliberate cancel. Rendered into ConfirmAborted's reason
      slot.
---
the confirmation timed out
