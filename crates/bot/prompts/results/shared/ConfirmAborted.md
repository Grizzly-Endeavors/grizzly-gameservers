---
id: ConfirmAborted
type: prompt
annotations:
  sent_when: >-
    tool result when a confirmation-gated destructive action (destroy, archive,
    restore) does not go ahead — either the user cancelled or the prompt timed
    out
  used_by:
    - file: discord/gary/tools.rs
      function: finish_destroy
  variables:
    reason:
      source: >-
        one of AbortCancelled / AbortTimedOut, selected by how the confirmation
        ended, rendered and spliced into the opener
      contents: the phrase naming why the action was aborted
    server:
      source: the target server's instance name
      contents: the server name
    verb:
      source: the past-tense action word chosen at the call site
      contents: >-
        the operation that did not happen — one of "deleted", "archived", or
        "restored" — a data word passed by the calling executor
  reasoning:
    - >-
      One template for every "confirmation didn't go through" outcome so the
      cancel and timeout wordings stay identical across destroy, archive, and
      restore. The reason composes in through a sub-prompt; the verb is a data
      word so the same template serves each destructive tool.
---
{{reason}} — {{server}} was not {{verb}}
