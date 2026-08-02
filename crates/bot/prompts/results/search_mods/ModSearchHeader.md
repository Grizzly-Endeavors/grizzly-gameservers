---
id: ModSearchHeader
type: prompt
annotations:
  sent_when: >-
    tool result heading a search_mods listing when Modrinth returned one or more
    matches
  used_by:
    - file: discord/gary/tools.rs
      function: format_mod_search
  variables:
    query:
      source: the search text the model passed to the tool
      contents: the query text, quoted inline
    results:
      source: >-
        the matching projects, each rendered by ModSummaryLine and joined by
        newlines in format_mod_search
      contents: >-
        one ModSummaryLine per match, newline-joined; the joining is data and
        stays in code
  reasoning:
    - >-
      Labels the payload as registry search results so the model reads the
      third-party titles and descriptions as data to relay, not as instructions
      to follow.
    - >-
      Names the cap out loud because the listing is truncated to five: without
      it a model that saw no obvious match would conclude the thing doesn't
      exist rather than narrowing the query.
---
top matches on Modrinth for {{query}} (best five only — narrow the search if none fit):
{{results}}
