---
id: ArchiveListEmpty
type: prompt
annotations:
  sent_when: >-
    tool result when list_archives finds no archived servers in this Discord
    server
  used_by:
    - file: discord/gary/tools.rs
      function: format_archive_list
  reasoning:
    - >-
      Scopes the empty result to this Discord server (archives are per-guild) so
      the model doesn't imply there are none anywhere, and reports nothing to
      recover here.
---
no servers are archived in this Discord server
