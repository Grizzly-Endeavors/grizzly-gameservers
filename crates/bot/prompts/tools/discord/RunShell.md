---
id: RunShell
type: tool
tool_schema:
  name:
    type: string
    description: Exact server name, as shown by `list_servers`.
  command:
    type: string
    description: >-
      A shell command line, run by `sh -c` in the server's data directory.
      Pipelines, globs and redirection all work. Anything that would change a
      file is refused by the kernel, so use write_file or edit_file to make a
      change.
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch_files
  reasoning:
    - >-
      Its own shape rather than a NameParams tool because it carries the command
      line. The description names the two things a model cannot discover by
      trying: where the working directory starts, and that writes fail — so it
      reaches for the typed edit tools instead of retrying a redirection that the
      sandbox will refuse again.
    - >-
      Framed around searching rather than around "running commands", because the
      reason it exists is that read_file returns a whole file from its front and
      read_logs only ever returns a tail: neither can reach a stack trace in the
      middle of a long boot, and every model already knows how to grep for one.
---
Search and inspect a server's files and logs with ordinary shell commands — grep, tail, find, sed and friends. Read-only: it can look at anything on the server, including logs the game keeps outside its data directory, but it cannot change a file.
