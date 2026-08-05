---
id: IngameQuestion
type: prompt
annotations:
  sent_when: >-
    the user turn for an in-game question, used when the player's message is
    non-empty (the alternative to IngameNoQuestion)
  used_by:
    - file: ingame/agent.rs
      function: framed_question
  variables:
    player:
      source: the in-game player name from the supervisor's chat trigger
      contents: the raw player name (untrusted)
    question:
      source: the player's raw in-game chat text from the supervisor's trigger
      contents: >-
        the untrusted player message, presented verbatim as data — the in-game
        system prompt (IngamePersona) is what hardens Gary against treating it as
        instructions.
  reasoning:
    - >-
      Wraps the raw player text as an attributed quote so the model reads it as a
      named player's question, not its own instructions. Presenting the text as
      data is deliberate; the injection defense lives in IngamePersona, so keep
      the two aligned.
    - >-
      Fenced for the same reason as DiscordQuestion, and it matters more here:
      this surface carries no Discord identity and is reachable by anyone who can
      connect to a public game port. The markers are matched by crate::untrusted,
      which removes any payload line that tries to write one.
---
Player {{player}} asked in game chat:
-----BEGIN UNTRUSTED MESSAGE-----
{{question}}
-----END UNTRUSTED MESSAGE-----
