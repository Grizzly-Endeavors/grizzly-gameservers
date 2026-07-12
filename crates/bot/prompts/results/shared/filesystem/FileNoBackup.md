---
id: FileNoBackup
type: prompt
annotations:
  sent_when: >-
    spliced into a write_file or edit_file result (via the saved slot) when the
    file is new, so there was no previous version to snapshot
  used_by:
    - file: discord/gary/tools.rs
      function: format_write
    - file: discord/gary/tools.rs
      function: format_edit
  reasoning:
    - >-
      The counterpart to FileBackupSaved: sets the model's expectation that
      there's nothing to restore to, so Gary doesn't offer an undo that can't
      work. Shared between write and edit.
---
this is a new file, so there's nothing to restore it to
