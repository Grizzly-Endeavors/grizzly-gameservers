---
id: NonManagerRefusal
type: prompt
annotations:
  sent_when: >-
    tool result when a read-only caller reaches a manager-tier tool (lifecycle,
    file edits, memory, backup) — defense in depth, since those tools aren't
    offered below manager
  used_by:
    - file: discord/gary/tools.rs
      function: tier_refusal
    - file: discord/gary/tools.rs
      function: dispatch_memory
  reasoning:
    - >-
      The read-only tier's refusal: Gary can look things up but not change them.
      Kept distinct from the admin refusal so the model relays the right ceiling
      (manager, not admin) and offers lookups instead.
---
that action needs a manager or an admin — I can only look things up for you here.
