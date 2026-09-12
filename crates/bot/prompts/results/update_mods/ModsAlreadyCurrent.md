---
id: ModsAlreadyCurrent
type: prompt
annotations:
  sent_when: >-
    tool result after update_mods found every pinned version already newest, in
    either mode; nothing was written
  used_by:
    - file: discord/gary/mod_list.rs
      function: exec_update_mods
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the server name, quoted inline
    current:
      source: how many pinned packages are at the newest version
      contents: a decimal count
    floating:
      source: install lines carrying no version
      contents: >-
        a comma-separated list of `Namespace-Name` packages, or the literal
        `none listed`
    deprecated:
      source: packages whose newest version its author has marked deprecated
      contents: a comma-separated list of packages, or `none listed`
    unknown:
      source: packages the registry has no record of
      contents: a comma-separated list of packages, or `none listed`
  reasoning:
    - >-
      A complete answer, said as one. "Everything is up to date" is what the
      friend asked for, and it needs no restart, no confirmation and no next
      step — so the message closes rather than offering one.
    - >-
      Still reports deprecated and unknown packages, because nothing being out of
      date is not the same as nothing being wrong: a mod whose author has walked
      away is the same news whether or not there is a newer build of it, and this
      is the only turn that will mention it.
    - >-
      Returned for an apply as well as a check, which is why it never claims to
      have written anything: rewriting a file to the bytes it already holds would
      spend a snapshot and a restart on nothing.
---
Every pinned mod on {{server}} is already at its newest version — {{current}} package(s) checked, nothing to change and nothing to restart.

- No version pinned, so already installing the latest every start: {{floating}}
- Deprecated by its author: {{deprecated}}
- The registry has no record of it: {{unknown}}
