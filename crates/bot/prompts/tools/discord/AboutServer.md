---
id: AboutServer
type: tool
name: about_server
params_from: NameParams
annotations:
  sent_when: offered on the Discord surface to every caller — read-only, manager, and admin.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch
  reasoning:
    - >-
      The single-server deep look. Named for what it returns rather than for a
      state field, because it answers "tell me about this server": state and
      address, plus who is online, how long the process has been up, how many
      times it has crash-restarted, when it was last backed up, and what was last
      done to it.
    - >-
      list_servers already gives state and address for the whole roster, so the
      description leads with the history and health that only this tool carries
      and points back at list_servers for the roster question. Without that split
      the two read as interchangeable and the model calls this one per server just
      to rebuild a list it could have fetched in one call.
    - >-
      Shares NameParams with the lifecycle tools so the server-name argument means
      the same thing everywhere.
    - >-
      The wire name is declared explicitly because the in-game surface offers a
      terser variant under the same name (IngameAboutServer); the all-explicit
      collision policy requires every colliding file to mark the shared name.
---
Look up everything known about one server by name: its current state and address, who is online right now, how long it has been running, how often it has crashed, when it was last backed up, and what was last done to it. Use this before restarting or changing a server. To see which servers exist, use list_servers instead — it covers the whole roster in one call.
