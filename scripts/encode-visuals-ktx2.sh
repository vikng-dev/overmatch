#!/bin/sh
# encode-visuals-ktx2.sh — cut the shared visual library (`assets/materials/visuals.ron`) into the
# KTX2 files the tank surface material binds (`assets/materials/visuals/<visual>/<visual>_*.ktx2`),
# and generate the shared camouflage patterns (`assets/materials/camouflage/<pattern>.ktx2`).
#
# One codec policy with `scripts/encode-terrain-ktx2.sh` (UASTC 4x4 + zstd, full mip chain; see its
# header for why KTX2 and why each map kind gets its own flags). A visual ships three maps at 1024²:
#
#   <visual>_diff.ktx2    albedo, sRGB
#   <visual>_nor_gl.ktx2  tangent-space normal, OpenGL convention (+Y green toward the image's top)
#   <visual>_arm.ktx2     R ambient occlusion, G roughness, B metallic (glTF ORM packing), linear
#
# SOURCES stay OUT of the repo (lossless masters): fetched into $VISUALS_SRC (default
# ~/Downloads/overmatch-visuals-src) and reused when already there. Every pack is CC0; each visual's
# folder carries a `cc.txt` naming its source.
#
#   usage: scripts/encode-visuals-ktx2.sh [visual | stripes ...]   (default: everything below)
#   needs: basisu (brew install basis_universal), magick, unzip, python3 with numpy and Pillow
#
#   visual           source     pack        maps the pack ships
#   rolled_steel     ambientCG  Metal046A   Color, NormalGL, Roughness, Metalness
#   cast_steel       ambientCG  Metal052C   Color, NormalGL, Roughness, Metalness
#   machined_steel   ambientCG  Metal009    Color, NormalGL, Roughness, Metalness
#   track_steel      ambientCG  Metal046B   Color, NormalGL, Roughness, Metalness
#   rubber           ambientCG  Rubber004   Color, NormalGL, Roughness
#   wood             Poly Haven rough_wood  diff, nor_gl, arm
#
# An ambientCG pack ships separate maps, so the ARM is packed here: R is the pack's
# AmbientOcclusion where it ships one and white (unoccluded) where it does not, G its Roughness,
# B its Metalness where it ships one and black (dielectric) where it does not.
#
# `stripes` is generated, not fetched: `scripts/visuals/camo_stripes.py` writes the pattern as a
# linear RGB mask, one palette colour's coverage per channel, and it is cut like an ARM map.
set -e
cd "$(git rev-parse --show-toplevel)"

SRC="${VISUALS_SRC:-$HOME/Downloads/overmatch-visuals-src}"
OUT="assets/materials/visuals"
CAMO="assets/materials/camouflage"
RES=1024

ALL="rolled_steel cast_steel machined_steel track_steel rubber wood stripes"
TARGETS="${*:-$ALL}"

for tool in basisu magick unzip python3; do
    command -v "$tool" >/dev/null || { echo "need $tool" >&2; exit 1; }
done
mkdir -p "$SRC"

# The ambientCG pack a visual is cut from, or nothing for a Poly Haven one.
acg_pack() {
    case "$1" in
        rolled_steel)   echo Metal046A ;;
        cast_steel)     echo Metal052C ;;
        machined_steel) echo Metal009 ;;
        track_steel)    echo Metal046B ;;
        rubber)         echo Rubber004 ;;
        *)              echo "" ;;
    esac
}

# The Poly Haven pack a visual is cut from.
ph_pack() {
    case "$1" in
        wood) echo rough_wood ;;
        *)    echo "" ;;
    esac
}

# Cut one KTX2: $1 = map kind (albedo | nor_gl | arm), $2 = source image, $3 = output path.
encode() {
    case "$1" in
        albedo) codec="-uastc_level 2 -mip_srgb" ;;
        nor_gl) codec="-uastc_level 3 -normal_map -mip_linear -mip_renorm" ;;
        arm)    codec="-uastc_level 2 -linear -mip_linear" ;;
        *) echo "unknown map kind: $1" >&2; exit 1 ;;
    esac
    # Word splitting on $codec is the point: it is an argument list.
    # shellcheck disable=SC2086
    basisu -uastc -ktx2 -ktx2_zstandard_level 9 -mipmap -resample "$RES" "$RES" \
        $codec -file "$2" -output_file "$3" >/dev/null
}

# 16-bit masters to 8-bit, bit depth only (basisu gains nothing from the extra depth).
to_8bit() {
    magick "$1" -depth 8 -strip "PNG24:$2"
}

# An ambientCG pack: fetch the 2K PNG zip, pack the ARM, cut the three maps.
cut_acg() {
    visual="$1"; pack="$2"
    zip="$SRC/${pack}_2K-PNG.zip"
    if [ ! -f "$zip" ]; then
        echo "fetch ▸ ${pack}_2K-PNG.zip"
        curl -sSfL --max-time 900 -o "$zip" "https://ambientcg.com/get?file=${pack}_2K-PNG.zip"
    fi
    work="$SRC/$pack"
    mkdir -p "$work"
    unzip -o -q "$zip" -d "$work"
    base="$work/${pack}_2K-PNG"
    to_8bit "${base}_Color.png" "$work/albedo.png"
    to_8bit "${base}_NormalGL.png" "$work/nor_gl.png"
    size="$(magick identify -format '%wx%h' "${base}_Roughness.png")"
    if [ -f "${base}_AmbientOcclusion.png" ]; then
        ao="${base}_AmbientOcclusion.png"
    else
        ao="$work/white.png"
        magick -size "$size" xc:white "$ao"
    fi
    if [ -f "${base}_Metalness.png" ]; then
        metal="${base}_Metalness.png"
    else
        metal="$work/black.png"
        magick -size "$size" xc:black "$metal"
    fi
    # Each source's first channel, raw: no colour-space conversion touches a data map.
    magick \( "$ao" -channel R -separate \) \( "${base}_Roughness.png" -channel R -separate \) \
        \( "$metal" -channel R -separate \) +channel -combine -depth 8 -strip "PNG24:$work/arm.png"
    mkdir -p "$OUT/$visual"
    encode albedo "$work/albedo.png" "$OUT/$visual/${visual}_diff.ktx2"
    encode nor_gl "$work/nor_gl.png" "$OUT/$visual/${visual}_nor_gl.ktx2"
    encode arm "$work/arm.png" "$OUT/$visual/${visual}_arm.ktx2"
}

# A Poly Haven pack: its diff, nor_gl and arm masters at 2k, cut as they are.
cut_ph() {
    visual="$1"; pack="$2"
    cdn="https://dl.polyhaven.org/file/ph-assets/Textures"
    for map in diff:jpg nor_gl:png arm:png; do
        name="${map%%:*}"; ext="${map##*:}"
        file="$SRC/${pack}_${name}_2k.$ext"
        if [ ! -f "$file" ]; then
            echo "fetch ▸ ${pack}_${name}_2k.$ext"
            curl -sSf --max-time 900 -o "$file" "$cdn/$ext/2k/$pack/${pack}_${name}_2k.$ext"
        fi
    done
    work="$SRC/$pack"
    mkdir -p "$work" "$OUT/$visual"
    to_8bit "$SRC/${pack}_nor_gl_2k.png" "$work/nor_gl.png"
    to_8bit "$SRC/${pack}_arm_2k.png" "$work/arm.png"
    encode albedo "$SRC/${pack}_diff_2k.jpg" "$OUT/$visual/${visual}_diff.ktx2"
    encode nor_gl "$work/nor_gl.png" "$OUT/$visual/${visual}_nor_gl.ktx2"
    encode arm "$work/arm.png" "$OUT/$visual/${visual}_arm.ktx2"
}

for target in $TARGETS; do
    if [ "$target" = stripes ]; then
        mkdir -p "$CAMO"
        python3 scripts/visuals/camo_stripes.py "$SRC/stripes.png"
        encode arm "$SRC/stripes.png" "$CAMO/stripes.ktx2"
        echo "ktx2  ▸ camouflage stripes — $(du -h "$CAMO/stripes.ktx2" | cut -f1)"
        continue
    fi
    acg="$(acg_pack "$target")"
    ph="$(ph_pack "$target")"
    if [ -n "$acg" ]; then
        cut_acg "$target" "$acg"
    elif [ -n "$ph" ]; then
        cut_ph "$target" "$ph"
    else
        echo "unknown target: $target" >&2; exit 1
    fi
    echo "ktx2  ▸ $target at ${RES}² — $(du -ch "$OUT/$target"/*.ktx2 | tail -1 | cut -f1)"
done

# NO -y_flip: basisu keeps the source's row order, so the encoded normal map's green axis stays
# toward the image's top, the convention the surface shader's projections are built to.
echo "done ▸ sources kept in $SRC (outside the repo)"
