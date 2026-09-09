---
id: FileLineCut
type: prompt
annotations:
  sent_when: >-
    appended to a read_file result when at least one line in the window was
    longer than the per-line cap and was cut short
  used_by:
    - file: discord/gary/tools.rs
      function: format_file
  reasoning:
    - >-
      A cut line is marked inline where it happens, but the mark sits at the end
      of a very long line the model may not read to. Saying it once at the top
      keeps a truncated value from being read as the real one — which for a
      config file would mean editing against text that isn't there.
---
(at least one line was too long to show in full and was cut short; grep the file with run_shell to see such a line whole)
