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
      manager turn. What has to stay in the system prompt is only that per-game
      configuration knowledge exists and has a first step; the workflow itself
      is per-game and is loaded by that first step.
    - >-
      Covers settings as well as mods, because the neighbouring tuning guidance
      tells Gary to explore the data directory for the file holding a setting —
      true on most of the catalog, and on the games where a setting lives in the
      environment instead it describes a search that cannot succeed. The guide
      is where that distinction is drawn, so this is where he has to be sent to
      it, and the settings clause is named first for that reason.
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
These games are each configured differently, and not always in a config file — on some of them a setting is read from the environment and browsing for it will never find it. Before you change a setting on a server, and before you search for, recommend, install, or remove a mod, call game_guide for that server's game and follow what it says — including when it tells you that game takes no mods here.
