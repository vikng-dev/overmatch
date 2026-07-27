"""glb_ktx2.py — unpack / repack / diff / verify halves of `scripts/encode-tank-ktx2.sh`.

Four subcommands. The first three are driven by the shell script; `verify` is the standalone
gate the pre-push hook runs (see `scripts/hooks/pre-push`):

    unpack <in.glb> <work>          split the glb, write every embedded image to <work>/src and
                                    derive each one's colour ROLE from the materials that sample it
    repack <in.glb> <work> <out>    rebuild the glb with <work>/ktx2/<i>.ktx2 in place of the images
    diff   <a.glb> <b.glb>          prove nothing but images/textures changed (hashes accessor data)
    verify [--allow-pointer] <glb>  assert every embedded image is mipped KTX2 — exit 1 if not

The role table is the point of the unpack step: glTF fixes the colour space of every texture slot,
so it can be read off the document instead of typed into a table that rots on the next re-export.
"""

import hashlib
import json
import struct
import sys
from pathlib import Path

KTX2_MAGIC = bytes([0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A])

# glTF texture slot -> colour role. `srgb` = display-referred colour, `normal` = tangent-space
# direction data, `linear` = scalar material data. Extension slots included so a future material
# tweak in Blender does not silently fall through to the wrong encoder flags.
SLOT_ROLES = {
    "baseColorTexture": "srgb",
    "emissiveTexture": "srgb",
    "specularColorTexture": "srgb",
    "sheenColorTexture": "srgb",
    "normalTexture": "normal",
    "clearcoatNormalTexture": "normal",
    "metallicRoughnessTexture": "linear",
    "occlusionTexture": "linear",
    "clearcoatTexture": "linear",
    "clearcoatRoughnessTexture": "linear",
    "specularTexture": "linear",
    "anisotropyTexture": "linear",
    "sheenRoughnessTexture": "linear",
    "transmissionTexture": "linear",
    "thicknessTexture": "linear",
    "iridescenceTexture": "linear",
    "iridescenceThicknessTexture": "linear",
}

EXT_BY_MIME = {"image/png": "png", "image/jpeg": "jpg", "image/ktx2": "ktx2"}


def read_glb(path):
    """Return (json_dict, bin_bytes) for a binary glTF."""
    raw = Path(path).read_bytes()
    magic, version, _total = struct.unpack_from("<III", raw, 0)
    if magic != 0x46546C67:
        raise SystemExit(f"{path}: not a glb")
    if version != 2:
        raise SystemExit(f"{path}: glb version {version}, expected 2")
    off, js, bin_ = 12, None, b""
    while off < len(raw):
        length, kind = struct.unpack_from("<II", raw, off)
        chunk = raw[off + 8 : off + 8 + length]
        if kind == 0x4E4F534A:
            js = json.loads(chunk)
        elif kind == 0x004E4942:
            bin_ = chunk
        off += 8 + length + (-length % 4)
    if js is None:
        raise SystemExit(f"{path}: no JSON chunk")
    return js, bin_


def write_glb(path, js, bin_):
    """Write a binary glTF, padding both chunks to the 4-byte alignment the format requires."""
    jb = json.dumps(js, separators=(",", ":")).encode()
    jb += b" " * (-len(jb) % 4)
    bb = bin_ + b"\0" * (-len(bin_) % 4)
    total = 12 + 8 + len(jb) + (8 + len(bb) if bb else 0)
    with open(path, "wb") as f:
        f.write(struct.pack("<III", 0x46546C67, 2, total))
        f.write(struct.pack("<II", len(jb), 0x4E4F534A))
        f.write(jb)
        if bb:
            f.write(struct.pack("<II", len(bb), 0x004E4942))
            f.write(bb)


def view_bytes(js, bin_, index):
    """Raw bytes of bufferView `index` (single-buffer glb only, which is what Blender exports)."""
    bv = js["bufferViews"][index]
    if bv.get("buffer", 0) != 0:
        raise SystemExit("multi-buffer glb is not supported by this bake")
    start = bv.get("byteOffset", 0)
    return bin_[start : start + bv["byteLength"]]


def image_roles(js):
    """image index -> role, derived from every material slot that samples it. Fails loud on a
    conflict (one image used as both colour and data) or on an image nothing references."""
    tex_source = {i: t["source"] for i, t in enumerate(js.get("textures", []))}
    roles = {}

    def visit(container):
        for slot, role in SLOT_ROLES.items():
            info = container.get(slot)
            if isinstance(info, dict) and "index" in info:
                img = tex_source[info["index"]]
                if roles.setdefault(img, role) != role:
                    raise SystemExit(
                        f"image {img} is sampled as both {roles[img]} and {role}; "
                        "the bake cannot pick one colour space for it"
                    )

    for mat in js.get("materials", []):
        visit(mat)
        visit(mat.get("pbrMetallicRoughness", {}))
        for ext in mat.get("extensions", {}).values():
            if isinstance(ext, dict):
                visit(ext)

    missing = [i for i in range(len(js.get("images", []))) if i not in roles]
    if missing:
        raise SystemExit(
            f"images {missing} are not referenced by any known material slot; add the slot to "
            "SLOT_ROLES rather than guessing a colour space"
        )
    return roles


def ktx2_header(data):
    """(vkFormat, width, height, levels, supercompression) from a KTX2 file header."""
    if data[:12] != KTX2_MAGIC:
        raise SystemExit("not a KTX2 file")
    fmt, _ts, w, h, _d, _layers, _faces, levels, sc = struct.unpack_from("<9I", data, 12)
    return fmt, w, h, levels, sc


def cmd_unpack(in_glb, work):
    js, bin_ = read_glb(in_glb)
    work = Path(work)

    # Refuse to bake a bake. basisu takes PNG/JPEG in, not KTX2, so this would either fail deep in
    # the encode loop or (worse) round-trip an already-lossy UASTC payload through a second
    # compression. The input is always a FRESH Blender export; the output is the tracked glb.
    if any(im.get("mimeType") == "image/ktx2" for im in js.get("images", [])):
        raise SystemExit(
            f"{in_glb} is already mip-baked (image/ktx2). The bake reads a fresh, mipless Blender "
            "export — you have pointed it at its own output."
        )

    roles = image_roles(js)
    lines = []
    for i, im in enumerate(js.get("images", [])):
        if "bufferView" not in im:
            raise SystemExit(f"image {i} is not embedded (uri images are out of scope for this bake)")
        ext = EXT_BY_MIME.get(im.get("mimeType"))
        if ext is None:
            raise SystemExit(f"image {i}: unhandled mimeType {im.get('mimeType')!r}")
        name = f"{i:02d}.{ext}"
        (work / "src" / name).write_bytes(view_bytes(js, bin_, im["bufferView"]))
        lines.append(f"{i} {roles[i]} {name}")
        print(f"image ▸ [{i}] {im.get('name','?')!r} {im['mimeType']} role={roles[i]}")
    (work / "roles.txt").write_text("\n".join(lines) + "\n")


def cmd_repack(in_glb, work, out_glb):
    js, bin_ = read_glb(in_glb)
    work = Path(work)

    # bufferView index -> replacement bytes, for the views the images live in.
    replace = {}
    for i, im in enumerate(js.get("images", [])):
        data = (work / "ktx2" / f"{i}.ktx2").read_bytes()
        ktx2_header(data)  # fails loud if basisu wrote something else
        replace[im["bufferView"]] = data
        im["mimeType"] = "image/ktx2"

    # A bufferView shared between an image and anything else would be silently corrupted below.
    spans = sorted(
        (bv.get("byteOffset", 0), bv.get("byteOffset", 0) + bv["byteLength"], i)
        for i, bv in enumerate(js["bufferViews"])
    )
    for (a0, a1, ai), (b0, _b1, bi) in zip(spans, spans[1:]):
        if b0 < a1 and (ai in replace or bi in replace):
            raise SystemExit(f"bufferViews {ai} and {bi} overlap and one holds an image")

    # Rebuild the BIN chunk in bufferView order. Indices and per-view contents are preserved, so
    # every accessor keeps its meaning; only offsets move. 4-byte alignment covers every accessor
    # component type glTF allows.
    out = bytearray()
    for i, bv in enumerate(js["bufferViews"]):
        data = replace.get(i)
        if data is None:
            data = view_bytes(js, bin_, i)
        out += b"\0" * (-len(out) % 4)
        bv["byteOffset"] = len(out)
        bv["byteLength"] = len(data)
        out += data
    js["buffers"][0]["byteLength"] = len(out)

    # The extension block is inert for bevy (it reads `source` directly) but makes the file honest
    # for every other tool. `extensionsRequired` stays unset on purpose — see the shell script.
    for t in js.get("textures", []):
        t.setdefault("extensions", {})["KHR_texture_basisu"] = {"source": t["source"]}
    used = js.setdefault("extensionsUsed", [])
    if "KHR_texture_basisu" not in used:
        used.append("KHR_texture_basisu")

    write_glb(out_glb, js, bytes(out))
    print(f"glb   ▸ {out_glb} — {Path(out_glb).stat().st_size / 1e6:.1f} MB")


def accessor_digest(js, bin_):
    """Hash of every accessor's actual bytes — the check that geometry survived the repack."""
    out = []
    for i, acc in enumerate(js.get("accessors", [])):
        if "bufferView" not in acc:
            out.append((i, "sparse-or-zero"))
            continue
        bv = js["bufferViews"][acc["bufferView"]]
        start = bv.get("byteOffset", 0) + acc.get("byteOffset", 0)
        data = bin_[start : bv.get("byteOffset", 0) + bv["byteLength"]]
        out.append((i, hashlib.sha256(data).hexdigest()))
    return out


def cmd_diff(a_path, b_path):
    a, abin = read_glb(a_path)
    b, bbin = read_glb(b_path)
    bad = 0

    # 1. Everything that is not textures/images/buffer plumbing must be byte-identical JSON.
    untouched = set(a) | set(b)
    untouched -= {"images", "textures", "bufferViews", "buffers", "extensionsUsed"}
    for key in sorted(untouched):
        same = a.get(key) == b.get(key)
        print(f"json  ▸ {key:<16} {'identical' if same else 'CHANGED'}")
        bad += not same

    # 2. bufferViews may move but must not otherwise change; image views may also resize.
    img_views = {im["bufferView"] for im in a.get("images", [])}
    if len(a["bufferViews"]) != len(b["bufferViews"]):
        print("view  ▸ bufferView COUNT CHANGED")
        bad += 1
    else:
        for i, (x, y) in enumerate(zip(a["bufferViews"], b["bufferViews"])):
            drop = {"byteOffset"} | ({"byteLength"} if i in img_views else set())
            if {k: v for k, v in x.items() if k not in drop} != {
                k: v for k, v in y.items() if k not in drop
            }:
                print(f"view  ▸ bufferView {i} CHANGED beyond its offset")
                bad += 1

    # 3. The real proof: every accessor's bytes hash the same on both sides.
    da, db = accessor_digest(a, abin), accessor_digest(b, bbin)
    same = da == db
    print(f"data  ▸ {len(da)} accessors {'byte-identical' if same else 'DIVERGED'}")
    bad += not same
    if not same:
        for (i, x), (_, y) in zip(da, db):
            if x != y:
                print(f"        accessor {i}: {x[:12]} != {y[:12]}")

    # 4. Non-image bufferViews (morph targets, anything an accessor does not reach) too.
    others = [i for i in range(len(a["bufferViews"])) if i not in img_views]
    ha = hashlib.sha256(b"".join(view_bytes(a, abin, i) for i in others)).hexdigest()
    hb = hashlib.sha256(b"".join(view_bytes(b, bbin, i) for i in others)).hexdigest()
    print(f"data  ▸ {len(others)} non-image bufferViews {'identical' if ha == hb else 'DIVERGED'}")
    bad += ha != hb

    # 5. Textures: the only legal change is the added extension pointing at the same image.
    for i, (x, y) in enumerate(zip(a["textures"], b["textures"])):
        legal = dict(x)
        legal["extensions"] = {"KHR_texture_basisu": {"source": x["source"]}}
        if y != legal:
            print(f"tex   ▸ texture {i} changed in an unexpected way: {y}")
            bad += 1
    print(f"tex   ▸ {len(a['textures'])} textures: source/sampler kept, KHR_texture_basisu added")

    # 6. Images: report the swap.
    print(f"{'img':<5} {'name':<24} {'role':<7} {'size':<11} {'was':>10} {'now':>10} {'mips':>5}")
    roles = image_roles(a)
    tot_a = tot_b = 0
    for i, (x, y) in enumerate(zip(a["images"], b["images"])):
        pa, pb = view_bytes(a, abin, x["bufferView"]), view_bytes(b, bbin, y["bufferView"])
        fmt, w, h, levels, sc = ktx2_header(pb)
        tot_a += len(pa)
        tot_b += len(pb)
        print(
            f"[{i}]   {x.get('name','?')[:24]:<24} {roles[i]:<7} {w}x{h:<7} "
            f"{len(pa)/1e6:9.2f}M {len(pb)/1e6:9.2f}M {levels:5d}"
            + ("" if levels > 1 else "   <-- NO MIP CHAIN")
            + ("" if sc == 2 else f"   <-- supercompression={sc}, expected zstd(2)")
        )
        bad += levels <= 1
    print(f"      {'total':<24} {'':<7} {'':<11} {tot_a/1e6:9.2f}M {tot_b/1e6:9.2f}M")
    sa, sb = Path(a_path).stat().st_size, Path(b_path).stat().st_size
    print(f"      {'glb file':<24} {'':<7} {'':<11} {sa/1e6:9.2f}M {sb/1e6:9.2f}M")

    print("VERDICT: " + ("structurally clean" if bad == 0 else f"{bad} PROBLEM(S)"))
    return 1 if bad else 0


def cmd_verify(*args):
    """The ship gate: every embedded image must be KTX2 with a real mip chain.

    Runs on any glb-shaped file — including a raw `.git/lfs/objects/**` blob, which is how the
    pre-push hook checks the COMMITTED bytes rather than whatever is sitting in the work tree.

    An unfetched git-lfs POINTER is an error by default, because "I could not read it" must not
    read as "it is fine" in the release workflow. `--allow-pointer` downgrades it to a skip, for
    the pre-push hook: a dev whose clone never smudged the glb cannot have changed it either, and
    failing their push over someone else's asset would be noise.

    Deliberately streaming: it reads the 12-byte header, the JSON chunk, and 48 bytes per image,
    never the ~63 MB payload. That keeps it at a few milliseconds, which is what earns it a place
    in a hook that already pays for a compile.
    """
    allow_pointer = "--allow-pointer" in args
    (path,) = [a for a in args if not a.startswith("--")]
    with open(path, "rb") as f:
        head = f.read(12)
        if head.startswith(b"version http"):  # first line of a git-lfs pointer
            if not allow_pointer:
                raise SystemExit(
                    f"{path} is an unfetched git-lfs pointer — cannot verify the shipped bytes. "
                    "Run `git lfs pull` first (or pass --allow-pointer to treat this as a skip)."
                )
            print(f"mip   ▸ {path}: git-lfs pointer, content not fetched — SKIPPED")
            return 0
        if len(head) < 12 or struct.unpack_from("<I", head, 0)[0] != 0x46546C67:
            raise SystemExit(f"{path}: not a glb")

        # Walk the chunk table without materializing the BIN chunk; remember where BIN starts so
        # image payloads can be reached by seek.
        js, bin_off, off = None, None, 12
        while True:
            f.seek(off)
            hdr = f.read(8)
            if len(hdr) < 8:
                break
            length, kind = struct.unpack("<II", hdr)
            if kind == 0x4E4F534A:
                js = json.loads(f.read(length))
            elif kind == 0x004E4942:
                bin_off = off + 8
            off += 8 + length + (-length % 4)
        if js is None:
            raise SystemExit(f"{path}: no JSON chunk")

        images = js.get("images", [])
        if not images:
            raise SystemExit(f"{path}: no embedded images — is this the tank glb?")

        bad, levels_seen, unsupercompressed = [], [], 0
        for i, im in enumerate(images):
            name = im.get("name", "?")
            mime = im.get("mimeType")
            if mime != "image/ktx2":
                bad.append(f"[{i}] {name}: mimeType {mime!r}, expected 'image/ktx2'")
                continue
            if "bufferView" not in im or bin_off is None:
                bad.append(f"[{i}] {name}: not embedded in the BIN chunk")
                continue
            bv = js["bufferViews"][im["bufferView"]]
            f.seek(bin_off + bv.get("byteOffset", 0))
            _fmt, w, h, lv, sc = ktx2_header(f.read(48))  # 12-byte magic + 9 u32 fields
            levels_seen.append(lv)
            unsupercompressed += sc != 2
            if lv <= 1:
                bad.append(f"[{i}] {name}: {w}x{h} KTX2 with {lv} mip level — no mip chain")

    if bad:
        print(f"mip   ▸ {path}", file=sys.stderr)
        for line in bad:
            print(f"        {line}", file=sys.stderr)
        print(
            "\n\033[31m✗ mipless tank glb.\033[0m  This is the shimmer-on-every-rivet build.\n"
            "  Re-export from Blender (the export helper bakes automatically), or bake in place:\n"
            "    scripts/encode-tank-ktx2.sh <freshly-exported.glb> assets/tiger_1/tiger_1.glb\n",
            file=sys.stderr,
        )
        return 1

    note = "" if not unsupercompressed else f", {unsupercompressed} not zstd-supercompressed"
    print(
        f"mip   ▸ {len(images)} images, all image/ktx2, "
        f"mip chains {min(levels_seen)}..{max(levels_seen)}{note}"
    )
    return 0


if __name__ == "__main__":
    cmd, *args = sys.argv[1:]
    sys.exit(
        {"unpack": cmd_unpack, "repack": cmd_repack, "diff": cmd_diff, "verify": cmd_verify}[cmd](
            *args
        )
        or 0
    )
