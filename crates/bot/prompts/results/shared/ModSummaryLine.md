---
id: ModSummaryLine
type: prompt
annotations:
  sent_when: >-
    tool result rendering one Modrinth project as a single labeled line — sent
    for each hit inside a search_mods listing
  used_by:
    - file: discord/gary/tools.rs
      function: format_mod_line
  variables:
    slug:
      source: the project's Modrinth slug
      contents: the exact slug, which is the identifier a server's config needs
    title:
      source: the project's title, bounded to 80 characters
      contents: >-
        the author-supplied title, cut at 80 characters with a trailing … when
        longer
    description:
      source: the project's summary, flattened to one line and bounded
      contents: >-
        the author-supplied summary with whitespace collapsed, cut at 200
        characters with a trailing … when longer
    downloads:
      source: the project's total download count
      contents: the download count as a plain number
    server_side:
      source: the project's server_side support field
      contents: >-
        the registry's own word — required, optional, unsupported, or unknown
    client_side:
      source: the project's client_side support field
      contents: >-
        the registry's own word — required, optional, unsupported, or unknown
    loaders:
      source: the project's loaders, split out of its categories
      contents: >-
        the loader names joined by commas, or the literal "none listed" when the
        project names none — the fallback is applied in code before rendering
    versions:
      source: the newest few Minecraft versions the project supports
      contents: >-
        up to six version strings joined by commas, oldest first, or the literal
        "none listed" when the project names none — the fallback is applied in
        code before rendering
  reasoning:
    - >-
      One line per hit so a listing stays scannable. The slug leads because it
      is the only field that ends up in a server's config and the only one that
      must be copied exactly.
    - >-
      client_side is labeled "players' machines" rather than by the registry's
      field name so the consequence is legible without knowing Modrinth's
      vocabulary: a project required there is one every player has to install
      themselves, which no server-side change can do for them.
    - >-
      The field values are third-party registry text entering as data through
      variables, already bounded in code; only the labels are prompt text.
---
{{slug}} — {{title}}: {{description}} (downloads: {{downloads}}; needed on the server: {{server_side}}; needed on players' machines: {{client_side}}; loaders: {{loaders}}; recent Minecraft versions: {{versions}})
