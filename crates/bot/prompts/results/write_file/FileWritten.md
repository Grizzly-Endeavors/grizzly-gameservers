---
id: FileWritten
type: prompt
annotations:
  sent_when: >-
    tool result when write_file successfully writes a file
  used_by:
    - file: discord/gary/tools.rs
      function: format_write
  variables:
    path:
      source: the file path that was written
      contents: the written file path
    saved:
      source: >-
        one of FileBackupSaved / FileNoBackup, selected in code by whether a
        snapshot was taken, rendered and spliced in
      contents: the reversibility clause — either the undo-available or new-file wording
  reasoning:
    - >-
      Closes with the verify step (restart and read the logs) so the model
      confirms the change is healthy rather than assuming a successful write is
      the end. The reversibility clause composes in through the saved variable.
---
wrote {{path}} ({{saved}}); restart the server and read the logs to confirm it comes back healthy
