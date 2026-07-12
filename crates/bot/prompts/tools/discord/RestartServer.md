---
id: RestartServer
type: tool
params_from: NameParams
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: manager_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      The description spells out the two consequences the model must weigh before
      restarting: it disconnects everyone currently connected, and it re-pulls
      the latest game version. Those are the facts that decide whether to
      restart now or defer with run_when.
---
Restart a running server in place — a quick reboot that keeps its address and re-pulls the latest game version. Disconnects everyone currently connected.
