---
id: LifecycleControlRefused
type: prompt
annotations:
  sent_when: >-
    tool result when the supervisor reached the server but its controls rejected
    the lifecycle command with a reason
  used_by:
    - file: discord/gary/tools.rs
      function: format_supervisor
  variables:
    server:
      source: the target server's instance name
      contents: the server name
    message:
      source: the supervisor's rejection reason
      contents: the raw reason text the supervisor returned, appended after the colon
  reasoning:
    - >-
      Surfaces the supervisor's own reason as data so Gary can relay the specific
      cause instead of a generic failure — the model can then decide whether to
      retry or explain.
---
{{server}}'s controls refused that: {{message}}
