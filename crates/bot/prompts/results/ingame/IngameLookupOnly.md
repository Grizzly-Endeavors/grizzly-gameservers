---
id: IngameLookupOnly
type: prompt
annotations:
  sent_when: >-
    tool result on the in-game surface when the model calls any tool other than
    the two read-only lookups (mutating tools are never offered in-game)
  used_by:
    - file: ingame/agent.rs
      function: dispatch_ingame
  reasoning:
    - >-
      Sets the in-game surface's hard boundary — lookups only, everything else
      goes through Discord — so the model relays what it can do here instead of
      attempting an action it can't perform from game chat.
---
I can only look up server info from in-game — an admin can do the rest in Discord.
