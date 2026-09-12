---
id: ProfileImported
type: prompt
annotations:
  sent_when: tool result after import_profile successfully rewrote a server's mod list
  used_by:
    - file: discord/gary/profiles.rs
      function: format_import
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the server name, quoted inline
    path:
      source: the mod list path the supervisor reported writing
      contents: the data-root-relative path of the mod list
    added:
      source: >-
        packages the new list names that the server's previous mod list did not
      contents: >-
        a comma-separated list of `Namespace-Name-1.2.3` releases, or the
        literal `none listed` when the import adds nothing
    removed:
      source: >-
        packages the server's previous mod list named that the new one does not
      contents: >-
        a comma-separated list of releases, or `none listed`
    changed:
      source: >-
        packages in both lists at a different version, rendered by the tool as
        `Namespace-Name old -> new`; a previously unpinned entry renders its old
        version as the literal `unpinned`
      contents: >-
        a comma-separated list of version changes, or `none listed`
    moved:
      source: >-
        packages in both lists that swapped sides, rendered by the tool as
        `Namespace-Name: now installed on the server` or
        `Namespace-Name: now players' machines only`
      contents: >-
        a comma-separated list of side changes, or `none listed`
    unchanged:
      source: how many packages are in both lists at the same version and side
      contents: a decimal count, `0` when nothing carried over
    identical:
      source: >-
        whether the four change categories are all empty, decided by the tool
      contents: >-
        one sentence saying the list is unchanged and the server was already
        running exactly this pack, or the empty string when anything did change
    installs:
      source: the profile entries written as install lines
      contents: >-
        a comma-separated list of `Namespace-Name-1.2.3` releases, or the
        literal `none listed` when the pack installs nothing on the server
    client_only:
      source: the profile entries written as `client:` lines
      contents: >-
        a comma-separated list of releases, or `none listed`
    disabled:
      source: the profile entries the exporting player had switched off
      contents: >-
        a comma-separated list of releases, or `none listed`
    provided:
      source: the profile entries this game's image already supplies
      contents: >-
        a comma-separated list of releases, or `none listed`
    missing:
      source: the profile entries this game's registry has no record of
      contents: >-
        a comma-separated list of releases, or `none listed`
  reasoning:
    - >-
      Leads with what *changed*, because an import replaces the mod list rather
      than adding to it and the removals are the half nobody asking for a
      modpack thinks to check. The contents of the pack answer "what am I
      running"; only the diff answers "what did this do to what I had", and that
      is the question a friend is about to be surprised by.
    - >-
      The diff is against the list the server declared, not against what was
      installed. Dependencies are resolved from the declared list and appear in
      neither, so diffing installs would report a dozen removals for a change
      that removed nothing — and the model would relay that.
    - >-
      Side changes get their own category rather than being reported as a
      removal plus an addition. A mod that moved to the players' side is still in
      the pack but is no longer running on the server, and "removed" would be a
      wrong answer to the only question that distinction affects.
    - >-
      Reports every category rather than a count, because each one is a
      different thing to tell a friend: what got installed, what they still have
      to install themselves, and three separate reasons something they expected
      isn't there. A summary that hid the last three would produce "where's my
      mod" as the next message.
    - >-
      The client-side entries are called out as work the players have to do,
      since nothing done on a server installs anything on anyone's machine —
      the single most consequential thing to get right for a non-technical
      audience, and the moment to say it is now rather than after everyone fails
      to connect.
    - >-
      Ends on the restart because the file is inert until one happens, and the
      rollback promise is repeated here rather than assumed: it is what makes it
      safe to say "go ahead" instead of hedging.
---
Rewrote {{path}} on {{server}} from the profile.

What changed against the list the server had. {{identical}}

- Added: {{added}}
- Removed, and no longer installed after the restart: {{removed}}
- Version changed: {{changed}}
- Moved between the server and players' machines: {{moved}}
- Unchanged: {{unchanged}} package(s)

What the pack contains, whatever changed:

- Installing on the server: {{installs}}
- For players' machines only, not installed here: {{client_only}}
- Left out, switched off in the profile: {{disabled}}
- Left out, the server image already provides it: {{provided}}
- Left out, this game's registry has no record of it: {{missing}}

Restart {{server}} to apply it; nothing changes until you do. If the new list stops the server, the previous one is restored and the server restarted automatically. Anything in the players'-machines line is work each player has to do in their own mod manager — nothing on the server does it for them, so say so before they try to join.
