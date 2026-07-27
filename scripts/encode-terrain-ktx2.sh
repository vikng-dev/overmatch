#!/bin/sh
# encode-terrain-ktx2.sh — re-encode the terrain surface packs into the KTX2 files the game
# actually ships (`assets/terrain/<pack>/<pack>_*.ktx2`).
#
# WHY KTX2 AND NOT PNG/JPG: bevy's PNG/JPG loaders produce a texture with ONE mip level. A 4k
# ground texture tiled every 8 m, sampled at grazing angles across the whole horizon, then misses
# the texture cache on nearly every fetch — the measured 30 fps. KTX2 carries a full mip chain AND
# GPU block compression (UASTC 4x4, transcoded at load to ASTC 4x4 on Apple Silicon / BC7 on
# desktop GPUs — both 8 bpp, vs 32 bpp for the uncompressed RGBA8 upload).
#
# SOURCES stay OUT of the repo (they are 4k lossless masters, ~200 MB): this script fetches them
# from Poly Haven's CDN into $TERRAIN_SRC (default ~/Downloads/overmatch-terrain-src) and reuses
# whatever is already there. Everything is CC0 — see each pack's `cc.txt` in the asset folder.
#
#   usage: scripts/encode-terrain-ktx2.sh [pack ...]     (default: every pack below)
#   needs: basisu (brew install basis_universal), magick (only for 16-bit source normalization)
#
# RESOLUTION POLICY: the pack the game renders is encoded at 4k; packs merely staged for the
# future surface-blending slice are encoded at 2k, which is still 256 px/m at an 8 m tile. Change
# a pack's `res` field below to re-cut it.
#
#   pack                  albedo file  res
#   coast_sand_rocks_02   diff         4096   ACTIVE — bound by `terrain_grid::TEXTURE_PATH`
#   brown_mud_leaves_01   diff         2048   staged
#   rocks_ground_02       col          2048   staged
set -e
cd "$(git rev-parse --show-toplevel)"

SRC="${TERRAIN_SRC:-$HOME/Downloads/overmatch-terrain-src}"
CDN="https://dl.polyhaven.org/file/ph-assets/Textures"
PACKS="${*:-coast_sand_rocks_02 brown_mud_leaves_01 rocks_ground_02}"

command -v basisu >/dev/null || { echo "need basisu: brew install basis_universal" >&2; exit 1; }
mkdir -p "$SRC"

# Fetch a source map into $SRC if it is not already there. $1 = pack, $2 = map, $3 = extension.
fetch() {
    file="$1_$2_4k.$3"
    [ -f "$SRC/$file" ] && return 0
    echo "fetch ▸ $file"
    curl -sSf --max-time 900 -o "$SRC/$file" "$CDN/$3/4k/$1/$file"
}

# Poly Haven ships some packs' data maps as 16-BIT PNGs. Bevy would upload those as Rgba16Unorm
# (a wgpu format behind an optional feature) and basisu gains nothing from the extra depth, so
# truncate to 8-bit — bit depth only, no colour-space change.
to_8bit() {
    case "$(magick identify -format '%z' "$1" 2>/dev/null)" in
        16) echo "8-bit ▸ $(basename "$1")"
            magick "$1" -depth 8 -define png:color-type=2 -strip "$1.8" && mv "$1.8" "$1" ;;
    esac
}

for pack in $PACKS; do
    case "$pack" in
        coast_sand_rocks_02) albedo=diff; res=4096 ;;
        brown_mud_leaves_01) albedo=diff; res=2048 ;;
        rocks_ground_02)     albedo=col;  res=2048 ;;
        *) echo "unknown pack: $pack" >&2; exit 1 ;;
    esac
    out="assets/terrain/$pack"
    mkdir -p "$out"
    fetch "$pack" "$albedo" jpg
    fetch "$pack" nor_gl png
    fetch "$pack" arm png
    to_8bit "$SRC/${pack}_nor_gl_4k.png"
    to_8bit "$SRC/${pack}_arm_4k.png"

    # ALBEDO — colour. sRGB metrics (basisu's default) and sRGB-correct mip filtering: mips are
    # built in LINEAR light and converted back, or every mip level darkens.
    basisu -uastc -uastc_level 2 -ktx2 -ktx2_zstandard_level 9 -mipmap -mip_srgb \
        -resample "$res" "$res" \
        -file "$SRC/${pack}_${albedo}_4k.jpg" -output_file "$out/${pack}_${albedo}.ktx2" >/dev/null

    # NORMAL — direction data, never colour. `-normal_map` switches the codec to linear metrics
    # and disables the RDO passes that smear directions; `-mip_renorm` re-normalizes each mip to
    # unit length (box-filtering unit vectors shortens them, which reads as flattening relief).
    # A higher effort level here because normals show artifacts first.
    basisu -uastc -uastc_level 3 -ktx2 -ktx2_zstandard_level 9 -normal_map -mipmap -mip_linear \
        -mip_renorm -resample "$res" "$res" \
        -file "$SRC/${pack}_nor_gl_4k.png" -output_file "$out/${pack}_nor_gl.ktx2" >/dev/null

    # ARM — AO / roughness / metallic scalars. Linear metrics and linear mip filtering: these are
    # material values, and an sRGB curve applied to them is simply wrong.
    basisu -uastc -uastc_level 2 -ktx2 -ktx2_zstandard_level 9 -linear -mipmap -mip_linear \
        -resample "$res" "$res" \
        -file "$SRC/${pack}_arm_4k.png" -output_file "$out/${pack}_arm.ktx2" >/dev/null

    echo "ktx2  ▸ $pack at ${res}² — $(du -ch "$out"/*.ktx2 | tail -1 | cut -f1)"
done

# NO -y_flip anywhere: basisu leaves row order alone, so the encoded texture matches the source
# PNG's top-down order exactly as bevy's PNG path did. Flipping would mirror the normal map's
# green axis against the mesh tangent basis and invert every bump into a dent.
echo "done ▸ sources kept in $SRC (outside the repo)"
