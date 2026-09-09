---
id: ShellSignalled
type: prompt
annotations:
  sent_when: >-
    the status fragment of a run_shell result, for a command killed by a signal
    that was not the supervisor's own timeout
  used_by:
    - file: discord/gary/tools.rs
      function: format_shell_status
  reasoning:
    - >-
      Distinguished from an exit code because the usual cause is the kernel
      stopping the command (an OOM kill on a huge file), which is a fact about
      the command rather than about the server being investigated.
---
was killed before it finished
