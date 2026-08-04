---
id: EscalateHandedOff
type: prompt
annotations:
  sent_when: >-
    tool result when escalate records the handoff; the loop stops on the next
    check, so this is the last thing the model is shown
  used_by:
    - file: discord/gary/tools.rs
      function: exec_escalate
  reasoning:
    - >-
      Short and terminal on purpose. The model never gets another turn after
      this — the loop reads the recorded handoff and stops — so anything longer
      would only be inviting a plan that can't run.
---
handed off to a human; your message has been sent and an operator has been notified. this turn is over.
