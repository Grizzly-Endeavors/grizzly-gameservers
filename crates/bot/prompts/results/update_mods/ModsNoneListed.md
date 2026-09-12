---
id: ModsNoneListed
type: prompt
annotations:
  sent_when: >-
    tool result after update_mods found the server's mod list absent or empty
  used_by:
    - file: discord/gary/mod_list.rs
      function: exec_update_mods
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the server name, quoted inline
  reasoning:
    - >-
      A final answer, not a gap: an empty mod list is the ordinary state of a
      server nobody has modded, and treating it as a missing file would send the
      model browsing for one that is exactly where it should be.
---
{{server}} has no mods listed, so there is nothing to update.
