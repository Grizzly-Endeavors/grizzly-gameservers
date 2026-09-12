---
id: ModsUpdatesAvailable
type: prompt
annotations:
  sent_when: >-
    tool result after update_mods ran in check mode and found at least one pinned
    version behind the registry; nothing was written
  used_by:
    - file: discord/gary/mod_list.rs
      function: format_update_check
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the server name, quoted inline
    path:
      source: the mod list path from the server's catalog entry
      contents: the data-root-relative path of the mod list
    count:
      source: how many pinned versions would move, both sides together
      contents: a decimal count, always one or more here
    bumped:
      source: the server-side pins that would be raised
      contents: >-
        a comma-separated list of `Namespace-Name old -> new`, or the literal
        `none listed`
    client_bumped:
      source: the players'-machines pins that would be raised
      contents: >-
        a comma-separated list of `Namespace-Name old -> new`, or `none listed`
    current:
      source: how many pinned packages are already at the newest version
      contents: a decimal count
    floating:
      source: install lines carrying no version
      contents: >-
        a comma-separated list of `Namespace-Name` packages, or `none listed`
    deprecated:
      source: >-
        packages whose newest version its author has marked deprecated, whether
        or not the pin would move
      contents: a comma-separated list of packages, or `none listed`
    unknown:
      source: packages the registry has no record of
      contents: a comma-separated list of packages, or `none listed`
  reasoning:
    - >-
      States up front that nothing was written, because the model asked for a
      check and the natural next sentence to a friend is "done" — which would be
      wrong, and wrong in a way nobody would catch until the versions they were
      promised weren't there.
    - >-
      Names every version on both sides rather than counting them. "Six updates
      available" is not something a friend can agree or object to; a mod they
      deliberately held back at an older version is exactly what they would
      object to, and they can only do that if it is named.
    - >-
      The players'-machines line is separate because it is the only part of an
      update that creates work outside the server. Folded into one list it reads
      as six free changes, and the players find out when they can't connect.
    - >-
      Deprecated and unknown packages are reported here rather than only on
      apply, since a check is when someone is deciding — and "its author has
      stopped maintaining this" is a reason to drop a mod, not a reason to bump
      it.
    - >-
      Unpinned entries are listed rather than silently excluded, so the counts
      add up against the file. An entry missing from every category reads as a
      bug in the report and invites a second, redundant pass with mod_info.
---
Checked every pinned mod in {{path}} on {{server}} against the registry. **Nothing was written** — this was a check.

{{count}} version(s) would move:

- On the server: {{bumped}}
- On the players' machines: {{client_bumped}}

Also worth saying:

- Already newest: {{current}} package(s)
- No version pinned, so already installing the latest every start: {{floating}}
- Deprecated by its author: {{deprecated}}
- The registry has no record of it, so it would be left exactly as it is: {{unknown}}

Anything on the players'-machines line is work each player has to do in their own mod manager, and their versions have to match the server's — so say that before they try to join. To make the change, run this again with `apply`, then restart {{server}}.
