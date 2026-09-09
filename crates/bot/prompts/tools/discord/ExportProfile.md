---
id: ExportProfile
type: tool
params_from: NameParams
annotations:
  sent_when: offered on the Discord surface to everyone, at every tier.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch
  reasoning:
    - >-
      The half that fixes a real recurring pain, which is why it is read-only
      tier and reachable by the friends who actually need it: modded servers
      break when a player's mod versions drift from the server's, and a code
      minted from the server's own pins makes everyone match by construction.
      A player asking for this is asking to stop being broken, not asking to
      change anything.
    - >-
      Says the code changes nothing on the server, because a tool that hands
      back a shareable identifier reads as an action and the model should offer
      it freely rather than treating it as a change to confirm.
    - >-
      Says it is a snapshot rather than a standing address. A code cannot be
      revoked and does not follow the server, so one handed out before a mod
      changes still describes the old pack — and someone who thinks they have a
      subscription finds out by drifting out of sync, which is the exact problem
      this tool exists to solve.
---
Mint a profile code for what this server is running, for a player to paste into r2modman or Thunderstore Mod Manager and get the matching setup. Covers the whole modpack — the mods the server runs and the ones that belong on players' machines — pinned to the exact versions the server resolved, which is what keeps everyone's versions in step. Changes nothing on the server, so offer it whenever someone asks how to set their game up to join. The code is a snapshot of the pack right now, not a standing address: it can't be taken back, and it won't follow later changes, so say what it contains when you hand it over and mint a fresh one after the mods change.
