---
id: GameGuideSettingsInFiles
type: prompt
annotations:
  sent_when: >-
    the opening section of every game_guide result for a real catalog game whose
    entry declares no per-instance settings overlay
  used_by:
    - file: discord/gary/guide.rs
      function: render_guide
  variables:
    game:
      source: the catalog id the guide was asked for
      contents: the game id, quoted inline
  reasoning:
    - >-
      The counterpart to GameGuideSettingsOverlay, and the reason the guide can
      be described as covering configuration rather than only mods: every game
      answers the settings question, so a model that calls game_guide before
      changing a setting is never told nothing and left to guess which branch it
      is in.
    - >-
      Confirms the ordinary explore-the-data-directory approach rather than
      staying silent about it. Silence would read as "this game has no settings"
      next to the other branch's explicit file, which is the opposite of true.
    - >-
      Deliberately does not name any file. Which file holds which setting is the
      one thing that genuinely differs across every game here, and a guess
      written into a prompt is worse than the search it would replace — it
      sounds authoritative and sends the search to the wrong place.
---
{{game}}'s settings live in the game's own config files on this server, so the ordinary approach is the right one: browse_files from the top of the data directory to find the file that holds the setting, read_file to see it, edit_file to change just that piece, then restart to apply it. Nothing is hidden in the environment on this game, so what you find in those files is the whole picture.
