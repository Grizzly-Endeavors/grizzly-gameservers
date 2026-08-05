---
id: RecoverClusterFull
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server is refused because the namespace has no room
    to recreate the archived server
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  variables:
    name:
      source: the archive name the user asked to recover
      contents: a server name
    detail:
      source: describe_capacity_refusal over the carried CapacityRefusal figures
      contents: >-
        one sentence naming which resource the service ran out of, how much is
        left, and how much the server needed
  reasoning:
    - >-
      The archive is untouched by a refused recover, and saying so matters more
      here than on the other two results: a friend who thinks a recover failed
      part-way may go looking for the archive or ask for it again.
---
{{name}} was not recovered — {{detail}}. the archive is untouched and can be recovered once there's room; destroying or stopping a server they no longer need frees some, otherwise say the operator needs to add capacity
