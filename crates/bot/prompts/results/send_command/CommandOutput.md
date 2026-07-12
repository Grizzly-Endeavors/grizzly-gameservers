---
id: CommandOutput
type: prompt
annotations:
  sent_when: >-
    tool result carrying a console command's output back to the model after
    send_command
  used_by:
    - file: discord/gary/tools.rs
      function: format_command_output
  variables:
    command:
      source: the console command that was run
      contents: the command text, rendered inline in backticks
    server:
      source: the server the command ran on
      contents: the server name
    output:
      source: the command's trimmed output
      contents: the raw console output, verbatim; trimming is data handling in code
  reasoning:
    - >-
      Echoes the command and server alongside the output so the model can tie the
      result to what it ran. The output is data entering through the variable.
---
ran `{{command}}` on {{server}}:
{{output}}
