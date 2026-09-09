---
id: CrashOutputPreserved
type: prompt
annotations:
  sent_when: >-
    appended to a rollback result, telling the model where the crashed launch's
    own output was saved
  used_by:
    - file: discord/gary/tools.rs
      function: crash_evidence_note
  reasoning:
    - >-
      A rollback repairs the server and, in doing so, restarts it — which floods
      the live log ring with a healthy boot and leaves the model able to report
      that something crashed but not why. The supervisor writes the dead launch's
      output to the volume before that happens; this is how the model learns it
      is there, in the same breath as being told the rollback occurred.
    - >-
      Names the directory rather than a filename because the bot doesn't know
      which file was written — the supervisor names it by timestamp on the pod.
      A listing is one command, and the directory is stable.
    - >-
      Points at the shell specifically: the file holds a whole launch, which is
      thousands of lines, and reading it whole is the wrong move where a grep for
      the error is the right one.
---
The crashed server's own output was saved before the restart — it's on the server under `.grizzly/crashes/`, newest last. Use run_shell to search it (grep for an error or exception) rather than reading it whole; that's what says why it failed.
