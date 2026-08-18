#!/bin/sh
# encode-terrain-ktx2.sh — re-encode the terrain surface packs into the KTX2 files the game
# actually ships (`assets/terrain/<pack>/<pack>_*.ktx2` and `assets/terrain/blend/blend_*.ktx2`).
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
#   usage: scripts/encode-terrain-ktx2.sh [pack | blend ...]   (default: everything below)
#   needs: basisu (brew install basis_universal), magick (only for 16-bit source normalization)
#
# TWO CUTS, ONE CODEC POLICY. A pack named alone is cut as three standalone maps into its own
# folder. `blend` cuts the SAME three map types as 2D ARRAYS whose layers are $LAYER_PACKS in
# order — one array per map type, because the blend material's texture slots are the constraint:
# nine separate bindings do not fit beside bevy's PBR bind group, three array bindings do. Naming
# any layer pack selects `blend`, since a layer cannot be cut on its own.
#
#   pack                  albedo file  res
#   coast_sand_rocks_02   diff         4096   ACTIVE base — bound by `terrain_grid::TEXTURE_PATH`
#   dirt_aerial_03        diff         2048   blend layer 0 (recesses)
#   coast_sand_05         diff         2048   blend layer 1 (slopes)
#   aerial_mud_1          diff         2048   blend layer 2 (lowlands)
#   brown_mud_leaves_01   diff         2048   staged — nothing loads it
#   rocks_ground_02       col          2048   staged — nothing loads it
set -e
cd "$(git rev-parse --show-toplevel)"

SRC="${TERRAIN_SRC:-$HOME/Downloads/overmatch-terrain-src}"
CDN="https://dl.polyhaven.org/file/ph-assets/Textures"

# The blend arrays' layer order — the mask's channel order (`terrain_grid::BLEND_LAYERS`), and the
# order the `-file` arguments are passed in below. Changing it re-numbers every layer index.
LAYER_PACKS="dirt_aerial_03 coast_sand_05 aerial_mud_1"
BLEND_DIR="assets/terrain/blend"
BLEND_RES=2048

TARGETS="${*:-coast_sand_rocks_02 brown_mud_leaves_01 rocks_ground_02 blend}"

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

# The albedo file name Poly Haven ships a pack's colour map under.
albedo_of() {
    case "$1" in
        rocks_ground_02) echo col ;;
        *)               echo diff ;;
    esac
}

# Pull a pack's three 4k masters down and normalize their bit depth.
stage_sources() {
    fetch "$1" "$(albedo_of "$1")" jpg
    fetch "$1" nor_gl png
    fetch "$1" arm png
    to_8bit "$SRC/$1_nor_gl_4k.png"
    to_8bit "$SRC/$1_arm_4k.png"
}

# Cut one KTX2. $1 = map kind (albedo | nor_gl | arm), $2 = side in texels, $3 = output path, rest
# = source files in LAYER ORDER. More than one source writes a 2D ARRAY, one layer per file, which
# is why every layer must be cut at the same side.
#
# ALBEDO is colour: sRGB metrics (basisu's default) and sRGB-correct mip filtering, or every mip
# level darkens. NORMAL is direction data — `-normal_map` switches the codec to linear metrics and
# drops the RDO passes that smear directions, `-mip_renorm` re-normalizes each mip to unit length
# (box-filtering unit vectors shortens them, which reads as flattening relief), and the effort
# level is higher because normals show artifacts first. ARM is AO / roughness / metallic scalars:
# linear metrics and linear mip filtering, an sRGB curve applied to them is simply wrong.
encode() {
    kind="$1"; side="$2"; output="$3"; shift 3
    case "$kind" in
        albedo) codec="-uastc_level 2 -mip_srgb" ;;
        nor_gl) codec="-uastc_level 3 -normal_map -mip_linear -mip_renorm" ;;
        arm)    codec="-uastc_level 2 -linear -mip_linear" ;;
        *) echo "unknown map kind: $kind" >&2; exit 1 ;;
    esac
    inputs=""
    layers=0
    for source; do
        inputs="$inputs -file $source"
        layers=$((layers + 1))
    done
    array=""
    if [ "$layers" -gt 1 ]; then
        array="-tex_type 2darray"
    fi
    # Word splitting on $codec / $array / $inputs is the point — they are argument lists.
    # shellcheck disable=SC2086
    basisu -uastc -ktx2 -ktx2_zstandard_level 9 -mipmap -resample "$side" "$side" \
        $array $codec $inputs -output_file "$output" >/dev/null
}

# One pack, three standalone maps in its own folder.
encode_pack() {
    pack="$1"
    case "$pack" in
        coast_sand_rocks_02) res=4096 ;;
        *)                   res=2048 ;;
    esac
    albedo="$(albedo_of "$pack")"
    out="assets/terrain/$pack"
    mkdir -p "$out"
    stage_sources "$pack"
    encode albedo "$res" "$out/${pack}_${albedo}.ktx2" "$SRC/${pack}_${albedo}_4k.jpg"
    encode nor_gl "$res" "$out/${pack}_nor_gl.ktx2" "$SRC/${pack}_nor_gl_4k.png"
    encode arm "$res" "$out/${pack}_arm.ktx2" "$SRC/${pack}_arm_4k.png"
    echo "ktx2  ▸ $pack at ${res}² — $(du -ch "$out"/*.ktx2 | tail -1 | cut -f1)"
}

# The three blend arrays: one per map type, $LAYER_PACKS as layers 0..n in that order.
encode_blend() {
    mkdir -p "$BLEND_DIR"
    albedo_sources=""
    nor_sources=""
    arm_sources=""
    for pack in $LAYER_PACKS; do
        stage_sources "$pack"
        albedo_sources="$albedo_sources $SRC/${pack}_$(albedo_of "$pack")_4k.jpg"
        nor_sources="$nor_sources $SRC/${pack}_nor_gl_4k.png"
        arm_sources="$arm_sources $SRC/${pack}_arm_4k.png"
    done
    # shellcheck disable=SC2086
    encode albedo "$BLEND_RES" "$BLEND_DIR/blend_diff.ktx2" $albedo_sources
    # shellcheck disable=SC2086
    encode nor_gl "$BLEND_RES" "$BLEND_DIR/blend_nor_gl.ktx2" $nor_sources
    # shellcheck disable=SC2086
    encode arm "$BLEND_RES" "$BLEND_DIR/blend_arm.ktx2" $arm_sources
    echo "ktx2  ▸ blend arrays at ${BLEND_RES}², layers: $LAYER_PACKS —" \
        "$(du -ch "$BLEND_DIR"/*.ktx2 | tail -1 | cut -f1)"
}

blend=""
packs=""
for target in $TARGETS; do
    case " $LAYER_PACKS blend " in
        *" $target "*) blend=yes; continue ;;
    esac
    case "$target" in
        coast_sand_rocks_02|brown_mud_leaves_01|rocks_ground_02) packs="$packs $target" ;;
        *) echo "unknown target: $target" >&2; exit 1 ;;
    esac
done
for pack in $packs; do
    encode_pack "$pack"
done
if [ -n "$blend" ]; then
    encode_blend
fi

# NO -y_flip anywhere: basisu leaves row order alone, so the encoded texture matches the source
# PNG's top-down order exactly as bevy's PNG path did. Flipping would mirror the normal map's
# green axis against the mesh tangent basis and invert every bump into a dent.
echo "done ▸ sources kept in $SRC (outside the repo)"
