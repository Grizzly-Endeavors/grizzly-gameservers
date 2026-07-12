---
id: LifecycleWaking
type: prompt
annotations:
  sent_when: >-
    tool result when a start/resume lifecycle op succeeds and the server is
    coming back up from a paused state
  used_by:
    - file: discord/gary/tools.rs
      function: format_supervisor
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      Sets the expectation that resume is near-instant ("a few seconds"), so
      Gary doesn't over-promise a long wait for a warm server.
---
{{server}} is waking up — ready in a few seconds
