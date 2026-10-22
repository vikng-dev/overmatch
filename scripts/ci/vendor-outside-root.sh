#!/usr/bin/env bash
# CI only. Points every `[patch.crates-io]` entry that takes a crate from `vendor/` at a copy of that
# crate outside the workspace root, for every cargo command the job runs afterwards.
#
# rust-cache saves the artifacts of dependencies whose manifest lies outside the workspace root and
# drops the rest; `vendor/` lies inside it, so without the copy every run recompiles the patched
# crates and every crate above them. A `[patch]` in a cargo config wins over the Cargo.toml one, and
# `$CARGO_HOME/config.toml` is read by every cargo invocation of the job, rust-cache's own
# `cargo metadata` included. (Workspace membership would keep them too, but `cargo clippy` lints
# every member it compiles, and the vendored sources are upstream's, not held to this crate's gate.)
#
# A path dependency's fingerprint is mtime only. A fresh copy is stamped "now", newer than the
# cached fingerprint, so every copied file is stamped one old constant instead. The constant hides a
# content change from cargo just as well: the rust-cache key carries
# `hashFiles('vendor/**', '!vendor/**/*.md')`, so a change to anything compiled misses the cache
# (the notes are Markdown, not compiled, and leave the key alone).
set -euo pipefail

dest="${RUNNER_TEMP:?RUNNER_TEMP names the runner scratch directory}/vendor"
config="${CARGO_HOME:-${HOME}/.cargo}/config.toml"
# Git-bash on Windows: a mixed path (`D:/a/_temp`) is one both its tools and cargo read.
if command -v cygpath >/dev/null 2>&1; then
  dest="$(cygpath -m "${dest}")"
  config="$(cygpath -m "${config}")"
fi

if [[ -f "${config}" ]] && grep -q '^\[patch\.crates-io\]' "${config}"; then
  echo "::error::${config} already has a [patch.crates-io] table"
  exit 1
fi

# A TOML literal string (single quotes): a Windows path's backslashes are not escapes there.
entries="$(awk -v dest="${dest}" -v q="'" '
  /^\[/ { in_patch = ($0 == "[patch.crates-io]"); next }
  in_patch && /^[A-Za-z0-9_-]+ = \{ path = "vendor\/[^"]+" \}$/ {
    dir = $0
    sub(/^[^"]*"vendor\//, "", dir)
    sub(/".*$/, "", dir)
    printf "%s = { path = %s%s/%s%s }\n", $1, q, dest, dir, q
  }
' Cargo.toml)"
if [[ -z "${entries}" ]]; then
  echo "::error::Cargo.toml's [patch.crates-io] takes no crate from vendor/"
  exit 1
fi

rm -rf "${dest}"
cp -R vendor "${dest}"
find "${dest}" -type f -exec touch -t 200001010000 {} +

mkdir -p "$(dirname "${config}")"
printf '[patch.crates-io]\n%s\n' "${entries}" >> "${config}"
printf 'patched from outside the workspace root, in %s:\n%s\n' "${config}" "${entries}"
