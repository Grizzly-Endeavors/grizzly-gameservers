#!/usr/bin/env bash
# Maps changed paths (one per line on stdin) to the checks they can affect.
# Prints one token per line, deduplicated:
#   pkg:<cargo package>   run that package's tests
#   workspace             run every package's tests
#   deny                  run the offline cargo-deny checks
# A path this table doesn't know selects everything, so a gap here makes the
# hook check more, never less.

select_checks() {
    local path
    while IFS= read -r path || [ -n "$path" ]; do
        [ -z "$path" ] && continue
        case "$path" in
            # Prompt Markdown under crates/ is compiled into the binary, so these
            # rules must come before the docs rules below.
            crates/bot/*) echo "pkg:grizzly-gameservers" ;;
            crates/supervisor/*) echo "pkg:grizzly-supervisor" ;;
            # Both other crates depend on the wire contract.
            crates/control-api/*) echo "workspace" ;;
            # The bot's catalog tests parse every game template.
            games/*) echo "pkg:grizzly-gameservers" ;;
            Cargo.toml | Cargo.lock | deny.toml)
                echo "workspace"
                echo "deny"
                ;;
            clippy.toml | rustfmt.toml | rust-toolchain | rust-toolchain.toml | .cargo/* | .githooks/*)
                echo "workspace"
                ;;
            # Read by no build or test: CI, Flux, cluster manifests, docs, scripts.
            docs/* | cluster/* | deploy/* | .github/* | .claude/* | scripts/* | *.md | *.png \
                | justfile | Dockerfile | .dockerignore | .gitignore | gate-config.json)
                ;;
            *) echo "workspace" ;;
        esac
    done | sort -u
}
