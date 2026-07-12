---
id: ServerAlreadyRunning
type: prompt
annotations:
  sent_when: >-
    tool result when a start/resume op targets a server that is already running
    — a no-op reported from the supervisor outcome and from the cold-start path
  used_by:
    - file: discord/gary/tools.rs
      function: format_supervisor
    - file: discord/gary/tools.rs
      function: exec_cold_start
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      States the no-op so Gary reports the server is up rather than retrying a
      start. Shared by the supervisor lifecycle path and the cold-start path,
      which reach the same condition from different entry points.
---
{{server}} is already running
