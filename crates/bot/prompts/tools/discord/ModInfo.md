---
id: ModInfo
type: tool
tool_schema:
  slug:
    type: string
    description: >-
      The project's exact Modrinth slug, as returned by `search_mods` or taken
      from a modrinth.com URL — e.g. `terralith`, not `Terralith`.
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
      rollback can't provide: a slug that doesn't exist fails the server's boot,
      and because the bad value is read back off the same volume, the replacement
      server fails the same way. Checking the slug before writing it is what
      keeps that from happening, so the description frames this as the step
      before an install rather than an optional lookup.
    - >-
      Separate from search_mods rather than folded into it because the questions
      differ: search answers "what exists", this answers "does this exact thing
      exist, and does it fit this server" — the check to run when a user names a
      mod directly or hands over a link.
---
Look up one Modrinth project by its exact slug. Use this to confirm a mod or datapack really exists and fits a server — check its loaders and Minecraft versions against what that server runs — before writing the slug into a server's config. A slug that doesn't exist, or that has no version for the server's Minecraft version, will stop the server from starting.
