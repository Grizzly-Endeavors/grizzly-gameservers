---
id: IngameServerUnclear
type: prompt
annotations:
  sent_when: >-
    tool result on the in-game surface when server_status is called but the
    server name argument didn't parse, so it's unclear which server was meant
  used_by:
    - file: ingame/agent.rs
      function: dispatch_ingame
  reasoning:
    - >-
      Kept short and plain for in-game chat (distinct from the Discord surface's
      JSON-parse wording), steering the model to ask which server rather than
      guessing or failing the loop.
---
I couldn't tell which server you meant.
