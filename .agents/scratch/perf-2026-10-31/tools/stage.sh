#!/bin/bash
# stage.sh <run-dir> <dest-dir> — the committable products of a curve run: top-level text tables and
# ledgers, every *.server.json, *.marker, *.window.tsv, *.client.jsonl.gz is NOT kept (frame streams
# stay in the scratchpad), and each server log reduced to its stats lines and the lever/bot lines
# (`<name>.window.log` locally, `<name>.server.log` on Edgegap), which is what regress.py reads.
set -euo pipefail
src="$1"; dst="$2"
mkdir -p "$dst"
cd "$src"
for f in manifest.txt summary.txt conditions.tsv deployments.tsv profile-left.txt notes.txt tables.txt; do
  [[ -f "$f" ]] && cp "$f" "$dst/"
done
while IFS= read -r -d '' f; do
  rel="${f#./}"
  mkdir -p "$dst/$(dirname "$rel")"
  case "$rel" in
    *.server.json | *.marker | *.window.tsv | */manifest.txt | */tables.txt) cp "$f" "$dst/$rel" ;;
    *.window.log | */*.server.log | */server.log)
      grep -a -E 'server_stats|debris stress|spawned circling bot|listening|threads_lever|cpu_quota' "$f" |
        sed 's/\x1b\[[0-9;]*m//g' >"$dst/$rel" || true ;;
  esac
done < <(find . -type f -print0)
du -sh "$dst"
