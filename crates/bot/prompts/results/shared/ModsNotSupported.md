---
id: ModsNotSupported
type: prompt
annotations:
  sent_when: >-
    tool result when search_mods or mod_info names a real catalog game that
    declares no mod registry
  used_by:
    - file: discord/gary/tools.rs
      function: mods_not_supported
  variables:
    game:
      source: the catalog id the model passed to the tool
      contents: the game id, quoted inline
  reasoning:
    - >-
      A final answer, not a retry: no other game id will make this one moddable,
      and a model that read this as a failed lookup would work through the whole
      catalog looking for one that answers.
    - >-
      Says the bot can't install mods here rather than that the game has no
      mods, because several of these games do have modding communities — what's
      missing is a supported path, and telling a friend their game has no mods
      would be false.
---
{{game}} has no mod registry wired up here, so there is nothing to search and no way to install mods on it through this bot. Say so plainly — don't try another game id, and don't offer to install anything by hand. If someone wants mods on this game, escalate it as a feature request.
