#!/bin/sh
# test_pushed_assets.sh — the assets lane's four questions, driven over synthetic revisions.
#
#     sh scripts/hooks/test_pushed_assets.sh
#
# RUN BY HAND AND BY THE CI ASSETS JOB, which invokes it beside the door's own suites.
#
# It sources `scripts/hooks/pushed_assets.sh` and drives the REAL functions that ship — the same
# file, the same shell — because a copy of the discovery rule tested here would be a second rule,
# and the one that ships would be the untested one. It also drives the real `pre-push`, whose whole
# remaining claim is that it is the git-lfs upload and nothing else.
#
# HERMETIC, with ONE named exception. Every run builds its own git repository under `mktemp -d` and
# deletes it: no network, no remote, no LFS daemon, and nothing read out of this work tree.
# Remote-tracking refs are written with `git update-ref`, and a git-lfs pointer is a few lines of
# text beside a file placed by hand in the scratch repo's own object store — which is exactly what a
# clone that committed an asset holds, and the only thing `assets_hydrate_file` ever reads.
#
# The exception is the shared-surface coverage case, which reads `scripts/tank/build.py`'s declared
# `PIPELINE_SOURCES` out of this tree. It has to: the thing it proves is that the two lists agree,
# and a synthetic copy of one of them would prove nothing about the pair that ships.

set -u

_here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$_here/pushed_assets.sh"

ZERO=0000000000000000000000000000000000000000

# ── the harness ──────────────────────────────────────────────────────────────────────────────────

passed=0
failed=0

group() { printf '\n%s\n' "$1"; }

ok() { passed=$((passed + 1)); printf '  ok    %s\n' "$1"; }

bad() {
    failed=$((failed + 1))
    printf '  FAIL  %s\n        expected: [%s]\n        actual:   [%s]\n' "$1" "$2" "$3"
}

# The case: a name, what it must produce, and what it produced.
is() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1" "$2" "$3"; fi }

# The same for a predicate: a name, `yes|no`, and the exit status of what was run.
says() {
    if [ "$3" -eq 0 ]; then _said=yes; else _said=no; fi
    is "$1" "$2" "$_said"
}

# ── the scratch repository ───────────────────────────────────────────────────────────────────────

WORK=$(mktemp -d "${TMPDIR:-/tmp}/pushed-assets-test.XXXXXX") || exit 1
trap 'rm -rf "$WORK"' EXIT INT TERM
REPO=$WORK/repo

# One file with one line of content, committed by the caller.
write() {
    mkdir -p "$(dirname "$REPO/$1")"
    printf '%s\n' "$2" > "$REPO/$1"
}

commit() { git -C "$REPO" add -A && git -C "$REPO" commit -q -m "$1"; }

at() { git -C "$REPO" rev-parse "$1"; }

sha256() { if command -v sha256sum >/dev/null; then sha256sum; else shasum -a 256; fi }

# A git-lfs pointer for `content`, with the real object put where this clone would hold it.
# `place` decides whether the object is there at all: the missing-object refusal is a case.
pointer() {   # <path> <content> <place|absent>
    _digest=$(printf '%s\n' "$2" | sha256 | cut -d' ' -f1)
    _size=$(printf '%s\n' "$2" | wc -c | tr -d ' ')
    mkdir -p "$(dirname "$REPO/$1")"
    printf 'version https://git-lfs.github.com/spec/v1\noid sha256:%s\nsize %s\n' \
        "$_digest" "$_size" > "$REPO/$1"
    [ "$3" = place ] || return 0
    _store=$REPO/.git/lfs/objects/$(printf %s "$_digest" | cut -c1-2)/$(printf %s "$_digest" | cut -c3-4)
    mkdir -p "$_store"
    printf '%s\n' "$2" > "$_store/$_digest"
}

git init -q "$REPO"
MAIN=$(git -C "$REPO" symbolic-ref --short HEAD)
git -C "$REPO" config user.email door@overmatch.test
git -C "$REPO" config user.name  door
git -C "$REPO" config commit.gpgsign false

# BASE — one asset trio with the two artifacts the build publishes beside it, the shared material
# library, and some code.
pointer assets/tiger_1/tiger_1.blend "tiger blend v1" place
write   assets/tiger_1/tiger_1.tank.ron "TankSpec(mass: 1.0)"
pointer assets/tiger_1/tiger_1.glb   "tiger model v1" place
pointer assets/tiger_1/tiger_1.sim.glb "tiger sim v1" place
write   assets/tiger_1/tiger_1.lod.json "{}"
pointer assets/materials/materials.blend "material library v1" place
write   assets/materials/materials.ron "SubstanceRegistry()"
write   src/bake.rs "// the consumer contract"
write   src/net.rs  "// the sim, which the door does not read"
write   scripts/tank/asset_door.py "# the door"
commit base
BASE=$(at HEAD)

# NOT ASSETS — one of each way to be two thirds of a trio.
write assets/shell/shell.blend "a workbench blend"
write assets/shell/shell.glb   "a workbench model"
write assets/proto/proto.blend "a source with no model yet"
write assets/proto/proto.tank.ron "TankSpec(mass: 3.0)"
write assets/derived/derived.tank.ron "TankSpec(mass: 4.0)"
write assets/derived/derived.glb "a model with no source"
write assets/staging/candidate.glb "a model nothing sourced"
write assets/staging/candidate.ron "not a tank sheet"
commit "partial assets"
PARTIAL=$(at HEAD)

# A SECOND VEHICLE, in a directory nobody wrote down.
pointer assets/panther/panther.blend "panther blend v1" place
write   assets/panther/panther.tank.ron "TankSpec(mass: 2.0)"
pointer assets/panther/panther.glb   "panther model v1" place
pointer assets/panther/panther.sim.glb "panther sim v1" place
write   assets/panther/panther.lod.json "{}"
commit "a second vehicle"
TWO=$(at HEAD)

# ONE VEHICLE'S OWN BYTES MOVE.
pointer assets/panther/panther.glb "panther model v2" place
commit "re-export the panther"
PANTHER=$(at HEAD)

# THE SHARED SURFACE MOVES, and no asset does.
write src/substances.rs "// the registry's interpreter"
commit "the registry gains an interpreter"
SUBSTANCES=$(at HEAD)

# A MERGE whose only content is the resolution.
git -C "$REPO" checkout -q -b side "$TWO"
write src/net.rs "// the sim, edited on a side branch"
commit "a side branch"
SIDE=$(at side)
git -C "$REPO" checkout -q "$MAIN"
git -C "$REPO" merge -q --no-ff --no-commit side >/dev/null 2>&1
write assets/tiger_1/tiger_1.tank.ron "TankSpec(mass: 1.5)"   # in neither parent
git -C "$REPO" add -A && git -C "$REPO" commit -q -m "merge side"
MERGE=$(at HEAD)

# THE LOD LANE'S LIBRARY MOVES, and no asset does — on a branch of its own so the revisions the rest
# of this file reads are untouched. `scripts/lod/config.py` is inside `build.py`'s `PIPELINE_SOURCES`
# and therefore inside every certificate's `blend_digest`: moving it stales every trio in the tree.
git -C "$REPO" checkout -q -b lodlane "$TWO"
write scripts/lod/config.py "# the ladder's constants"
commit "the ladder's constants move"
LOD_LANE=$(at HEAD)
git -C "$REPO" checkout -q "$MAIN"

# THE CRATE ROOT MOVES, three ways, on a branch of its own: declaring a contract module in,
# gaining a module that is only game code, and dropping a contract module out. Only the first and
# last are exposure.
git -C "$REPO" checkout -q -b liblane "$TWO"
write src/lib.rs "mod bake;
mod net;"
commit "the crate root declares a contract module"
LIB_WIRED=$(at HEAD)
write src/lib.rs "mod bake;
mod net;
mod hud;"
commit "the crate root gains a game module"
LIB_GAME=$(at HEAD)
write src/lib.rs "mod net;
mod hud;"
commit "the crate root drops the contract module"
LIB_UNWIRED=$(at HEAD)
git -C "$REPO" checkout -q "$MAIN"

# DELETIONS, on a branch of their own so the revisions the rest of this file reads still hold the
# trio. Each one removes asset files and nothing puts them back.
git -C "$REPO" checkout -q -b deletions "$TWO"
git -C "$REPO" rm -q assets/panther/panther.glb
commit "delete one file of a trio"
DELETE_FILE=$(at HEAD)
git -C "$REPO" rm -q assets/panther/panther.blend assets/panther/panther.tank.ron
commit "delete the rest of that trio"
DELETE_TRIO=$(at HEAD)
git -C "$REPO" rm -q assets/tiger_1/tiger_1.glb
write src/net.rs "// the sim, edited beside a deletion"
commit "delete an asset file, and change code no verdict reads"
DELETE_MIXED=$(at HEAD)
git -C "$REPO" checkout -q "$MAIN"

# The remote, as this clone knows it: refs only, never a connection.
git -C "$REPO" update-ref "refs/remotes/origin/$MAIN" "$BASE"

cd "$REPO" || exit 1
SCRATCH=$WORK/scratch
mkdir -p "$SCRATCH"

# ── discovery: what a revision HOLDS ─────────────────────────────────────────────────────────────

group "discovery — assets_trios"

is "a sibling trio is an asset" \
   "assets/tiger_1/tiger_1" "$(assets_trios "$BASE")"

is "a second vehicle is a directory, not a line of code" \
   "assets/panther/panther
assets/tiger_1/tiger_1" "$(assets_trios "$TWO")"

is "a blend with no spec sheet is not an asset" \
   "assets/tiger_1/tiger_1" "$(assets_trios "$PARTIAL")"

is "a blend and a sheet with no model is not an asset" \
   "" "$(assets_trios "$PARTIAL" | grep 'proto')"

is "a sheet and a model with no blend is not an asset" \
   "" "$(assets_trios "$PARTIAL" | grep 'derived')"

is "a model with no blend and no sheet is not an asset" \
   "" "$(assets_trios "$PARTIAL" | grep 'candidate')"

is "a .ron that is not a .tank.ron is not a spec sheet" \
   "" "$(assets_trios "$PARTIAL" | grep 'assets/staging')"

is "a revision predating any asset holds none" \
   "" "$(git -C "$REPO" hash-object -t tree /dev/null >/dev/null; assets_trios "$(git -C "$REPO" commit-tree "$(git -C "$REPO" hash-object -w -t tree /dev/null)" -m empty </dev/null)")"

is "the list is sorted, byte by byte" \
   "$(assets_trios "$TWO")" "$(assets_trios "$TWO" | LC_ALL=C sort)"

is "a stem names the source trio and the two artifacts the build publishes, and nothing else" \
   "5" "$(git -C "$REPO" ls-tree -r --name-only "$TWO" | grep -c '^assets/panther/panther\.')"

# ── selection: which trios a push must verify ────────────────────────────────────────────────────

group "selection — assets_changed_trios"

changed_of() { printf '%s\n' "$1" > "$SCRATCH/changed"; assets_trios "$TWO" | assets_changed_trios "$SCRATCH/changed"; }

is "a trio whose blend changed is selected" \
   "assets/tiger_1/tiger_1" "$(changed_of assets/tiger_1/tiger_1.blend)"

is "a trio whose spec sheet changed is selected" \
   "assets/tiger_1/tiger_1" "$(changed_of assets/tiger_1/tiger_1.tank.ron)"

is "a trio whose model changed is selected" \
   "assets/panther/panther" "$(changed_of assets/panther/panther.glb)"

is "a trio nothing of which changed is not selected" \
   "" "$(changed_of src/bake.rs)"

is "a path the stem merely prefixes is not the trio" \
   "" "$(changed_of assets/tiger_1/tiger_1_extra.blend)"

is "a path that merely CONTAINS the trio's is not the trio" \
   "" "$(changed_of assets/tiger_1/tiger_1.blend.orig)"

is "the same path under another root is not the trio" \
   "" "$(changed_of vendor/assets/tiger_1/tiger_1.blend)"

is "a path inside the trio's directory is not the trio" \
   "" "$(changed_of assets/tiger_1/notes.txt)"

group "selection — assets_shared_surface"

surface() { printf '%s\n' "$1" | assets_shared_surface; }

for path in \
    assets/materials/materials.ron \
    assets/materials/materials.blend \
    scripts/toolchain.py \
    scripts/encode-tank-ktx2.sh \
    scripts/tank/asset_door.py \
    scripts/tank/build.py \
    scripts/tank/trio.py \
    scripts/tank/chains.py \
    scripts/tank/glb_ktx2.py \
    scripts/tank/report.py \
    scripts/lod/config.py \
    scripts/lod/measure.py \
    scripts/lod/generate.py \
    assets/maps/kalinovo/level.json \
    src/map.rs \
    .agents/blender/export_tank.py \
    src/bake.rs \
    src/bake/embedding.rs \
    src/spec.rs \
    src/exact.rs \
    src/substances.rs \
    src/bin/asset_verify.rs
do
    surface "$path"
    says "$path moves every verdict" yes $?
done

# EVERY SOURCE THE BUILD DECLARES, READ OUT OF THE BUILD AND FED TO THE REAL PREDICATE. The one case
# in this file that reads THIS work tree rather than the scratch repository, and it has to: a list of
# shared-surface paths kept by hand beside `build.py`'s own `PIPELINE_SOURCES` is a second list, and
# the drift it had was real — the three `scripts/lod` sources `blend_digest` hashes were absent from
# the pattern, so a push moving one of them staled every certificate and skipped every
# `build.py verify`.
#
# The names come out of `build.py` by TEXT rather than by import: importing it drags in numpy and the
# whole door, and this suite is `sh` and git and nothing else.
uncovered=$(sed -n '/^SEARCH_SOURCES = (/,/^)$/p;/^PIPELINE_SOURCES = SEARCH_SOURCES + (/,/^)$/p' \
                "$_here/../tank/build.py" |
            tr ',' '\n' | sed -n 's/^[^"]*"\([^"]*\)".*$/\1/p' |
            while read -r _name; do
                # Relative to `scripts/tank/`, where `sources_digest` resolves them.
                _path=$(printf 'scripts/tank/%s' "$_name" |
                        sed -e ':a' -e 's![^/][^/]*/\.\./!!;ta' -e 's!^\./!!')
                printf '%s\n' "$_path" | assets_shared_surface || printf '%s\n' "$_path"
            done)
is "the predicate covers every source blend_digest hashes" "" "$uncovered"

for path in \
    assets/tiger_1/tiger_1.glb \
    assets/tiger_1/tiger_1.blend \
    assets/maps/kalinovo/derived/map_ui.png \
    src/net.rs \
    src/lib.rs \
    scripts/tank/test_asset_door.py \
    scripts/lod/test_refusals.py \
    README.md
do
    surface "$path"
    says "$path moves no verdict of its own" no $?
done

is "one shared path in a long changed set is enough" \
   "yes" "$(printf 'README.md\nsrc/net.rs\nsrc/substances.rs\nCargo.lock\n' |
            assets_shared_surface && echo yes || echo no)"

group "selection — assets_lib_exposure"

exposure() { printf 'src/lib.rs\n' | assets_lib_exposure "$1" "$2"; }

exposure "$TWO" "$LIB_WIRED"
says "a contract module declared into the crate root is exposure" yes $?

exposure "$LIB_WIRED" "$LIB_GAME"
says "a game module joining the crate root is not" no $?

exposure "$LIB_GAME" "$LIB_UNWIRED"
says "a contract module dropped from the crate root is" yes $?

printf 'src/net.rs\n' | assets_lib_exposure "$LIB_GAME" "$LIB_UNWIRED"
says "a changed set that does not name lib.rs never reads the diff" no $?

printf 'src/lib.rs\n' | assets_lib_exposure "$ZERO" "$LIB_GAME"
says "no resolvable baseline fails toward running" yes $?

is "a crate-root game refactor selects no trio" \
   "" \
   "$(printf 'refs/heads/liblane %s refs/heads/liblane %s\n' "$LIB_GAME" "$LIB_WIRED" |
      assets_push_targets origin "$SCRATCH")"

is "a crate-root contract rewire selects every trio, as shared-surface" \
   "refs/heads/liblane $LIB_UNWIRED assets/panther/panther shared-surface
refs/heads/liblane $LIB_UNWIRED assets/tiger_1/tiger_1 shared-surface" \
   "$(printf 'refs/heads/liblane %s refs/heads/liblane %s\n' "$LIB_UNWIRED" "$LIB_GAME" |
      assets_push_targets origin "$SCRATCH")"

# ── the ref list: every (revision, asset) pair a push must verify ────────────────────────────────

group "ref list — assets_pushed_commit and assets_pushed_paths"

assets_pushed_commit "$BASE"; says "a commit this clone holds resolves" yes $?
assets_pushed_commit "$ZERO"; says "the all-zero sha of a deleted ref resolves to nothing" no $?
assets_pushed_commit "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
says "a sha this clone does not have resolves to nothing" no $?
assets_pushed_commit ""; says "an empty sha resolves to nothing" no $?

is "a known remote sha is an exact two-tree diff" \
   "assets/panther/panther.glb" \
   "$(assets_pushed_paths "$PANTHER" "$TWO" origin)"

# Every commit past the remote's tip, not just the last one: `shell.blend` and `panther.glb` were
# written two commits apart, and both are in the push.
is "a ref the remote does not have is measured against everything it holds" \
   "assets/panther/panther.glb assets/shell/shell.blend" \
   "$(assets_pushed_paths "$PANTHER" "$ZERO" origin | LC_ALL=C sort -u |
      grep -e 'panther\.glb$' -e 'shell\.blend$' | tr '\n' ' ' | sed 's/ $//')"

is "a merge's own resolution is in the push and in the diff" \
   "assets/tiger_1/tiger_1.tank.ron" \
   "$(assets_pushed_paths "$MERGE" "$ZERO" origin | grep 'tiger_1.tank.ron' | head -1)"

is "a clone that knows no remote refs yields the whole history" \
   "assets/tiger_1/tiger_1.blend" \
   "$(assets_pushed_paths "$BASE" "$ZERO" nosuchremote | LC_ALL=C sort -u | grep 'tiger_1.blend')"

group "ref list — assets_push_targets"

targets() { printf '%s\n' "$1" | assets_push_targets origin "$SCRATCH"; }

is "an ordinary push verifies the trios it changed" \
   "refs/heads/main $PANTHER assets/panther/panther changed" \
   "$(targets "refs/heads/main $PANTHER refs/heads/main $TWO")"

is "a push that moves the shared surface verifies every trio" \
   "refs/heads/main $SUBSTANCES assets/panther/panther shared-surface
refs/heads/main $SUBSTANCES assets/tiger_1/tiger_1 shared-surface" \
   "$(targets "refs/heads/main $SUBSTANCES refs/heads/main $PANTHER")"

is "a ref being deleted verifies nothing" \
   "" "$(targets "refs/heads/gone $ZERO refs/heads/gone $TWO")"

is "…and says nothing: git is never asked about the revision that is not there" \
   "" "$(printf 'refs/heads/gone %s refs/heads/gone %s\n' "$ZERO" "$TWO" |
         assets_push_targets origin "$SCRATCH" 2>&1 >/dev/null)"

is "a local sha this clone cannot resolve verifies nothing" \
   "" "$(targets "refs/heads/x deadbeefdeadbeefdeadbeefdeadbeefdeadbeef refs/heads/x $TWO")"

is "…and says nothing either" \
   "" "$(printf 'refs/heads/x deadbeefdeadbeefdeadbeefdeadbeefdeadbeef refs/heads/x %s\n' "$TWO" |
         assets_push_targets origin "$SCRATCH" 2>&1 >/dev/null)"

is "a push that moves the LOD lane's library verifies every trio" \
   "refs/heads/lodlane $LOD_LANE assets/panther/panther shared-surface
refs/heads/lodlane $LOD_LANE assets/tiger_1/tiger_1 shared-surface" \
   "$(targets "refs/heads/lodlane $LOD_LANE refs/heads/lodlane $TWO")"

is "a push that changed no trio and no shared surface verifies nothing" \
   "" "$(targets "refs/heads/main $PARTIAL refs/heads/main $BASE")"

is "a revision holding no trio verifies nothing" \
   "" "$(targets "refs/heads/x $(git commit-tree "$(git hash-object -w -t tree /dev/null)" -m empty </dev/null) refs/heads/x $ZERO")"

is "several refs at once each contribute their own lines" \
   "refs/heads/main $PANTHER assets/panther/panther changed
refs/tags/v1 $SUBSTANCES assets/panther/panther shared-surface
refs/tags/v1 $SUBSTANCES assets/tiger_1/tiger_1 shared-surface" \
   "$(printf 'refs/heads/main %s refs/heads/main %s\nrefs/tags/v1 %s refs/tags/v1 %s\n' \
        "$PANTHER" "$TWO" "$SUBSTANCES" "$PANTHER" | assets_push_targets origin "$SCRATCH")"

is "a new ref carries its own local ref name" \
   "refs/heads/topic" \
   "$(targets "refs/heads/topic $PANTHER refs/heads/topic $ZERO" | head -1 | cut -d' ' -f1)"

# ── the CI gate: whether a RANGE can have moved any verdict ──────────────────────────────────────
#
# CI's assets lane pays a MEASURED ~35 minutes to re-cut every trio from Blender. This is what tells
# it not to, and the whole of what it may skip on. Every case below drives the real function over a
# real range in the scratch repository; nothing about the decision lives in the workflow.

group "the CI gate — assets_range_affected"

affected() { assets_range_affected "$1" "$2" "$SCRATCH" >/dev/null 2>&1; }
because() { assets_range_affected "$1" "$2" "$SCRATCH" 2>/dev/null; }

affected "$TWO" "$PANTHER"
says "a range that re-exports a trio is affected" yes $?

affected "$PANTHER" "$SUBSTANCES"
says "a range that moves the shared surface is affected" yes $?

affected "$TWO" "$LOD_LANE"
says "a range that moves the LOD lane's library is affected" yes $?

affected "$BASE" "$PARTIAL"
says "a range of paths no verdict is computed from is unaffected" no $?

is "…and says so, and says what it looked at" \
   "yes" \
   "$(because "$BASE" "$PARTIAL" | grep -q '^assets ▸ unaffected:.*no asset trio and no shared surface' &&
      echo yes || echo no)"

# DELETION IS A CHANGE, and the head holds none of the evidence for it. A range whose deletion is
# read off the head's own listing selects nothing, reports unaffected, and the lane that would have
# refused a tree with no trio left in it never runs.
affected "$TWO" "$DELETE_FILE"
says "a range that only deletes one file of a trio is affected" yes $?

affected "$TWO" "$DELETE_TRIO"
says "a range that deletes a whole trio is affected" yes $?

affected "$DELETE_TRIO" "$DELETE_MIXED"
says "a range that deletes an asset file beside a code change is affected" yes $?

is "…and a deletion names the trio it removed" \
   "yes" \
   "$(because "$TWO" "$DELETE_TRIO" | grep -q 'assets/panther/panther' && echo yes || echo no)"

is "an affected range names what it found" \
   "yes" \
   "$(because "$TWO" "$PANTHER" | grep -q 'assets/panther/panther' && echo yes || echo no)"

# EVERY WAY OF NOT KNOWING RUNS THE LANE. A gate that skips when it cannot see is worse than no
# gate: it reports success over the one push nobody looked at.
affected "$ZERO" "$PANTHER"
says "a zero baseline — a branch's first push — is affected" yes $?

affected "" "$PANTHER"
says "an absent baseline is affected" yes $?

affected "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef" "$PANTHER"
says "a baseline this clone cannot resolve is affected" yes $?

affected "$BASE" "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
says "a head this clone cannot resolve is affected" yes $?

affected "$BASE" ""
says "an absent head is affected" yes $?

ORPHAN=$(git commit-tree "$(git hash-object -w -t tree /dev/null)" -m orphan </dev/null)
affected "$ORPHAN" "$PANTHER"
says "a baseline with no merge base — a force-push onto another history — is affected" yes $?

# THE BASELINE IS THE MERGE BASE. `$SIDE` branched at `$TWO` and changed only `src/net.rs`; `$PANTHER`
# re-exported an asset on the main line after it. A two-tree diff of the two tips reports that asset
# as changed — it differs between them — and would run the lane for somebody else's commit, printing
# a reason that is not true of this range.
is "a base branch that moved under the range does not become this range's change" \
   "no" \
   "$(affected "$PANTHER" "$SIDE" && echo yes || echo no)"

is "…while the two-tree diff of the same pair does see the asset" \
   "assets/panther/panther.glb" \
   "$(git diff --name-only "$PANTHER" "$SIDE" | grep 'panther')"

group "the CI gate — assets_ci_scope"

scope() {   # <event> <base> <head>; prints the decision, writes $SCRATCH/github-output
    : > "$SCRATCH/github-output"
    (
        GITHUB_EVENT_NAME=$1 ASSETS_BASE=$2 ASSETS_HEAD=$3 GITHUB_OUTPUT=$SCRATCH/github-output
        export GITHUB_EVENT_NAME ASSETS_BASE ASSETS_HEAD GITHUB_OUTPUT
        assets_ci_scope 2>/dev/null | tail -1
    )
}

is "a push whose range moves a trio is affected" \
   "true" "$(scope push "$TWO" "$PANTHER")"

is "a push whose range moves nothing a verdict reads is not" \
   "false" "$(scope push "$BASE" "$PARTIAL")"

is "…and the step reads that decision off GITHUB_OUTPUT, not off stdout" \
   "affected=false" "$(scope push "$BASE" "$PARTIAL" >/dev/null; cat "$SCRATCH/github-output")"

# A hand-started run is what bounds how long a defect the gate could not see survives (the weekly
# cron that used to is retired), so it never consults a range at all — there is no baseline on a
# dispatch, and it must not need one. `schedule` keeps answering the same way: no cron triggers CI
# today, and the answer must not become "skip" if one is ever added back.
is "a hand-started run is affected with no range whatsoever" \
   "true" "$(scope workflow_dispatch "" "")"

is "a scheduled run would be too" \
   "true" "$(scope schedule "" "")"

# ── hydration: the bytes of the pushed revision, never of the work tree ──────────────────────────

group "hydration — assets_lfs_object, assets_hydrate_file, assets_hydrate"

OID=abcd000000000000000000000000000000000000000000000000000000000000
is "an object's path is its own first four hex digits" \
   "$(git rev-parse --git-common-dir)/lfs/objects/ab/cd/$OID" \
   "$(assets_lfs_object "$OID")"

assets_hydrate_file "$BASE" assets/tiger_1/tiger_1.tank.ron "$SCRATCH/sheet.ron" 2>/dev/null
says "a small non-pointer blob hydrates" yes $?
is "…verbatim" "TankSpec(mass: 1.0)" "$(cat "$SCRATCH/sheet.ron")"

assets_hydrate_file "$BASE" assets/tiger_1/tiger_1.glb "$SCRATCH/model.glb" 2>/dev/null
says "a pointer hydrates through this clone's object store" yes $?
is "…as the real bytes, not the pointer" "tiger model v1" "$(cat "$SCRATCH/model.glb")"

is "the revision's bytes, never the work tree's" \
   "tiger model v1" \
   "$(printf 'the work tree, edited\n' > assets/tiger_1/tiger_1.glb
      assets_hydrate_file "$BASE" assets/tiger_1/tiger_1.glb "$SCRATCH/tree.glb" 2>/dev/null
      cat "$SCRATCH/tree.glb"
      git checkout -q -- assets/tiger_1/tiger_1.glb)"

is "an earlier revision's bytes are that revision's" \
   "panther model v1" \
   "$(assets_hydrate_file "$TWO" assets/panther/panther.glb "$SCRATCH/old.glb" 2>/dev/null
      cat "$SCRATCH/old.glb")"

assets_hydrate_file "$BASE" assets/panther/panther.glb "$SCRATCH/absent.glb" 2>/dev/null
says "a path the revision does not hold refuses" no $?

is "…naming the file and the revision" \
   "yes" \
   "$(assets_hydrate_file "$BASE" assets/panther/panther.glb "$SCRATCH/absent.glb" 2>&1 >/dev/null |
      grep -q "assets/panther/panther.glb is not in $BASE" && echo yes || echo no)"

pointer assets/tiger_1/tiger_1.glb "tiger model v3, uploaded nowhere" absent
commit "an asset whose object never left the machine that made it"
UNFETCHED=$(at HEAD)
assets_hydrate_file "$UNFETCHED" assets/tiger_1/tiger_1.glb "$SCRATCH/gone.glb" 2>/dev/null
says "a pointer whose object is not in this clone refuses" no $?

is "…naming the object and the way out" \
   "yes" \
   "$(assets_hydrate_file "$UNFETCHED" assets/tiger_1/tiger_1.glb "$SCRATCH/gone.glb" 2>&1 >/dev/null |
      grep -q 'git lfs fetch' && echo yes || echo no)"

is "a whole asset hydrates as the trio the door reads" \
   "assets/tiger_1/tiger_1.blend" \
   "$(assets_hydrate "$BASE" assets/tiger_1/tiger_1 "$SCRATCH/whole" 2>/dev/null |
      sed "s|$SCRATCH/whole/||")"

is "…beside the shared material library its source links" \
   "material library v1" "$(cat "$SCRATCH/whole/assets/materials/materials.blend" 2>/dev/null)"

is "…and the registry half of that library, from the same revision" \
   "SubstanceRegistry()" "$(cat "$SCRATCH/whole/assets/materials/materials.ron" 2>/dev/null)"

is "…with every sibling the build reads and publishes, and their real bytes" \
   "tiger blend v1 TankSpec(mass: 1.0) tiger model v1 tiger sim v1 {}" \
   "$(cat "$SCRATCH/whole/assets/tiger_1/tiger_1.blend" \
          "$SCRATCH/whole/assets/tiger_1/tiger_1.tank.ron" \
          "$SCRATCH/whole/assets/tiger_1/tiger_1.glb" \
          "$SCRATCH/whole/assets/tiger_1/tiger_1.sim.glb" \
          "$SCRATCH/whole/assets/tiger_1/tiger_1.lod.json" 2>/dev/null | tr '\n' ' ' | sed 's/ $//')"

is "a second vehicle hydrates by the same rule" \
   "panther blend v1 TankSpec(mass: 2.0) panther model v2 panther sim v1 {}" \
   "$(assets_hydrate "$PANTHER" assets/panther/panther "$SCRATCH/second" >/dev/null 2>&1
      cat "$SCRATCH/second/assets/panther/panther.blend" \
          "$SCRATCH/second/assets/panther/panther.tank.ron" \
          "$SCRATCH/second/assets/panther/panther.glb" \
          "$SCRATCH/second/assets/panther/panther.sim.glb" \
          "$SCRATCH/second/assets/panther/panther.lod.json" 2>/dev/null | tr '\n' ' ' | sed 's/ $//')"

assets_hydrate "$UNFETCHED" assets/tiger_1/tiger_1 "$SCRATCH/broken" >/dev/null 2>&1
says "an asset with one unfetchable file refuses whole" no $?

# The registry is DATA, so it is the pushed revision's registry or it is nobody's: a lane that
# hydrated an older trio and read today's substance numbers would certify a pair that never existed.
write assets/materials/materials.ron "SubstanceRegistry(edited)"
# …and the trio is whole again, so the two absence cases below refuse for the reason they name
# rather than for the unfetchable model the case above left at HEAD.
pointer assets/tiger_1/tiger_1.glb "tiger model v4" place
commit "the registry moves"
is "an earlier revision's registry is that revision's" \
   "SubstanceRegistry()" \
   "$(assets_hydrate "$BASE" assets/tiger_1/tiger_1 "$SCRATCH/oldreg" >/dev/null 2>&1
      cat "$SCRATCH/oldreg/assets/materials/materials.ron" 2>/dev/null)"

git rm -q --cached assets/materials/materials.ron >/dev/null
rm -f assets/materials/materials.ron
git commit -q -m "a revision without the substance registry"
assets_hydrate "$(at HEAD)" assets/tiger_1/tiger_1 "$SCRATCH/noregistry" >/dev/null 2>&1
says "a revision without the substance registry refuses" no $?

git rm -q --cached assets/materials/materials.blend >/dev/null
rm -f assets/materials/materials.blend
git commit -q -m "a revision without the canonical material library"
assets_hydrate "$(at HEAD)" assets/tiger_1/tiger_1 "$SCRATCH/nolib" >/dev/null 2>&1
says "a revision without the material library refuses" no $?

# ── tracking: which blends a revision can hold at all ────────────────────────────────────────────
#
# Discovery can only find a trio a revision HOLDS, so the lane's generality ends at `.gitignore`: a
# second vehicle whose source is ignored is discovered by nothing and verified by nobody. The law is
# LOCATION — `assets/<id>/<id>.blend` is canonical, everything else blend-shaped is authoring
# scratch — and it is driven over the REAL file, in a repository of its own, because `git
# check-ignore` is the only thing that answers what git would do with a path.
#
# `.gitignore` states the FIRST half of that and cannot state the second: `!assets/*/*.blend`
# unignores a directory's whole blend population, so it lets `assets/tiger_1/scratch.blend` be
# tracked as readily as the source beside it. The shape law is the lane's, driven here over pushed
# revisions.

group "tracking — the canonical shape, in the lane"

# The revisions built above: canonical trios, plus `assets/shell/shell.blend` and
# `assets/proto/proto.blend`, which are canonical in shape and merely not trios.
assets_blend_shape "$BASE" 2>/dev/null
says "a revision of canonical sources passes" yes $?

assets_blend_shape "$PARTIAL" 2>/dev/null
says "…including blends that are no trio but are named for their directory" yes $?

write assets/tiger_1/scratch.blend "an authoring scratch file, in a canonical directory"
commit "a scratch blend beside a source"
STRAY=$(at HEAD)
assets_blend_shape "$STRAY" 2>/dev/null
says "a second blend in an asset's own directory refuses the push" no $?

is "…naming the file and the shape it is not" \
   "yes" \
   "$(assets_blend_shape "$STRAY" 2>&1 >/dev/null |
      grep -q 'assets/tiger_1/scratch.blend' && echo yes || echo no)"

git rm -q assets/tiger_1/scratch.blend
write assets/tiger_1/backup/tiger_1.blend "a backup, one directory too deep"
commit "a blend under a subdirectory of an asset"
assets_blend_shape "$(at HEAD)" 2>/dev/null
says "a blend below assets/<id>/ refuses the push" no $?

git rm -q assets/tiger_1/backup/tiger_1.blend
write scratch/panther/panther.blend "the canonical shape, in the wrong tree"
commit "a blend named for its directory, outside assets/"
assets_blend_shape "$(at HEAD)" 2>/dev/null
says "the canonical shape outside assets/ is not canonical" no $?

git rm -q scratch/panther/panther.blend
write panther.blend "a blend at the top of the tree"
commit "a blend outside assets/"
assets_blend_shape "$(at HEAD)" 2>/dev/null
says "a blend outside assets/ refuses the push" no $?

git rm -q panther.blend
commit "the tree is canonical again"
assets_blend_shape "$(at HEAD)" 2>/dev/null
says "…and the revision that removes it passes again" yes $?

group "tracking — the repository's own .gitignore"

IGNORE=$WORK/ignore
git init -q "$IGNORE"
cp "$_here/../../.gitignore" "$IGNORE/.gitignore"

ignored() { git -C "$IGNORE" check-ignore -q "$1"; }

for path in \
    assets/tiger_1/tiger_1.blend \
    assets/materials/materials.blend \
    assets/panther/panther.blend
do
    ignored "$path"
    says "$path is version-controlled" no $?
done

for path in \
    assets/tiger_1/tiger_1.blend1 \
    assets/tiger_1/tiger_1.blend.pre-weld.bak \
    assets/tiger_1/backup/tiger_1.blend \
    scratch/panther.blend \
    panther.blend
do
    ignored "$path"
    says "$path is authoring scratch" yes $?
done

# ── the hook itself: the LFS transport, and nothing else ─────────────────────────────────────────
#
# The REAL `scripts/hooks/pre-push`, run over the scratch repository with an EMPTY ref list, and
# what is measured is which commands it ran. The claim is a closed one: the hook is the git-lfs
# upload alone. Every other verdict — fmt, clippy, the asset door, the suite — is CI's, post-hoc,
# on the pushed commit.
#
# `cargo`, `python3` and `git-lfs` are stood in for by shims that record their arguments and exit 0.
# A SHIM ONLY SEES WHAT IT STANDS IN FOR, so the behavioural cases below catch the lanes that were
# retired and nothing else — a `curl` or a bare `git status` added to the hook would run unseen.
# The last case closes that: it reads the hook's own TEXT and allows exactly four executable lines.

group "the hook — the LFS transport, and nothing else"

HOOK_BIN=$WORK/bin
LANE_LOG=$WORK/lanes.log
export LANE_LOG
mkdir -p "$HOOK_BIN"
for _tool in cargo python3 git-lfs; do
    printf '#!/bin/sh\nprintf "%%s %%s\\n" "$(basename "$0")" "$*" >> "$LANE_LOG"\nexit 0\n' \
        > "$HOOK_BIN/$_tool"
    chmod +x "$HOOK_BIN/$_tool"
done
# The hook sources this beside itself, out of the work tree it is run in.
mkdir -p "$REPO/scripts/hooks"
cp "$_here/pushed_assets.sh" "$REPO/scripts/hooks/pushed_assets.sh"

# One hook run with an empty ref list. Prints every shimmed command it ran, one per line.
hook() {   # <env assignment>…
    : > "$LANE_LOG"
    ( cd "$REPO" && PATH=$HOOK_BIN:$PATH env "$@" sh "$_here/pre-push" origin \
        </dev/null > "$WORK/hook.out" 2>&1 ) || printf 'HOOK EXITED %s\n' "$?"
    cut -d' ' -f1-2 < "$LANE_LOG"
}

is "the hook runs the lfs upload and nothing else" \
   "git-lfs pre-push" "$(hook OVERMATCH_LANE_PROBE=1)"

# The transport is not a lane and cannot be named off: the retired switches are inert vocabulary
# now, and a push that sets them still uploads its objects.
is "…even when the retired lane switches are set" \
   "git-lfs pre-push" \
   "$(hook OVERMATCH_SKIP=lfs,fmt,clippy,assets,test OVERMATCH_FULL=1)"

# `git lfs pre-push` takes the remote as its argument; the hook passes its own through untouched.
is "…and hands git-lfs the remote it was called with" \
   "git-lfs pre-push origin" \
   "$(hook OVERMATCH_LANE_PROBE=1 >/dev/null; cut -d' ' -f1-3 < "$LANE_LOG")"

is "…and says which transport it is" \
   "yes" \
   "$(hook OVERMATCH_LANE_PROBE=1 >/dev/null
      grep -q 'pre-push ▸ git lfs pre-push' "$WORK/hook.out" && echo yes || echo no)"

# THE CLOSED HALF, and the reason it is a text test: the shims above can only report commands they
# were written to stand in for. This lists every line of the hook that is not a comment and not
# blank, minus the exactly four the contract allows — the `set -e`, the announcement, the upload,
# and the shebang (a comment by shape). Anything else a lane brings back, shimmed or not, prints
# here and fails. A deliberate fourth line means editing this list and saying why.
is "the hook's TEXT holds nothing but those four lines" \
   "" \
   "$(grep -v '^[[:space:]]*#' "$_here/pre-push" |
      grep -v '^[[:space:]]*$' |
      grep -vx 'set -e' |
      grep -vx 'echo "pre-push ▸ git lfs pre-push"' |
      grep -vx 'git lfs pre-push "$@"')"

# ── verdict ──────────────────────────────────────────────────────────────────────────────────────

printf '\ntest_pushed_assets ▸ %s cases, %s passed, %s failed\n' \
    "$((passed + failed))" "$passed" "$failed"
[ "$failed" -eq 0 ]
