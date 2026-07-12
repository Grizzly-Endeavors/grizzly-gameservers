---
id: ChangeRollbackFailedFlagged
type: prompt
annotations:
  sent_when: >-
    tool result when a config change crashed the server, the automatic rollback
    ran, and the server still wouldn't come up after it — the guardrail's give-up
    point, which escalates to a human
  used_by:
    - file: discord/gary/tools.rs
      function: verify_change
  variables:
    path:
      source: the edited config file path
      contents: the file whose change crashed the server
    server:
      source: the server that crashed
      contents: the server name
    escalation:
      source: the OperatorFlagged prompt, rendered and spliced into the tail
      contents: the shared "flagged for an operator" closing clause
  reasoning:
    - >-
      The bounded-to-one-rollback failure: the change AND its rollback both left
      the server down, so the loop stops and hands off to a human. The escalation
      tail is shared with the other two rollback-failure results via OperatorFlagged.
---
the change to {{path}} crashed {{server}}, and rolling it back didn't bring it up either — {{escalation}}
