---
id: LogsEmpty
type: prompt
annotations:
  sent_when: >-
    tool result when read_logs finds the server has produced no output yet
  used_by:
    - file: discord/gary/tools.rs
      function: format_logs
  variables:
    server:
      source: the server whose logs were read
      contents: the server name
  reasoning:
    - >-
      Distinguishes "no output yet" from an error so the model reports a quiet
      server rather than a failed read, and doesn't invent log lines.
---
{{server}} hasn't produced any output yet
