---
id: ListArchives
type: tool
tool_schema: {}
annotations:
  sent_when: offered on the Discord surface to every caller — read-only, manager, and admin.
  used_by:
    - file: discord/gary/tools.rs
      function: available_tools
    - file: discord/gary/tools.rs
      function: dispatch
  reasoning:
    - >-
      Lists the guild's cold-storage servers so the model knows what
      recover_server can bring back. Zero-argument — it scopes its own listing
      to the caller's guild, so it takes no server name.
---
List the servers archived in this Discord server — ones that were put into cold storage and can be recovered.
