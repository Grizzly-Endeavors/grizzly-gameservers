---
id: ThunderstorePackageDetail
type: prompt
annotations:
  sent_when: >-
    tool result when mod_info resolves a Namespace-Name pair to a real package
    in a Thunderstore game's community
  used_by:
    - file: discord/gary/tools.rs
      function: format_thunderstore_info
  variables:
    package:
      source: the package's Namespace-Name pair, as the registry spells it
      contents: >-
        the canonical Namespace-Name pair — worth reading back, since the
        registry resolves a pair the caller may have mis-cased
    pinned:
      source: >-
        the package's Namespace-Name pair joined to its current version number
      contents: >-
        the exact Namespace-Name-Major.Minor.Patch string a pinned mod-list
        entry uses
    description:
      source: the current version's summary, flattened to one line and bounded
      contents: >-
        the author-supplied summary with whitespace collapsed, cut at 200
        characters with a trailing … when longer
    downloads:
      source: the current version's download count
      contents: the download count as a plain number
    side:
      source: >-
        the side classification derived from this package's categories in the
        game's own community
      contents: >-
        one of four phrases naming where it has to be installed — "the server
        only", "players' machines only — nothing to install on the server", "the
        server and every player's machine", or "not stated by the registry —
        check the mod page" when the author tagged neither category, which is
        also what a package not listed in this game's community renders as
    status:
      source: whether the author has marked the package deprecated
      contents: >-
        the literal "deprecated by its author — do not install it" or "active";
        the wording is applied in code before rendering
    dependencies:
      source: >-
        up to eight packages the current version declares it needs, with a note
        appended in code when the list was cut
      contents: >-
        up to eight Namespace-Name-Version strings joined by commas, or the
        literal "none listed" when the version declares none, with ", and more"
        appended when the list was cut — both are applied in code before
        rendering
  reasoning:
    - >-
      The pre-install check, so the fields that decide whether an install is
      safe lead: the pinned string is the thing that gets written, the side
      decides whether the players have work to do, and deprecation is the one
      field that should stop an install outright.
    - >-
      Gives the pinned string ready-made rather than the version alone, because
      composing it is exactly the step a model gets subtly wrong — and a
      malformed entry stops the server rather than being ignored.
    - >-
      States that dependencies install themselves, because the opposite belief
      produces the specific failure this registry invites: listing a loader pack
      the image already supplies, which is a package the platform then refuses
      to install and has to explain.
    - >-
      Reached only for a package that exists; the not-found case is
      ThunderstoreNotFound.
---
{{package}} — {{description}}
install this exact line: {{pinned}}
downloads: {{downloads}}
needs installing on: {{side}}
status: {{status}}
needs alongside it: {{dependencies}}
Anything listed above installs automatically — list only the mod itself. Call game_guide for this game before writing it anywhere.
