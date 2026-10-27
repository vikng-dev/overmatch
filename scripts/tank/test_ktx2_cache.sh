#!/bin/sh
# test_ktx2_cache.sh — the texture cache in `scripts/encode-tank-ktx2.sh`, over a synthetic glb.
#
#     sh scripts/tank/test_ktx2_cache.sh
#
# It runs the REAL bake script on a glb written here: three 16x16 PNGs sampled as a baseColor, a
# normal map and a metallicRoughness map. OVERMATCH_BASISU names a wrapper that logs every
# invocation and then execs the real basisu; OVERMATCH_KTX2_CACHE names a fresh dir. No Blender, no
# cargo, and nothing in the work tree is written.
#
# Three invariants, each with the mutant of the key that breaks it:
#   1. A second bake of the same input runs the encoder zero times, prints one `ktx2  ▸ … — cached`
#      line per image and writes a byte-identical glb. The time saved is the point of the cache; the
#      line per image is the GUI adapter's progress count; the identical bytes are what
#      `build.py verify`'s byte-compare against the committed glb relies on.
#      Mutant: the key holds a per-run value (the work dir) — every bake misses.
#   2. The same image bytes sampled under another role miss. A hit would hand a linear slot an sRGB
#      encode, or a colour slot a linear one.
#      Mutant: the key holds neither the role nor the role's flags — the re-roled image hits. The
#      role name alone is an equivalent mutant while every role's flags differ, and survives.
#   3. Another encoder identity (`basisu -version`'s first line) misses. A hit would let `verify`
#      pass a basisu upgrade on bytes the old encoder made.
#      Mutant: the key leaves the encoder out — the "upgraded" bake hits.
#
# Needs a real basisu (OVERMATCH_BASISU, else PATH); without one it prints a skip line and exits 0.

set -u

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
BAKE=$ROOT/scripts/encode-tank-ktx2.sh

REAL=$(command -v "${OVERMATCH_BASISU:-basisu}" 2>/dev/null) || REAL=""
if [ -z "$REAL" ]; then
    echo "test_ktx2_cache ▸ skip — no basisu (brew install basis_universal, or set OVERMATCH_BASISU)"
    exit 0
fi

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/ktx2-cache-test.XXXXXX") || exit 1
trap 'rm -rf "$SCRATCH"' EXIT
trap 'exit 1' INT TERM

# ── the encoder double ───────────────────────────────────────────────────────────────────────────
# One log line per invocation, then the real encoder. KTX2_CACHE_TEST_ENCODER, when set, is the
# first line `-version` prints instead: an encoder identity no installed basisu has.
cat > "$SCRATCH/basisu" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$KTX2_CACHE_TEST_LOG"
if [ "$*" = -version ] && [ -n "${KTX2_CACHE_TEST_ENCODER:-}" ]; then
    printf '%s\n' "$KTX2_CACHE_TEST_ENCODER"
    exit 0
fi
exec "$KTX2_CACHE_TEST_REAL" "$@"
EOF
chmod +x "$SCRATCH/basisu"

# ── the synthetic glbs ───────────────────────────────────────────────────────────────────────────
# a.glb samples image 0 as baseColor (srgb), 1 as normal, 2 as metallicRoughness (linear). b.glb
# holds the same three images byte for byte and swaps the colour and data slots, so images 0 and 2
# change role and image 1 keeps its own. No meshes: nothing here is normal-mapped geometry, so the
# bake's tangent law has nothing to ask. `accessors` is present and empty because the bake's
# derivation check reads that key unconditionally.
python3 - "$SCRATCH" <<'EOF' || { echo "test_ktx2_cache ▸ could not write the glbs"; exit 1; }
import json, struct, sys, zlib
from pathlib import Path

scratch = Path(sys.argv[1])
SIDE = 16


def png(seed):
    raw = bytearray()
    for y in range(SIDE):
        raw.append(0)
        for x in range(SIDE):
            raw += bytes(((x * 16 + seed * 50) % 256, (y * 16 + seed * 90) % 256,
                          (x * y + seed * 70) % 256))

    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data)))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", SIDE, SIDE, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(raw))) + chunk(b"IEND", b""))


IMAGES = [png(seed) for seed in (1, 2, 3)]


def glb(material):
    bin_, views = bytearray(), []
    for data in IMAGES:
        bin_ += b"\0" * (-len(bin_) % 4)
        views.append({"buffer": 0, "byteOffset": len(bin_), "byteLength": len(data)})
        bin_ += data
    bin_ += b"\0" * (-len(bin_) % 4)
    js = {
        "asset": {"version": "2.0"},
        "accessors": [],
        "buffers": [{"byteLength": len(bin_)}],
        "bufferViews": views,
        "images": [{"bufferView": i, "mimeType": "image/png"} for i in range(len(IMAGES))],
        "textures": [{"source": i} for i in range(len(IMAGES))],
        "materials": [material],
    }
    jb = json.dumps(js, separators=(",", ":")).encode()
    jb += b" " * (-len(jb) % 4)
    return (struct.pack("<III", 0x46546C67, 2, 12 + 8 + len(jb) + 8 + len(bin_))
            + struct.pack("<II", len(jb), 0x4E4F534A) + jb
            + struct.pack("<II", len(bin_), 0x004E4942) + bytes(bin_))


def material(colour, data):
    return {"name": "hull", "normalTexture": {"index": 1}, "pbrMetallicRoughness": {
        "baseColorTexture": {"index": colour}, "metallicRoughnessTexture": {"index": data}}}


(scratch / "a.glb").write_bytes(glb(material(0, 2)))
(scratch / "b.glb").write_bytes(glb(material(2, 0)))
EOF

# ── the harness ──────────────────────────────────────────────────────────────────────────────────

passed=0
failed=0

ok() { passed=$((passed + 1)); printf '  ok    %s\n' "$1"; }

bad() {
    failed=$((failed + 1))
    printf '  FAIL  %s\n        %s\n' "$1" "$2"
    sed 's/^/        | /' "$SCRATCH/bake.out"
}

# bake <in.glb> <out.glb> [encoder identity] — the real script, against the shared cache. Its
# output lands in bake.out and the wrapper's log in calls.log, both fresh per bake. TMPDIR points
# into the scratch dir because a failed bake keeps its work dir there for inspection.
bake() {
    : > "$SCRATCH/calls.log"
    mkdir -p "$SCRATCH/tmp"
    (cd "$ROOT" && TMPDIR="$SCRATCH/tmp" \
        OVERMATCH_BASISU="$SCRATCH/basisu" OVERMATCH_KTX2_CACHE="$SCRATCH/cache" \
        KTX2_CACHE_TEST_LOG="$SCRATCH/calls.log" KTX2_CACHE_TEST_REAL="$REAL" \
        KTX2_CACHE_TEST_ENCODER="${3:-}" "$BAKE" "$1" "$2") > "$SCRATCH/bake.out" 2>&1
}

encodes() { grep -c -e '-output_file' "$SCRATCH/calls.log"; }

encoded() { grep -q -e "-file .*/src/$1 " "$SCRATCH/calls.log"; }

hits() { grep -c '^ktx2  ▸ .* — cached$' "$SCRATCH/bake.out"; }

# ── the cold bake every invariant reads ──────────────────────────────────────────────────────────

echo "test_ktx2_cache ▸ $REAL — $("$REAL" -version 2>/dev/null | sed -n 1p)"
NAME="setup: a cold bake into an empty cache encodes all 3 images"
if ! bake "$SCRATCH/a.glb" "$SCRATCH/cold.glb"; then
    bad "$NAME" "the cold bake failed"
elif [ "$(encodes)" -ne 3 ]; then
    bad "$NAME" "it ran $(encodes) encode(s)"
fi
if [ "$failed" -ne 0 ]; then
    echo "test_ktx2_cache ▸ setup failed — no invariant can be read"
    exit 1
fi

# ── the invariants ───────────────────────────────────────────────────────────────────────────────

NAME="1. a second bake of the same input encodes nothing and writes the same bytes"
if ! bake "$SCRATCH/a.glb" "$SCRATCH/warm.glb"; then
    bad "$NAME" "the warm bake failed"
elif [ "$(encodes)" -ne 0 ] || [ "$(hits)" -ne 3 ]; then
    bad "$NAME" "it ran $(encodes) encode(s) and printed $(hits) cached line(s); want 0 and 3"
elif ! cmp -s "$SCRATCH/cold.glb" "$SCRATCH/warm.glb"; then
    bad "$NAME" "the warm glb differs from the cold one"
else
    ok "$NAME"
fi

NAME="2. the same image under another role misses"
if ! bake "$SCRATCH/b.glb" "$SCRATCH/reroled.glb"; then
    bad "$NAME" "the re-roled bake failed ($(encodes) encode(s))"
elif ! encoded 00.png || ! encoded 02.png; then
    bad "$NAME" "images 0 and 2 changed role and were not both encoded: $(encodes) encode(s)"
else
    ok "$NAME"
fi

NAME="3. another encoder identity misses"
if ! bake "$SCRATCH/a.glb" "$SCRATCH/upgraded.glb" "Basis Universal (test double) v0.0.0"; then
    bad "$NAME" "the bake under another encoder identity failed"
elif [ "$(encodes)" -ne 3 ]; then
    bad "$NAME" "it ran $(encodes) encode(s); want 3"
else
    ok "$NAME"
fi

# ── verdict ──────────────────────────────────────────────────────────────────────────────────────

printf 'test_ktx2_cache ▸ %s invariants, %s passed, %s failed\n' \
    "$((passed + failed))" "$passed" "$failed"
[ "$failed" -eq 0 ]
