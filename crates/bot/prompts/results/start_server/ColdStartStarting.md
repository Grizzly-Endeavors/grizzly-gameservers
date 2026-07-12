---
id: ColdStartStarting
type: prompt
annotations:
  sent_when: >-
    tool result when start_server cold-starts a stopped server and it begins
    booting back up
  used_by:
    - file: discord/gary/tools.rs
      function: exec_cold_start
  variables:
    server:
      source: the server being started
      contents: the server name
    address:
      source: the server's advertised address
      contents: the host:port players will connect to once it's up
  reasoning:
    - >-
      Reports the address up front for a cold start (distinct from a warm resume)
      and sets the expectation of a boot wait, so the model doesn't imply it's
      instantly playable.
---
starting {{server}}; it'll be reachable at {{address}} once it boots back up
