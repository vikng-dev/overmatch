"""Dump embedded glb images (by index) to files."""

import os
import sys

import glblib

SRC, OUTDIR = sys.argv[1], sys.argv[2]
idxs = [int(x) for x in sys.argv[3:]]
os.makedirs(OUTDIR, exist_ok=True)
g = glblib.Glb.load(SRC)
for i in idxs:
    im = g.gltf["images"][i]
    bv = g.gltf["bufferViews"][im["bufferView"]]
    off = bv.get("byteOffset", 0)
    data = bytes(g.bin[off : off + bv["byteLength"]])
    ext = ".ktx2" if "ktx2" in im.get("mimeType", "") else ".png"
    name = f"{i}_{im.get('name', 'img')}{ext}".replace("/", "_")
    open(os.path.join(OUTDIR, name), "wb").write(data)
    print(name, len(data))
