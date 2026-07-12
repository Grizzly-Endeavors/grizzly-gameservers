---
id: CommandNoOutput
type: prompt
annotations:
  sent_when: >-
    tool result when send_command runs a console command that returns no output
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
  reasoning:
    - >-
      Confirms the command ran even with empty output, so the model reports
      success rather than treating silence as failure and re-running it.
---
ran `{{command}}` on {{server}}; the server returned no output
