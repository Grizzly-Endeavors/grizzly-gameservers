---
id: RecoverPortsExhausted
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server can't recreate an archive because every
    server port slot is in use
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  reasoning:
    - >-
      Explains the capacity limit and the way out for the recover path (archive
      or destroy one first — note "archive" is offered here, unlike the create
      path which only offers destroy), so the model frees a slot rather than
      retrying.
---
all server slots are in use right now — archive or destroy one first
