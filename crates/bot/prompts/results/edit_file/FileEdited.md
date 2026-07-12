---
id: FileEdited
type: prompt
annotations:
  sent_when: >-
    tool result when edit_file successfully applies a text replacement
  used_by:
    - file: discord/gary/tools.rs
      function: format_edit
  variables:
    path:
      source: the file path that was edited
      contents: the edited file path
    saved:
      source: >-
        one of FileBackupSaved / FileNoBackup, selected in code by whether a
        snapshot was taken, rendered and spliced in
      contents: the reversibility clause — either the undo-available or new-file wording
  reasoning:
    - >-
      Mirrors FileWritten's verify-after-change steer for edits (the verb differs
      deliberately) so the model confirms health after applying a change. Shares
      the saved reversibility clause with write_file.
---
edited {{path}} ({{saved}}); restart the server and read the logs to confirm it comes back healthy
