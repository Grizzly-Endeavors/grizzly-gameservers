---
id: ModNotFound
type: prompt
annotations:
  sent_when: >-
    tool result when mod_info reaches Modrinth successfully but the registry has
    no project with that slug
  used_by:
    - file: discord/gary/tools.rs
      function: exec_mod_info
  variables:
    slug:
      source: the slug the model passed to the tool
      contents: the unresolved slug, quoted inline
  reasoning:
    - >-
      An ordinary answer rather than a failure, and the exact case this tool
      exists to catch, so it steers to search_mods instead of leaving the model
      to conclude the registry is broken. A recalled or guessed slug landing
      here is the check working.
    - >-
      Says outright not to install it, because the alternative — writing the
      unresolved slug into the config anyway — stops the server from starting.
---
Modrinth has no project with the slug {{slug}} — it may be misspelled, renamed, or remembered wrong. Search for it with search_mods and use the exact slug from the results; don't install this one.
