"""Minimal dependency-free GLB (glTF binary) reader/writer + accessor access."""

import json
import struct

COMP = {
    5120: ("b", 1),
    5121: ("B", 1),
    5122: ("h", 2),
    5123: ("H", 2),
    5125: ("I", 4),
    5126: ("f", 4),
}
NCOMP = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}

# Every rewrite of tiger_1.glb pads its JSON chunk to this fixed length so the BIN chunk
# (59.6 MB of it embedded KTX2 that never changes) keeps a constant file offset across
# revisions. Original chunk is 84 880 B; 96 KiB leaves room for added accessors.
JSON_PAD = 98304


class Glb:
    def __init__(self, gltf, bin_, json_chunk_len=None):
        self.gltf = gltf
        self.bin = bytearray(bin_)
        self.json_chunk_len = json_chunk_len

    @classmethod
    def load(cls, path):
        data = open(path, "rb").read()
        magic, ver, total = struct.unpack_from("<III", data, 0)
        assert magic == 0x46546C67, "not a glb"
        off = 12
        gltf = None
        bin_ = b""
        jlen = None
        while off < total:
            clen, ctype = struct.unpack_from("<II", data, off)
            chunk = data[off + 8 : off + 8 + clen]
            if ctype == 0x4E4F534A:
                gltf = json.loads(chunk.decode("utf-8"))
                jlen = clen
            elif ctype == 0x004E4942:
                bin_ = chunk
            off += 8 + clen
        return cls(gltf, bin_, jlen)

    def save(self, path, json_pad_to=None):
        js = json.dumps(self.gltf, separators=(",", ":")).encode("utf-8")
        js += b" " * ((4 - len(js) % 4) % 4)
        # Padding the JSON chunk back to its original length keeps the BIN chunk at the
        # same file offset, so a rewritten glb stays a cheap binary delta against the
        # previous revision (the 59.6 MB of embedded KTX2 never moves).
        if json_pad_to is not None:
            if len(js) > json_pad_to:
                raise ValueError(f"json {len(js)} B exceeds pad target {json_pad_to} B")
            js += b" " * (json_pad_to - len(js))
        bn = bytes(self.bin)
        bn += b"\x00" * ((4 - len(bn) % 4) % 4)
        total = 12 + 8 + len(js) + (8 + len(bn) if bn else 0)
        out = bytearray()
        out += struct.pack("<III", 0x46546C67, 2, total)
        out += struct.pack("<II", len(js), 0x4E4F534A) + js
        if bn:
            out += struct.pack("<II", len(bn), 0x004E4942) + bn
        open(path, "wb").write(bytes(out))

    # -- accessor reads -------------------------------------------------
    def read_accessor(self, idx):
        acc = self.gltf["accessors"][idx]
        n = NCOMP[acc["type"]]
        fmt, size = COMP[acc["componentType"]]
        count = acc["count"]
        if "bufferView" not in acc:
            return [tuple([0] * n) for _ in range(count)]
        bv = self.gltf["bufferViews"][acc["bufferView"]]
        base = bv.get("byteOffset", 0) + acc.get("byteOffset", 0)
        stride = bv.get("byteStride") or n * size
        out = []
        for i in range(count):
            o = base + i * stride
            vals = struct.unpack_from("<" + fmt * n, self.bin, o)
            out.append(vals if n > 1 else vals[0])
        return out

    # -- appending ------------------------------------------------------
    def add_bufferview(self, data, target=None, stride=None):
        while len(self.bin) % 4:
            self.bin.append(0)
        off = len(self.bin)
        self.bin += data
        bv = {"buffer": 0, "byteOffset": off, "byteLength": len(data)}
        if target:
            bv["target"] = target
        if stride:
            bv["byteStride"] = stride
        self.gltf.setdefault("bufferViews", []).append(bv)
        return len(self.gltf["bufferViews"]) - 1

    def add_accessor(self, bv, comp_type, type_, count, minmax=None):
        acc = {
            "bufferView": bv,
            "componentType": comp_type,
            "count": count,
            "type": type_,
        }
        if minmax:
            acc["min"], acc["max"] = minmax
        self.gltf.setdefault("accessors", []).append(acc)
        return len(self.gltf["accessors"]) - 1

    def sync_buffer_len(self):
        while len(self.bin) % 4:
            self.bin.append(0)
        self.gltf["buffers"][0]["byteLength"] = len(self.bin)
        self.gltf["buffers"][0].pop("uri", None)


def tri_count(glb, prim):
    if "indices" in prim:
        return glb.gltf["accessors"][prim["indices"]]["count"] // 3
    return glb.gltf["accessors"][prim["attributes"]["POSITION"]]["count"] // 3


def vert_count(glb, prim):
    return glb.gltf["accessors"][prim["attributes"]["POSITION"]]["count"]
