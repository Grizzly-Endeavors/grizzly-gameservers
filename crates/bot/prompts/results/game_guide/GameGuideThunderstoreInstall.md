---
id: GameGuideThunderstoreInstall
type: prompt
annotations:
  sent_when: >-
    the second half of the game_guide result for a game whose mods come from a
    Thunderstore community
  used_by:
    - file: discord/gary/guide.rs
      function: render_guide
  reasoning:
    - >-
      Names the one file an install touches and its exact grammar. Without it
      Gary would go looking for a plugins directory — which exists, on the
      image, and writing into it by hand would be undone by the next launch
      while appearing to have worked.
    - >-
      "Delete the line to remove it" is the opposite of the Minecraft rule and
      has to be said out loud for that reason. The installed set here is rebuilt
      from this file on every start, so an absent line means absent files; on
      Minecraft an absent key means no reconciliation at all. Anyone carrying
      one habit to the other game gets it exactly backwards.
    - >-
      Pinning is insisted on because the version is what makes a restart
      reproducible and offline-recoverable: a pinned list resolves from the
      instance's own cache with no registry call, so a pod recreated during a
      registry outage comes back with the mods it had.
    - >-
      Names update_mods immediately after the pinning rule, because insisting on
      pins is what creates the update problem: a pinned list goes stale and
      somebody has to move it. Said here rather than left to the tool's own
      description because the wrong habit — a mod_info call per mod — is the one
      a model falls into naturally, and it is both expensive and the place a
      wrong version gets typed.
    - >-
      Says not to list dependencies or the loader pack, because listing them is
      the natural thing to do after mod_info reports them — and the loader pack
      in particular is refused, since the image already supplies it.
    - >-
      Points at the two artifacts that make an install checkable after the fact
      (the log lines and the resolved file) so "did it work" is answered by
      reading rather than by assuming.
    - >-
      States that removing a mod removes its settings file too, which is what
      makes the settings directory readable as the current install set. Without
      it the model has no reason to trust that a settings file it finds belongs
      to a mod that is running, and the habit that follows — reading a stale file
      and editing it — produces a change that appears to land and does nothing.
    - >-
      The backup-first rule and the minute-or-so expectation exist for the same
      reasons they do on Minecraft: a mod loader touching a world with state is
      the one change a manager cannot undo alone, and a first install that
      fetches packages before the world loads should read as normal rather than
      as a hang.
---
Installing is a config change to a single file: .grizzly/mods, at the top of the server's data directory. It is always there, often empty. One mod per line, written as Namespace-Name-1.2.3 — pinned to exactly the version mod_info reported, because a pinned list reinstalls identically every restart and can be rebuilt from the server's own copy even when the registry is unreachable. Blank lines and lines starting with # are ignored. Read the file first, then change it like any other config file.

Updating every mod is one call, not one per mod: update_mods with mode `check` reports every pinned version that has moved and writes nothing, and mode `apply` rewrites the list. Reach for it for both "are my mods up to date" and "update the mods" — never walk the list with mod_info, which costs a lookup per mod and is where a version gets typed wrong.

List only the mods someone actually asked for. Anything a mod depends on is installed automatically, and the mod loader itself is already part of the image — listing it is refused. To remove a mod, delete its line: the installed set is rebuilt from this file at every start, so a line that isn't there is a mod that isn't installed. That is the opposite of how Minecraft's list works, so don't carry the habit across.

Restart to apply it, and that restart self-guards like any other config change: a line naming a mod that doesn't exist, or a malformed line, stops the server, and the previous version of the file is restored and the server restarted automatically. Afterwards, read_logs shows lines beginning with [mods] naming exactly what was installed, and .grizzly/thunderstore/resolved lists the same thing as plain text — check one of them rather than assuming it worked. Each mod's own settings file appears under the data directory after its first boot and is edited like any other config file; your edits there are never overwritten while the mod is installed, and when you remove a mod its settings file is deleted with it on the next start that comes up cleanly — so a settings file you can still find belongs to a mod that is still installed. Before the first install on a world people have already played on, take a backup_server — putting a world back afterwards is something only an admin can do. Tell them up front that a first install takes a minute or so: every mod is fetched before the world even loads.
