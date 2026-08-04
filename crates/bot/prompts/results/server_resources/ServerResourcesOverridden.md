---
id: ServerResourcesOverridden
type: prompt
annotations:
  sent_when: >-
    spliced into ServerResourcesReport's closing line when the instance carries
    a resource override, so its figures came from a resize rather than the game
  used_by:
    - file: discord/gary/tools.rs
      function: exec_server_resources
  variables:
    defaults:
      source: >-
        the game's catalog template, read by agones::catalog_sizing and
        rendered by format_sizing_summary
      contents: >-
        the four figures every other server of this game gets, worded exactly as
        set_resources reports a change; "unknown" when the game has no catalog
        entry or its template doesn't state all four, never a partial reading
  reasoning:
    - >-
      Naming the game's own figures is what makes an override readable. Without
      them a resize that no longer changes anything looks identical to one that
      is holding a server up, and neither the model nor a person can tell which
      they are looking at.
    - >-
      It is also what makes "put it back to normal" an exact instruction. There
      is no reset tool by design, so going back means calling set_resources with
      these four figures — stating them turns that from a guess into a copy.
    - >-
      Kept to one clause on the existing closing line rather than a line of its
      own: it qualifies where the numbers above came from, and a separate line
      invites reporting the defaults as if they were what the server has.
---
set for this server; this game's defaults are {{defaults}}
