---
id: BadToolArguments
type: prompt
annotations:
  sent_when: >-
    tool result when a tool's arguments don't parse as valid JSON — returned in
    place of running the tool so the model can fix the call
  used_by:
    - file: discord/gary/tools.rs
      function: parse
  variables:
    error:
      source: the serde_json parse error
      contents: the raw deserializer error detail, in parentheses
  reasoning:
    - >-
      Steers the model to re-check argument names and types and call again,
      rather than erroring the whole loop. The parser error is included as data
      so the model can see exactly what was malformed.
---
the arguments for that tool weren't valid JSON ({{error}}); check the argument names and types and call it again
