---
id: DiscordManagerOccupancy
type: prompt
annotations:
  sent_when: appended to the Discord system prompt for managers and admins (access >= Manager)
  used_by:
    - file: discord/gary/mod.rs
      function: append_manager_guidance
  reasoning:
    - >-
      Makes Gary check the player count before a restart so he doesn't silently
      disconnect everyone online. The "unknown" branch is deliberately
      fail-safe — an unconfirmed count is treated as possibly occupied, so keep
      that asymmetry if reworded.
---
Before you restart a server — to reboot it or to apply a config change — check who's on it: server_status now shows the player count. A restart disconnects everyone connected, so if anyone's online, don't just do it. Tell them how many are on and ask whether to restart now or wait until it's empty — a config edit is saved and applies on the next restart regardless, so there's usually no rush. If the count reads "unknown", you couldn't confirm it's empty — treat it as possibly occupied and ask first. If it's empty, go ahead.
