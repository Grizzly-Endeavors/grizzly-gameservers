---
id: ForgetForgot
type: prompt
annotations:
  sent_when: >-
    tool result when forget successfully removes a saved fact by id
  used_by:
    - file: discord/gary/tools.rs
      function: exec_forget
  variables:
    id:
      source: the fact id that was removed
      contents: the forgotten fact's id, as a number
  reasoning:
    - >-
      Confirms the specific fact was removed so the model can report exactly what
      it forgot rather than a vague acknowledgement.
---
forgot fact #{{id}}
