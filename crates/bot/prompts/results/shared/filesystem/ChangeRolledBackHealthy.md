---
id: ChangeRolledBackHealthy
type: prompt
annotations:
  sent_when: >-
    tool result when a config change crashed the server on restart and the
    automatic rollback restored the previous version and brought it back healthy
  used_by:
    - file: discord/gary/tools.rs
      function: verify_change
  variables:
    path:
      source: the edited config file path
      contents: the file whose change was rolled back
    server:
      source: the server that was restarted
      contents: the server name
  reasoning:
    - >-
      Reports the full deterministic-recovery story — crash, rollback, healthy —
      so Gary relays that the bad change was undone automatically and the server
      is fine, without the model having to reason its way back to restore_file.
---
the change to {{path}} crashed {{server}} on restart, so I rolled it back to the previous version and restarted — it's healthy again now
