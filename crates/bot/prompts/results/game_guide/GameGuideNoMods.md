---
id: GameGuideNoMods
type: prompt
annotations:
  sent_when: >-
    the whole game_guide result for a real catalog game that declares no mod
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
      A final answer, not a retry. Without it a model that read this as a gap
      would work through the catalog looking for a game id that answers, or
      improvise an install by hand through the file tools — which is exactly the
      unsupervised, unrollback-able change the whole design exists to avoid.
    - >-
      Draws the line at "through this bot" rather than "at all", because several
      of these games do have modding communities. Telling a friend their game
      has no mods would be false; telling them there is no supported path here
      is true and is what they can act on.
---
There is no supported way to install mods on {{game}} through this bot — no registry is wired up for it, so there is nothing to search and nothing to write. Say so plainly. Don't improvise an install by hand through the file tools: anything put there that way is outside everything that would otherwise check it and put it back. If someone wants mods on this game, escalate it as a feature request.
