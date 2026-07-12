---
id: Remember
type: tool
tool_schema:
  scope:
    type: string
    description: >-
      Which game this fact is about — a catalog game id (e.g. `palworld`), or
      `general` for something that isn't tied to one game.
  note:
    type: string
    description: >-
      The fact to remember, in one short sentence — a durable operational detail
      you'd otherwise have to rediscover, e.g. "soft-stop before editing configs
      or the change won't apply".
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: manager_tools
    - file: discord/gary/tools.rs
      function: dispatch_memory
  reasoning:
    - >-
      Gary's cross-session memory. The description is strict about what deserves
      saving (durable operational facts, one sentence) because the saved notes
      are re-injected every session — junk here costs tokens and attention
      forever. Memory is cross-guild, so this tool carries no server name and
      skips the scope gate (dispatched via dispatch_memory, not
      dispatch_mutating).
---
Save a durable fact about a game so you keep it across sessions — an operational detail you'd otherwise rediscover every time (e.g. a game needs to be stopped before a config edit will apply, or where a setting lives). Scope it to the game id, or 'general' if it's not game-specific. Keep it to one short factual sentence. Your saved facts are shown to you each session under "Things you've learned". Don't save one-off state, chit-chat, or anything you can just look up.
