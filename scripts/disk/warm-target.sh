#!/usr/bin/env bash
# warm-target.sh — give a fresh worktree a warm target/ cloned from another checkout.
#
# Usage, from the worktree root:  scripts/disk/warm-target.sh [SOURCE_CHECKOUT]
# SOURCE_CHECKOUT defaults to the main worktree (first entry of `git worktree list`).
#
# Two steps:
#   1. `cp -c -R SOURCE/target ./target` — an APFS clone: seconds, no disk cost (macOS only).
#   2. Every tracked file whose content is byte-identical to SOURCE's copy takes SOURCE's
#      mtime. Cargo judges a path package (our crate and the `vendor/` [patch] crates) fresh
#      by comparing its source mtimes against the dep-info in target/; a checkout stamps every
#      file with checkout time, so without this step the vendored bevy_reflect looks newer than
#      the cloned artifacts and everything above it — Bevy, lightyear, overmatch — rebuilds.
#      Registry crates are fingerprinted by version, not mtime, and need no help.
#
# Why step 2 is sound: for each mirrored file, cargo in this worktree sees exactly the
# (content, mtime) pair cargo in SOURCE sees, so it reaches SOURCE's freshness verdict, which
# is correct for SOURCE's target. Files that differ keep their checkout mtime and rebuild.
# The verdict is only borrowed for a target that came from SOURCE, so the script refuses to
# run over an existing target/: there, an older mtime could hide an edit the local target
# never compiled.
set -euo pipefail

dest=$(git rev-parse --show-toplevel)
src=${1:-$(git worktree list --porcelain | awk '/^worktree /{ print substr($0, 10); exit }')}
src=$(cd "$src" && pwd -P)
dest=$(cd "$dest" && pwd -P)

if [[ "$src" == "$dest" ]]; then
  echo "warm-target: source and destination are the same checkout ($dest)" >&2
  exit 1
fi
if [[ ! -d "$src/target" ]]; then
  echo "warm-target: $src has no target/ to clone" >&2
  exit 1
fi
if [[ -e "$dest/target" ]]; then
  echo "warm-target: $dest/target already exists; delete it first" >&2
  exit 1
fi

cp -c -R "$src/target" "$dest/target"

# A worktree is checked out without the LFS smudge, so every LFS asset is a pointer file until
# the content is written from the shared object store (offline; `git lfs pull` only if an object
# is missing). Before the mtime mirror, so the materialized assets mirror too.
if git -C "$dest" lfs checkout >/dev/null 2>&1; then
  pointers=$(git -C "$dest" lfs ls-files 2>/dev/null | grep -c '^[0-9a-f]* - ' || true)
  if [[ "${pointers:-0}" -gt 0 ]]; then
    echo "warm-target: $pointers LFS objects missing locally; run 'git lfs pull' in $dest" >&2
  fi
else
  echo "warm-target: git-lfs not available; LFS assets stay pointer files" >&2
fi

mirrored=0
differ=0
while IFS= read -r -d '' f; do
  [[ -f "$dest/$f" && ! -L "$dest/$f" && -f "$src/$f" && ! -L "$src/$f" ]] || continue
  if cmp -s "$src/$f" "$dest/$f"; then
    touch -r "$src/$f" "$dest/$f"
    mirrored=$((mirrored + 1))
  else
    differ=$((differ + 1))
  fi
done < <(git -C "$dest" ls-files -z)

echo "warm-target: cloned $src/target; mirrored $mirrored mtimes, $differ files differ"
