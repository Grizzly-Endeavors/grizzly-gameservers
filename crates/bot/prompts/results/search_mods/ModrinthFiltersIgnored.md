---
id: ModrinthFiltersIgnored
type: prompt
annotations:
  sent_when: >-
    prepended to a search_mods result when the call set Minecraft-only filters
    on a game whose registry is not Modrinth
  used_by:
    - file: discord/gary/tools.rs
      function: exec_search_mods
  variables:
    game:
      source: the catalog id the model passed to the tool
      contents: the game id, quoted inline
    filters:
      source: >-
        the names of the Minecraft-only parameters that were set, comma-joined
      contents: >-
        one or more of kind, loader, game_version, comma-separated; the joining
        is data and stays in code
  reasoning:
    - >-
      Says the filters were dropped rather than dropping them silently, because
      a narrower result set than was asked for reads as a real answer: without
      this, a model that filtered to `datapack` and got a full page back would
      conclude every one of those Valheim packages is a datapack.
    - >-
      Stated as a fact about the registry rather than an error, since the call
      still succeeded and its results are good — the only wrong thing would be
      trusting a narrowing that never happened.
---
note: {{filters}} only apply to Minecraft's registry and were ignored — {{game}} does not have those concepts. The results below are unfiltered by them.
