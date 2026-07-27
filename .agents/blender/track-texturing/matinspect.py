"""Inspect one glTF material's textures inside a .glb: names, payload hashes, channel stats.

Usage: matinspect.py <file.glb> <MaterialName> [dump_dir]

Prints, for every texture the material references, the source image name, mime type,
payload size + sha256, and per-channel min/mean/max of the decoded pixels. That is enough
to prove which images a material actually ships (payloads, not just names) and to verify
ORM channel semantics (G=roughness, B=metallic).
"""
import json, struct, sys, hashlib, io, os
import numpy as np
from PIL import Image


def load(path):
    with open(path, 'rb') as f:
        magic, ver, total = struct.unpack("<III", f.read(12))
        assert magic == 0x46546C67, "not a glb"
        js = None
        bin_chunk = None
        while f.tell() < total:
            hdr = f.read(8)
            if len(hdr) < 8:
                break
            ln, kind = struct.unpack("<II", hdr)
            data = f.read(ln)
            if kind == 0x4E4F534A:
                js = json.loads(data)
            elif kind == 0x004E4942:
                bin_chunk = data
        return js, bin_chunk


def image_bytes(d, blob, img_idx):
    img = d['images'][img_idx]
    if 'bufferView' in img:
        bv = d['bufferViews'][img['bufferView']]
        off = bv.get('byteOffset', 0)
        return blob[off:off + bv['byteLength']]
    raise SystemExit(f"image {img_idx} is external (uri={img.get('uri')})")


def stats(raw):
    im = Image.open(io.BytesIO(raw))
    a = np.asarray(im).astype(np.float64) / 255.0
    out = {'size': f"{im.width}x{im.height}", 'mode': im.mode}
    if a.ndim == 2:
        a = a[:, :, None]
    for i, ch in enumerate("RGBA"[:a.shape[2]]):
        c = a[:, :, i]
        out[ch] = f"min={c.min():.3f} mean={c.mean():.3f} max={c.max():.3f} std={c.std():.3f}"
    return out


def main():
    path, matname = sys.argv[1], sys.argv[2]
    dump = sys.argv[3] if len(sys.argv) > 3 else None
    d, blob = load(path)

    # every embedded image in the file, so we can prove absence of old payloads too
    print(f"== all images in {os.path.basename(path)} ==")
    for i, im in enumerate(d.get('images', [])):
        raw = image_bytes(d, blob, i)
        print(f"  [{i}] name={im.get('name')!r} mime={im.get('mimeType')} "
              f"bytes={len(raw)} sha256={hashlib.sha256(raw).hexdigest()[:16]}")

    mats = [m for m in d.get('materials', []) if m.get('name') == matname]
    if not mats:
        raise SystemExit(f"material {matname!r} not found; have: "
                         f"{[m.get('name') for m in d.get('materials', [])]}")
    m = mats[0]
    print(f"\n== material {matname!r} ==")
    print(json.dumps(m, indent=1, sort_keys=True))

    pbr = m.get('pbrMetallicRoughness', {})
    slots = {
        'baseColor': pbr.get('baseColorTexture'),
        'metallicRoughness': pbr.get('metallicRoughnessTexture'),
        'normal': m.get('normalTexture'),
        'occlusion': m.get('occlusionTexture'),
        'emissive': m.get('emissiveTexture'),
    }
    total = 0
    for slot, ref in slots.items():
        if not ref:
            print(f"\n  {slot}: (none)")
            continue
        tex = d['textures'][ref['index']]
        src = tex['source']
        raw = image_bytes(d, blob, src)
        total += len(raw)
        st = stats(raw)
        print(f"\n  {slot}: texture={ref['index']} image={src} "
              f"name={d['images'][src].get('name')!r} texcoord={ref.get('texCoord', 0)}")
        print(f"    bytes={len(raw)} ({len(raw)/1e6:.3f} MB) "
              f"sha256={hashlib.sha256(raw).hexdigest()[:16]}")
        print(f"    {st['size']} {st['mode']}")
        for ch in "RGBA":
            if ch in st:
                print(f"      {ch}: {st[ch]}")
        if dump:
            os.makedirs(dump, exist_ok=True)
            with open(os.path.join(dump, f"{slot}.png"), 'wb') as f:
                f.write(raw)
    print(f"\n  material texture payload total: {total} bytes ({total/1e6:.3f} MB)")

    # KHR_texture_transform presence (a Mapping node would show up here)
    used = d.get('extensionsUsed', [])
    print(f"\n  extensionsUsed = {used}")
    if 'KHR_texture_transform' in used:
        print("  !! KHR_texture_transform present — a Mapping node may have crept in")


if __name__ == '__main__':
    main()
