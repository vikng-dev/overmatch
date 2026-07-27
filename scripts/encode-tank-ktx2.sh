#!/bin/sh
# encode-tank-ktx2.sh — POST-EXPORT bake: convert a freshly exported tank glb's embedded PNG/JPEG
# textures to mipped, block-compressed KTX2 *inside* the glb.
#
# NOT A STEP ANYONE RUNS BY HAND. `.agents/blender/export_tiger.py` — the script door the Tiger glb
# leaves Blender through, and the thing the GUI add-on `.agents/blender/addons/overmatch_export.py`
# calls for File ▸ Export — invokes it as
#
#     scripts/encode-tank-ktx2.sh <temp-export.glb> assets/tiger_1/tiger_1.glb
#
# i.e. Blender exports to a throwaway mipless file and THIS script produces the tracked glb. That
# ordering is deliberate: the tracked path is only ever written by a bake that succeeded, so a
# missing basisu or an unhandled texture slot leaves the last good glb in place instead of quietly
# reinstating the shimmer. `scripts/hooks/pre-push` and `release.yml` gate the result
# (`scripts/tank/glb_ktx2.py verify`). Re-running it on its own output is refused.
#
# WHY: bevy's PNG/JPEG loaders produce a texture with ONE mip level. The Tiger carries three 4k
# maps on the hull/turret atlas, two 2k maps on the road-wheel rubber and three 512s on the track
# link — every one of them minified hard at combat range. One mip level means shimmer on every
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
#   usage: scripts/encode-tank-ktx2.sh [in.glb [out.glb]]   (defaults are the one-off manual form:
#          a mipless glb in the asset folder -> a .mipped.glb beside it, which is gitignored so the
#          repo keeps exactly ONE tank glb in LFS)
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

IN="${1:-assets/tiger_1/tiger_1.glb}"
OUT="${2:-assets/tiger_1/tiger_1.mipped.glb}"
WORK="${TANK_KTX2_WORK:-${TMPDIR:-/tmp}/overmatch-tank-ktx2}"

command -v basisu >/dev/null || { echo "need basisu: brew install basis_universal" >&2; exit 1; }
rm -rf "$WORK"; mkdir -p "$WORK/src" "$WORK/ktx2"

# ── unpack ───────────────────────────────────────────────────────────────────────────────────────
# Split the glb into JSON + BIN, write every embedded image out as its own file, and record the
# role each image plays in the materials that sample it. Roles drive the encoder flags below.
python3 scripts/tank/glb_ktx2.py unpack "$IN" "$WORK"

# ── encode ───────────────────────────────────────────────────────────────────────────────────────
# One basisu invocation per image. `roles.txt` lines are: <index> <role> <file>.
while read -r idx role file; do
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
    basisu "$@" -ktx2 -ktx2_zstandard_level 9 -mipmap \
        -file "$WORK/src/$file" -output_file "$out" >"$WORK/ktx2/$idx.log" 2>&1 ||
        { echo "basisu failed on image $idx ($file) — see $WORK/ktx2/$idx.log" >&2; exit 1; }
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
