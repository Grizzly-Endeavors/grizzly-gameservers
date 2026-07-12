---
id: RecoverStopped
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server recreates an archived server but it stays
    paused and won't come up on its own
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  variables:
    name:
      source: the recovered server's name
      contents: the server name
    address:
      source: the recovered server's advertised address
      contents: the host:port players will connect to once started
  reasoning:
    - >-
      Distinguishes a paused-after-recover server from a crash: recovery worked,
      it just needs an explicit start, so the model offers to start it.
---
recovered {{name}} at {{address}}, but it's paused and won't come up on its own — start it when you're ready
