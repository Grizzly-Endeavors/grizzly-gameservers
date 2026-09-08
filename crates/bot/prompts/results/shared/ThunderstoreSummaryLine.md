---
id: ThunderstoreSummaryLine
type: prompt
annotations:
  sent_when: >-
    tool result rendering one Thunderstore package as a single labeled line —
    sent for each hit inside a search_mods listing for a Thunderstore game
  used_by:
    - file: discord/gary/tools.rs
      function: format_thunderstore_line
  variables:
    package:
      source: the package's Namespace-Name pair, bounded to 80 characters
      contents: >-
        the exact Namespace-Name pair, which is the identifier every other step
        needs — cut at 80 characters with a trailing … in the impossible case of
        a longer one
    description:
      source: the package's summary, flattened to one line and bounded
      contents: >-
        the author-supplied summary with whitespace collapsed, cut at 200
        characters with a trailing … when longer
    downloads:
      source: the package's total download count in this community
      contents: the download count as a plain number
    side:
      source: >-
        the side classification derived from the package's community categories
      contents: >-
        one of four phrases naming where it has to be installed — "the server
        only", "players' machines only — nothing to install on the server", "the
        server and every player's machine", or "not stated by the registry —
        check the mod page" when the author tagged neither category
    updated:
      source: the day the package was last updated
      contents: >-
        an ISO date (2026-03-12), or the literal "unknown" when the registry
        reported no usable timestamp — the fallback is applied in code before
        rendering
  reasoning:
    - >-
      One line per hit so a listing stays scannable. The Namespace-Name pair
      leads because it is the only field that ends up in a server's mod list and
      the only one that must be copied exactly.
    - >-
      No version: a Thunderstore search result carries none at all, and a
      pinned entry needs one — so the line deliberately can't be acted on
      without a mod_info call, which is where the version and the dependency
      list come from.
    - >-
      The last-updated day is here rather than in the detail alone because
      abandonment is the failure this registry actually produces: a package that
      still installs cleanly against a game that has moved on is the common bad
      recommendation, and a date is the cheapest signal for it.
    - >-
      The field values are third-party registry text entering as data through
      variables, already bounded in code; only the labels are prompt text.
---
{{package}} — {{description}} (downloads: {{downloads}}; needs installing on: {{side}}; last updated: {{updated}})
