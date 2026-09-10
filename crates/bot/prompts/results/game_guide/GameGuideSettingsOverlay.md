---
id: GameGuideSettingsOverlay
type: prompt
annotations:
  sent_when: >-
    the opening section of every game_guide result for a game whose catalog
    entry declares a per-instance settings overlay
  used_by:
    - file: discord/gary/guide.rs
      function: render_guide
  variables:
    game:
      source: the catalog id the guide was asked for
      contents: the game id, quoted inline
    file:
      source: >-
        the overlay path the game's GameServer template declares in
        SUPERVISOR_CHILD_ENV_FILE, read at catalog load
      contents: >-
        a data-root-relative path, e.g. ".grizzly/env"; the same string
        read_file and write_file take
    keys:
      source: >-
        the allowlist the same template declares in
        SUPERVISOR_CHILD_ENV_ALLOWLIST, rendered as an "A, B and C" sentence
      contents: >-
        the setting names, in the order the catalog lists them; never empty,
        since a game with no keys declares no overlay and gets
        GameGuideSettingsInFiles instead
  reasoning:
    - >-
      This section exists because on at least one game the settings it names
      exist nowhere else. Valheim's join password is an environment variable
      read at launch and never written to disk, so the standing "browse the data
      directory to find the file that holds a setting" instruction sends Gary
      looking for a file that cannot be found. Three consecutive turns burned
      their whole round budget on exactly that search before this section
      existed.
    - >-
      Says the overlay is the only home for these settings, rather than merely
      offering it as one option. Without that, a model that has just been told
      to explore for config files will explore anyway and treat the overlay as a
      fallback it never reaches.
    - >-
      The key list arrives as a variable rather than as prose so it cannot drift
      from the catalog. Widening the allowlist is a manifest edit; if the names
      were written here it would silently become a two-file edit, and the half
      nobody remembers is this one.
    - >-
      Names the rejected-value case as safe rather than listing each key's
      constraints. The per-game rules (minimum lengths, values a game refuses)
      are upstream's and change without notice, so encoding them here would rot;
      the self-guarding restart already catches a bad value and puts the old one
      back. Saying so is what makes trying the change the right move instead of
      a risk to hedge against.
    - >-
      Warns off the neighbouring keys explicitly. The file is plain dotenv and
      nothing stops a line being written for a key the allowlist doesn't carry —
      the supervisor just drops it, so the change appears to have been made and
      silently does nothing.
---
Some of {{game}}'s settings are not stored in any config file on this server — they are read from the environment when the game starts, and the only place an instance can set them is {{file}}, at the top of the data directory. It is always there, often empty. Don't go looking for a config file for these: there isn't one, and searching for it is time spent on something that cannot be found.

Exactly these settings do anything in that file, and the rest are ignored: {{keys}}. A line naming anything else is dropped when the game launches, so the change will look like it worked and won't have.

Read the file first to see what's already set, then change it like any other config file — one setting at a time — and restart to apply it. That restart self-guards: if the game refuses a value and won't start, the previous version of the file is restored and the server restarted automatically, and you'll be told. So a value you're unsure of is worth trying rather than talking around; afterwards, read_logs to confirm it took rather than assuming.
