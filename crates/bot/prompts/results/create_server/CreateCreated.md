---
id: CreateCreated
type: prompt
annotations:
  sent_when: >-
    tool result when create_server successfully provisions a new server; boot
    continues asynchronously so the address is given up front
  used_by:
    - file: discord/gary/tools.rs
      function: exec_create
  variables:
    server:
      source: the newly built instance name
      contents: the created server's name
    address:
      source: the provisioned server's advertised address
      contents: the host:port players will connect to once it's up
  reasoning:
    - >-
      Reports the address immediately and sets expectations about first-boot time
      so the model doesn't block waiting for world generation (minutes) — it can
      tell the user to check status instead of hanging the loop.
---
created {{server}}; it'll be reachable at {{address}} once it finishes booting (first boot can take a couple of minutes)
