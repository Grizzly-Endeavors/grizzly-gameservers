---
id: ServerSummaryLine
type: prompt
annotations:
  sent_when: >-
    tool result rendering one server as a single labeled line — sent inside
    server_status and inside each entry of a server listing, on both the Discord
    and in-game surfaces
  used_by:
    - file: discord/gary/tools.rs
      function: format_summary
    - file: ingame/agent.rs
      function: format_summary
  variables:
    name:
      source: the server's Kubernetes instance name
      contents: the server name
    game:
      source: the server's catalog game id
      contents: >-
        the game id, or the literal "unknown game" when the server carries no
        game label — the fallback is applied in code before rendering
    state:
      source: the server's runtime state
      contents: the lifecycle state word (e.g. running, stopped)
    address:
      source: the server's advertised address
      contents: >-
        the host:port players connect to, or the literal "no address yet" when
        the server hasn't been assigned one — the fallback is applied in code
        before rendering
  reasoning:
    - >-
      The one server-line format used verbatim on both surfaces (ADR-008): one
      place to tune the labels so Discord and in-game never drift. The labels are
      prompt text; the field values are data entering through variables.
---
{{name}} (game: {{game}}, state: {{state}}, address: {{address}})
