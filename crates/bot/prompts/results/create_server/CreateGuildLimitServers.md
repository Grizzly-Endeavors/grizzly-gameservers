---
id: CreateGuildLimitServers
type: prompt
annotations:
  sent_when: >-
    tool result when create_server is refused because the community already has
    its maximum number of servers (running, paused, and stopped all count)
  used_by:
    - file: discord/gary/tools.rs
      function: exec_create
  variables:
    current:
      source: the guild's existing instance count, measured live off the cluster
      contents: a whole number
    cap:
      source: the guild's effective server-count cap (override or global default)
      contents: a whole number
  reasoning:
    - >-
      Names both figures so the model can explain the bound instead of retrying.
      Stopped servers count too (each holds a port and disk), so the way out is
      destroy or archive — not stop — plus the operator escalation path, which
      is the honest answer when the community has genuinely outgrown its cap.
---
nothing was created — this community is at its server limit ({{current}} of {{cap}} in use, and stopped servers count because they keep their port and world storage). destroy or archive a server first, or suggest asking the operator to raise the limit
