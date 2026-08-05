---
id: CreateNameUnavailable
type: prompt
annotations:
  sent_when: >-
    tool result when create_server picks a name that is held outside the asking
    guild, so the name is refused without confirming that a server holds it
  used_by:
    - file: discord/gary/tools.rs
      function: refusal_reply
  variables:
    server:
      source: the instance name that was refused
      contents: the unavailable server name
  reasoning:
    - >-
      Server names are unique across every Discord server on the platform, so a
      name can be refused because another guild holds it. Saying only that the
      name is unavailable keeps the model from reporting, or inferring, that a
      server exists there — a name-by-name confirmation would be a way to
      enumerate another guild's servers.
    - >-
      Names the next move (pick another name) so the model retries instead of
      reading this as a fault to investigate or escalate.
---
the name {{server}} is not available — choose a different one
