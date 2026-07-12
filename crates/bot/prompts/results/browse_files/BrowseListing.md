---
id: BrowseListing
type: prompt
annotations:
  sent_when: >-
    tool result when browse_files lists a directory that has entries
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
    listing:
      source: the directory entries, each formatted and joined by newlines in format_entries
      contents: >-
        one line per entry — "<name>/ (folder)", "<name> (<n> bytes)", or
        "<name> (other)" — joined by newlines; the per-entry formatting is data
        and stays in code
  reasoning:
    - >-
      The "contains:" header is prompt text; the entry lines are data joined in
      code and enter through the variable. Keeping the header separate lets the
      listing format evolve without touching prose.
---
{{location}} contains:
{{listing}}
