---
id: UpdateMods
type: tool
tool_schema:
  name:
    type: string
    description: Exact server name, as shown by `list_servers`.
  mode:
    type: enum
    description: >-
      `check` looks up every mod and reports what an update would move, writing
      nothing. `apply` rewrites the mod list onto the new versions. Use `check`
      when they asked whether anything is out of date, `apply` when they asked
      for the update — and prefer `check` first whenever the ask could be either,
      since the report names every version change and they can then say go.
    values:
      - check
      - apply
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch_mod_list
  reasoning:
    - >-
      Exists because the alternative is a mod_info call per mod. A twenty-mod
      list makes "are my mods up to date" a twenty-round turn that costs twenty
      times as much and gets a version wrong somewhere in the middle; this is one
      call whose answer is a table.
    - >-
      The mode is a required argument rather than an optional flag, because "any
      updates?" and "update them" are the same sentence in most phrasings and an
      omitted flag would resolve that ambiguity by writing. Naming both values in
      the description is what lets the model pick deliberately.
    - >-
      Says only pinned versions move. An unpinned line already resolves to the
      latest at every launch, and a model that thought this tool covered it would
      report a list as updated when one entry was never part of the operation.
    - >-
      Points at the restart, because the rewritten list changes nothing until one
      happens — the same thing every mod-list change has to say, and the reason a
      friend told "updated" would otherwise go looking for a new feature that
      isn't loaded yet.
---
Bump every pinned mod in a server's mod list to the newest version its registry reports, in one call. This is the tool for "are my mods up to date" and "update the mods" — do **not** check them one at a time with mod_info, which costs a lookup per mod and is where a wrong version gets typed. Only pinned entries move: a line with no version already installs the latest at every start, and is reported separately rather than pinned. A version is only ever raised, never lowered. Mods listed for the players' machines are bumped too and called out on their own line — those versions have to match what players run, so every player has to update in their own mod manager before they can connect. The report also names anything now deprecated by its author and anything the registry no longer has. With `apply`, restart the server afterwards to load the new versions; like any config change, a list that stops the server is rolled back automatically.
