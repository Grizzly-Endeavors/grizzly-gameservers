# justfile — discoverable dev tasks
#
# Every recipe is also documented in README.md as the raw cargo command,
# so `just` is convenience, not required.

set shell := ["bash", "-cu"]

# Default target runs the full local CI suite.
default: ci-local

# Run the binary (if applicable).
run *args:
    cargo run --bin grizzly-gameservers -- {{args}}

# Start the bot locally (detached), reading secrets from .env.
bot-start:
    ./scripts/local-bot.sh start

# Stop the locally-running bot.
bot-stop:
    ./scripts/local-bot.sh stop

# Restart the local bot (pick up .env changes).
bot-restart:
    ./scripts/local-bot.sh restart

# Follow the local bot's logs.
bot-logs:
    ./scripts/local-bot.sh logs

# Build a game's composite image and push it to the in-cluster registry (dev).
game-push game="minecraft" tag="dev":
    ./scripts/push-game-image.sh {{game}} {{tag}}

# Build the bot/ops-agent image and push it to the in-cluster registry (dev).
bot-push tag="dev":
    ./scripts/push-bot-image.sh {{tag}}

# Run the full test suite (quiet format).
test:
    cargo test --quiet

# Auto-format all Rust code.
fmt:
    cargo fmt --all

# Verify formatting without applying changes.
fmt-check:
    cargo fmt --all -- --check

# Run clippy with deny-warnings across all targets.
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Run cargo-deny (advisories, licenses, bans, sources).
deny:
    cargo deny check

# Run every gate a CI job would run, locally.
ci-local: fmt-check lint test deny

# Install git hooks from .githooks/ into .git/hooks/.
hooks:
    ./.githooks/install.sh

# Update local main from origin without leaving the current branch.
sync:
    #!/usr/bin/env bash
    set -euo pipefail
    git fetch --prune origin
    current=$(git rev-parse --abbrev-ref HEAD)
    if [ "$current" = "main" ]; then
        git pull --ff-only origin main
    else
        git fetch origin main:main
    fi

# Merge the current branch into main, push main, and delete the branch.
merge:
    #!/usr/bin/env bash
    set -euo pipefail
    branch=$(git rev-parse --abbrev-ref HEAD)
    if [ "$branch" = "main" ]; then
        echo "already on main — nothing to merge" >&2
        exit 1
    fi
    git switch main
    git pull --ff-only origin main
    git merge --no-ff "$branch"
    git push origin main
    git branch -d "$branch"
    if git ls-remote --exit-code --heads origin "$branch" >/dev/null 2>&1; then
        git push origin --delete "$branch"
    fi

# Stage all changes, commit with MSG, and push the current branch.
ship msg:
    git add -A
    git commit -m "{{msg}}"
    git push -u origin HEAD
