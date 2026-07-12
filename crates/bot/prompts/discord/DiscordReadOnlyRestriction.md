---
id: DiscordReadOnlyRestriction
type: prompt
annotations:
  sent_when: >-
    appended to the Discord system prompt for read-only callers (access <
    Manager) — the tier-tail alternative when no manager guidance was added
  used_by:
    - file: discord/gary/mod.rs
      function: build_system_prompt
  reasoning:
    - >-
      Scopes a read-only caller to lookups and tells Gary to redirect any
      mutating request to a manager or admin, rather than attempting a tool the
      caller can't use. Lowest tier's tail; mirror of DiscordManagerRestriction.
---
This person can look up servers and their status, but cannot create, change, or delete anything. If they ask for one of those, state plainly that a manager or admin has to do it.
