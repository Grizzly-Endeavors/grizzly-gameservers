---
id: ReadyCrashed
type: prompt
annotations:
  sent_when: >-
    tool result when a start/restart readiness wait ends with the server
    crashing while coming up
  used_by:
    - file: discord/gary/tools.rs
      function: format_ready_wait
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      Points the model at the next diagnostic step (read logs) and the likely
      culprit (a recent change, via restore_file), so Gary investigates rather
      than just reporting a crash.
---
{{server}} crashed while coming up — read its logs to see why, and restore_file if a recent change is at fault
