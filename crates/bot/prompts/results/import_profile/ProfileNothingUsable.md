---
id: ProfileNothingUsable
type: prompt
annotations:
  sent_when: >-
    tool result when every mod in an imported profile was unknown to this game's
    registry, so writing the list would have emptied it
  used_by:
    - file: discord/gary/profiles.rs
      function: exec_import_profile
  variables:
    code:
      source: the profile code the model passed to the tool
      contents: the code, quoted inline
    missing:
      source: the profile entries this community has no record of
      contents: >-
        a comma-separated list of `Namespace-Name-1.2.3` releases; never empty,
        since this result is only produced when at least one entry is missing
  reasoning:
    - >-
      Refuses rather than writing an empty list, because an empty list uninstalls
      everything the server runs — the one outcome nobody asking for a modpack
      could have wanted, and one whose cause would be invisible afterwards.
    - >-
      The overwhelmingly likely cause is a profile exported for a different game,
      so the result says that outright rather than leaving the model to present a
      list of unknown names as a mystery.
---
None of the mods in {{code}} exist in this game's registry: {{missing}}. Nothing was written — emptying the mod list would have removed everything the server currently runs. A profile exported for a different game looks exactly like this, so check with them which game they exported it from.
