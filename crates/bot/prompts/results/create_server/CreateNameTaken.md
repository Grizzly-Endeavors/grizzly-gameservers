---
id: CreateNameTaken
type: prompt
annotations:
  sent_when: >-
    tool result when create_server picks a name that already belongs to an
    existing server
  used_by:
    - file: discord/gary/tools.rs
      function: exec_create
  variables:
    server:
      source: the instance name that collided
      contents: the already-taken server name
  reasoning:
    - >-
      Tells the model the name is taken so it either picks a different one or
      checks the existing server, rather than assuming creation succeeded.
---
a server named {{server}} already exists
