---
id: ChangeRollbackImpossible
type: prompt
annotations:
  sent_when: >-
    tool result when a config change crashed the server and the automatic
    rollback couldn't even be issued — nothing more the loop can do on its own,
    so it escalates
  used_by:
    - file: discord/gary/tools.rs
      function: rollback_failed
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
      The worst-case rollback failure — the restore itself couldn't run, so the
      bad change may still be in place. Kept distinct from the other two so a
      human sees the exact failure mode. Shares the escalation tail via
      OperatorFlagged.
---
the change to {{path}} crashed {{server}} and I couldn't roll it back automatically — {{escalation}}
