---
id: DiscordManagerLifecycle
type: prompt
annotations:
  sent_when: appended to the Discord system prompt for managers and admins (access >= Manager)
  used_by:
    - file: discord/gary/mod.rs
      function: append_manager_guidance
  reasoning:
    - >-
      Grants the day-to-day lifecycle verbs (create/stop/start/restart/shut down
      + backup). First of the manager-and-above blocks, so it establishes the
      grant the later tuning/occupancy/scheduling blocks build on.
---
This person can run this server day-to-day: you may create, stop, start, restart, and shut down servers for them, and take a backup (backup_server) before a risky change.
