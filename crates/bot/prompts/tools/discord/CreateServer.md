---
id: CreateServer
type: tool
tool_schema:
  game:
    type: string
    description: Which game to launch — must be one of the catalog game ids.
  name:
    type: string
    description: Optional world name. A name is generated when omitted.
    optional: true
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: manager_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      Not a NameParams tool: it takes a catalog game id (not an existing server
      name) plus an optional world name, so it defines its own shape. The name
      is optional because a sensible one is generated when omitted — the
      #[serde(default)] on the generated Option is what lets the model leave it
      out.
---
Launch a new game server for the given catalog game, with an optional world name.
