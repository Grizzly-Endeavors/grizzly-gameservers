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
    - >-
      Fenced because this is where attacker-controlled text actually arrives:
      player chat lands verbatim in the server's log buffer, so anyone who can
      connect to a public game port can put a line in here and wait for something
      to read it — the watchdog's incident prompt opens by reading logs, at
      manager tier, with nobody watching. The markers are matched by
      crate::untrusted, which removes any log line that tries to write one.
---
recent output from {{server}}:
-----BEGIN UNTRUSTED LOGS-----
{{lines}}
-----END UNTRUSTED LOGS-----
