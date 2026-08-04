---
id: ServerResourcesNotFound
type: prompt
annotations:
  sent_when: >-
    tool result when server_resources names an instance with no managed volume —
    it never existed, or it was destroyed
  used_by:
    - file: discord/gary/tools.rs
      function: exec_server_resources
  variables:
    server:
      source: the instance name the tool was called with
      contents: the instance name
  reasoning:
    - >-
      The sampling universe is the managed PVC list, so an absent volume means
      an absent instance. Says so plainly rather than reporting zeroes, which
      would read as a server that exists and uses nothing.
---
no server called {{server}} exists, so it has no resources to report
