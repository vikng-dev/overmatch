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
# is the same two predicates asked over a range.
#
# The vocabulary is one word: a STEM. An asset is a sibling trio in one directory —
# `<id>.blend`, `<id>.tank.ron`, `<id>.glb` — and the stem is their common path prefix,
# `assets/tiger_1/tiger_1`, which names the whole trio and nothing else. No path is written down
# anywhere in this file; a second vehicle is discovered by the same rule that found the first.
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
# `.tank.ron` and `.glb` are both there too. A blend with no spec sheet (`assets/shell/`) is not an
# asset, and a glb with no blend beside it is not one either; neither is discovered.
assets_trios() {   # <rev>
    git ls-tree -r --name-only "$1" | awk '
        /\.blend$/     { blend[substr($0, 1, length($0) - 6)] = 1; next }
        /\.tank\.ron$/ { sheet[substr($0, 1, length($0) - 9)] = 1; next }
        /\.glb$/       { model[substr($0, 1, length($0) - 4)] = 1 }
        END { for (stem in blend) if (stem in sheet && stem in model) print stem }
    ' | LC_ALL=C sort
}

# Every blend a revision holds is at `assets/<id>/<id>.blend`, or the push is refused.
#
# The canonical location is one directory under `assets/`, named by the directory it is in — the
# shape the trio rule looks for and every derived path is cut from, and the shape
# `assets/materials/materials.blend` already has. `.gitignore` cannot state that: its negation is
# broad by construction (`!assets/*/*.blend` unignores `assets/tiger_1/scratch.blend` as readily as
# the source beside it), and a pattern language with no backreference has no way to say "named after
# its directory". So the law lives here, where it is executable, and the lane refuses a push
# carrying a blend of any other shape rather than letting an authoring scratch file be tracked
# forever as though it were a source.
assets_blend_shape() {   # <rev>
    _stray=$(git ls-tree -r --name-only "$1" | awk '
        /\.blend$/ {
            depth = split($0, part, "/")
            stem = substr(part[depth], 1, length(part[depth]) - 6)
            if (depth != 3 || part[1] != "assets" || stem != part[2]) print
        }
    ')
    [ -n "$_stray" ] || return 0
    printf '\033[31m  a tracked blend is not a canonical source\033[0m — %s\n' \
        "a tracked blend is exactly assets/<id>/<id>.blend" >&2
    printf '%s\n' "$_stray" | sed 's/^/    /' >&2
    printf '  move it out of assets/<id>/, or name it after the directory it is in — every other\n' >&2
    printf '  blend is authoring scratch, and .gitignore unignores this shape too broadly to say so\n' >&2
    return 1
}

# The stems on stdin whose trio has a file in the changed set.
assets_changed_trios() {   # <changed-paths-file>; stems on stdin
    while read -r _stem; do
        if grep -qxF -e "$_stem.blend" -e "$_stem.tank.ron" -e "$_stem.glb" \
                     -e "$_stem.sim.glb" -e "$_stem.lod.json" "$1"; then
            printf '%s\n' "$_stem"
        fi
    done
}

# Whether the changed set touches the surface EVERY asset's verdict is computed from: the shared
# material library, the pinned toolchain, the source pass, the door, the finding shape, the encoder,
# the derivation verifier, the LOD lane's production half, the world the ladder's right wall is cut
# from, and the Rust consumer contract with the modules it certifies against.
# `src/substances.rs` is in that list because it INTERPRETS the registry: it decides what the shared
# material data means to the gate and hands the canon file its key list, so it moves every asset's
# verdict exactly as `assets/materials/` does. `src/bin/asset_verify.rs` is in it because it is the
# contract's EXECUTABLE surface: the adapter owns the exit status and the report the lane reads — a
# push that moves only it changes every verdict while touching no law. Any of those moving means the
# door now answers differently about bytes that did not move, so every discovered trio is
# re-verified rather than none of them.
#
# `src/lib.rs` is NOT in this list. The crate root is where the contract modules are wired into the
# crate at all, but it is also the whole game's root and moves on nearly every refactor — as a path
# it re-cut every asset for pushes that never went near the contract. It is read by CONTENT instead:
# `assets_lib_exposure`, below.
#
# THE LIST IS THE BUILD'S OWN DECLARED DEPENDENCIES, NOT A SEPARATE OPINION. `scripts/tank/build.py`
# names them: `PIPELINE_SOURCES` is what `blend_digest` hashes, so anything in it moving makes every
# tracked certificate STALE by the build's own arithmetic — `scripts/lod/{config,measure,generate}`
# are in that list, and a push that moved one of them used to skip every `build.py verify` and ship
# a trio the next verify would refuse.
#
# `src/map.rs` and the map manifests are here for the same reason one level out: `config.RIGHT_WALL_M`
# is PARSED from `assets/maps/<DEFAULT_MAP_ID>/level.json` at import, and the wall decides where a
# chain stops. A grown map is a different ladder, so it is a re-verification. The pattern takes every
# map's `level.json` rather than resolving the default: which map is default is `src/map.rs`'s
# answer, and this predicate refuses to be a second reader of it.
assets_shared_surface() {   # changed paths on stdin
    grep -qE '^(assets/materials/|assets/maps/[^/]+/level\.json$|scripts/toolchain\.py$|scripts/encode-tank-ktx2\.sh$|scripts/tank/(build|trio|chains|asset_door|glb_ktx2|report)\.py$|scripts/lod/(config|measure|generate)\.py$|\.agents/blender/export_tank\.py$|src/(bake|spec|exact|substances)|src/map\.rs$|src/bin/asset_verify\.rs$)'
}

# Whether a push rewires the contract in the crate root. Only a diff inside `src/lib.rs` that
# touches a declaration of one of the four certified modules is contract exposure — swapping
# `mod bake;` for a cfg-gated or inline stub changes what the door's binary certifies against while
# touching no path `assets_shared_surface` watches. Everything else lib.rs does is game code, and a
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

# EVERY (revision, asset) pair a push must verify, one per line:
#
#     <local_ref> <local_sha> <stem> changed|shared-surface
#
# Reads git's own pre-push ref list on stdin — `<local_ref> <local_sha> <remote_ref> <remote_sha>`,
# one line per ref and several at once when several refs are pushed. A ref being deleted, a sha
# this clone cannot resolve, and a revision holding no trio at all each contribute no lines.
#
# The whole decision is here rather than in the hook's loop, so what the exercise script drives is
# what the hook runs.
assets_push_targets() {   # <remote> <scratch-dir>; ref lines on stdin
    while read -r _local_ref _local_sha _remote_ref _remote_sha; do
        assets_pushed_commit "$_local_sha" || continue
        _changed=$2/changed.$_local_sha
        assets_pushed_paths "$_local_sha" "$_remote_sha" "$1" > "$_changed"
        if assets_shared_surface < "$_changed" ||
           assets_lib_exposure "$_remote_sha" "$_local_sha" < "$_changed"; then
            _stems=$(assets_trios "$_local_sha")
            _why=shared-surface
        else
            _stems=$(assets_trios "$_local_sha" | assets_changed_trios "$_changed")
            _why=changed
        fi
        [ -n "$_stems" ] || continue
        printf '%s\n' "$_stems" |
            while read -r _stem; do
                printf '%s %s %s %s\n' "$_local_ref" "$_local_sha" "$_stem" "$_why"
            done
    done
}

# ── whether a range of revisions can have moved any verdict ──────────────────────────────────────

# Whether anything an asset verdict is a function of changed between two revisions.
#
# The SAME two predicates the per-push selection above uses — `assets_shared_surface` and the trio
# rule — asked over a range instead of over one push's refs. What is answered here is only "is this
# lane's work possibly affected", never "which trios": a lane that runs, runs whole.
#
# FAIL TOWARD RUNNING. Every way of not knowing — a head this clone cannot resolve, a baseline that
# is absent, all zeros (a branch's first push, a tag, a shallow clone) or unrelated to the head
# after a force-push — answers YES and pays the 35 minutes. The opposite error is a lane that skips
# the push which broke an asset and reports success, and a lane nobody can trust to have run is not
# cheaper than one that runs.
#
# The baseline is the MERGE BASE, not the raw baseline sha: a pull request's base branch moves under
# it, and a two-tree diff against a moved base attributes other people's commits to this one. That
# error is toward running too, but it makes the reason it printed a lie.
#
# Prints one line saying which it was and why. Exit 0 = affected, 1 = not.
assets_range_affected() {   # <base> <head> <scratch-dir>
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
    if assets_shared_surface < "$_changed"; then
        printf 'assets ▸ affected: %s..%s moves the shared surface every verdict is computed from\n' \
            "$_base" "$2"
        return 0
    fi
    if assets_lib_exposure "$_base" "$2" < "$_changed"; then
        printf 'assets ▸ affected: %s..%s rewires the contract modules in src/lib.rs\n' \
            "$_base" "$2"
        return 0
    fi
    # BOTH ENDPOINTS' trios. A range that DELETES an asset holds it at the baseline and nowhere in
    # the head, so a head-side listing drops the stem before selection and the lane never runs; the
    # deleted paths are in the diff, and the trio they belong to is in the baseline's list.
    _stems=$({ assets_trios "$_base"; assets_trios "$2"; } | LC_ALL=C sort -u |
             assets_changed_trios "$_changed")
    if [ -n "$_stems" ]; then
        printf 'assets ▸ affected: %s..%s changes %s\n' "$_base" "$2" \
            "$(printf '%s' "$_stems" | tr '\n' ' ')"
        return 0
    fi
    printf 'assets ▸ unaffected: %s..%s changes no asset trio and no shared surface (%s path(s))\n' \
        "$_base" "$2" "$(wc -l < "$_changed" | tr -d ' ')"
    return 1
}

# The same decision, as the CI lane asks it: off the event, into `$GITHUB_OUTPUT`.
#
# A HAND-STARTED RUN IS ALWAYS AFFECTED, and that is what bounds the gate: a `workflow_dispatch`
# re-cuts every trio from Blender whatever changed, which is the move before a release. (`schedule`
# is still answered because the vocabulary costs nothing to keep, but no cron triggers CI any more
# — it was retired 2026-08-16.) `GITHUB_EVENT_NAME` is empty outside a runner, where this file is
# being driven by the suite over ranges it built itself.
#
# It writes `affected=true|false` and nothing else, because a workflow step that decides anything is
# a step no suite can run.
assets_ci_scope() {   # env: GITHUB_EVENT_NAME, ASSETS_BASE, ASSETS_HEAD, GITHUB_OUTPUT
    _scratch=$(mktemp -d)
    case "${GITHUB_EVENT_NAME:-}" in
        schedule|workflow_dispatch)
            printf 'assets ▸ affected: a %s run verifies every asset\n' "$GITHUB_EVENT_NAME"
            _affected=true ;;
        *)
            if assets_range_affected "${ASSETS_BASE:-}" "${ASSETS_HEAD:-}" "$_scratch"; then
                _affected=true
            else
                _affected=false
            fi ;;
    esac
    rm -rf "$_scratch"
    [ -z "${GITHUB_OUTPUT:-}" ] || printf 'affected=%s\n' "$_affected" >> "$GITHUB_OUTPUT"
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
