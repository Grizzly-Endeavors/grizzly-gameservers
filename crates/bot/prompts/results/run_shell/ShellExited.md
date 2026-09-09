---
id: ShellExited
type: prompt
annotations:
  sent_when: >-
    the status fragment of a run_shell result, for a command that ran to
    completion
  used_by:
    - file: discord/gary/tools.rs
      function: format_shell_status
  variables:
    code:
      source: the command's exit status
      contents: the exit code as a number
  reasoning:
    - >-
      Reported for every command, not only failures: a model that has to infer
      success from the presence of output cannot tell an empty result from a
      broken one.
---
exited {{code}}
