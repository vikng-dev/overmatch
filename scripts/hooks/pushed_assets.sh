# pushed_assets.sh — what a push contains, read as ASSETS.
#
# Sourced by CI's assets job, and by nothing else that ships. Every function here answers a
# question about a REVISION or a RANGE OF REVISIONS, never about the work tree: the work tree is a
# different, mutable thing from the bytes a commit holds, and a gate that blesses the former while
# shipping the latter certifies nothing.
#
# The pre-push hook USED to source this file for a local asset lane; that lane is retired and the
# hook is the git-lfs upload alone. The per-push selection and hydration functions
# (`assets_pushed_commit`, `assets_pushed_paths`, `assets_push_targets`, `assets_hydrate`)
# therefore have no caller today outside `test_pushed_assets.sh`, which still drives them: they are
# what a lane that verifies only what a push CHANGED is built from, and the CI lane's own scoping
# is the same predicates — the lanes and the trio rule — asked over a range.
#
# The vocabulary is one word: a STEM. An asset is a sibling trio in one directory —
# `<id>.blend`, a SHEET (`<id>.tank.ron` for a vehicle, `<id>.tree.ron` for a tree) and
# `<id>.glb` — and the stem is their common path prefix, `assets/tiger_1/tiger_1` or
# `assets/trees/fir_1/fir_1`, which names the whole trio and nothing else. The sheet's extension
# names the build that verifies the trio (`assets_trio_lane`). No path is written down anywhere in
# this file; a second vehicle or a second tree is discovered by the same rule that found the first.
#
# These live in their own file so the exercise script can source them and drive the real code.

# ── what the push is ─────────────────────────────────────────────────────────────────────────────

# A ref line's local sha, when it is a commit this clone holds. The all-zero sha git sends for a ref
# being DELETED resolves to nothing, exactly like a sha this clone does not have, so one test covers
# both: there is no revision to verify either way.
assets_pushed_commit() {   # <local_sha>
    git cat-file -e "${1:-}^{commit}" 2>/dev/null
}

# The paths a push CHANGES on one ref.
#
# The remote's own sha is the baseline when the remote already has the ref, which is the ordinary
# push and an exact two-tree diff. A ref the remote does not have yet (all zeros, or an object this
# clone cannot resolve) has no such baseline, so the baseline becomes everything the remote already
# holds ANYWHERE: `--not --remotes=<remote>`. A clone that knows no remote refs at all therefore
# yields the whole history — every asset gets verified, which is the safe direction to be wrong in.
#
# `--diff-merges=first-parent` is what makes a merge commit's own content visible; without it a
# change made only in the merge resolution is in the push and in no listed diff.
assets_pushed_paths() {   # <local_sha> <remote_sha> <remote>
    if assets_pushed_commit "${2:-}"; then
        git diff --name-only "$2" "$1"
    else
        git log --format= --name-only --diff-merges=first-parent "$1" --not --remotes="$3"
    fi
}

# ── what the push holds ──────────────────────────────────────────────────────────────────────────

# Every asset in one revision, as stems, sorted. The rule IS the trio: a `.blend` whose sibling
# sheet (`.tank.ron` or `.tree.ron`) and `.glb` are both there too. A blend with no sheet
# (`assets/shell/`) is not an asset, and a glb with no blend beside it is not one either; neither
# is discovered.
assets_trios() {   # <rev>
    git ls-tree -r --name-only "$1" | awk '
        /\.blend$/     { blend[substr($0, 1, length($0) - 6)] = 1; next }
        /\.tank\.ron$/ { sheet[substr($0, 1, length($0) - 9)] = 1; next }
        /\.tree\.ron$/ { sheet[substr($0, 1, length($0) - 9)] = 1; next }
        /\.glb$/       { model[substr($0, 1, length($0) - 4)] = 1 }
        END { for (stem in blend) if (stem in sheet && stem in model) print stem }
    ' | LC_ALL=C sort
}

# Which build verifies a trio: `tank` for a `.tank.ron` sheet, `tree` for a `.tree.ron` one. The
# sheet's extension is the whole dispatch — no directory name is read — and a stem with neither
# sheet is no trio, so it names no lane and the call fails.
assets_trio_lane() {   # <rev> <stem>
    if git cat-file -e "$1:$2.tank.ron" 2>/dev/null; then
        printf 'tank\n'
    elif git cat-file -e "$1:$2.tree.ron" 2>/dev/null; then
        printf 'tree\n'
    else
        return 1
    fi
}

# Every blend a revision holds is at `assets/<id>/<id>.blend` or `assets/trees/<id>/<id>.blend`,
# or the push is refused.
#
# The canonical location is a directory named by the blend it holds — directly under `assets/` for
# a vehicle (the shape `assets/materials/materials.blend` already has) and under `assets/trees/`
# for a tree — the shapes the trio rule looks for and every derived path is cut from.
# `assets/trees/` itself is the tree lane's directory, so `assets/trees/trees.blend` is not a
# vehicle named after it. `.gitignore`
# cannot state that: its negations are broad by construction (`!assets/*/*.blend` unignores
# `assets/tiger_1/scratch.blend` as readily as the source beside it, `!/assets/trees/*/*.blend`
# likewise one level down), and a pattern language with no backreference has no way to say "named
# after its directory". So the law lives here, where it is executable, and the lane refuses a push
# carrying a blend of any other shape rather than letting an authoring scratch file be tracked
# forever as though it were a source.
assets_blend_shape() {   # <rev>
    _stray=$(git ls-tree -r --name-only "$1" | awk '
        /\.blend$/ {
            depth = split($0, part, "/")
            stem = substr(part[depth], 1, length(part[depth]) - 6)
            vehicle = depth == 3 && part[1] == "assets" && part[2] != "trees" && stem == part[2]
            tree = depth == 4 && part[1] == "assets" && part[2] == "trees" && stem == part[3]
            if (!vehicle && !tree) print
        }
    ')
    [ -n "$_stray" ] || return 0
    printf '\033[31m  a tracked blend is not a canonical source\033[0m — %s\n' \
        "a tracked blend is exactly assets/<id>/<id>.blend or assets/trees/<id>/<id>.blend" >&2
    printf '%s\n' "$_stray" | sed 's/^/    /' >&2
    printf '  move it out of the asset directory, or name it after the directory it is in — every\n' >&2
    printf '  other blend is authoring scratch, and .gitignore unignores these shapes too broadly to say so\n' >&2
    return 1
}

# The stems on stdin whose trio has a file in the changed set.
assets_changed_trios() {   # <changed-paths-file>; stems on stdin
    while read -r _stem; do
        if grep -qxF -e "$_stem.blend" -e "$_stem.tank.ron" -e "$_stem.tree.ron" -e "$_stem.glb" \
                     -e "$_stem.sim.glb" -e "$_stem.lod.json" "$1"; then
            printf '%s\n' "$_stem"
        fi
    done
}

# THE SHARED SURFACE: what a build reads besides a trio's own files, split by the build that reads
# it. A path in a lane moving means that build now answers differently about bytes that did not
# move, so every trio of that lane is re-verified; a path both builds read re-verifies both.
#
# Each path sits in the lane whose build READS it — an import, a subprocess, a file opened:
#
#   tank  `scripts/tank/{build,trio,chains,asset_door,glb_ktx2}.py` and `scripts/lod/{config,
#         measure,generate}.py` (the build, its door and its search); `scripts/encode-tank-ktx2.sh`
#         and `.agents/blender/export_tank.py` (the door's encoder and source pass); `src/map.rs`
#         and every map's `level.json` (`scripts/lod/config.py` parses `RIGHT_WALL_M` from them).
#   tree  `scripts/prop/` (the build, the exporter it runs in Blender, the generator, and
#         `test_prop.py`, which runs in this lane and nowhere else) and `assets/trees/species.ron`
#         (the build's species table).
#   both  `assets/materials/` (the tank source links `materials.blend` and the tank build reads
#         `materials.ron`; the tree build reads neither, and the library stays in both as the
#         contract does, a tree re-verify being the cheap direction to be wrong);
#         `scripts/toolchain.py` (both builds import it) and
#         `scripts/tank/report.py` (`scripts/toolchain.py` imports it); the Rust consumer contract
#         with the modules it certifies against, `src/{bake,spec,exact,substances}*` and
#         `src/bin/asset_verify.rs`. The contract is in both by decision rather than by a read: only
#         the tank door runs `asset_verify`, and a tree re-verify is the cheap direction to be wrong.
#
# FAIL TOWARD RUNNING: a path that cannot be placed in one lane belongs in both.
#
# `src/substances.rs` is in the contract because it INTERPRETS the registry: it decides what the
# shared material data means to the gate and hands the canon file its key list, so it moves verdicts
# exactly as `assets/materials/` does. `src/bin/asset_verify.rs` is in it because it is the
# contract's EXECUTABLE surface: the adapter owns the exit status and the report the lane reads — a
# push that moves only it changes verdicts while touching no law.
#
# `src/lib.rs` is NOT in this list. The crate root is where the contract modules are wired into the
# crate at all, but it is also the whole game's root and moves on nearly every refactor — as a path
# it would re-cut every asset for pushes that never went near the contract. It is read by CONTENT
# instead: `assets_lib_exposure`, below.
#
# THE LANES ARE THE BUILDS' OWN DECLARED DEPENDENCIES, NOT A SEPARATE OPINION. Each build's
# `PIPELINE_SOURCES` names what can move its artifacts, and `test_pushed_assets.sh` holds every
# entry of `scripts/tank/build.py`'s list to the tank lane and of `scripts/prop/build.py`'s to the
# tree lane.
#
# The map pattern takes every map's `level.json` rather than resolving the default: which map is
# default is `src/map.rs`'s answer, and this predicate refuses to be a second reader of it.
ASSETS_SURFACE_TANK='^(assets/maps/[^/]+/level\.json$|scripts/encode-tank-ktx2\.sh$|scripts/tank/(build|trio|chains|asset_door|glb_ktx2)\.py$|scripts/lod/(config|measure|generate)\.py$|\.agents/blender/export_tank\.py$|src/map\.rs$)'
ASSETS_SURFACE_TREE='^(scripts/prop/|assets/trees/species\.ron$)'
ASSETS_SURFACE_BOTH='^(assets/materials/|scripts/toolchain\.py$|scripts/tank/report\.py$|src/(bake|spec|exact|substances)|src/bin/asset_verify\.rs$)'

# The lanes whose shared surface the changed set moves, one per line, `tank` before `tree`. Exit 0
# when it names one.
assets_shared_lanes() {   # changed paths on stdin
    _paths=$(cat)
    if printf '%s\n' "$_paths" | grep -qE "$ASSETS_SURFACE_BOTH"; then
        _lanes='tank tree'
    else
        _lanes=
        if printf '%s\n' "$_paths" | grep -qE "$ASSETS_SURFACE_TANK"; then _lanes=tank; fi
        if printf '%s\n' "$_paths" | grep -qE "$ASSETS_SURFACE_TREE"; then _lanes="$_lanes tree"; fi
    fi
    [ -n "$_lanes" ] || return 1
    printf '%s\n' $_lanes
}

# Whether a push rewires the contract in the crate root. Only a diff inside `src/lib.rs` that
# touches a declaration of one of the four certified modules is contract exposure — swapping
# `mod bake;` for a cfg-gated or inline stub changes what the door's binary certifies against while
# touching no path `assets_shared_lanes` watches. Everything else lib.rs does is game code, and a
# change that BREAKS the exposure outright never reaches this lane: clippy compiles the contract
# binary earlier in the same hook run.
#
# FAIL TOWARD RUNNING: a baseline this clone cannot resolve (a branch's first push, a tag) answers
# yes whenever the changed set names lib.rs at all. A comment line mentioning a declaration trips it
# too — toward running, which is the cheap direction to be wrong in.
assets_lib_exposure() {   # <base> <head>; changed paths on stdin
    grep -q '^src/lib\.rs$' || return 1
    assets_pushed_commit "${1:-}" || return 0
    git diff -U0 "$1" "${2:-}" -- src/lib.rs 2>/dev/null |
        grep -qE '^[+-].*mod[[:space:]]+(bake|exact|spec|substances)[[:space:]]*[;{]'
}

# The lanes a changed set moves wholesale, one per line: both for a contract rewire in `src/lib.rs`
# (the contract is in both lanes), else what `assets_shared_lanes` reads off the set.
assets_moved_lanes() {   # <changed-paths-file> <base> <head>
    if assets_lib_exposure "$2" "$3" < "$1"; then
        printf 'tank\ntree\n'
    else
        assets_shared_lanes < "$1"
    fi
}

# A SELECTION is a file of words, one per line: a lane (`tank`, `tree`), whose every trio is
# selected, or a stem, which selects that trio. A canonical stem holds a `/` (`assets_blend_shape`)
# and a lane word does not, so the two never meet.
#
# The trios of <rev> a selection names, sorted. A stem <rev> does not hold as a trio — one the range
# deleted — names nothing to verify.
assets_selected_trios() {   # <rev> <selection-file>
    assets_trios "$1" | while read -r _stem; do
        if grep -qxF -e "$_stem" "$2"; then
            printf '%s\n' "$_stem"
        elif _lane=$(assets_trio_lane "$1" "$_stem") && grep -qxF -e "$_lane" "$2"; then
            printf '%s\n' "$_stem"
        fi
    done
}

# EVERY (revision, asset) pair a push must verify, one per line:
#
#     <local_ref> <local_sha> <stem> changed|shared-surface
#
# Reads git's own pre-push ref list on stdin — `<local_ref> <local_sha> <remote_ref> <remote_sha>`,
# one line per ref and several at once when several refs are pushed. A ref being deleted, a sha
# this clone cannot resolve, and a revision holding no trio at all each contribute no lines. A trio
# whose lane the push moves is `shared-surface`; one it only changes is `changed`.
#
# The whole decision is here rather than in the hook's loop, so what the exercise script drives is
# what the hook runs.
assets_push_targets() {   # <remote> <scratch-dir>; ref lines on stdin
    while read -r _local_ref _local_sha _remote_ref _remote_sha; do
        assets_pushed_commit "$_local_sha" || continue
        _changed=$2/changed.$_local_sha
        _selection=$2/selection.$_local_sha
        assets_pushed_paths "$_local_sha" "$_remote_sha" "$1" > "$_changed"
        {
            assets_moved_lanes "$_changed" "$_remote_sha" "$_local_sha"
            assets_trios "$_local_sha" | assets_changed_trios "$_changed"
        } > "$_selection"
        assets_selected_trios "$_local_sha" "$_selection" |
            while read -r _stem; do
                if grep -qxF -e "$(assets_trio_lane "$_local_sha" "$_stem")" "$_selection"; then
                    _why=shared-surface
                else
                    _why=changed
                fi
                printf '%s %s %s %s\n' "$_local_ref" "$_local_sha" "$_stem" "$_why"
            done
    done
}

# ── whether a range of revisions can have moved any verdict ──────────────────────────────────────

# Whether anything an asset verdict is a function of changed between two revisions, and which.
#
# The SAME predicates the per-push selection above uses — the lanes (`assets_moved_lanes`) and the
# trio rule — asked over a range instead of over one push's refs. The answer is left as a selection
# in `<scratch-dir>/selection`: every lane the range moves, and every trio it changes.
#
# FAIL TOWARD RUNNING. Every way of not knowing — a head this clone cannot resolve, a baseline that
# is absent, all zeros (a branch's first push, a tag, a shallow clone) or unrelated to the head
# after a force-push — answers YES, selects both lanes and pays the 35 minutes. The opposite error
# is a lane that skips the push which broke an asset and reports success, and a lane nobody can
# trust to have run is not cheaper than one that runs.
#
# The baseline is the MERGE BASE, not the raw baseline sha: a pull request's base branch moves under
# it, and a two-tree diff against a moved base attributes other people's commits to this one. That
# error is toward running too, but it makes the reason it printed a lie.
#
# Prints one line saying which it was and why. Exit 0 = affected, 1 = not.
assets_range_affected() {   # <base> <head> <scratch-dir>
    printf 'tank\ntree\n' > "$3/selection"
    if ! assets_pushed_commit "${2:-}"; then
        printf 'assets ▸ affected: %s is not a commit this clone holds\n' "${2:-<empty>}"
        return 0
    fi
    if ! assets_pushed_commit "${1:-}"; then
        printf 'assets ▸ affected: no baseline (%s) — every asset is in range\n' "${1:-<empty>}"
        return 0
    fi
    _base=$(git merge-base "$1" "$2" 2>/dev/null) || _base=
    if [ -z "$_base" ]; then
        printf 'assets ▸ affected: %s and %s have no merge base\n' "$1" "$2"
        return 0
    fi
    _changed=$3/range-changed
    git diff --name-only "$_base" "$2" > "$_changed" || {
        printf 'assets ▸ affected: %s..%s cannot be diffed\n' "$_base" "$2"
        return 0
    }
    _lanes=$(assets_moved_lanes "$_changed" "$_base" "$2")
    # BOTH ENDPOINTS' trios. A range that DELETES an asset holds it at the baseline and nowhere in
    # the head, so a head-side listing drops the stem before selection and the lane never runs; the
    # deleted paths are in the diff, and the trio they belong to is in the baseline's list.
    _stems=$({ assets_trios "$_base"; assets_trios "$2"; } | LC_ALL=C sort -u |
             assets_changed_trios "$_changed")
    {
        [ -z "$_lanes" ] || printf '%s\n' "$_lanes"
        [ -z "$_stems" ] || printf '%s\n' "$_stems"
    } > "$3/selection"
    if [ -z "$_lanes$_stems" ]; then
        printf 'assets ▸ unaffected: %s..%s changes no asset trio and no shared surface (%s path(s))\n' \
            "$_base" "$2" "$(wc -l < "$_changed" | tr -d ' ')"
        return 1
    fi
    _why=
    [ -z "$_lanes" ] ||
        _why="moves the $(printf '%s' "$_lanes" | tr '\n' ' ' | sed 's/ $//; s/ / and /') shared surface"
    [ -z "$_stems" ] || _why="${_why:+$_why; }changes $(printf '%s' "$_stems" | tr '\n' ' ')"
    printf 'assets ▸ affected: %s..%s %s\n' "$_base" "$2" "$_why"
    return 0
}

# The same decision, as the CI lane asks it: off the event, into `$GITHUB_OUTPUT`.
#
# A HAND-STARTED RUN IS ALWAYS AFFECTED, and that is what bounds the gate: a `workflow_dispatch`
# selects both lanes, so it re-cuts every trio from Blender whatever changed, which is the move
# before a release. (`schedule` is answered the same way; no cron triggers CI, and the answer must
# not become "skip" if one is ever added.) `GITHUB_EVENT_NAME` is empty outside a runner, where this
# file is being driven by the suite over ranges it built itself.
#
# It writes `affected=true|false` and `stems`, the trios the verify step re-verifies, one per line
# — the CHECKOUT's (`HEAD`) trios the selection names, because the checkout is what that step reads
# off disk; on a pull request it is the merge commit, which holds the base's trios and the pull
# request's. `stems` is a multiline output rather than a file path: one channel, the one `affected`
# already uses and the suite already reads, and no file whose lifetime has to span two steps. Its
# delimiter is drawn at random, so no line of the list can end it early. Nothing else is written,
# because a workflow step that decides anything is a step no suite can run.
assets_ci_scope() {   # env: GITHUB_EVENT_NAME, ASSETS_BASE, ASSETS_HEAD, GITHUB_OUTPUT
    _scratch=$(mktemp -d)
    case "${GITHUB_EVENT_NAME:-}" in
        schedule|workflow_dispatch)
            printf 'assets ▸ affected: a %s run verifies every asset\n' "$GITHUB_EVENT_NAME"
            printf 'tank\ntree\n' > "$_scratch/selection"
            _affected=true ;;
        *)
            if assets_range_affected "${ASSETS_BASE:-}" "${ASSETS_HEAD:-}" "$_scratch"; then
                _affected=true
            else
                _affected=false
            fi ;;
    esac
    _stems=
    [ "$_affected" = false ] || _stems=$(assets_selected_trios HEAD "$_scratch/selection")
    rm -rf "$_scratch"
    if [ -n "${GITHUB_OUTPUT:-}" ]; then
        _end=assets_stems_$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')
        {
            printf 'affected=%s\n' "$_affected"
            printf 'stems<<%s\n' "$_end"
            [ -z "$_stems" ] || printf '%s\n' "$_stems"
            printf '%s\n' "$_end"
        } >> "$GITHUB_OUTPUT"
    fi
    [ -z "$_stems" ] || printf '%s\n' "$_stems" | sed 's/^/assets ▸ verify: /'
    printf '%s\n' "$_affected"
}

# ── the bytes ────────────────────────────────────────────────────────────────────────────────────

# Where a git-lfs pointer's real bytes live in this clone.
assets_lfs_object() {   # <oid>
    printf '%s/lfs/objects/%.2s/%s/%s\n' \
        "$(git rev-parse --git-common-dir)" "$1" "$(printf %s "$1" | cut -c3-4)" "$1"
}

# The refusal. Named twice — what is missing, and what that makes impossible — because a gate that
# cannot run is not a gate that passed.
assets_refuse() {   # <missing> <consequence>
    printf '\033[31m  the asset door cannot verify this revision\033[0m — %s\n' "$1" >&2
    printf '  %s\n' "$2" >&2
}

# One file of a revision, on disk, with its REAL bytes.
#
# A `.blend` or `.glb` is a git-lfs pointer in the commit; the bytes are in this clone's object
# store, put there by the commit that made them, so a just-committed asset resolves without any
# network. An object that is not there is a refusal — never the work tree, whose bytes are not the
# ones being pushed and would turn a stale file into a green verdict.
assets_hydrate_file() {   # <rev> <path> <dest>
    if ! git cat-file -e "$1:$2" 2>/dev/null; then
        assets_refuse "$2 is not in $1" \
            "the door reads a whole asset out of the revision that ships it"
        return 1
    fi
    # A pointer is a few lines of text; anything larger is the file itself, tracked outside lfs.
    _oid=
    if [ "$(git cat-file -s "$1:$2")" -lt 1024 ]; then
        _oid=$(git cat-file blob "$1:$2" |
               LC_ALL=C sed -n 's/^oid sha256:\([0-9a-f]\{64\}\)$/\1/p')
    fi
    mkdir -p "$(dirname "$3")"
    if [ -z "$_oid" ]; then
        git cat-file blob "$1:$2" > "$3"
        return 0
    fi
    _object=$(assets_lfs_object "$_oid")
    if [ ! -f "$_object" ]; then
        assets_refuse "the git-lfs object for $2 at $1 (sha256 $_oid) is not in this clone" \
            "run \`git lfs fetch\` for it, or push from the clone that committed it"
        return 1
    fi
    cp "$_object" "$3"
}

# One asset's bytes, laid out the way the build and the source pass read them: the source trio at
# `<dest>/assets/<id>/<id>.{blend,tank.ron,glb}` — `L1.SAVED_SOURCE` measures those two directory
# names — the two artifacts the build publishes beside the model (`.sim.glb` and `.lod.json`, ADR
# 0035), and the shared material library, `<dest>/assets/materials/materials.{blend,ron}`. The
# blend is where the tank source's own relative library link resolves to; the RON is the numeric
# half of the same library, and the door hands it to the consumer contract as `--registry` so the
# gate reads THIS revision's substance data rather than the work tree's. Both are DATA, and data
# must be taken from the revision that ships it. Every path is derived from the discovered stem.
#
# Prints the hydrated `.blend`, which is the door's whole argument: it derives the other four.
assets_hydrate() {   # <rev> <stem> <dest-root>
    _directory=$3/$(dirname "$2")
    _name=$(basename "$2")
    mkdir -p "$_directory"
    for _extension in .blend .tank.ron .glb .sim.glb .lod.json; do
        assets_hydrate_file "$1" "$2$_extension" "$_directory/$_name$_extension" || return 1
    done
    _materials=$(dirname "$(dirname "$2")")/materials
    for _half in materials.blend materials.ron; do
        assets_hydrate_file "$1" "$_materials/$_half" "$3/$_materials/$_half" || return 1
    done
    printf '%s\n' "$_directory/$_name.blend"
}
