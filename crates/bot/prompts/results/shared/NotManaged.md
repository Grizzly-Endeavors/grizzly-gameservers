---
id: NotManaged
type: prompt
annotations:
  sent_when: >-
    tool result when a lifecycle/file/backup tool targets a server that exists
    but is platform-managed (not one Gary may control) — the refusal the model
    relays
  used_by:
    - file: discord/gary/tools.rs
      function: not_managed
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the platform-managed server's name, quoted inline
  reasoning:
    - >-
      Draws the blast-radius boundary for the model: a platform-managed server is
      off-limits from Gary's surface, so the result states it plainly rather than
      letting Gary keep trying operations that will keep being refused.
---
{{server}} is managed by the platform and can't be controlled from here
