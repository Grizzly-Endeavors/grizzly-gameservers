---
id: LifecycleRestarted
type: prompt
annotations:
  sent_when: >-
    tool result when a plain restart (no tracked config change to verify)
    succeeds
  used_by:
    - file: discord/gary/tools.rs
      function: format_supervisor
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      The immediate-return restart result (a config-change restart takes the
      verify_change path instead), worded so Gary relays a quick bounce.
---
restarted {{server}} — back up in a few seconds
