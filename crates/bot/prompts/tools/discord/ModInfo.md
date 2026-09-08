---
id: ModInfo
type: tool
tool_schema:
  game:
    type: string
    description: >-
      Which game's registry to look in, as a catalog id — `minecraft`,
      `valheim`. Take it from the server you are working on.
  slug:
    type: string
    description: >-
      The mod's exact identifier as its registry spells it: a Modrinth slug for
      Minecraft (`terralith`, not `Terralith`), or a Thunderstore
      `Namespace-Name` pair for the other games (`ValheimModding-Jotunn`).
      Use what `search_mods` returned, or what a registry URL shows — never a
      display name.
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch
  reasoning:
    - >-
      The validation half of mod management, and the one guard the automatic
      rollback can't provide cheaply: an identifier that doesn't exist fails the
      server's next launch, and because the bad value is read back off the same
      volume, a replacement server fails the same way. Checking it before
      writing it is what keeps that from happening, so the description frames
      this as the step before an install rather than an optional lookup.
    - >-
      Separate from search_mods rather than folded into it because the questions
      differ: search answers "what exists", this answers "does this exact thing
      exist, and does it fit this server" — the check to run when a user names a
      mod directly or hands over a link.
    - >-
      The slug description spells out both registries' shapes because the two
      are not interchangeable and a Modrinth-style bare slug sent to a
      Thunderstore game is the most likely mistake — it looks like a real
      identifier and would otherwise read back as "no such mod".
---
Look up one mod by its exact identifier and confirm it really exists before you write it into a server's config. Pass the `game` whose registry it belongs to. Reports what the mod is, its current version, where it has to be installed, whether the author has deprecated it, and what else it needs installed alongside it. An identifier that doesn't exist, or that doesn't fit what the server runs, will stop the server from starting — call `game_guide` for that game to see how to install it once it checks out.
