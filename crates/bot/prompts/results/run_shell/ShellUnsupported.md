---
id: ShellUnsupported
type: prompt
annotations:
  sent_when: >-
    tool result after run_shell against a server whose supervisor predates the
    shell route
  used_by:
    - file: discord/gary/tools.rs
      function: exec_run_shell
  reasoning:
    - >-
      A server keeps the supervisor baked into its running image until it next
      starts, so a shell can be genuinely unavailable on a live server while
      working everywhere else. Without this the model would see the supervisor's
      bare "not found" and read it as a missing *file*, then keep rephrasing the
      command against a route that will never exist.
    - >-
      Names the fallbacks and the fix, because both are actionable: the older
      tools still work now, and a restart is what picks up the newer image.
---
this server is running an older supervisor without shell support — read_logs and read_file still work, and the shell becomes available the next time the server restarts onto a current image
