---
id: FileBackupSaved
type: prompt
annotations:
  sent_when: >-
    spliced into a write_file or edit_file result (via the saved slot) when a
    snapshot of the previous version was saved before the change
  used_by:
    - file: discord/gary/tools.rs
      function: format_write
    - file: discord/gary/tools.rs
      function: format_edit
  reasoning:
    - >-
      Tells the model the change is reversible via restore_file, so Gary can
      offer to undo. Shared between write and edit so the reversibility promise
      reads identically for both.
---
saved the previous version first, so restore_file can undo this
