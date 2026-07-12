---
id: ReadyStopped
type: prompt
annotations:
  sent_when: >-
    tool result when a readiness wait finds the server stopped, so it won't come
    up on its own
  used_by:
    - file: discord/gary/tools.rs
      function: format_ready_wait
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      Tells the model the server needs an explicit start (it's stopped, not
      crashing), so Gary offers to start it rather than waiting for a boot that
      won't happen.
---
{{server}} is stopped, so it won't come up on its own — start it first
