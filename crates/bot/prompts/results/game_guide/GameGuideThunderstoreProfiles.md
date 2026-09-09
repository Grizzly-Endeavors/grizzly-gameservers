---
id: GameGuideThunderstoreProfiles
type: prompt
annotations:
  sent_when: >-
    the third and last part of the game_guide result for a game whose mods come
    from a Thunderstore community
  used_by:
    - file: discord/gary/guide.rs
      function: render_guide
  reasoning:
    - >-
      A separate block rather than an edit to the install one, because it is a
      different job: the install block is how to change one mod, this is how a
      whole modpack moves in or out in a single step. Groups already agree packs
      by code, so a code is the fast path in both directions and going mod by
      mod when one exists is slower and more error-prone for everybody.
    - >-
      States that the mod list is the *group's* pack and not just this server's
      installs, since that is what makes the client: form make sense at all —
      without it, a line naming something the server never installs reads as a
      mistake to clean up.
    - >-
      Spells out that a client: entry must carry a version. Nothing downstream
      resolves one, so an unpinned entry is silently skipped rather than
      failing, which is the one place in this file where a mistake does not
      announce itself.
    - >-
      Insists a code is a snapshot rather than a standing address, and says it
      out loud at the moment of handing one over. A code cannot be revoked, and
      the drift it is meant to prevent is exactly what a stale one causes — a
      player confident they are in sync while their versions have moved apart.
    - >-
      Warns that importing replaces the list, because the word "import" reads
      as "add" to most people and the mods that quietly disappear would be
      noticed only by whoever was using them.
---
A whole modpack moves in one step through an r2modman profile code — a UUID people copy out of r2modman or Thunderstore Mod Manager. This is how modded groups already agree on a pack, so reach for it before going mod by mod. Someone who hands you a code wants import_profile; someone asking how to set their own game up to join wants export_profile, and that one is worth offering unprompted, because the usual reason a modded server "doesn't work" for one player is that their versions have drifted from the server's.

Importing **replaces** the mod list rather than adding to it — read the list first if they meant to add. A code is a *player's* profile, so it lists things the server has no business installing; the import sorts that out and tells you what it left out and why.

That is also why the mod list is the group's whole pack rather than just this server's installs. A line prefixed `client:` — `client:RandyKnapp-EquipmentAndQuickSlots-3.0.2` — names a mod that belongs on the players' machines only: the server installs nothing for it, but it travels with the pack and appears in any code you export. A `client:` line must always carry a version, since nothing works one out for it. Remove either kind by deleting its line.

A code you hand out describes the pack at that moment and cannot be taken back. Say what is in it when you give it out, and mint a fresh one after the mods change rather than pointing anyone at an old one.
