---
id: ClusterUnreachable
type: prompt
annotations:
  sent_when: >-
    tool result when a listing/status tool can't reach the Kubernetes cluster to
    answer — returned to the model in place of the data it asked for
  used_by:
    - file: discord/gary/tools.rs
      function: cluster_error
  reasoning:
    - >-
      A transient-failure result worded as "try again in a moment" so the model
      relays a recoverable hiccup rather than a hard error, and doesn't invent a
      server list it couldn't actually read.
---
I couldn't reach the cluster just now — try again in a moment
