---
id: ShellTimedOut
type: prompt
annotations:
  sent_when: >-
    the status fragment of a run_shell result, for a command the supervisor
    killed at its time limit
  used_by:
    - file: discord/gary/tools.rs
      function: format_shell_status
  reasoning:
    - >-
      Names the limit as the cause so the model narrows the command — a bounded
      `grep` over one file — rather than retrying the same unbounded scan of a
      whole world directory and losing another turn to it.
---
ran past its 30-second limit and was stopped
