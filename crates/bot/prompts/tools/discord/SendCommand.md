---
id: SendCommand
type: tool
tool_schema:
  name:
    type: string
    description: Exact server name, as shown by `list_servers`.
  command:
    type: string
    description: >-
      The in-game console command to run, without a leading slash — e.g. `list`,
      `say hello everyone`, `weather clear`, `whitelist add steve`.
annotations:
  sent_when: offered on the Discord surface to admins only.
  used_by:
    - file: discord/gary/tools.rs
      function: admin_only_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      Live RCON access — admin-only because it runs arbitrary console commands
      that take effect immediately. The description gives concrete examples so
      the model formats the command right (no leading slash) and notes it only
      works where RCON is enabled.
---
Run an in-game console command on a running server over RCON (e.g. list, say, weather, whitelist, op) and return the game's reply. Takes effect immediately — no restart needed. Only works on games that have RCON enabled.
