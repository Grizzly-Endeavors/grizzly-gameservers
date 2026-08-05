---
id: ColdStartClusterFull
type: prompt
annotations:
  sent_when: >-
    tool result when a cold start is refused because the namespace has no room
    for the pod it would bring back
  used_by:
    - file: discord/gary/tools.rs
      function: exec_cold_start
  variables:
    server:
      source: the instance name the user asked to start
      contents: a server name
    detail:
      source: describe_capacity_refusal over the carried CapacityRefusal figures
      contents: >-
        one sentence naming which resource the service ran out of, how much is
        left, and how much the server needed
  reasoning:
    - >-
      A stopped server keeps its port and its world, so the friend reasonably
      reads "start" as free. It isn't: the pod's CPU and memory have to be found
      again. Saying so is what stops the model from treating this as a fault and
      retrying.
---
{{server}} was not started — {{detail}}. its world and address are untouched and it can be started once there's room; stopping another server that isn't being played frees some, otherwise say the operator needs to add capacity
