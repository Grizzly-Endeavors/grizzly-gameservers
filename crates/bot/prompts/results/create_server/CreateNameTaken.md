---
id: CreateNameTaken
type: prompt
annotations:
  sent_when: >-
    tool result when create_server picks a name that already belongs to a server
    in the asking guild; a name held by a different guild returns
    CreateNameUnavailable instead, which does not confirm that a server holds it
  used_by:
    - file: discord/gary/tools.rs
      function: refusal_reply
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
