---
id: ModsUpdated
type: prompt
annotations:
  sent_when: >-
    tool result after update_mods rewrote a server's mod list onto newer versions
  used_by:
    - file: discord/gary/mod_list.rs
      function: format_update_applied
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the server name, quoted inline
    path:
      source: the mod list path the supervisor reported writing
      contents: the data-root-relative path of the mod list
    count:
      source: how many pinned versions were raised, both sides together
      contents: a decimal count, always one or more here
    bumped:
      source: the server-side pins that were raised
      contents: >-
        a comma-separated list of `Namespace-Name old -> new`, or the literal
        `none listed`
    client_bumped:
      source: the players'-machines pins that were raised
      contents: >-
        a comma-separated list of `Namespace-Name old -> new`, or `none listed`
    current:
      source: how many pinned packages were already at the newest version
      contents: a decimal count
    floating:
      source: install lines carrying no version
      contents: >-
        a comma-separated list of `Namespace-Name` packages, or `none listed`
    deprecated:
      source: >-
        packages whose newest version its author has marked deprecated, whether
        or not the pin moved
      contents: a comma-separated list of packages, or `none listed`
    unknown:
      source: packages the registry has no record of
      contents: a comma-separated list of packages, or `none listed`
  reasoning:
    - >-
      Names every version change rather than counting them, for the same reason
      the check does: a mod somebody deliberately held back is the one thing they
      would want reverted, and they can only ask for that if it is named.
    - >-
      Ends on the restart and the rollback together. A mod update is the change
      most likely to stop a server — a new version can want a loader the image
      doesn't have — so the automatic restore is the part that makes "go ahead"
      an honest thing to say.
    - >-
      Repeats the players'-machines warning even though the check already gave
      it, because an apply is reachable without a check and this is the message
      that gets read out. Leaving it to the earlier turn is how a group finds out
      by failing to connect.
---
Rewrote {{path}} on {{server}}, raising {{count}} version(s).

- On the server: {{bumped}}
- On the players' machines: {{client_bumped}}

Left as they were:

- Already newest: {{current}} package(s)
- No version pinned, so already installing the latest every start: {{floating}}
- Deprecated by its author: {{deprecated}}
- The registry has no record of it, so its line is untouched: {{unknown}}

Restart {{server}} to load the new versions; nothing changes until you do. If the new versions stop the server, the previous list is restored and the server restarted automatically. Every player has to update the players'-machines mods in their own mod manager to the same versions before they can connect — nothing on the server does it for them.
