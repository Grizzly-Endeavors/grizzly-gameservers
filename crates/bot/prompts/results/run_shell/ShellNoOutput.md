---
id: ShellNoOutput
type: prompt
annotations:
  sent_when: >-
    tool result after run_shell when the command produced nothing on either
    stream
  used_by:
    - file: discord/gary/tools.rs
      function: format_shell
  variables:
    server:
      source: the server the command ran on
      contents: the server name
    command:
      source: the command line the model asked for
      contents: the command as given
    status:
      source: >-
        how the command ended, rendered from ShellExited, ShellSignalled or
        ShellTimedOut in format_shell
      contents: >-
        the rendered status fragment; it is the platform's own account, never
        anything the command printed
  reasoning:
    - >-
      A silent command is a real answer — `grep` finding nothing is the usual way
      to rule a cause out — so it gets a result that says so plainly rather than
      an empty fence the model might read as a failure and retry.
---
`{{command}}` on {{server}} {{status}} and printed nothing.
