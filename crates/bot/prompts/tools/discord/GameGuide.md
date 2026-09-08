---
id: GameGuide
type: tool
tool_schema:
  game:
    type: string
    description: >-
      Which game's mod workflow to load, as a catalog id — `minecraft`,
      `valheim`. Take it from the server you are working on; `about_server`
      reports it.
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch
  reasoning:
    - >-
      Mod management is per-game knowledge — different identifiers, a different
      file, and opposite removal rules — and it used to ride the system prompt
      on every manager turn for every guild, whether or not that guild ran the
      game it described. Loading it on demand is what keeps a third and fourth
      game's workflow from costing every conversation.
    - >-
      Framed as a required first step rather than a reference, because the two
      games' install rules invert each other: on one, removing a mod means
      rewriting a list and never deleting the line; on the other it means
      deleting the line. A model working from the general shape of "edit a
      config file" gets one of them exactly backwards.
    - >-
      Targets no server, so it is safe to call before knowing which server is
      meant and costs nothing when the answer is "this game takes no mods".
---
Load the mod workflow for one game: how mods are identified there, where they have to be installed, which file to write, and how to remove one. Call this before searching for, recommending, installing, or removing a mod on any server — the rules differ per game and two of them are exact opposites, so working from the other game's habits produces a change that looks right and isn't. Targets no server and changes nothing.
