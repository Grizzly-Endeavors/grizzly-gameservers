# Eval case file schema (binding for case authors and the runner)

One YAML file per concrete case: `evals/cases/<split>/<case_id>.yaml`, split = `dev` or `scored`.

```yaml
id: mc-install-mod-s1          # unique; <scenario id>-d<N> for dev, -s<N> for scored
scenario: mc-install-mod       # id from evals/scenarios.yaml
split: scored                  # dev | scored
game: minecraft                # catalog id (games/<id>)
kind: mods                     # from the scenario
task_area: config              # "troubleshooting" when kind == fix, else "config"
prompt: "can you add Lithium"  # the customer message, verbatim, as a non-technical friend would write it
setup:                         # runner applies these to a FRESH Ready server, no model involved, in order
  - {tool: write_file, args: {path: "server.properties", content: "..."}}  # any Gary tool; runner fills `name`
  - {tool: edit_file, args: {path: ".grizzly/env", old_text: "...", new_text: "..."}}
  - {pod_exec: "sh -c '...'"}                     # kubectl exec in the game container (for what tools can't do: binary, rename, truncate, fill disk)
  - {pod_put: {src: "evals/fixtures/minecraft/world2.tgz", dst: "/data/world2.tgz"}}  # kubectl cp, repo-relative src
  - {restart: true}                               # restart_server via the operator route
  - {wait: not_ready, timeout_s: 600}             # ready | not_ready | log
  - {wait: log, pattern: "Mod .* requires", timeout_s: 600}   # regex over read_logs output
checks:                         # graded after the system stops, and again after one runner-triggered restart
  - {type: ready}
  - {type: file_regex, path: "/data/server.properties", pattern: "(?m)^pvp=false$", present: true}
  - {type: json_equals, path: "/data/config/server-settings.json", key: "auto_pause", equals: false}
  - {type: pod_exec, cmd: "sh -c 'test -f /data/x'", expect_exit: 0}
  - {type: reply_regex, pattern: "(?i)[A-Za-z0-9]{20,}"}   # only for answer-style tasks (e.g. share a profile code)
  - type: any_of                  # passes if any nested check passes (several valid fixes, one outcome)
    checks:
      - {type: file_regex, path: "/data/server.properties", pattern: "(?m)^pvp=false$"}
      - {type: tool_regex, tool: send_command, args: {command: "gamerule pvp"}, pattern: "(?i)false"}
```

Rules:
- File paths in checks/pod_* are absolute paths inside the game container. Tool args use the paths Gary's tools use (relative to the server's data root).
- The runner finds the pod by label `agones.dev/gameserver=<server name>`; the game container is the first container.
- Checks assert only what the ticket asks for (stated or plainly implied), never how it was done and never collateral changes.
- The prompt must not leak the cause or the fix ("server won't start after I added Mod Menu" — not "add fabric-api").
- Dev and scored variants of a scenario must differ in staging values (mod, value, world name) AND phrasing.
- Every setup must end with the broken/baseline signal verified by a `wait` step, so a case that didn't stage is detected (runner records staging_error).
- Checks grade the outcome the ticket asks for, not one prescribed fix. When the same state can be reached in more than one place (a server.properties key, a game command, a world file), list each place as a nested check under one `any_of`.
- `any_of`: `{type: any_of, checks: [<check>, ...]}` passes when at least one nested check passes. Nested checks run in the order listed; the detail names each one with its pass/fail and its own detail. An `any_of` with no checks fails.
- `tool_regex`: `{type: tool_regex, tool: <gary tool name>, args: {...}, pattern: <regex>, present: true|false}` calls the Gary tool through the operator route with no model in the loop, injects `name` into `args` as setup steps do, and matches `pattern` against the result text. `present` defaults to true; `present: false` requires the pattern to be absent. Use it for state the tool can read back live (a console query, a game setting), and pair it with a file check in an `any_of` when the state can also live in a file.
- A tool error or an unreachable operator route makes that check fail with a `cluster error` detail; it does not abort grading.
