#!/bin/sh
# Does the range BASE..HEAD touch the backend? Prints and writes `affected=true|false` to
# $GITHUB_OUTPUT. Fails toward running: a manual dispatch, a missing or all-zero base (new branch,
# force push, shallow clone) and an unresolvable range all answer true.
#
# usage: ci-scope.sh <event> <base> <head>
set -eu
event="$1" base="$2" head="$3"

affected=true
case "$event" in
	workflow_dispatch) ;;
	*)
		if [ -n "$base" ] && [ "$base" != "0000000000000000000000000000000000000000" ] \
			&& git cat-file -e "$base^{commit}" 2>/dev/null && git cat-file -e "$head^{commit}" 2>/dev/null; then
			if git diff --quiet "$base" "$head" -- backend .github/workflows/backend.yml; then
				affected=false
			fi
		fi
		;;
esac

echo "backend affected: $affected"
[ -z "${GITHUB_OUTPUT:-}" ] || echo "affected=$affected" >> "$GITHUB_OUTPUT"
