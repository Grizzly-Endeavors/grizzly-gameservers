---
id: DeleteArchive
type: tool
name: delete_archive
params_from: NameParams
annotations:
  sent_when: offered on the Discord surface to admins only.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch
  reasoning:
    - >-
      Admin-only and gated behind the same confirmation as destroy_server, because
      it is the more final of the two: destroying a server deliberately leaves its
      archive standing as the way back, so this is the only action that makes a
      world unrecoverable.
    - >-
      The description says so explicitly rather than describing it as freeing
      storage. A model that reads this as routine housekeeping will offer it when
      someone complains about clutter, which is exactly when the confirmation is
      most likely to be waved through.
    - >-
      Shares NameParams with the other server-targeted tools, so the archive is
      named the same way the server was.
---
Permanently delete a server's archives from cold storage. This removes every saved copy of that world, and recover_server will no longer be able to bring it back — if the server isn't running anywhere, this is the end of it. Not a way to free up space: only use this when someone has clearly said they are finished with that world for good. Deleting a server does not delete its archives, so this is a separate, deliberate step.
