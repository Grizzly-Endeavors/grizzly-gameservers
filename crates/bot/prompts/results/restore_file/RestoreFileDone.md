---
id: RestoreFileDone
type: prompt
annotations:
  sent_when: >-
    tool result when restore_file rolls a config file back to its previous
    snapshot
  used_by:
    - file: discord/gary/tools.rs
      function: format_restore
  variables:
    path:
      source: the file path that was restored
      contents: the restored file path
  reasoning:
    - >-
      Closes with the apply step (restart the server) so the model knows the
      rollback isn't live until a restart — distinct from a server-level restore,
      this is the config-snapshot undo.
---
restored {{path}} to its previous version; restart the server to apply it
