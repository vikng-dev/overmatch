#!/bin/sh
# encode-tank-ktx2.sh — POST-EXPORT bake: convert a freshly exported tank glb's embedded PNG/JPEG
# textures to mipped, block-compressed KTX2 *inside* the glb.
#
# NOT A STEP ANYONE RUNS BY HAND. The one asset door — `scripts/tank/asset_door.py`, which the GUI
# add-on `.agents/blender/addons/overmatch_export.py` is an adapter over — invokes it as
#
#     scripts/encode-tank-ktx2.sh <raw-candidate.glb> <baked-candidate.glb>
#
# i.e. Blender exports to a throwaway mipless candidate and THIS script produces the baked one, and
# only a chain that passed every stage after it replaces the tracked glb. That ordering is
# deliberate: a missing basisu or an unhandled texture slot leaves the last good glb in place
# instead of quietly reinstating the shimmer. `scripts/tank/glb_ktx2.py verify` gates the result.
# Re-running it on its own output is refused.
#
# WHY: bevy's PNG/JPEG loaders produce a texture with ONE mip level. A tank carries several 4k and
# 2k maps, every one of them minified hard at combat range. One mip level means shimmer on every
# rivet edge plus a texture-cache miss per fetch: exactly the pathology that measured 30 fps on the
# terrain before `scripts/encode-terrain-ktx2.sh` moved it to KTX2. Same medicine, applied to the
# glb instead of loose files.
#
# HOW BEVY LOADS THIS: bevy_gltf 0.19 does NOT implement the `KHR_texture_basisu` *syntax*
# (bevy_gltf-0.19.0/src/lib.rs:117) — it never reads the extension block. It DOES load KTX2 when it
# arrives through the ordinary path: `textures[i].source` -> `images[i]` with
# `mimeType: "image/ktx2"` (loader/mod.rs:1201 -> `ImageType::MimeType` -> bevy_image image.rs:439
# -> `ImageFormat::Ktx2`). So this bake keeps `source` pointing at the KTX2 image AND writes the
# `KHR_texture_basisu` block next to it for other tools. `extensionsRequired` is deliberately NOT
# set: gltf-json 1.4.1 fails validation on a required extension it does not know
# (gltf-json-1.4.1/src/root.rs:160), and bevy validates by default.
#
#   usage: scripts/encode-tank-ktx2.sh <in.glb> <out.glb>   (both required — an asset default here
#          would be one vehicle's name compiled into a generic stage)
#   needs: basisu (brew install basis_universal), python3
#
# ENCODING POLICY — UASTC 4x4 everywhere, no ETC1S. ETC1S is a palettized codec: it destroys
# normal maps and smears the hull atlas' rivet detail, and this model is the thing the player
# stares at. UASTC transcodes at load to ASTC 4x4 (Apple Silicon) or BC7 (desktop) — 8 bpp against
# 32 bpp for the RGBA8 upload the PNG path does today, so VRAM drops even with the +1/3 mip chain.
# Zstandard supercompression (level 9) is on; it is free at load and pays for most of the mips.
#
# The colour-space flags are the easy silent failure, so they are derived, not typed: the repack
# step reads each image's ROLE off the materials that reference it (baseColor/emissive -> sRGB,
# normal -> normal-map, metallicRoughness/occlusion -> linear data) and this script switches
# basisu accordingly. A wrong flag here is a washed-out or dark tank, not an error message.
set -e
cd "$(git rev-parse --show-toplevel)"

IN="$1"
OUT="$2"
[ -n "$IN" ] && [ -n "$OUT" ] || {
    echo "usage: scripts/encode-tank-ktx2.sh <in.glb> <out.glb>" >&2; exit 2; }

# The same resolution rule as scripts/toolchain.py: OVERMATCH_BASISU names the binary when the
# pinned build is not the one on PATH (CI installs it outside PATH).
BASISU="${OVERMATCH_BASISU:-basisu}"
command -v "$BASISU" >/dev/null || {
    echo "need basisu: brew install basis_universal (or set OVERMATCH_BASISU)" >&2; exit 1; }

# ── work dir ─────────────────────────────────────────────────────────────────────────────────────
# A FRESH dir per run, because two bakes can be in flight at once: the user exporting from the
# Blender GUI while an agent runs a headless export. This used to be one fixed path
# (`$TMPDIR/overmatch-tank-ktx2`) that the script opened by `rm -rf`-ing, so the second run deleted
# the first one's half-encoded KTX2s and both produced garbage or died. mktemp makes that
# impossible. `TANK_KTX2_WORK` still pins the path for tests and for poking at intermediates, but a
# pinned dir is only cleared if it is empty/absent or carries the marker this script writes — it
# will not `rm -rf` a directory that is somebody else's.
MARKER=".overmatch-tank-ktx2"
if [ -n "${TANK_KTX2_WORK:-}" ]; then
    WORK="$TANK_KTX2_WORK"
    KEEP_WORK=1     # caller named it, so it is the caller's to delete
    if [ -e "$WORK" ]; then
        [ -d "$WORK" ] || { echo "TANK_KTX2_WORK=$WORK exists and is not a directory" >&2; exit 1; }
        if [ ! -e "$WORK/$MARKER" ] && [ -n "$(ls -A "$WORK" 2>/dev/null)" ]; then
            echo "TANK_KTX2_WORK=$WORK is not empty and has no $MARKER — refusing to rm -rf it." >&2
            echo "  Point TANK_KTX2_WORK somewhere disposable, or empty that dir yourself." >&2
            exit 1
        fi
        rm -rf "$WORK"
    fi
else
    WORK="$(mktemp -d "${TMPDIR:-/tmp}/overmatch-tank-ktx2.XXXXXXXX")"
    KEEP_WORK=0     # ours: deleted on success, kept (and printed) on failure
fi
mkdir -p "$WORK/src" "$WORK/ktx2"
: > "$WORK/$MARKER"

cleanup() {
    status=$?
    if [ "$status" -ne 0 ]; then
        echo "bake failed — work dir kept for inspection: $WORK" >&2
    elif [ "$KEEP_WORK" -eq 0 ]; then
        rm -rf "$WORK"
    fi
    exit "$status"
}
trap cleanup EXIT

# ── unpack ───────────────────────────────────────────────────────────────────────────────────────
# Split the glb into JSON + BIN, write every embedded image out as its own file, and record the
# role each image plays in the materials that sample it. Roles drive the encoder flags below.
python3 scripts/tank/glb_ktx2.py unpack "$IN" "$WORK"

# An uncached encode is the slow phase the caller sits through, and the only phase with a countable
# unit of work. Announcing the total here lets the GUI adapter — which reads the door's stdout line
# by line — turn the `ktx2  ▸` lines below into a real percentage instead of a guess. A cache hit
# prints its `ktx2  ▸` line too, so the count reaches the total either way.
echo "images ▸ $(grep -c '[^[:space:]]' "$WORK/roles.txt") to encode"

# ── cache ────────────────────────────────────────────────────────────────────────────────────────
# One encode is a pure function of four inputs, and the key is a sha256 over exactly those four:
#   source   the image file's bytes;
#   role     the colour role. Each role's flags already differ, and the role is hashed beside them
#            so a role-dependent step outside the argument list can never share an entry;
#   args     the full basisu argument list minus the two paths — the role's flags and the common
#            ones, read from the same "$@" the invocation passes, so a flag cannot reach the encoder
#            without reaching the key;
#   encoder  the first line of `basisu -version`, because another version is other bytes.
# Each field is length-prefixed, so no field can run into the next. This script's own bytes are not
# in the key: a comment edit is not a different encode, and a flag edit already moves `args`.
# Outputs are byte-identical hit or miss — `build.py verify` byte-compares the re-export against
# the committed glb, which already relies on one basisu version and argument list being
# deterministic, and the key carries both.
#
# Entries are `<key>.ktx2` and nothing else is ever read. A miss publishes by writing a dot-file in
# the cache dir and renaming it onto its key: two bakes can run at once (see the work dir), and a
# rename inside one directory is atomic, so a reader sees a whole entry or none. A failed encode
# publishes nothing. A cache that cannot be written turns itself off with one line and the bake
# encodes uncached; it never fails the bake.
CACHE="${OVERMATCH_KTX2_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/overmatch/ktx2}"
ENCODER="$("$BASISU" -version 2>/dev/null | sed -n 1p)"
if [ -z "$ENCODER" ]; then
    echo "cache ▸ off — \`$BASISU -version\` names no encoder to key on; encoding every image"
    CACHE=""
elif ! { mkdir -p "$CACHE" && probe="$(mktemp "$CACHE/.probe.XXXXXXXX")" && rm -f "$probe"; } \
        2>/dev/null; then
    echo "cache ▸ off — cannot create or write $CACHE; encoding every image"
    CACHE=""
fi

cache_key() {   # <source file> <role> <encoder> <basisu args...>
    python3 -c '
import hashlib, json, sys
source, role, encoder, *args = sys.argv[1:]
digest = hashlib.sha256()
with open(source, "rb") as handle:
    fields = (("source", handle.read()), ("role", role.encode()),
              ("args", json.dumps(args).encode()), ("encoder", encoder.encode()))
for name, value in fields:
    digest.update(b"%s %d\n" % (name.encode(), len(value)) + value)
print(digest.hexdigest())
' "$@"
}

cache_publish() {   # <encoded file> <key>
    tmp="$(mktemp "$CACHE/.$2.XXXXXXXX" 2>/dev/null)" && cp "$1" "$tmp" 2>/dev/null &&
        mv -f "$tmp" "$CACHE/$2.ktx2" 2>/dev/null && return 0
    [ -z "$tmp" ] || rm -f "$tmp"
    echo "cache ▸ off — cannot write $CACHE; encoding the remaining images uncached"
    CACHE=""
}

# ── encode ───────────────────────────────────────────────────────────────────────────────────────
# One basisu invocation per uncached image. `roles.txt` lines are: <index> <role> <file>.
while read -r idx role file; do
    src="$WORK/src/$file"
    out="$WORK/ktx2/$idx.ktx2"
    case "$role" in
        # COLOUR — sRGB metrics (basisu's default) and sRGB-correct mip filtering: `-mip_srgb`
        # converts to linear light before each box filter and back again, or every mip darkens.
        srgb)
            set -- -uastc -uastc_level 2 -mip_srgb ;;
        # NORMAL — direction data, never colour. `-normal_map` selects linear metrics, linear mip
        # filtering, a linear KTX2 transfer function and disables the RDO passes that smear
        # directions. `-mip_renorm` re-normalizes each mip to unit length (box-filtering unit
        # vectors shortens them, which reads as relief flattening with distance). Higher effort
        # level because normals show artifacts first.
        normal)
            set -- -uastc -uastc_level 3 -normal_map -mip_renorm ;;
        # DATA — AO / roughness / metallic scalars. Linear metrics and linear mip filtering: these
        # are material values and an sRGB curve applied to them is simply wrong.
        linear)
            set -- -uastc -uastc_level 2 -linear -mip_linear ;;
        *) echo "unknown role: $role" >&2; exit 1 ;;
    esac
    # Every encoding flag lives in "$@" from here on; the key hashes that list, and the invocation
    # adds nothing to it but the two paths.
    set -- "$@" -ktx2 -ktx2_zstandard_level 9 -mipmap
    key=""
    [ -z "$CACHE" ] || key="$(cache_key "$src" "$role" "$ENCODER" "$@")"
    if [ -n "$key" ] && cp "$CACHE/$key.ktx2" "$out" 2>/dev/null; then
        echo "ktx2  ▸ [$idx] $role $(basename "$file") — cached"
        continue
    fi
    "$BASISU" "$@" -file "$src" -output_file "$out" >"$WORK/ktx2/$idx.log" 2>&1 ||
        { echo "basisu failed on image $idx ($file) — see $WORK/ktx2/$idx.log" >&2; exit 1; }
    [ -z "$CACHE" ] || cache_publish "$out" "$key"
    echo "ktx2  ▸ [$idx] $role $(basename "$file") — $(du -h "$out" | cut -f1)"
done < "$WORK/roles.txt"

# NO -y_flip anywhere: basisu leaves row order alone, so the encoded texture keeps the source PNG's
# top-down order exactly as bevy's PNG path delivered it. Flipping would mirror the normal map's
# green axis against the mesh tangent basis and turn every bump into a dent.

# ── repack ───────────────────────────────────────────────────────────────────────────────────────
# Rebuild the glb: same JSON except `images` (new bufferView bytes + `image/ktx2` mime) and
# `textures` (added `KHR_texture_basisu`), and a compacted BIN chunk whose bufferViews keep their
# indices and their byte-for-byte contents. Accessor payloads are hashed on both sides and must
# match, so a geometry regression here is an error, not a surprise in game.
python3 scripts/tank/glb_ktx2.py repack "$IN" "$WORK" "$OUT"

# ── verify ───────────────────────────────────────────────────────────────────────────────────────
python3 scripts/tank/glb_ktx2.py diff "$IN" "$OUT"
