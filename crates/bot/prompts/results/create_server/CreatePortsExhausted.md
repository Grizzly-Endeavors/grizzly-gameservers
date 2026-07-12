---
id: CreatePortsExhausted
type: prompt
annotations:
  sent_when: >-
    tool result when create_server can't provision because every server port
    slot is in use
  used_by:
    - file: discord/gary/tools.rs
      function: exec_create
  reasoning:
    - >-
      Explains the capacity limit and the way out (destroy one first), so the
      model offers to free a slot rather than retrying a create that can't
      succeed. Worded for the create path specifically (destroy, not archive).
---
all server slots are in use right now — destroy one first, then try again
