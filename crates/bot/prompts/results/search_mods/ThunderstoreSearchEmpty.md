---
id: ThunderstoreSearchEmpty
type: prompt
annotations:
  sent_when: >-
    tool result when a search_mods call for a Thunderstore game reaches the
    registry successfully but nothing survives the query and its side filter
  used_by:
    - file: discord/gary/tools.rs
      function: format_thunderstore_search
  variables:
    game:
      source: the catalog id the model passed to the tool
      contents: the game id, quoted inline
    query:
      source: the search text the model passed to the tool
      contents: the query text, quoted inline
  reasoning:
    - >-
      Distinguishes an empty result from a failed call so the model retries
      instead of reporting the registry down.
    - >-
      Names the side filter specifically as the likely cause, because it is the
      one that empties a result silently: authors tag sidedness inconsistently,
      so a real server-side mod with no category is filtered out and looks like
      it doesn't exist.
---
nothing in the {{game}} mod registry matched {{query}}. If a side filter was set, drop it first — authors tag which machines a mod belongs on inconsistently, so an untagged mod that would have fit is filtered out. Otherwise try different words; this registry matches on names and descriptions, not on what a mod does.
