---
id: ThunderstoreNotFound
type: prompt
annotations:
  sent_when: >-
    tool result when a mod_info call for a Thunderstore game reaches the
    registry successfully but it has no such package
  used_by:
    - file: discord/gary/tools.rs
      function: exec_mod_info
  variables:
    game:
      source: the catalog id the model passed to the tool
      contents: the game id, quoted inline
    slug:
      source: the identifier the model passed to the tool
      contents: the unresolved identifier, quoted inline
  reasoning:
    - >-
      An ordinary answer rather than a failure, and the exact case this tool
      exists to catch, so it steers to search_mods instead of leaving the model
      to conclude the registry is broken.
    - >-
      Names the Namespace-Name shape, because the likeliest cause of landing
      here is a bare Modrinth-style slug or a display name rather than a
      genuinely missing package — and that mistake is fixed by reshaping the
      identifier, not by searching again.
    - >-
      Says outright not to install it, because the alternative — writing the
      unresolved identifier into the mod list anyway — stops the server from
      starting.
---
the {{game}} mod registry has no package called {{slug}} — it may be misspelled, renamed, or written in the wrong shape. Identifiers here are a Namespace-Name pair (`ValheimModding-Jotunn`), not a single word and not a display name. Find it with search_mods and use the exact pair from the results; don't install this one.
