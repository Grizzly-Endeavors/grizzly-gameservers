---
id: GameGuideIntro
type: prompt
annotations:
  sent_when: >-
    the opening line of every game_guide result for a game that does have a mod
    registry
  used_by:
    - file: discord/gary/guide.rs
      function: render_guide
  variables:
    game:
      source: the catalog id the guide was asked for
      contents: the game id, quoted inline
  reasoning:
    - >-
      Names the game the workflow belongs to, because the rules below are not
      transferable: each registry has its own identifiers and its own install
      path, and a guide relayed onto the wrong game produces confident,
      unworkable instructions.
    - >-
      Says the guide is for this server's game rather than for mods in general,
      so a second game in the same conversation is a second call rather than an
      assumption that what was learned still applies.
---
Here is how mods work on {{game}}. These rules are specific to this game — another game's identifiers and install steps are different, so ask for its guide separately rather than reusing this one.
