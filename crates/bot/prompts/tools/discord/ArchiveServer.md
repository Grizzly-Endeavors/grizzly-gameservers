---
id: ArchiveServer
type: tool
params_from: NameParams
annotations:
  sent_when: offered on the Discord surface to admins only.
  used_by:
    - file: discord/gary/tools.rs
      function: admin_only_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      Cold storage: backs the world up, then releases the slot. Unlike destroy,
      the world is kept and recover_server brings it back, so the description
      says so and notes the user must approve the posted confirmation before
      anything is released.
---
Archive a server: save a durable backup and then release its storage, freeing a slot. The world is kept safe and recover_server brings it back later. Posts a confirmation the user must approve before anything is released.
