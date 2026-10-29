#!/usr/bin/env bash
# Fails when the tracked files differ from the commit, or when new files that
# are not ignored appeared: a build that rewrites a lockfile or generates a
# tracked file passes locally and leaves the commit behind.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
changes="$(git -C "$root" status --porcelain)"
if [ -n "$changes" ]; then
    echo "tree: the build changed the working tree:" >&2
    printf '%s\n' "$changes" >&2
    git -C "$root" diff --stat >&2
    exit 1
fi
echo "tree: clean"
