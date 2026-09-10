---
id: GameGuide
type: tool
tool_schema:
  game:
    type: string
    description: >-
      Which game's configuration workflow to load, as a catalog id —
      `minecraft`, `valheim`. Take it from the server you are working on;
      `about_server` reports it.
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch
  reasoning:
    - >-
      Covers two per-game questions rather than one. Mod management was the
      original reason; where a game's settings live is the second, and it was
      added because on at least one game they are not in any file — a search of
      the data directory for them cannot succeed, and nothing else told Gary
      that before he spent a whole round budget looking.
    - >-
      Per-game knowledge used to ride the system prompt on every manager turn
      for every guild, whether or not that guild ran the game it described.
      Loading it on demand is what keeps a third and fourth game's workflow from
      costing every conversation.
    - >-
      Framed as a required first step rather than a reference, because on both
      halves the wrong habit produces a confident, wrong change. The two games'
      mod rules invert each other — on one, removing a mod means rewriting a
      list and never deleting the line; on the other it means deleting the line.
      And a setting that lives in the environment is invisible to the file
      search that would otherwise find it.
    - >-
      Targets no server, so it is safe to call before knowing which server is
      meant and costs nothing when the answer is "settings are in the usual
      files and this game takes no mods".
---
Load how one game is configured: where its settings are kept — which differs per game, and on some of them is not a config file at all — and, if it takes mods, how they are identified, which file to write, and how to remove one. Call this before changing a setting on a server, and before searching for, recommending, installing, or removing a mod. The rules differ per game and some are exact opposites, so working from another game's habits produces a change that looks right and isn't. Targets no server and changes nothing.
