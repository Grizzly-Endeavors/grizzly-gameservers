---
id: OperatorFlagged
type: prompt
annotations:
  sent_when: >-
    the shared closing clause of every auto-rollback escalation result — spliced
    into the tail of the three change-rollback failure messages when a human
    operator has actually been notified
  used_by:
    - file: discord/gary/tools.rs
      function: verify_change
    - file: discord/gary/tools.rs
      function: roll_back
    - file: discord/gary/tools.rs
      function: rollback_failed
  reasoning:
    - >-
      One home for the "flagged for an operator" promise so the three escalation
      results stay in lockstep — the phrasing is a promise the code keeps by
      actually DMing the operators, so it must not drift between the three
      failure paths. Rendered and spliced into each via the {{escalation}} slot.
---
I've flagged this for an operator to look at
