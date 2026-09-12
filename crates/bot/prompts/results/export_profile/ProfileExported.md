---
id: ProfileExported
type: prompt
annotations:
  sent_when: tool result after export_profile produced a profile code for a server
  used_by:
    - file: discord/gary/mod_list.rs
      function: format_export
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the server name, quoted inline
    code:
      source: the minted or cached profile code
      contents: a UUID, the thing the player pastes into their mod manager
    count:
      source: how many packages the code names
      contents: a whole number, always at least one
    mods:
      source: the packages the code names
      contents: >-
        a comma-separated list of `Namespace-Name-1.2.3` releases; never the
        empty-list fallback, since an empty pack is answered elsewhere
  reasoning:
    - >-
      Leads with the code because that is the thing being handed over, and the
      user's next action is to paste it somewhere.
    - >-
      Lists what the code contains so the model says it out loud when handing it
      over. A code is a snapshot with no way to revoke it, and the failure this
      whole feature exists to prevent — versions drifting apart — is exactly
      what happens when somebody keeps using a code after the pack moved on.
    - >-
      Names the loader pack's absence, because a player comparing the list to
      their own install will notice it missing and conclude the code is
      incomplete; their mod manager installs it for them either way.
    - >-
      Says the same code comes back on a repeat ask, so the model doesn't warn
      about minting a new one every time somebody in the channel asks for it.
---
Profile code for {{server}}: {{code}}

It names {{count}} package(s): {{mods}}

Tell them to paste that into r2modman or Thunderstore Mod Manager (Import → code) to get the same set at the same versions. Their mod manager installs the mod loader itself, so it isn't in the list. Say what the code contains when you hand it over: it describes the pack as it is right now and can't be taken back, so if the mods change later they'll need a new one. Asking again while the pack is unchanged gives back this same code.
