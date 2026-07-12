---
id: CreateBadName
type: prompt
annotations:
  sent_when: >-
    tool result when create_server is given a name that fails validation (bad
    characters, too long, etc.)
  used_by:
    - file: discord/gary/tools.rs
      function: exec_create
  variables:
    error:
      source: the name-validation error from build_instance_name
      contents: the specific reason the name was rejected, appended after the colon
  reasoning:
    - >-
      Surfaces the exact naming rule that was broken so the model can suggest a
      valid name, rather than guessing blindly at what's allowed.
---
that name won't work: {{error}}
