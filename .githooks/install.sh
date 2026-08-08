#!/usr/bin/env bash
# Install git hooks
# Run this once after cloning the repository

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
# Ask git for the hooks directory rather than assuming `$REPO_ROOT/.git` is one.
# Inside a linked worktree `.git` is a *file* pointing at the real gitdir, so the
# assumption fails outright — and hooks live in the common dir shared by every
# worktree regardless.
GIT_COMMON_DIR="$(git -C "$REPO_ROOT" rev-parse --path-format=absolute --git-common-dir)"
HOOK_DIR="$GIT_COMMON_DIR/hooks"

mkdir -p "$HOOK_DIR"

echo "Symlinking .githooks into .git/hooks..."

for hook in "$SCRIPT_DIR"/pre-commit "$SCRIPT_DIR"/commit-msg; do
    name="$(basename "$hook")"
    ln -sf "$hook" "$HOOK_DIR/$name"
    echo "  $name -> .githooks/$name"
done

# Clear core.hooksPath if set — symlinks make it unnecessary
git -C "$REPO_ROOT" config --unset core.hooksPath 2>/dev/null || true

echo ""
echo "Git hooks installed successfully!"
