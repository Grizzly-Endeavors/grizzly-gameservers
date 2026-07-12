---
id: EditFile
type: tool
tool_schema:
  name:
    type: string
    description: Exact server name, as shown by `list_servers`.
  path:
    type: string
    description: >-
      Path within the server's data directory to edit, e.g. `server.properties`.
      The previous version is saved first so `restore_file` can undo the change.
  old_text:
    type: string
    description: >-
      The exact text to find and replace. Must appear once in the file — copy it
      verbatim, whitespace included, and include enough surrounding text to be
      unique. If it's missing or appears more than once, the edit is refused and
      nothing changes.
  new_text:
    type: string
    description: The text to put in its place.
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: manager_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      The preferred surgical change: the body steers the model here over
      write_file so a targeted edit can't clobber other settings, and spells out
      the exact-match-once contract that keeps the edit safe or refused.
    - >-
      old_text/new_text are the find-and-replace pair; the description insists on
      verbatim, unique old_text because an ambiguous anchor is refused rather
      than guessed.
---
Change one setting in a config file in place: find old_text and replace it with new_text, leaving the rest of the file untouched. Prefer this over write_file for a targeted change — you don't rewrite the whole file, so you can't accidentally clobber other settings. old_text must match exactly once; if it's missing or ambiguous the edit is refused and nothing changes. Saves the previous version first (restore_file undoes it). Takes effect on the next restart.
