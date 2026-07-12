---
id: ReadyBackUp
type: prompt
annotations:
  sent_when: >-
    tool result when a start/restart readiness wait confirms the server came up
    and is accepting players
  used_by:
    - file: discord/gary/tools.rs
      function: format_ready_wait
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      The success end of a readiness wait, distinct from the immediate lifecycle
      confirmations because it means players can actually connect now — worded so
      Gary tells them it's playable.
---
{{server}} is back up and accepting players
