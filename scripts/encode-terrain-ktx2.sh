#!/bin/sh
# encode-terrain-ktx2.sh — re-encode the surface packs into the KTX2 files the game actually ships:
# the terrain's (`assets/terrain/<pack>/<pack>_*.ktx2`, `assets/terrain/blend/blend_*.ktx2`) and
# the scatter constructions' (`assets/constructions/<pack>/<pack>_*.ktx2`).
#
# WHY KTX2 AND NOT PNG/JPG: bevy's PNG/JPG loaders produce a texture with ONE mip level. A 4k
# ground texture tiled every 8 m, sampled at grazing angles across the whole horizon, then misses
# the texture cache on nearly every fetch — the measured 30 fps. KTX2 carries a full mip chain AND
# GPU block compression (UASTC 4x4, transcoded at load to ASTC 4x4 on Apple Silicon / BC7 on
# desktop GPUs — both 8 bpp, vs 32 bpp for the uncompressed RGBA8 upload).
#
# SOURCES stay OUT of the repo (they are lossless masters, ~200 MB): this script fetches them from
# Poly Haven's CDN into $TERRAIN_SRC (default ~/Downloads/overmatch-terrain-src) and reuses
# whatever is already there. Everything is CC0 — see each pack's `cc.txt` in the asset folder.
#
#   usage: scripts/encode-terrain-ktx2.sh [pack | blend ...]   (default: everything below)
#   needs: basisu (brew install basis_universal), magick (only for 16-bit source normalization),
#          python3 (reads the ground's packs from `assets/terrain/surface.ron`)
#
# TWO CUTS, ONE CODEC POLICY. A pack named alone is cut as three standalone maps into its own
# folder. `blend` cuts the SAME three map types as 2D ARRAYS whose layers are the surface's layers
# in order — one array per map type, because the blend material's texture slots are the constraint:
# nine separate bindings do not fit beside bevy's PBR bind group, three array bindings do. Naming
# any layer pack selects `blend`, since a layer cannot be cut on its own.
#
#   pack                                res   source
#   the surface's base                  4096  4k
#   each of the surface's layers        2048  4k  one slice of the blend arrays
#   brick_wall_006                      1024  2k  the primary construction lane (visuals.ron "brick")
#   dark_planks                         1024  2k  the secondary construction lane (visuals.ron "dark_planks")
#
# Every pack's albedo ships as Poly Haven's `diff`.
#
# A construction pack is cut at 1024², from 2k masters: a house face spans metres, not a horizon, and
# the LOW preset's GPU is a GTX 1650-class part. Two master texels per shipped texel is all the
# resample's filter uses.
set -e
cd "$(git rev-parse --show-toplevel)"

SRC="${TERRAIN_SRC:-$HOME/Downloads/overmatch-terrain-src}"
CDN="https://dl.polyhaven.org/file/ph-assets/Textures"

# The ground's packs, from their owner `assets/terrain/surface.ron`: its base, or its layers in the
# mask's channel order, which is the order the blend's `-file` arguments are passed in below.
surface_packs() {
    python3 -I - "$1" <<'PY'
import sys
sys.path.insert(0, "scripts/prop")
import ron
surface = ron.load("assets/terrain/surface.ron")
packs = [surface["base"]] if sys.argv[1] == "base" else surface["layers"]
print(" ".join(pack["pack"] for pack in packs))
PY
}
BASE_PACK="$(surface_packs base)"
LAYER_PACKS="$(surface_packs layers)"
BLEND_DIR="assets/terrain/blend"
BLEND_RES=2048

# The scatter constructions' packs: standalone maps into `assets/constructions/<pack>/`.
BUILDING_PACKS="brick_wall_006 dark_planks"

TARGETS="${*:-$BASE_PACK blend $BUILDING_PACKS}"

command -v basisu >/dev/null || { echo "need basisu: brew install basis_universal" >&2; exit 1; }
mkdir -p "$SRC"

# Is $1 one of the scatter constructions' packs?
is_construction() {
    case " $BUILDING_PACKS " in
        *" $1 "*) return 0 ;;
        *)        return 1 ;;
    esac
}

# The master resolution a pack is cut from (see the table above).
source_res_of() {
    if is_construction "$1"; then echo 2k; else echo 4k; fi
}

# A source map's path in $SRC. $1 = pack, $2 = map, $3 = extension.
source_of() {
    echo "$SRC/$1_$2_$(source_res_of "$1").$3"
}

# Fetch a source map into $SRC if it is not already there. $1 = pack, $2 = map, $3 = extension.
fetch() {
    master="$(source_res_of "$1")"
    file="$1_$2_$master.$3"
    [ -f "$SRC/$file" ] && return 0
    echo "fetch ▸ $file"
    curl -sSf --max-time 900 -o "$SRC/$file" "$CDN/$3/$master/$1/$file"
}

# Poly Haven ships some packs' data maps as 16-BIT PNGs. Bevy would upload those as Rgba16Unorm
# (a wgpu format behind an optional feature) and basisu gains nothing from the extra depth, so
# truncate to 8-bit — bit depth only, no colour-space change.
to_8bit() {
    depth="$(magick identify -format '%z' "$1")" || {
        echo "magick cannot read $1; delete it so the next run fetches it again" >&2
        exit 1
    }
    case "$depth" in
        16) echo "8-bit ▸ $(basename "$1")"
            magick "$1" -depth 8 -define png:color-type=2 -strip "$1.8" && mv "$1.8" "$1" ;;
    esac
}

# Pull a pack's three masters down and normalize their bit depth.
stage_sources() {
    fetch "$1" diff jpg
    fetch "$1" nor_gl png
    fetch "$1" arm png
    to_8bit "$(source_of "$1" nor_gl png)"
    to_8bit "$(source_of "$1" arm png)"
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
    out="assets/terrain/$pack"
    res=4096
    if is_construction "$pack"; then
        out="assets/constructions/$pack"
        res=1024
    fi
    mkdir -p "$out"
    stage_sources "$pack"
    encode albedo "$res" "$out/${pack}_diff.ktx2" "$(source_of "$pack" diff jpg)"
    encode nor_gl "$res" "$out/${pack}_nor_gl.ktx2" "$(source_of "$pack" nor_gl png)"
    encode arm "$res" "$out/${pack}_arm.ktx2" "$(source_of "$pack" arm png)"
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
        albedo_sources="$albedo_sources $(source_of "$pack" diff jpg)"
        nor_sources="$nor_sources $(source_of "$pack" nor_gl png)"
        arm_sources="$arm_sources $(source_of "$pack" arm png)"
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
    if [ "$target" != "$BASE_PACK" ] && ! is_construction "$target"; then
        echo "unknown target: $target" >&2
        exit 1
    fi
    packs="$packs $target"
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
