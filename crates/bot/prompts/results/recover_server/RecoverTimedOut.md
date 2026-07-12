---
id: RecoverTimedOut
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server recreates an archived server but it hasn't
    finished booting within the wait window
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  variables:
    name:
      source: the recovered server's name
      contents: the server name
    address:
      source: the recovered server's advertised address
      contents: the host:port players will reach once it boots
  reasoning:
    - >-
      Reports the recovery took and boot is just slow, giving the address up
      front so the model can share where to connect once it's up.
---
recovering {{name}}; it'll be reachable at {{address}} once it finishes booting
