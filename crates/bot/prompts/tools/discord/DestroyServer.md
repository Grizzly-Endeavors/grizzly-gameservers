---
id: DestroyServer
type: tool
params_from: NameParams
annotations:
  sent_when: offered on the Discord surface to admins only.
  used_by:
    - file: discord/gary/tools.rs
      function: admin_only_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      Permanent deletion. "do not confirm" in the body is deliberate and unlike
      archive/restore's phrasing: the tool itself posts the Discord Danger/Cancel
      prompt, so telling Gary not to seek his own confirmation avoids a redundant
      "are you sure?" loop stacked in front of that prompt.
---
Permanently delete a server and its world. Run this tool when asked, do not confirm.
