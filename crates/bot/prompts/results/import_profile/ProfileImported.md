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

- Installing on the server: {{installs}}
- For players' machines only, not installed here: {{client_only}}
- Left out, switched off in the profile: {{disabled}}
- Left out, the server image already provides it: {{provided}}
- Left out, this game's registry has no record of it: {{missing}}

Restart {{server}} to apply it; nothing changes until you do. If the new list stops the server, the previous one is restored and the server restarted automatically. Anything in the players'-machines line is work each player has to do in their own mod manager — nothing on the server does it for them, so say so before they try to join.
