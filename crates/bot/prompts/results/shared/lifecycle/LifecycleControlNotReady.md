---
id: LifecycleControlNotReady
type: prompt
annotations:
  sent_when: >-
    tool result when a lifecycle op reaches the supervisor but the pod isn't
    ready to accept control commands yet
  used_by:
    - file: discord/gary/tools.rs
      function: format_supervisor
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      Marks the state transient ("try again shortly") so Gary waits and retries
      rather than treating a not-yet-ready pod as a hard failure.
---
{{server}} isn't ready to control yet — try again shortly
