---
id: NonAdminRefusal
type: prompt
annotations:
  sent_when: >-
    tool result when a non-admin caller reaches an admin-only tool (destroy,
    send_command, archive, restore, recover) — defense in depth, since those
    tools aren't offered below admin
  used_by:
    - file: discord/gary/tools.rs
      function: tier_refusal
  reasoning:
    - >-
      Tells the model it lacks the tier for destructive actions but can still do
      lookups and day-to-day changes, so Gary offers what it can rather than a
      flat "no". Belt-and-braces refusal — the tool was never advertised at this
      tier, but a hallucinated call still lands here.
---
that action needs an admin — I can only look things up or run day-to-day changes for you here.
