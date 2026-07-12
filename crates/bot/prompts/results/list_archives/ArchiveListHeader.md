---
id: ArchiveListHeader
type: prompt
annotations:
  sent_when: >-
    tool result heading a list_archives listing when this Discord server has one
    or more archived servers
  used_by:
    - file: discord/gary/tools.rs
      function: format_archive_list
  variables:
    lines:
      source: the archive entries, each formatted and joined by newlines in format_archive_list
      contents: >-
        one line per archive — "<name> (<size>, <created_at>)" — joined by
        newlines; the per-entry formatting is data and stays in code
  reasoning:
    - >-
      The header is prompt text, scoped to this Discord server; the archive
      entries are data joined in code and enter through the variable.
---
archived servers in this Discord server:
{{lines}}
