---
id: ProfileNothingToExport
type: prompt
annotations:
  sent_when: >-
    tool result when export_profile found the server's resolved set to be empty
  used_by:
    - file: discord/gary/profiles.rs
      function: exec_export_profile
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the server name, quoted inline
  reasoning:
    - >-
      A code for an empty pack would be worse than no code: the player imports
      it, gets nothing, and believes they are in sync when they have not been
      given anything to sync to.
    - >-
      Says vanilla outright, because "no mods" is a perfectly good answer to
      "how do I set up to join" and the model should relay it as one rather than
      as a shortfall.
---
{{server}} isn't running any mods, so there's no pack to hand over — anyone can join it with a plain unmodded game. If they expected mods, check the mod list first; there may be nothing installed yet.
