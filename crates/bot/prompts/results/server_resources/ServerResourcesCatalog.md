---
id: ServerResourcesCatalog
type: prompt
annotations:
  sent_when: >-
    spliced into ServerResourcesReport's closing line when the instance carries
    no resource override, so its figures are the game's own
  used_by:
    - file: discord/gary/tools.rs
      function: exec_server_resources
  reasoning:
    - >-
      Says the figures were nobody's decision about this server in particular,
      which is what tells the model a change here would be the first one rather
      than an adjustment to someone else's.
    - >-
      Deliberately says nothing more. This is the ordinary case, and repeating
      the same numbers back as "the defaults" would pad every report to make one
      sentence land in the rarer one.
---
the defaults for this game
