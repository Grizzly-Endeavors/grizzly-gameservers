---
id: Forget
type: tool
tool_schema:
  id:
    type: integer
    description: >-
      The id of the fact to delete, as shown in the "Things you've learned" list
      (the number after the `#`).
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: manager_tools
    - file: discord/gary/tools.rs
      function: dispatch_memory
  reasoning:
    - >-
      The counterpart to remember: deletes a fact that turned out wrong or
      stale. The id is the number shown next to each saved fact, so the
      description points the model at where to read it. Cross-guild like
      remember, so it skips the scope gate (dispatch_memory).
---
Delete a saved fact by its id (the number after the # in "Things you've learned") when it turns out wrong or no longer applies.
