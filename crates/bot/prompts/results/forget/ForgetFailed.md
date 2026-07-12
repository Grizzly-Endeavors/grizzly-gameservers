---
id: ForgetFailed
type: prompt
annotations:
  sent_when: >-
    tool result when forget errors while removing a fact (an unexpected failure,
    not a clean "offline" or "not found")
  used_by:
    - file: discord/gary/tools.rs
      function: exec_forget
  reasoning:
    - >-
      Reports a genuine failure so the model doesn't claim the fact was removed;
      the underlying error is logged for developers, not shown here.
---
something went wrong forgetting that.
