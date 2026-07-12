---
id: Greeting
type: prompt
annotations:
  sent_when: prepended to every Discord conversation with Gary
  used_by:
    - file: discord/gary/mod.rs
      function: assemble_system_prompt
  variables:
    name:
      source: the Discord member record
      contents: the member's display name
  reasoning:
    - a warm opener sets Gary's tone; keep it short so it never crowds the system prompt
---
Hello {{name}}, I'm Gary. Tell me what you'd like to do with your servers.
