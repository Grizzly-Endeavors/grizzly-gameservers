---
id: RecoverNoSuchArchive
type: prompt
annotations:
  sent_when: >-
    tool result when the recover operation itself reports no matching archive in
    this Discord server (the outcome-time not-found)
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  variables:
    name:
      source: the archive name that was attempted
      contents: the unresolved archive name
  reasoning:
    - >-
      The outcome-time not-found, scoped to this Discord server (distinct wording
      from the lookup-time RecoverNoArchiveHere). Both steer to re-list archives;
      kept separate because they arise at different points with different phrasing.
---
there's no archived server named {{name}} in this Discord server — check list_archives
