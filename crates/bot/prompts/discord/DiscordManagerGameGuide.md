---
id: DiscordManagerGameGuide
type: prompt
annotations:
  sent_when: appended to the Discord system prompt for managers and admins (access >= Manager)
  used_by:
    - file: discord/gary/mod.rs
      function: append_manager_guidance
  reasoning:
    - >-
      Replaces the per-game mod workflows that used to be appended to every
      manager turn. What has to stay in the system prompt is only that mod
      management exists and has a first step; the workflow itself is per-game
      and is loaded by that first step.
    - >-
      Kept to one instruction on purpose. This block is paid for on every
      manager message in every guild, so anything that could live behind
      game_guide should.
    - >-
      Names the four verbs (search, recommend, install, remove) rather than
      saying "before mod work", because recommending is the one that reads as
      conversation rather than as an action — and a recommendation made without
      the guide is how a both-sided mod gets suggested without the warning that
      every player has to install it themselves.
---
Some of these games take mods, and each one handles them completely differently. Before you search for, recommend, install, or remove a mod on a server, call game_guide for that server's game and follow what it says — including when it tells you that game takes no mods here.
