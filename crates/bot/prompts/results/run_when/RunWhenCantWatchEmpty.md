---
id: RunWhenCantWatchEmpty
type: prompt
annotations:
  sent_when: >-
    tool result when run_when is asked to wait for a server to be empty or idle
    but that game can't report a live player count, so the condition can't be
    watched
  used_by:
    - file: discord/gary/tools.rs
      function: empty_condition_feasibility
  variables:
    server:
      source: the server the empty/idle wait targeted
      contents: the server name
  reasoning:
    - >-
      A hard feasibility refusal — the empty/idle wait is unwatchable for this
      game — so the model doesn't queue a wait that would never fire. Offers the
      two real alternatives (do it now, or have the user signal) instead.
---
I can't tell when {{server}} is empty — this game doesn't report a live player count, so I can't wait for it to clear. Offer to make the change now, or ask them to tell you when to.
