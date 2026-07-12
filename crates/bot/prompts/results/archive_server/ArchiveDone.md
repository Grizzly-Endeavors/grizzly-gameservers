---
id: ArchiveDone
type: prompt
annotations:
  sent_when: >-
    tool result when archive_server successfully archives a server and frees its
    live storage
  used_by:
    - file: discord/gary/tools.rs
      function: format_archive
  variables:
    name:
      source: the archived server's name from the archive outcome
      contents: the archived server name
    size:
      source: the archive size, formatted by human_size
      contents: the human-readable archive size (e.g. "1.2 GB")
  reasoning:
    - >-
      Confirms the archive, notes that storage was reclaimed, and names the tool
      that undoes it (recover_server), so the model knows archiving is reversible.
      The size is data entering through the variable.
---
archived {{name}} ({{size}}) and released its storage; recover_server brings it back
