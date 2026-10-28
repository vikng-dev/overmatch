#!/usr/bin/env bash
# test_bays.sh — the bay pool's invariants (lib.sh), driven against a throwaway repository.
#
#     scripts/bays/test_bays.sh
#
# RUN BY HAND, on macOS (the pool locks with /usr/bin/lockf). The orchestrator, the WorktreeCreate
# hook and every Codex dispatch rely on these invariants; nothing else checks them.
#
# HERMETIC. Every run builds its own git repository under `mktemp -d`, with the global and system
# git config shut out, and deletes it. It copies the REAL lease, release, status and lib.sh, and the
# two hooks in .claude/hooks/ (bay-lease.sh, bay-release.sh), into that repository and runs the
# copies: they find their pool from their own location, so the copies drive the scratch pool with
# exactly the code that ships. Nothing else in this work tree is read, and nothing in it is written.
#
# Each case names the mutant that breaks it:
#   1. A fourth lease fails, names the three holders and creates nothing past bay-3.
#      Mutant: the pool grows on demand, or the loop ignores BAYS_COUNT.
#   2. A dirty bay is never recycled, and `release` refuses it without --force; --force saves the
#      work to refs/bays/salvage/ before it clears the bay.
#      Mutants: the dirt check dropped from lease or from release; --force clearing with no salvage.
#   3. A lease recycles to main's tip, and never resets a lane branch holding commits main lacks.
#      Mutants: the switch targets the bay's old HEAD instead of main; the ahead check dropped.
#   4. Concurrent leases never share a bay.
#      Mutant: `bays_locked` removed, so two leases both find one bay free before either records it.
#   5. A lease deletes a bay's target/ exactly when main's Cargo.lock differs from the bay's.
#      Mutants: the comparison dropped (a stale target is kept) or inverted (a warm one is lost).
#   6. The hooks keep Claude Code's contract (hooks.md): WorktreeCreate's stdout is the bay's path
#      alone and its exit is nonzero with no bay free; WorktreeRemove frees a clean bay in place
#      and exits nonzero, keeping the bay, when it holds work or is not a bay.
#      Mutants: lease progress printed to stdout (Claude Code would read a wrong path); the
#      WorktreeRemove hook deleting the directory, or exiting 0 over uncommitted work.

set -u

HERE=$(cd "$(dirname "$0")" && pwd -P)
HOOKS=$HERE/../../.claude/hooks

# ── the harness ──────────────────────────────────────────────────────────────────────────────────

passed=0
failed=0

group() { printf '\n%s\n' "$1"; }
ok() {
  passed=$((passed + 1))
  printf '  ok    %s\n' "$1"
}
bad() {
  failed=$((failed + 1))
  printf '  FAIL  %s\n        expected: [%s]\n        actual:   [%s]\n' "$1" "$2" "$3"
}

# A name, what it must produce, and what it produced.
is() { if [[ "$2" == "$3" ]]; then ok "$1"; else bad "$1" "$2" "$3"; fi; }

# A name, `yes|no`, and the exit status of what was run.
says() {
  local said=no
  [[ "$3" -eq 0 ]] && said=yes
  is "$1" "$2" "$said"
}

exists() { if [[ -e "$1" ]]; then echo yes; else echo no; fi; }

# ── the scratch repository ───────────────────────────────────────────────────────────────────────

WORK=$(mktemp -d "${TMPDIR:-/tmp}/bays-test.XXXXXX") || exit 1
WORK=$(cd "$WORK" && pwd -P)
trap 'rm -rf "$WORK"' EXIT INT TERM
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
unset BAYS_LOCK_HELD

REPO=$WORK/repo
POOL=$REPO/.claude/worktrees
ERR=$WORK/stderr

git init -q -b main "$REPO"
git -C "$REPO" config user.name bays-test
git -C "$REPO" config user.email bays-test@example.invalid
printf '/target\n.claude/worktrees/\n' >"$REPO/.gitignore"
echo base >"$REPO/README"
echo lock-v1 >"$REPO/Cargo.lock"
mkdir -p "$REPO/scripts/bays" "$REPO/.claude/hooks"
cp "$HERE/lib.sh" "$HERE/lease" "$HERE/release" "$HERE/status" "$REPO/scripts/bays/"
cp "$HOOKS/bay-lease.sh" "$HOOKS/bay-release.sh" "$REPO/.claude/hooks/"
git -C "$REPO" add -A
git -C "$REPO" commit -q -m base

lease() { "$REPO/scripts/bays/lease" "$@" 2>"$ERR"; }
release() { "$REPO/scripts/bays/release" "$@" 2>"$ERR"; }
release_all() { for n in 1 2 3; do [[ -e "$POOL/bay-$n" ]] && release "bay-$n"; done; }
tip() { git -C "$1" rev-parse HEAD; }
leased_to() { cut -d' ' -f1 "$(git -C "$POOL/$1" rev-parse --absolute-git-dir)/bay-lease" 2>/dev/null; }
commit_in() { git -C "$1" add -A && git -C "$1" commit -q -m "$2"; }

# ── 1. three bays, no overflow ───────────────────────────────────────────────────────────────────

group "1. three bays, no overflow"
p1=$(lease a)
p2=$(lease b)
p3=$(lease c)
is "three leases take bay-1, bay-2, bay-3 in order" \
  "$POOL/bay-1 $POOL/bay-2 $POOL/bay-3" "$p1 $p2 $p3"
lease d >/dev/null
says "a fourth lease fails" no $?
named=0
for h in a b c; do grep -q "leased to $h " "$ERR" && named=$((named + 1)); done
is "its message names all three holders" 3 "$named"
is "nothing is created past bay-3" no "$(exists "$POOL/bay-4")"
git -C "$REPO" rev-parse -q --verify refs/heads/lane/d >/dev/null
says "the refused lane gets no branch" no $?
release_all

# ── 2. a dirty bay is never recycled, nor released without --force ──────────────────────────────

group "2. a dirty bay is never recycled, nor released without --force"
pe=$(lease e)
echo lane-work >"$pe/notes.txt"
release bay-1
says "release refuses a bay with an untracked file" no $?
is "the untracked file survives the refusal" lane-work "$(cat "$pe/notes.txt" 2>/dev/null)"
is "the bay stays leased to its lane" e "$(leased_to bay-1)"

pf=$(lease f)
release bay-2
echo stray-edit >>"$pf/README"
pg=$(lease g)
is "a free bay with a tracked edit is passed over" "$POOL/bay-3" "$pg"
lease h >/dev/null
says "with only that dirty bay free, a lease fails rather than recycle it" no $?
grep -q "bay-2: free, but holds" "$ERR"
says "the message names the dirty free bay" yes $?
is "the tracked edit survives both leases" "base stray-edit" "$(tr '\n' ' ' <"$pf/README" | sed 's/ $//')"

release --force bay-1
says "release --force frees the dirty bay" yes $?
is "--force clears the untracked file" no "$(exists "$pe/notes.txt")"
salvage=$(git -C "$REPO" for-each-ref --format='%(refname)' refs/bays/salvage/ | grep bay-1-)
is "--force first saved the file under refs/bays/salvage/" lane-work \
  "$(git -C "$REPO" show "$salvage:notes.txt" 2>/dev/null)"
release bay-3
release --force bay-2

# ── 3. a lease recycles to main's tip ────────────────────────────────────────────────────────────

group "3. a lease recycles to main's tip"
pi=$(lease i)
echo from-lane-i >"$pi/lane.txt"
commit_in "$pi" "lane i work"
lane_i=$(tip "$pi")
release bay-1
echo main-2 >>"$REPO/README"
commit_in "$REPO" "main moves on"
pj=$(lease j)
is "the recycled bay is the one lane i left" "$POOL/bay-1" "$pj"
is "its HEAD is main's tip" "$(tip "$REPO")" "$(tip "$pj")"
is "it is on lane/j" lane/j "$(git -C "$pj" symbolic-ref --short HEAD)"
is "lane i's file is gone from the bay" no "$(exists "$pj/lane.txt")"
is "lane/i keeps its commit" "$lane_i" "$(git -C "$REPO" rev-parse lane/i)"
release bay-1
lease i >/dev/null
says "lease refuses a lane name whose branch holds commits main lacks" no $?
is "and lane/i still holds its commit" "$lane_i" "$(git -C "$REPO" rev-parse lane/i)"
lease j >/dev/null
says "a lane name whose branch main already holds is leased again" yes $?
release_all

# ── 4. concurrent leases never share a bay ───────────────────────────────────────────────────────

group "4. concurrent leases never share a bay"
rounds=6
clean_rounds=0
for r in $(seq 1 "$rounds"); do
  for k in 1 2 3; do "$REPO/scripts/bays/lease" "r$r-$k" >"$WORK/out.$k" 2>/dev/null & done
  wait
  got=$(cat "$WORK/out.1" "$WORK/out.2" "$WORK/out.3" | sort -u | grep -c .)
  [[ $got -eq 3 ]] && clean_rounds=$((clean_rounds + 1))
  release_all
done
is "three racing leases take three distinct bays, every round" "$rounds" "$clean_rounds"
for k in 1 2 3 4; do
  ("$REPO/scripts/bays/lease" "race-$k" >"$WORK/out.$k" 2>/dev/null; echo $? >"$WORK/rc.$k") &
done
wait
wins=$(cat "$WORK/rc.1" "$WORK/rc.2" "$WORK/rc.3" "$WORK/rc.4" | grep -c '^0$')
is "four racing leases: exactly three win" 3 "$wins"
is "and the winners hold distinct bays" 3 "$(cat "$WORK"/out.[1-4] | sort -u | grep -c .)"
release_all

# ── 5. Cargo.lock decides whether a bay's target/ survives ───────────────────────────────────────

group "5. Cargo.lock decides whether a bay's target/ survives"
mkdir -p "$POOL/bay-1/target"
echo warm >"$POOL/bay-1/target/marker"
pk=$(lease k)
is "same Cargo.lock: the target survives the recycle" warm "$(cat "$pk/target/marker" 2>/dev/null)"
release bay-1
echo lock-v2 >"$REPO/Cargo.lock"
commit_in "$REPO" "dependency bump"
pl=$(lease l)
is "main's Cargo.lock moved: the target is deleted" no "$(exists "$pl/target")"
release_all

# ── 6. the Claude Code hooks keep the harness's contract ─────────────────────────────────────────

group "6. the Claude Code hooks keep the harness's contract"
create_hook() { echo "{\"session_id\":\"s\",\"hook_event_name\":\"WorktreeCreate\",\"name\":\"$1\"}" |
  "$REPO/.claude/hooks/bay-lease.sh" 2>"$ERR"; }
remove_hook() { echo "{\"session_id\":\"s\",\"hook_event_name\":\"WorktreeRemove\",\"worktree_path\":\"$1\"}" |
  "$REPO/.claude/hooks/bay-release.sh" 2>"$ERR"; }
is "WorktreeCreate prints the bay's path and nothing else on stdout" "$POOL/bay-1" "$(create_hook hk1)"
create_hook hk2 >/dev/null
create_hook hk3 >/dev/null
create_hook hk4 >/dev/null
says "WorktreeCreate fails when no bay is free, so the spawn fails" no $?
echo unsaved >"$POOL/bay-2/scratch.txt"
remove_hook "$POOL/bay-2"
says "WorktreeRemove fails for a bay holding work" no $?
is "and that bay stays leased" hk2 "$(leased_to bay-2)"
mkdir -p "$POOL/bay-1/target"
echo warm >"$POOL/bay-1/target/marker"
remove_hook "$POOL/bay-1"
says "WorktreeRemove releases a clean bay" yes $?
is "and leaves it in place, free, with its target/" "free warm" \
  "$(leased_to bay-1 || echo free) $(cat "$POOL/bay-1/target/marker" 2>/dev/null)"
remove_hook "$WORK"
says "WorktreeRemove refuses a path that is not a bay" no $?

printf '\n%d passed, %d failed\n' "$passed" "$failed"
[[ $failed -eq 0 ]]
