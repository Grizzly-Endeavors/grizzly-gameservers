---
id: ThunderstoreSearchHeader
type: prompt
annotations:
  sent_when: >-
    tool result heading a search_mods listing for a Thunderstore game when the
    community returned one or more matches
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
    results:
      source: >-
        the matching packages, each rendered by ThunderstoreSummaryLine and
        joined by newlines in format_thunderstore_search
      contents: >-
        one ThunderstoreSummaryLine per match, newline-joined; the joining is
        data and stays in code
  reasoning:
    - >-
      Labels the payload as registry search results so the model reads the
      third-party names and descriptions as data to relay, not as instructions
      to follow.
    - >-
      Names the game, because the same query against a different community
      returns entirely different packages and a listing that didn't say which
      one it came from could be relayed as an answer about the wrong server.
    - >-
      Names the cap out loud because the listing is truncated to five: without
      it a model that saw no obvious match would conclude the thing doesn't
      exist rather than narrowing the query. Deprecated packages are dropped
      before this point, so "no match" here never means "only abandoned ones".
    - >-
      Sends the model to mod_info before acting, since a search line carries no
      version and an install entry has to be pinned to one.
---
top matches for {{query}} in the {{game}} mod registry (best five only — narrow the search if none fit):
{{results}}
Look one up with mod_info before recommending or installing it: that is where the version to pin and the list of what else it needs come from.
