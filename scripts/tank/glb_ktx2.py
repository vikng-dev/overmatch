"""glb_ktx2.py — the texture derivation's halves, and the three laws that judge what it produced.

Four subcommands. The first three are driven by `scripts/encode-tank-ktx2.sh`; `verify` is the
standalone gate the asset door and the pre-push hook run (`scripts/hooks/pre-push`):

    unpack <in.glb> <work>          split the glb, write every embedded image to <work>/src and
                                    derive each one's colour ROLE from the materials that sample it
    repack <in.glb> <work> <out>    rebuild the glb with <work>/ktx2/<i>.ktx2 in place of the images
    diff   <raw.glb> <baked.glb>    the whole derivation law, both documents in hand
    verify [--allow-pointer] <glb>  the two laws a baked document answers on its own

The role table is the point of the unpack step: glTF fixes the colour space of every texture slot,
so it can be read off the document instead of typed into a table that rots on the next re-export.

WHAT THIS FILE MAY CHECK
------------------------
Three laws, and nothing else. `D.STRUCTURAL_DERIVATION` says the bake moved nothing but texture
payloads; `D.TANGENTS` says a normal-mapped primitive carries the basis that map is read in;
`D.KTX2_MIPS` says every baked image is the UASTC/Zstd, fully mipped KTX2 the encoder promised.
No transform, animation, material-classification, node-reference, topology, UV, naming or census
lint belongs here — those are the source pass (`.agents/blender/export_tank.py`) and the consumer
contract (`src/bake/`), and a fourth home for them is how one law ends up with three answers.

`D.STRUCTURAL_DERIVATION` is the raw→baked half only. That a rebuilt candidate is byte-identical to
the TRACKED glb is the door's own `door.candidate-mismatch` (`scripts/tank/asset_door.py`), which is
where the tracked path is known.

Findings are `scripts/tank/report.Finding`, rendered and exit-coded by that module: every stage of
the door reads the same rows in the same order whichever half produced them.
"""

from __future__ import annotations

import hashlib
import json
import os
import struct
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import report  # noqa: E402  — the path above is what makes it importable
from report import Check, Finding, Severity, Stage, Subject, SubjectKind  # noqa: E402

# ── the laws ─────────────────────────────────────────────────────────────────────────────────────

STRUCTURAL_DERIVATION = Check(
    id="D.STRUCTURAL_DERIVATION",
    stage=Stage.DERIVATION,
    severity=Severity.ERROR,
    law="the raw and baked documents hold identical counts, order and non-texture JSON; non-image "
        "bufferView bytes and every accessor's bytes are identical apart from legal offsets; image "
        "and texture counts and references are unchanged; and the only texture mutation is an "
        "embedded PNG/JPEG payload becoming KTX2, plus the matching KHR_texture_basisu declaration",
)

TANGENTS = Check(
    id="D.TANGENTS",
    stage=Stage.DERIVATION,
    severity=Severity.ERROR,
    law="every primitive whose exported material has a normal texture carries a TANGENT accessor "
        "with the same element count as POSITION",
)

KTX2_MIPS = Check(
    id="D.KTX2_MIPS",
    stage=Stage.DERIVATION,
    severity=Severity.ERROR,
    law="every exported image has exactly one known material role, and every baked image is an "
        "embedded UASTC KTX2 of the raw image's dimensions carrying role-correct transfer "
        "metadata, Zstd supercompression, a complete mip chain and valid nonempty in-bounds level "
        "records, still selected by the same material texture references",
)

#: Said by every finding whose only honest fix is to run the derivation again.
REBAKE = ("re-run the export — `scripts/encode-tank-ktx2.sh <raw.glb> <out.glb>` is the derivation, "
          "and the tracked glb is whatever it last certified")

# ── the formats ──────────────────────────────────────────────────────────────────────────────────

KTX2_MAGIC = bytes([0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A])

#: KTX2 §3: 12-byte identifier, nine u32 header fields, then the 32-byte index.
KTX2_HEADER = 80

#: One level index record: byteOffset, byteLength, uncompressedByteLength, all u64.
KTX2_LEVEL_RECORD = 24

#: KHR_DF colour model of a UASTC payload, and the two transfer functions basisu writes.
DFD_UASTC = 166
DFD_LINEAR = 1
DFD_SRGB = 2

#: KTX2 §3.9 supercompression scheme 2 — Zstandard, which the encoder is invoked with.
ZSTD = 2

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

#: What each role's payload must declare as its transfer function. A colour map read as linear is a
#: washed-out tank; a normal map read as sRGB is a curved one.
ROLE_TRANSFER = {"srgb": DFD_SRGB, "normal": DFD_LINEAR, "linear": DFD_LINEAR}

EXT_BY_MIME = {"image/png": "png", "image/jpeg": "jpg", "image/ktx2": "ktx2"}

#: What an embedded image may be before the bake, and the magic that proves it is.
RAW_MAGIC = {"image/png": b"\x89PNG\r\n\x1a\n", "image/jpeg": b"\xff\xd8\xff"}

BAKED_MIME = "image/ktx2"

BASISU = "KHR_texture_basisu"

#: The document keys the derivation is allowed to touch. Everything else must survive it unchanged,
#: which is most of `D.STRUCTURAL_DERIVATION`.
DERIVED_KEYS = ("buffers", "bufferViews", "extensionsUsed", "images", "textures")

#: glTF component types, in bytes, and how many components each element type holds.
COMPONENT_BYTES = {5120: 1, 5121: 1, 5122: 2, 5123: 2, 5125: 4, 5126: 4}
TYPE_COMPONENTS = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT2": 4, "MAT3": 9, "MAT4": 16}
TYPE_COLUMNS = {"MAT2": 2, "MAT3": 3, "MAT4": 4}


# ── reading a glb ────────────────────────────────────────────────────────────────────────────────

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


def stream_glb(handle, path):
    """(json_dict, offset of the BIN chunk) without materializing the BIN chunk — the gate reads
    the JSON and a few hundred bytes per image, never the tens of megabytes behind them."""
    js, bin_off, off = None, None, 12
    while True:
        handle.seek(off)
        header = handle.read(8)
        if len(header) < 8:
            break
        length, kind = struct.unpack("<II", header)
        if kind == 0x4E4F534A:
            js = json.loads(handle.read(length))
        elif kind == 0x004E4942:
            bin_off = off + 8
        off += 8 + length + (-length % 4)
    if js is None:
        raise SystemExit(f"{path}: no JSON chunk")
    return js, bin_off


def view_bytes(js, bin_, index):
    """Raw bytes of bufferView `index` (single-buffer glb only, which is what Blender exports)."""
    bv = js["bufferViews"][index]
    if bv.get("buffer", 0) != 0:
        raise SystemExit("multi-buffer glb is not supported by this bake")
    start = bv.get("byteOffset", 0)
    return bin_[start : start + bv["byteLength"]]


class Payload:
    """One embedded image's bytes, read where they lie: a slice when the document is already in
    memory, a seek when the gate must not pull 63 MB through it."""

    def __init__(self, size, read):
        self.size = size
        self.read = read


def payloads_in_memory(js, bin_):
    """image index -> `Payload`, for a document read whole."""
    def payload(index):
        image = js["images"][index]
        if "bufferView" not in image:
            return None
        data = view_bytes(js, bin_, image["bufferView"])
        return Payload(len(data), lambda offset, length: data[offset : offset + length])
    return payload


def payloads_by_seek(js, handle, bin_off):
    """image index -> `Payload`, reading the file under `handle` in place."""
    def payload(index):
        image = js["images"][index]
        if "bufferView" not in image or bin_off is None:
            return None
        view = js["bufferViews"][image["bufferView"]]
        base = bin_off + view.get("byteOffset", 0)

        def read(offset, length):
            handle.seek(base + offset)
            return handle.read(min(length, max(view["byteLength"] - offset, 0)))
        return Payload(view["byteLength"], read)
    return payload


# ── the pieces a law is measured in ──────────────────────────────────────────────────────────────

def texture_slots(material):
    """Every known texture slot a material samples, as (slot, role, info). One traversal: the role
    table and the normal-map law read the same slots in the same order."""
    containers = [material, material.get("pbrMetallicRoughness", {})]
    containers += [ext for ext in material.get("extensions", {}).values() if isinstance(ext, dict)]
    for container in containers:
        if not isinstance(container, dict):
            continue
        for slot, role in SLOT_ROLES.items():
            info = container.get(slot)
            if isinstance(info, dict) and "index" in info:
                yield slot, role, info


def element_bytes(accessor):
    """One accessor element's size. glTF pads each matrix COLUMN to four bytes, so a MAT3 of bytes
    is twelve bytes and not nine."""
    component = COMPONENT_BYTES.get(accessor.get("componentType"))
    kind = accessor.get("type")
    if component is None or kind not in TYPE_COMPONENTS:
        raise ValueError("componentType {!r} type {!r}".format(
            accessor.get("componentType"), accessor.get("type")
        ))
    columns = TYPE_COLUMNS.get(kind)
    if columns is None:
        return component * TYPE_COMPONENTS[kind]
    return columns * (-(-component * columns // 4) * 4)


def accessor_spans(js, index):
    """(what, first byte, byte count) of every span accessor `index` owns in the BIN chunk — its
    elements, and the two arrays behind a sparse substitution."""
    accessor = js["accessors"][index]
    spans = []

    def add(what, holder, size, count):
        view = js["bufferViews"][holder["bufferView"]]
        stride = view.get("byteStride") or size
        start = view.get("byteOffset", 0) + holder.get("byteOffset", 0)
        spans.append((what, start, 0 if count < 1 else (count - 1) * stride + size))

    if "bufferView" in accessor:
        add("", accessor, element_bytes(accessor), accessor.get("count", 0))
    sparse = accessor.get("sparse")
    if isinstance(sparse, dict):
        count = sparse.get("count", 0)
        indices, values = sparse.get("indices", {}), sparse.get("values", {})
        add(" sparse indices", indices, COMPONENT_BYTES.get(indices.get("componentType"), 0), count)
        add(" sparse values", values, element_bytes(accessor), count)
    return spans


def expected_levels(width, height):
    """A complete mip chain: every halving from the base dimension down to 1x1."""
    return max(width, height, 1).bit_length()


def raster_size(data):
    """(width, height) of a PNG or JPEG, from its header alone. None when neither."""
    if data[:8] == RAW_MAGIC["image/png"] and len(data) >= 24:
        return struct.unpack_from(">II", data, 16)
    if data[:3] == RAW_MAGIC["image/jpeg"]:
        at = 2
        while at + 9 < len(data):
            if data[at] != 0xFF:
                return None
            marker, length = data[at + 1], struct.unpack_from(">H", data, at + 2)[0]
            if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
                height, width = struct.unpack_from(">HH", data, at + 5)
                return (width, height)
            at += 2 + length
    return None


class Ktx2:
    """A KTX2 file's header, index and level records — everything a law reads, and no payload."""

    def __init__(self, fields, index, records, model, transfer, size):
        (self.vk_format, self.type_size, self.width, self.height, self.depth,
         self.layers, self.faces, self.levels, self.supercompression) = fields
        (self.dfd_offset, self.dfd_length, self.kvd_offset, self.kvd_length,
         self.sgd_offset, self.sgd_length) = index
        self.records = records
        self.model = model
        self.transfer = transfer
        self.size = size

    @property
    def data_floor(self):
        """The first byte a level image may occupy: KTX2 §3 orders the level index, the descriptor,
        the key/value data and the supercompression global data ahead of every level."""
        floor = KTX2_HEADER + KTX2_LEVEL_RECORD * max(self.levels, 1)
        for offset, length in ((self.dfd_offset, self.dfd_length),
                               (self.kvd_offset, self.kvd_length),
                               (self.sgd_offset, self.sgd_length)):
            if length:
                floor = max(floor, offset + length)
        return floor


def parse_ktx2(payload):
    """Read a `Ktx2` off a payload. None when it does not start with the KTX2 identifier.

    The level index is read only as far as the payload actually reaches, so a truncated one comes
    back short rather than as an exception — being short is itself a finding.
    """
    head = payload.read(0, KTX2_HEADER)
    if len(head) < KTX2_HEADER or head[:12] != KTX2_MAGIC:
        return None
    fields = struct.unpack_from("<9I", head, 12)
    index = struct.unpack_from("<4I", head, 48) + struct.unpack_from("<2Q", head, 64)
    available = max(payload.size - KTX2_HEADER, 0) // KTX2_LEVEL_RECORD
    table = payload.read(KTX2_HEADER, KTX2_LEVEL_RECORD * min(fields[7], available))
    records = tuple(
        struct.unpack_from("<3Q", table, KTX2_LEVEL_RECORD * level)
        for level in range(len(table) // KTX2_LEVEL_RECORD)
    )
    model = transfer = None
    block = payload.read(index[0], index[1]) if index[1] else b""
    if len(block) >= 16:
        model, _primaries, transfer, _flags = struct.unpack_from("<4B", block, 12)
    return Ktx2(fields, index, records, model, transfer, payload.size)


def ktx2_header(data):
    """(vkFormat, width, height, levels, supercompression), for the repack's own sanity check."""
    parsed = parse_ktx2(Payload(len(data), lambda offset, length: data[offset : offset + length]))
    if parsed is None:
        raise SystemExit("not a KTX2 file")
    return (parsed.vk_format, parsed.width, parsed.height, parsed.levels, parsed.supercompression)


# ── the laws a baked document answers on its own ─────────────────────────────────────────────────

def _file(path, element):
    return Subject(SubjectKind.FILE, path, element)


def _image_subject(path, index, image):
    name = image.get("name")
    return _file(path, "image {} `{}`".format(index, name) if name else "image {}".format(index))


def image_roles(js, path):
    """(image index -> role, findings), derived from every material slot that samples it. An image
    sampled as two roles has no colour space the encoder could pick, and one nothing samples has
    none to derive; both are the first clause of `D.KTX2_MIPS`."""
    textures = js.get("textures", [])
    images = js.get("images", [])
    roles, sampled_by, findings = {}, {}, []

    for number, material in enumerate(js.get("materials", [])):
        named = material.get("name") or "material {}".format(number)
        for slot, role, info in texture_slots(material):
            where = "`{}` {}".format(named, slot)
            index = info["index"]
            if not isinstance(index, int) or not 0 <= index < len(textures):
                findings.append(Finding(
                    KTX2_MIPS, _file(path, "material {} {}".format(number, slot)),
                    "{} selects texture {}, and the document holds {}".format(
                        where, index, len(textures)),
                    REBAKE,
                ))
                continue
            source = textures[index].get("source")
            if not isinstance(source, int) or not 0 <= source < len(images):
                findings.append(Finding(
                    KTX2_MIPS, _file(path, "texture {}".format(index)),
                    "{} selects texture {}, whose source is {!r}, and the document holds {} "
                    "image(s)".format(where, index, source, len(images)),
                    REBAKE,
                ))
                continue
            if roles.setdefault(source, role) != role:
                findings.append(Finding(
                    KTX2_MIPS, _image_subject(path, source, images[source]),
                    "sampled as {} by {} and as {} by {}".format(
                        roles[source], sampled_by[source], role, where),
                    "give the two slots their own image — one texture cannot be both "
                    "display-referred colour and material data, so the encoder has no colour space "
                    "to pick",
                ))
            else:
                sampled_by.setdefault(source, where)

    for index, image in enumerate(images):
        if index in roles:
            continue
        findings.append(Finding(
            KTX2_MIPS, _image_subject(path, index, image),
            "no known material slot samples it, so it has no colour role",
            "sample it from a material, or add its slot to SLOT_ROLES in "
            "scripts/tank/glb_ktx2.py — an image with no role is one the encoder would guess a "
            "colour space for",
        ))
    return roles, findings


def check_texture_references(js, path):
    """Every texture selects an image, and the `KHR_texture_basisu` declaration the bake adds
    selects the same one — the extension block is a second reference, and a second reference is a
    second chance to point somewhere else."""
    images, textures = js.get("images", []), js.get("textures", [])
    findings = []
    for index, texture in enumerate(textures):
        subject = _file(path, "texture {}".format(index))
        source = texture.get("source")
        if not isinstance(source, int) or not 0 <= source < len(images):
            findings.append(Finding(
                KTX2_MIPS, subject,
                "source is {!r}, and the document holds {} image(s)".format(source, len(images)),
                REBAKE,
            ))
            continue
        declared = texture.get("extensions", {}).get(BASISU)
        if not isinstance(declared, dict):
            findings.append(Finding(
                KTX2_MIPS, subject,
                "selects image {} and carries no {} declaration".format(source, BASISU),
                REBAKE,
            ))
        elif declared.get("source") != source:
            findings.append(Finding(
                KTX2_MIPS, subject,
                "selects image {}, and its {} declaration selects image {!r}".format(
                    source, BASISU, declared.get("source")),
                REBAKE,
            ))
    if textures and BASISU not in js.get("extensionsUsed", []):
        findings.append(Finding(
            KTX2_MIPS, _file(path, "extensionsUsed"),
            "{} textures carry KTX2 images and extensionsUsed is {}".format(
                len(textures), js.get("extensionsUsed", [])),
            REBAKE,
        ))
    return findings


def check_tangents(js, path):
    """`D.TANGENTS`. A normal map is read in the tangent basis of the primitive it is sampled on;
    without one the shader falls back to a generated basis that does not match the bake, and with a
    short one the attribute stream runs out before the vertices do."""
    materials, accessors = js.get("materials", []), js.get("accessors", [])
    findings = []
    for index, mesh in enumerate(js.get("meshes", [])):
        for number, primitive in enumerate(mesh.get("primitives", [])):
            material = primitive.get("material")
            if not isinstance(material, int) or not 0 <= material < len(materials):
                continue
            slot = next(
                (slot for slot, role, _ in texture_slots(materials[material]) if role == "normal"),
                None,
            )
            if slot is None:
                continue
            attributes = primitive.get("attributes", {})
            subject = Subject(
                SubjectKind.MESH, mesh.get("name") or "mesh {}".format(index),
                "primitive {}".format(number),
            )
            named = materials[material].get("name") or "material {}".format(material)
            position = attributes.get("POSITION")
            elements = accessors[position]["count"] \
                if isinstance(position, int) and 0 <= position < len(accessors) else None
            tangent = attributes.get("TANGENT")
            if tangent is None:
                findings.append(Finding(
                    TANGENTS, subject,
                    "material `{}` samples {} and the primitive has no TANGENT accessor "
                    "({} POSITION elements)".format(named, slot, elements),
                    "unwrap the mesh and re-export — `export_tangents` is already frozen on in "
                    ".agents/blender/export_tank.py's EXPORT_SETTINGS, and the exporter emits no "
                    "tangents for a primitive it has no UV map to generate them from",
                ))
                continue
            carried = accessors[tangent]["count"] \
                if isinstance(tangent, int) and 0 <= tangent < len(accessors) else None
            if carried != elements:
                findings.append(Finding(
                    TANGENTS, subject,
                    "material `{}` samples {}; TANGENT accessor {} holds {} element(s) against "
                    "POSITION's {}".format(named, slot, tangent, carried, elements),
                    REBAKE,
                ))
    return findings


def check_baked_images(js, roles, payload, path, raw_sizes=None):
    """`D.KTX2_MIPS` over the payloads: what each image IS, against what its role requires."""
    findings = []
    for index, image in enumerate(js.get("images", [])):
        subject = _image_subject(path, index, image)
        mime = image.get("mimeType")
        if mime != BAKED_MIME:
            findings.append(Finding(
                KTX2_MIPS, subject,
                "mimeType is {!r}, and the bake writes {!r}".format(mime, BAKED_MIME), REBAKE,
            ))
            continue
        if "bufferView" not in image:
            findings.append(Finding(
                KTX2_MIPS, subject, "declares no bufferView, so it is not embedded", REBAKE,
            ))
            continue
        data = payload(index)
        if data is None:
            findings.append(Finding(
                KTX2_MIPS, subject,
                "bufferView {} is not reachable in the BIN chunk".format(image["bufferView"]),
                REBAKE,
            ))
            continue
        parsed = parse_ktx2(data)
        if parsed is None:
            findings.append(Finding(
                KTX2_MIPS, subject,
                "{} byte(s) that do not begin with the KTX2 identifier".format(data.size), REBAKE,
            ))
            continue
        findings.extend(_ktx2_findings(
            parsed, subject, roles.get(index), (raw_sizes or {}).get(index)
        ))
    return findings


def _ktx2_findings(ktx2, subject, role, raw_size):
    """Every clause one KTX2 payload answers: codec, colour, supercompression, chain and index."""
    findings = []

    def refuse(evidence, repair=REBAKE):
        findings.append(Finding(KTX2_MIPS, subject, evidence, repair))

    if ktx2.model != DFD_UASTC:
        refuse("descriptor colour model is {!r}, and UASTC is {}".format(ktx2.model, DFD_UASTC),
               "encode with `basisu -uastc` — ETC1S is a palettized codec, and it smears both the "
               "hull's rivet detail and every normal map on the model")
    if ktx2.supercompression != ZSTD:
        refuse("supercompression scheme is {}, and Zstd is {}".format(ktx2.supercompression, ZSTD),
               "encode with `-ktx2_zstandard_level 9` — Zstd is free at load and pays for the mip "
               "chain the level above adds")
    wanted = ROLE_TRANSFER.get(role)
    if wanted is not None and ktx2.transfer != wanted:
        refuse("role {} wants transfer function {}, and the descriptor declares {!r}".format(
            role, wanted, ktx2.transfer))
    if not ktx2.width or not ktx2.height:
        refuse("declares {}x{}".format(ktx2.width, ktx2.height))
        return findings
    if raw_size is not None and raw_size != (ktx2.width, ktx2.height):
        refuse("is {}x{}, and the raw image it was encoded from is {}x{}".format(
            ktx2.width, ktx2.height, raw_size[0], raw_size[1]))
    complete = expected_levels(ktx2.width, ktx2.height)
    if ktx2.levels != complete:
        refuse("{}x{} carries {} mip level(s), and a chain down to 1x1 is {}".format(
            ktx2.width, ktx2.height, ktx2.levels, complete),
            "encode with `-mipmap` — one level is shimmer on every rivet at combat range, and a "
            "short chain is the same thing further out")
    if len(ktx2.records) < ktx2.levels:
        refuse("declares {} mip level(s) and the {} byte payload holds {} level record(s)".format(
            ktx2.levels, ktx2.size, len(ktx2.records)))
    floor = ktx2.data_floor
    for level, (offset, length, uncompressed) in enumerate(ktx2.records):
        if not length or not uncompressed:
            refuse("level {} record is {} byte(s) at offset {}, {} uncompressed".format(
                level, length, offset, uncompressed))
        elif offset < floor or offset + length > ktx2.size:
            refuse("level {} record spans {}..{} of a {} byte payload whose level data starts at "
                   "{}".format(level, offset, offset + length, ktx2.size, floor))
    return findings


def document_findings(js, payload, path, raw_sizes=None):
    """Every law a baked document answers with nothing else in hand: `D.TANGENTS`, and the whole of
    `D.KTX2_MIPS` except the raw dimensions, which only `diff` holds."""
    roles, findings = image_roles(js, path)
    findings += check_texture_references(js, path)
    findings += check_tangents(js, path)
    findings += check_baked_images(js, roles, payload, path, raw_sizes)
    return findings


# ── the law that reads both documents ────────────────────────────────────────────────────────────

def _digest(data):
    return hashlib.sha256(data).hexdigest()[:16]


def _untouched_json(a, b, path):
    """Every collection the derivation may not touch, by count, order and content."""
    findings = []
    for key in sorted(set(a) | set(b)):
        if key in DERIVED_KEYS:
            continue
        subject = _file(path, key)
        if (key in a) != (key in b):
            findings.append(Finding(
                STRUCTURAL_DERIVATION, subject,
                "present in the {} document only".format("raw" if key in a else "baked"), REBAKE,
            ))
            continue
        if a[key] == b[key]:
            continue
        if isinstance(a[key], list) and isinstance(b[key], list):
            if len(a[key]) != len(b[key]):
                findings.append(Finding(
                    STRUCTURAL_DERIVATION, subject,
                    "{} entries raw, {} baked".format(len(a[key]), len(b[key])), REBAKE,
                ))
                continue
            moved = next(i for i, (x, y) in enumerate(zip(a[key], b[key])) if x != y)
            findings.append(Finding(
                STRUCTURAL_DERIVATION, subject,
                "{} entries on both sides, and entry {} differs: {} became {}".format(
                    len(a[key]), moved,
                    json.dumps(a[key][moved], sort_keys=True)[:120],
                    json.dumps(b[key][moved], sort_keys=True)[:120]),
                REBAKE,
            ))
            continue
        findings.append(Finding(
            STRUCTURAL_DERIVATION, subject,
            "{} became {}".format(json.dumps(a[key], sort_keys=True)[:120],
                                  json.dumps(b[key], sort_keys=True)[:120]),
            REBAKE,
        ))
    return findings


def _extensions_used(a, b, path):
    """`extensionsUsed` gains `KHR_texture_basisu` and nothing else, in the order it was in."""
    expected = list(a.get("extensionsUsed", []))
    if BASISU not in expected:
        expected.append(BASISU)
    found = b.get("extensionsUsed", [])
    if found == expected:
        return []
    return [Finding(
        STRUCTURAL_DERIVATION, _file(path, "extensionsUsed"),
        "{} became {}, and the derivation appends only {}".format(
            a.get("extensionsUsed", []), found, BASISU),
        REBAKE,
    )]


def _in_bounds(js, bin_, index, path):
    """One baked bufferView's offsets, against the buffer that is supposed to hold it."""
    view = js["bufferViews"][index]
    declared = js.get("buffers", [{}])[0].get("byteLength", 0)
    start, length = view.get("byteOffset", 0), view.get("byteLength", 0)
    if start >= 0 and start + length <= declared <= len(bin_):
        return []
    return [Finding(
        STRUCTURAL_DERIVATION, _file(path, "bufferView {}".format(index)),
        "spans {}..{} of a buffer declared {} byte(s) long, in a {} byte BIN chunk".format(
            start, start + length, declared, len(bin_)),
        REBAKE,
    )]


def _bufferviews(a, abin, b, bbin, path):
    """(whether the views still line up, findings). Offsets move because the BIN chunk is rebuilt;
    an image view also resizes, because that is the mutation. Nothing else may change, and no
    non-image view's bytes may."""
    a_views, b_views = a.get("bufferViews", []), b.get("bufferViews", [])
    if len(a_views) != len(b_views):
        return (False, [Finding(
            STRUCTURAL_DERIVATION, _file(path, "bufferViews"),
            "{} raw, {} baked".format(len(a_views), len(b_views)), REBAKE,
        )])
    images = {image["bufferView"] for image in a.get("images", []) if "bufferView" in image}
    images |= {image["bufferView"] for image in b.get("images", []) if "bufferView" in image}
    findings = []
    for index in range(len(a_views)):
        subject = _file(path, "bufferView {}".format(index))
        moves = {"byteOffset", "byteLength"} if index in images else {"byteOffset"}
        raw = {k: v for k, v in a_views[index].items() if k not in moves}
        baked = {k: v for k, v in b_views[index].items() if k not in moves}
        if raw != baked:
            findings.append(Finding(
                STRUCTURAL_DERIVATION, subject,
                "{} became {}, and only {} may move".format(
                    json.dumps(raw, sort_keys=True), json.dumps(baked, sort_keys=True),
                    ", ".join(sorted(moves))),
                REBAKE,
            ))
        findings += _in_bounds(b, bbin, index, path)
        if index in images:
            continue
        before, after = view_bytes(a, abin, index), view_bytes(b, bbin, index)
        if before != after:
            findings.append(Finding(
                STRUCTURAL_DERIVATION, subject,
                "holds no image, and its {} bytes (sha256 {}) became {} bytes (sha256 {})".format(
                    len(before), _digest(before), len(after), _digest(after)),
                REBAKE,
            ))
    return (True, findings)


def _accessor_bytes(a, abin, b, bbin, path):
    """Every accessor's own bytes, span by span. The JSON is compared whole above; this is the
    payload it points at, which an offset rewrite is exactly able to get wrong."""
    if len(a.get("accessors", [])) != len(b.get("accessors", [])):
        return []
    findings = []
    for index in range(len(a["accessors"])):
        subject = _file(path, "accessor {}".format(index))
        try:
            spans = list(zip(accessor_spans(a, index), accessor_spans(b, index)))
        except (ValueError, KeyError, IndexError) as error:
            findings.append(Finding(
                STRUCTURAL_DERIVATION, subject, "cannot be measured: {}".format(error), REBAKE,
            ))
            continue
        for (what, start, size), (_, other, other_size) in spans:
            before, after = abin[start : start + size], bbin[other : other + other_size]
            if before != after:
                findings.append(Finding(
                    STRUCTURAL_DERIVATION, _file(path, "accessor {}{}".format(index, what)),
                    "{} bytes (sha256 {}) became {} bytes (sha256 {})".format(
                        len(before), _digest(before), len(after), _digest(after)),
                    REBAKE,
                ))
    return findings


def _textures(a, b, path):
    """A texture gains its `KHR_texture_basisu` declaration, pointed at the image it already had."""
    findings = []
    if len(a.get("textures", [])) != len(b.get("textures", [])):
        return [Finding(
            STRUCTURAL_DERIVATION, _file(path, "textures"),
            "{} raw, {} baked".format(len(a.get("textures", [])), len(b.get("textures", []))),
            REBAKE,
        )]
    for index, (raw, baked) in enumerate(zip(a.get("textures", []), b.get("textures", []))):
        expected = dict(raw)
        extensions = dict(raw.get("extensions", {}))
        extensions[BASISU] = {"source": raw.get("source")}
        expected["extensions"] = extensions
        if baked != expected:
            findings.append(Finding(
                STRUCTURAL_DERIVATION, _file(path, "texture {}".format(index)),
                "{} became {}, and the derivation writes {}".format(
                    json.dumps(raw, sort_keys=True), json.dumps(baked, sort_keys=True),
                    json.dumps(expected, sort_keys=True)),
                REBAKE,
            ))
    return findings


def _images(a, abin, b, bbin, path):
    """An image keeps every field but its mimeType, and its payload goes from PNG/JPEG to KTX2."""
    if len(a.get("images", [])) != len(b.get("images", [])):
        return [Finding(
            STRUCTURAL_DERIVATION, _file(path, "images"),
            "{} raw, {} baked".format(len(a.get("images", [])), len(b.get("images", []))), REBAKE,
        )]
    findings = []
    for index, (raw, baked) in enumerate(zip(a.get("images", []), b.get("images", []))):
        subject = _image_subject(path, index, raw)
        if {k: v for k, v in raw.items() if k != "mimeType"} != \
                {k: v for k, v in baked.items() if k != "mimeType"}:
            findings.append(Finding(
                STRUCTURAL_DERIVATION, subject,
                "{} became {}, and only the mimeType may move".format(
                    json.dumps(raw, sort_keys=True), json.dumps(baked, sort_keys=True)),
                REBAKE,
            ))
        if raw.get("mimeType") not in RAW_MAGIC:
            findings.append(Finding(
                STRUCTURAL_DERIVATION, subject,
                "is {!r} in the raw document, and the bake reads {}".format(
                    raw.get("mimeType"), " or ".join(sorted(RAW_MAGIC))),
                REBAKE,
            ))
        elif "bufferView" in raw and \
                not view_bytes(a, abin, raw["bufferView"]).startswith(RAW_MAGIC[raw["mimeType"]]):
            findings.append(Finding(
                STRUCTURAL_DERIVATION, subject,
                "declares {} in the raw document and its payload does not begin with that "
                "format's magic".format(raw["mimeType"]),
                REBAKE,
            ))
        if baked.get("mimeType") != BAKED_MIME:
            findings.append(Finding(
                STRUCTURAL_DERIVATION, subject,
                "is {!r} in the baked document, and the bake writes {!r}".format(
                    baked.get("mimeType"), BAKED_MIME),
                REBAKE,
            ))
        elif "bufferView" in baked and \
                not view_bytes(b, bbin, baked["bufferView"]).startswith(KTX2_MAGIC):
            findings.append(Finding(
                STRUCTURAL_DERIVATION, subject,
                "declares {} in the baked document and its payload does not begin with the KTX2 "
                "identifier".format(BAKED_MIME),
                REBAKE,
            ))
    return findings


def derivation_findings(a, abin, b, bbin, path):
    """`D.STRUCTURAL_DERIVATION`, whole: the baked document against the raw one it came from."""
    findings = _untouched_json(a, b, path) + _extensions_used(a, b, path)
    aligned, views = _bufferviews(a, abin, b, bbin, path)
    findings += views
    if aligned:
        findings += _accessor_bytes(a, abin, b, bbin, path)
    return findings + _textures(a, b, path) + _images(a, abin, b, bbin, path)


def raw_sizes(js, bin_):
    """image index -> (width, height) read off the raw PNG/JPEG headers, for the one clause of
    `D.KTX2_MIPS` that needs the document the encoder read."""
    sizes = {}
    for index, image in enumerate(js.get("images", [])):
        if "bufferView" not in image:
            continue
        size = raster_size(view_bytes(js, bin_, image["bufferView"]))
        if size is not None:
            sizes[index] = size
    return sizes


# ── the subcommands ──────────────────────────────────────────────────────────────────────────────

def rendered(label, findings):
    """One report, and the exit code it carries. Every entry point ends here, so the console reads
    the same rows in the same order whichever one produced them."""
    findings = report.sorted_findings(findings)
    print(report.render_text(findings), end="", flush=True)
    print("{} ▸ {}".format(label.ljust(5), report.summary(findings)), flush=True)
    return report.exit_code(findings)


def cmd_unpack(in_glb, work):
    js, bin_ = read_glb(in_glb)
    work = Path(work)

    # Refuse to bake a bake. basisu takes PNG/JPEG in, not KTX2, so this would either fail deep in
    # the encode loop or (worse) round-trip an already-lossy UASTC payload through a second
    # compression. The input is always a FRESH Blender export; the output is the tracked glb.
    if any(im.get("mimeType") == BAKED_MIME for im in js.get("images", [])):
        raise SystemExit(
            f"{in_glb} is already mip-baked (image/ktx2). The bake reads a fresh, mipless Blender "
            "export — you have pointed it at its own output."
        )

    roles, findings = image_roles(js, in_glb)
    if findings:
        return rendered("unpack", findings)
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
    return 0


def cmd_repack(in_glb, work, out_glb):
    js, bin_ = read_glb(in_glb)
    work = Path(work)

    # bufferView index -> replacement bytes, for the views the images live in.
    replace = {}
    for i, im in enumerate(js.get("images", [])):
        data = (work / "ktx2" / f"{i}.ktx2").read_bytes()
        ktx2_header(data)  # fails loud if basisu wrote something else
        replace[im["bufferView"]] = data
        im["mimeType"] = BAKED_MIME

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
        t.setdefault("extensions", {})[BASISU] = {"source": t["source"]}
    used = js.setdefault("extensionsUsed", [])
    if BASISU not in used:
        used.append(BASISU)

    write_glb(out_glb, js, bytes(out))
    print(f"glb   ▸ {out_glb} — {Path(out_glb).stat().st_size / 1e6:.1f} MB")
    return 0


def cmd_diff(raw_glb, baked_glb):
    """The whole derivation law, with both documents in hand: what the bake did to the raw
    candidate, and what the baked images now are."""
    a, abin = read_glb(raw_glb)
    b, bbin = read_glb(baked_glb)
    findings = derivation_findings(a, abin, b, bbin, baked_glb)
    findings += document_findings(b, payloads_in_memory(b, bbin), baked_glb, raw_sizes(a, abin))
    return rendered("diff", findings)


def cmd_verify(*args):
    """The ship gate: the two laws a baked document answers on its own.

    Runs on any glb-shaped file — including a raw `.git/lfs/objects/**` blob, which is how the
    pre-push hook checks the COMMITTED bytes rather than whatever is sitting in the work tree.

    An unfetched git-lfs POINTER is an error by default, because "I could not read it" must not
    read as "it is fine" in the release workflow. `--allow-pointer` downgrades it to a skip, for
    the pre-push hook: a dev whose clone never smudged the glb cannot have changed it either, and
    failing their push over someone else's asset would be noise.

    Deliberately streaming: it reads the 12-byte header, the JSON chunk, and a few hundred bytes
    per image, never the ~63 MB payload. That keeps it at a few milliseconds, which is what earns
    it a place in a hook that already pays for a compile.
    """
    allow_pointer = "--allow-pointer" in args
    (path,) = [a for a in args if not a.startswith("--")]
    with open(path, "rb") as handle:
        head = handle.read(12)
        if head.startswith(b"version http"):  # first line of a git-lfs pointer
            if not allow_pointer:
                raise SystemExit(
                    f"{path} is an unfetched git-lfs pointer — cannot verify the shipped bytes. "
                    "Run `git lfs pull` first (or pass --allow-pointer to treat this as a skip)."
                )
            print(f"verify ▸ {path}: git-lfs pointer, content not fetched — SKIPPED")
            return 0
        if len(head) < 12 or struct.unpack_from("<I", head, 0)[0] != 0x46546C67:
            raise SystemExit(f"{path}: not a glb")

        js, bin_off = stream_glb(handle, path)
        if not js.get("images"):
            raise SystemExit(f"{path}: no embedded images — is this a tank glb?")
        findings = document_findings(js, payloads_by_seek(js, handle, bin_off), path)
    return rendered("verify", findings)


if __name__ == "__main__":
    cmd, *args = sys.argv[1:]
    sys.exit(
        {"unpack": cmd_unpack, "repack": cmd_repack, "diff": cmd_diff, "verify": cmd_verify}[cmd](
            *args
        )
        or 0
    )
