---
id: ModProjectDetail
type: prompt
annotations:
  sent_when: tool result when mod_info resolves a slug to a real Modrinth project
  used_by:
    - file: discord/gary/tools.rs
      function: format_mod_info
  variables:
    slug:
      source: the project's Modrinth slug, as the registry spells it
      contents: >-
        the canonical slug — worth reading back, since the registry resolves a
        slug the caller may have mis-cased or abbreviated
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
      source: the project's loaders field
      contents: >-
        every loader name joined by commas, or the literal "none listed" when
        the project names none — the fallback is applied in code before
        rendering
    versions:
      source: the newest few Minecraft versions the project supports
      contents: >-
        up to six version strings joined by commas, oldest first, or the literal
        "none listed" when the project names none — the fallback is applied in
        code before rendering
  reasoning:
    - >-
      The pre-install check, so the fields that decide whether an install is
      safe lead: the loaders and versions determine whether it fits the server
      at all, and the two side flags determine whether the players have work to
      do. The closing line states the compatibility rule outright because a
      version mismatch is not a degraded install — it stops the server booting.
    - >-
      Shows a "datapack" loader plainly rather than as a separate concept: the
      registry models datapacks as a loader, and a project offering both that
      and a jar loader can be installed either way.
    - >-
      Reached only for a slug that exists; the not-found case is ModNotFound.
---
{{slug}} — {{title}}: {{description}}
downloads: {{downloads}}
needed on the server: {{server_side}}
needed on players' machines: {{client_side}}
loaders: {{loaders}}
recent Minecraft versions: {{versions}}
Only install this on a server whose loader and Minecraft version appear above — an entry with no matching version stops the server from starting.
