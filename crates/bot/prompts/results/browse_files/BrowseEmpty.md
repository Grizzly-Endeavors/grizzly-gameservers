---
id: BrowseEmpty
type: prompt
annotations:
  sent_when: >-
    tool result when browse_files lists a directory that has no entries
  used_by:
    - file: discord/gary/tools.rs
      function: format_entries
  variables:
    location:
      source: the browsed path, or a fallback label when the path is empty
      contents: >-
        the directory path, or the literal "the data directory" when the path is
        empty (the server's data root) — the fallback is applied in code before
        rendering
  reasoning:
    - >-
      Names the empty location so Gary reports "nothing here" against the right
      place rather than a bare "empty"; the location value (including its
      empty-path fallback) is data entering through the variable.
---
{{location}} is empty
