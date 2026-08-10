#!/bin/sh
# test_pushed_assets.sh — the assets lane's four questions, driven over synthetic revisions.
#
#     sh scripts/hooks/test_pushed_assets.sh
#
# RUN BY HAND AND BY THE CI ASSETS JOB, which invokes it beside the door's own suites. The pre-push
# hook does not run it: a lane that tested itself on every push would pay a scratch repository per
# push to learn nothing about that push.
#
# It sources `scripts/hooks/pushed_assets.sh` and drives the REAL functions the hook drives — the
# same file, the same shell — because a copy of the discovery rule tested here would be a second
# rule, and the one that ships would be the untested one.
#
# HERMETIC. Every run builds its own git repository under `mktemp -d` and deletes it: no network, no
# remote, no LFS daemon, and nothing read out of this work tree. Remote-tracking refs are written
# with `git update-ref`, and a git-lfs pointer is a few lines of text beside a file placed by hand
# in the scratch repo's own object store — which is exactly what a clone that committed an asset
# holds, and the only thing `assets_hydrate_file` ever reads.

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

# BASE — one asset trio, the shared material library, and some code.
pointer assets/tiger_1/tiger_1.blend "tiger blend v1" place
write   assets/tiger_1/tiger_1.tank.ron "TankSpec(mass: 1.0)"
pointer assets/tiger_1/tiger_1.glb   "tiger model v1" place
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
write assets/lod/tiger_1_link.rung3.glb "a generated rung"
write assets/lod/tiger_1_link.rung3.ron "not a tank sheet"
commit "partial assets"
PARTIAL=$(at HEAD)

# A SECOND VEHICLE, in a directory nobody wrote down.
pointer assets/panther/panther.blend "panther blend v1" place
write   assets/panther/panther.tank.ron "TankSpec(mass: 2.0)"
pointer assets/panther/panther.glb   "panther model v1" place
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
git -C "$REPO" checkout -q "$MAIN"
git -C "$REPO" merge -q --no-ff --no-commit side >/dev/null 2>&1
write assets/tiger_1/tiger_1.tank.ron "TankSpec(mass: 1.5)"   # in neither parent
git -C "$REPO" add -A && git -C "$REPO" commit -q -m "merge side"
MERGE=$(at HEAD)

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

is "a generated model with no blend is not an asset" \
   "" "$(assets_trios "$PARTIAL" | grep 'rung3')"

is "a .ron that is not a .tank.ron is not a spec sheet" \
   "" "$(assets_trios "$PARTIAL" | grep 'assets/lod')"

is "a revision predating any asset holds none" \
   "" "$(git -C "$REPO" hash-object -t tree /dev/null >/dev/null; assets_trios "$(git -C "$REPO" commit-tree "$(git -C "$REPO" hash-object -w -t tree /dev/null)" -m empty </dev/null)")"

is "the list is sorted, byte by byte" \
   "$(assets_trios "$TWO")" "$(assets_trios "$TWO" | LC_ALL=C sort)"

is "a stem names the trio and nothing else" \
   "3" "$(git -C "$REPO" ls-tree -r --name-only "$TWO" | grep -c '^assets/panther/panther\.')"

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
    scripts/tank/glb_ktx2.py \
    scripts/tank/report.py \
    .agents/blender/export_tank.py \
    src/bake.rs \
    src/bake/embedding.rs \
    src/spec.rs \
    src/exact.rs \
    src/substances.rs \
    src/bin/asset_verify.rs \
    src/lib.rs
do
    surface "$path"
    says "$path moves every verdict" yes $?
done

for path in \
    assets/tiger_1/tiger_1.glb \
    assets/tiger_1/tiger_1.blend \
    src/net.rs \
    scripts/tank/test_asset_door.py \
    scripts/lod/chain.py \
    README.md
do
    surface "$path"
    says "$path moves no verdict of its own" no $?
done

is "one shared path in a long changed set is enough" \
   "yes" "$(printf 'README.md\nsrc/net.rs\nsrc/substances.rs\nCargo.lock\n' |
            assets_shared_surface && echo yes || echo no)"

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

is "…with all three siblings, and their real bytes" \
   "tiger blend v1 TankSpec(mass: 1.0) tiger model v1" \
   "$(cat "$SCRATCH/whole/assets/tiger_1/tiger_1.blend" \
          "$SCRATCH/whole/assets/tiger_1/tiger_1.tank.ron" \
          "$SCRATCH/whole/assets/tiger_1/tiger_1.glb" 2>/dev/null | tr '\n' ' ' | sed 's/ $//')"

is "a second vehicle hydrates by the same rule" \
   "panther blend v1 TankSpec(mass: 2.0) panther model v2" \
   "$(assets_hydrate "$PANTHER" assets/panther/panther "$SCRATCH/second" >/dev/null 2>&1
      cat "$SCRATCH/second/assets/panther/panther.blend" \
          "$SCRATCH/second/assets/panther/panther.tank.ron" \
          "$SCRATCH/second/assets/panther/panther.glb" 2>/dev/null | tr '\n' ' ' | sed 's/ $//')"

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

# ── verdict ──────────────────────────────────────────────────────────────────────────────────────

printf '\ntest_pushed_assets ▸ %s cases, %s passed, %s failed\n' \
    "$((passed + failed))" "$passed" "$failed"
[ "$failed" -eq 0 ]
