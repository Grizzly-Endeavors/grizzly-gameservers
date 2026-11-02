#!/usr/bin/env bash
# Install git hooks. Run once after cloning (or `just hooks`).
#
# Each installed hook is a small dispatcher that runs the copy in whichever
# checkout is committing. The hooks directory is shared by every worktree, so a
# symlink would make every worktree run one checkout's version.
set -e

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
hook_dir="$(git -C "$repo_root" rev-parse --path-format=absolute --git-common-dir)/hooks"
mkdir -p "$hook_dir"

for name in pre-commit pre-push commit-msg; do
    rm -f "$hook_dir/$name"
    cat >"$hook_dir/$name" <<EOF
#!/usr/bin/env bash
exec "\$(git rev-parse --show-toplevel)/.githooks/$name" "\$@"
EOF
    chmod +x "$hook_dir/$name"
    echo "  installed $name"
done

git -C "$repo_root" config --unset core.hooksPath 2>/dev/null || true
