---
id: GameGuideThunderstoreChoice
type: prompt
annotations:
  sent_when: >-
    the first half of the game_guide result for a game whose mods come from a
    Thunderstore community
  used_by:
    - file: discord/gary/guide.rs
      function: render_guide
  reasoning:
    - >-
      Teaches the two-step discovery this registry needs, which is not optional
      the way it looks: a search result here carries no version at all, and an
      install entry has to be pinned to one — so mod_info is a required step
      rather than a confirmation.
    - >-
      Spells out the Namespace-Name shape because the most likely mistake is a
      single-word slug carried over from Minecraft habits. It looks like a real
      identifier, so it fails as "no such mod" rather than as "wrong shape",
      and the correction is reshaping rather than searching again.
    - >-
      The four side cases are stated explicitly because this registry has no
      server/client flag — the communities express it with categories that
      authors apply inconsistently, so "the registry didn't say" is a common and
      genuinely different answer from "server only". Guessing on an untagged mod
      is what leaves every player unable to connect.
    - >-
      The both-sided warning is the single most consequential thing to get right
      for a non-technical audience: nothing done on the server installs anything
      on a player's own machine, and the moment to say so is before the install,
      not after the server comes back up and nobody can join.
---
Never name a mod from memory. Mods here are identified by a Namespace-Name pair — `ValheimModding-Jotunn`, not `jotunn` and not a display name — and a name that doesn't exist stops the server from starting. Use search_mods with this game to find candidates, then mod_info on the exact pair to get the version to install, what else it needs, and where it has to be installed. A search result deliberately carries no version, so mod_info is a required step and not a double-check.

Where a mod has to be installed is the thing to get right before anything else. The registry reports one of four answers: the server only, which costs the players nothing; the server *and* every player's machine, which means every one of them has to install that file and the mod loader themselves and nothing you do on the server does it for them; players' machines only, which means there is nothing to install server-side at all and you should say so rather than adding it; or not stated, which happens often — libraries in particular are usually untagged — and means you should point them at the mod page rather than guess. When a mod needs installing on players' machines too, say so and ask whether they still want it before you touch anything.
