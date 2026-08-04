---
id: EscalateNeedBoth
type: prompt
annotations:
  sent_when: >-
    tool result when escalate is called with an empty message or an empty
    reason; the handoff is not recorded and the turn continues
  used_by:
    - file: discord/gary/tools.rs
      function: exec_escalate
  reasoning:
    - >-
      A blank message would end the turn with nothing said to the person who
      asked, and a blank reason would page an operator with no diagnosis. Both
      are worse than not escalating, so the call is refused and the model gets
      another round to fill them in.
    - >-
      Names both fields and what each is for, because the likely mistake is
      writing one message and leaving the other empty rather than omitting both.
---
escalation not sent — it needs both a message (what the person who asked will read) and a reason (what the operator needs to know). fill in both and call it again.
