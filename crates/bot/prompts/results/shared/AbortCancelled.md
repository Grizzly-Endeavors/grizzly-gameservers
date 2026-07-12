---
id: AbortCancelled
type: prompt
annotations:
  sent_when: >-
    the reason opener of a ConfirmAborted result when the user clicked cancel on
    a destructive confirmation
  used_by:
    - file: discord/gary/tools.rs
      function: finish_destroy
    - file: discord/gary/tools.rs
      function: exec_archive
    - file: discord/gary/tools.rs
      function: exec_restore
  reasoning:
    - >-
      Names the deliberate cancel (a human said no) so the model relays a chosen
      abort rather than a failure. Rendered into ConfirmAborted's reason slot.
---
the user cancelled
