---
id: Ping
type: tool
tool_schema: {}
annotations:
  sent_when: offered on every surface as a liveness check
  used_by:
    - file: discord/gary/tools.rs
      function: dispatch_tool
  reasoning:
    - a zero-parameter tool exercises the empty-schema codegen path
---
Check whether the server is responding.
