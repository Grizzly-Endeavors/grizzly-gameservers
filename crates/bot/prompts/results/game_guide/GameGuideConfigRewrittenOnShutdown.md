---
id: GameGuideConfigRewrittenOnShutdown
type: prompt
annotations:
  sent_when: >-
    in a game_guide result, right after the settings section, for a game whose
    catalog entry declares a config file it rewrites at shutdown
  used_by:
    - file: discord/gary/guide.rs
      function: render_guide
  variables:
    file:
      source: the config-rewritten-on-shutdown annotation on the game's catalog template
      contents: the file's path relative to the data root
  reasoning:
    - >-
      Some games write their config file from in-memory settings when they shut
      down, so an edit made while running is overwritten by the very restart
      that was meant to apply it. Only a stopped game keeps the edit.
---
This game writes {{file}} from its running settings when it shuts down, so an edit made while it runs is overwritten by the next restart. To change it: stop_server, then edit the file, then start_server, then read the file back to confirm the change is still there.
