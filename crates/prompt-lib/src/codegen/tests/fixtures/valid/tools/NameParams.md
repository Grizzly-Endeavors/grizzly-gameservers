---
id: NameParams
type: params
tool_schema:
  name:
    type: string
    description: the server name to act on
annotations:
  used_by:
    - file: src/discord/gary/tools.rs
      function: dispatch_tool
  reasoning:
    - a dozen lifecycle tools share this one shape so their contracts can't drift apart
---
