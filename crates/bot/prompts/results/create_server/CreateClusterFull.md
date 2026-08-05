---
id: CreateClusterFull
type: prompt
annotations:
  sent_when: >-
    tool result when create_server is refused because the namespace itself has
    no room left — the whole service is full, not just this community's share
  used_by:
    - file: discord/gary/tools.rs
      function: refusal_reply
  variables:
    detail:
      source: describe_capacity_refusal over the carried CapacityRefusal figures
      contents: >-
        one sentence naming which resource the service ran out of, how much is
        left, and how much the new server needed; renders as "the service is
        out of room" when a live admission refusal left the dimension
        unresolved
  reasoning:
    - >-
      Kept apart from the guild-allowance results next door because the remedy
      differs and so does whose problem it is. An allowance is the community's
      own share and someone in the room can free it; this is the hardware being
      full across every community, which usually only the operator can fix.
    - >-
      The "won't clear on its own" clause is the point of this result. The
      previous behaviour reported a quota rejection as a transient error, which
      invites a retry loop against a wall that will still be there.
---
nothing was created — {{detail}}. this is the whole service being full, not this community's allowance, so it won't clear on its own: destroying or stopping a server they no longer need frees room, otherwise say the operator needs to add capacity
