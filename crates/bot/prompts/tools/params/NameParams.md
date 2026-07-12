---
id: NameParams
type: params
tool_schema:
  name:
    type: string
    description: Exact server name, as shown by `list_servers`.
annotations:
  used_by:
    - file: discord/gary/tools.rs
      function: dispatch
    - file: discord/gary/tools.rs
      function: dispatch_mutating
    - file: ingame/agent.rs
      function: dispatch_ingame
  reasoning:
    - >-
      A dozen lifecycle and lookup tools across both surfaces take exactly one
      argument — the server name — and share this one shape so their contracts
      can't drift into subtly different server-targeting arguments. One
      definition, one generated struct, tuned in one place.
    - >-
      The scope gate in dispatch reads just this `name` field off any targeted
      tool's arguments; keeping the shared description accurate here keeps that
      field's meaning consistent everywhere it is advertised.
---
