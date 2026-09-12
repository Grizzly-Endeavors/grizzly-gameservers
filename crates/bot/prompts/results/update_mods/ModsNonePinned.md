---
id: ModsNonePinned
type: prompt
annotations:
  sent_when: >-
    tool result after update_mods found a mod list with entries but no pinned
    version among them
  used_by:
    - file: discord/gary/mod_list.rs
      function: exec_update_mods
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the server name, quoted inline
    path:
      source: the mod list path from the server's catalog entry
      contents: the data-root-relative path of the mod list
  reasoning:
    - >-
      Says why there is nothing to do, rather than reporting zero updates. A list
      of unpinned entries is already installing the latest of everything at every
      start, so "no updates available" would be true and completely misleading.
    - >-
      Names the tradeoff instead of recommending a fix. Pinning is what makes a
      restart reproducible and offline-recoverable, and not pinning is what keeps
      a list current by itself; which one a group wants is their call, and the
      model has no basis to pick for them.
---
Nothing in {{path}} on {{server}} is pinned to a version, so there is nothing to bump — every entry already installs the newest version at each start, which is the same result by a different route.

Worth telling them what that costs, if they ask: an unpinned list can change under them on any restart and cannot be rebuilt from the server's own cache when the registry is unreachable, while a pinned one reinstalls identically every time and has to be updated deliberately — with this tool.
