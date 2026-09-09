---
id: ProfilesNotSupported
type: prompt
annotations:
  sent_when: >-
    tool result when import_profile or export_profile targets a server whose
    game does not use r2modman profile codes
  used_by:
    - file: discord/gary/profiles.rs
      function: target
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the server name, quoted inline
    game:
      source: >-
        the catalog id of the game the server runs, from its live listing;
        rendered as the literal `unknown game` when the instance carries no
        game label at all
      contents: the game id, quoted inline
  reasoning:
    - >-
      A final answer rather than a gap, so it says what this game does instead
      of leaving the model to retry with a different argument or conclude the
      registry is down.
    - >-
      Points at game_guide rather than restating a workflow, because the game
      that most often lands here is Minecraft — which does have mods, just not
      profile codes — and the correction is to use that game's own install
      route, not to give up on the request.
---
{{server}} runs {{game}}, whose mods don't come from an r2modman registry, so profile codes don't apply to it. That doesn't mean it takes no mods — call game_guide for {{game}} to see how mods work there, and tell them the code they have is for a different modding ecosystem.
