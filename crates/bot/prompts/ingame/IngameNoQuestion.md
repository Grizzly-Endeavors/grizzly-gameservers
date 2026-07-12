---
id: IngameNoQuestion
type: prompt
annotations:
  sent_when: >-
    the user turn for an in-game ping, used when the player's message is blank
    (the alternative to IngameQuestion)
  used_by:
    - file: ingame/agent.rs
      function: framed_question
  variables:
    player:
      source: the in-game player name from the supervisor's chat trigger
      contents: the raw player name (untrusted)
  reasoning:
    - >-
      A bare @Gary with no question shouldn't dead-end; this turns it into a
      prompt for Gary to ask what the player needs, keeping the in-game exchange
      moving instead of answering an empty message.
---
Player {{player}} pinged you in game chat with no question. Ask what they need.
