---
id: ChangeRollbackUnclean
type: prompt
annotations:
  sent_when: >-
    tool result when a config change crashed the server, the previous version was
    restored, but the restart after the rollback couldn't be issued cleanly — the
    world is back but not running, so it escalates
  used_by:
    - file: discord/gary/tools.rs
      function: roll_back
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
      Distinguishes "old version restored but couldn't restart" from the other
      two rollback failures so a human knows the data is safe but the process is
      down. Shares the escalation tail via OperatorFlagged.
---
the change to {{path}} crashed {{server}}; I put the previous version back but couldn't restart it cleanly — {{escalation}}
