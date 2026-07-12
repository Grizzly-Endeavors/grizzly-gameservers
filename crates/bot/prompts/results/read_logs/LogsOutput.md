---
id: LogsOutput
type: prompt
annotations:
  sent_when: >-
    tool result carrying recent log output back to the model after read_logs
  used_by:
    - file: discord/gary/tools.rs
      function: format_logs
  variables:
    server:
      source: the server whose logs were read
      contents: the server name
    lines:
      source: the recent log lines, joined by newlines in format_logs
      contents: the log lines as returned, newline-joined; the joining is data and stays in code
  reasoning:
    - >-
      Labels the payload as recent output so the model reads it as diagnostics,
      not instructions. The log lines are data entering through the variable;
      only the "recent output from" framing is prompt text.
---
recent output from {{server}}:
{{lines}}
