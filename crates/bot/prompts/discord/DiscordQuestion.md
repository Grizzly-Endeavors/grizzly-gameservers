---
id: DiscordQuestion
type: prompt
annotations:
  sent_when: >-
    the user turn for every Discord message routed to Gary — mention, DM, or
    home channel. The operator harness renders it through the same framed_turn,
    attributed to the harness rather than to the user it impersonates.
  used_by:
    - file: discord/gary/mod.rs
      function: framed_turn
  variables:
    asker:
      source: the Discord display name of the message author
      contents: the raw display name (untrusted — the user chose it)
    access:
      source: >-
        the speaker's access tier for this turn, recomputed per message by
        caller_access_level
      contents: "one of: read-only, manager, admin"
    question:
      source: >-
        the message content with the bot's own mention markup stripped, or the
        whole message on the DM and home-channel paths
      contents: >-
        the untrusted message, presented verbatim as data — DiscordPersona is
        what hardens Gary against treating it as instructions.
  reasoning:
    - >-
      A Discord channel is one shared conversation, so the transcript holds
      several people's turns. Without attribution the model reads them as one
      voice and cannot tell who asked for what — which breaks the case this
      exists for, an admin answering or approving something a friend started.
    - >-
      The tier travels with the speaker because Gary has no other way to know
      whether a "go ahead" is authorized. It is a hint for his wording, never the
      enforcement: the tool gate re-checks every call regardless of what the
      transcript says.
    - >-
      Wraps the raw message as an attributed quote for the same injection reason
      as IngameQuestion — the defense lives in the persona prompt, so keep the
      two aligned.
---
{{asker}} ({{access}}) says: {{question}}
