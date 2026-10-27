"""build.py — the prop build: one command per tree, the trio of the slice contract.

    python3 scripts/prop/build.py build  assets/trees/<id>/<id>.blend
    python3 scripts/prop/build.py verify assets/trees/<id>/<id>.blend
    python3 scripts/prop/build.py lint   assets/trees/<id>/<id>.blend

A `.blend` (and the `<id>.tree.ron` the generator wrote beside it) goes in; `<id>.glb` and
`<id>.lod.json` come out and the `.tree.ron` is carried through: checked against the glb and
hashed into the certificate. The pattern is the tank build's (`scripts/tank/build.py`, ADR 0035)
cut down to what a tree has. The contract itself is
`.agents/scratch/destruction-2026-10-26/slice-1-fallen-tree.md` (§2 the node tree, §3 the species
table, amendments A3, A5, A6, A12, A13); this module states only what it enforces.

`build`: the toolchain preflight (`scripts/toolchain.py`); the export (`export_tree.py` inside
Blender, custom properties as glTF extras) into this invocation's work directory; the three
contracts below on the candidate; the certificate; a staged publish, the glb first and the
certificate last. `lint` is the same without the publish. `verify` reads the TRACKED trio: its
coherence (both hashes, the staleness digest, the five fields), the three contracts, and then the
re-export of the blend compared byte for byte with the tracked glb — the tracked model must be
what this source and this pipeline produce, as the tank verifier requires. It writes nothing.

THE THREE CONTRACTS, each a list of named failures, empty on the pass:
- `contract_failures`: the node tree. One root named `<id>`, identity, at the origin; every
  descendant with extras `{tree_segment, tree_part}` and a mesh; nodes carry a finite translation
  on the parent's +Y axis and no rotation, scale or matrix (a fall is a rotation about a
  segment's origin); `trunk_00` at the root origin, `trunk_k` under `trunk_{k-1}`, two caps per
  segment, an optional canopy, each (part, segment) bound once.
- `ron_failures`: the `.tree.ron` against the glb and the species table. A `TreeAsset` whose
  species row is in the table with a positive `hp` and `fell_k` and a substance that is a key of
  the substance registry (`assets/materials/materials.ron`, or `--registry`); at most 255
  segments (the wire word's cut index, A3); positive finite dimensions; adjacent radii that meet;
  no `dbh_m` (A6: DBH is derived at load from the taper at 1.3 m); segment heights that are the
  chain's node offsets and the cap_top offsets, summing to the height the root records.
- `geometry_failures`: the stored vertices against the ron, decoded from the BIN chunk. Finite
  positions, triangle indices that address them, every vertex within the tree's height of its
  node; a trunk as two flat rings at y = 0 and y = height_m with the ron's two radii; each cap a
  flat disc on its plane reaching its ring's radius and facing out (cap_bottom -Y, cap_top +Y);
  adjacent trunks sharing one boundary loop vertex for vertex.
Tolerances: `TOL_M` on metres (the glb stores float32, the ron four decimals), `NORMAL_TOL` on
a unit normal's components.

THE CERTIFICATE is the tank's five fields with `sim_ron_sha` in `sim_glb_sha`'s place (A13): a
tree's sim artifact is the `.tree.ron` the sim is built from at spawn (ADR 0014), and the field
hashes those bytes. `chains` is EMPTY, and that is a SCOPED EXCEPTION, not a measured outcome:
ADR 0033 wants a generated ladder for every renderable mesh or an explicit unsupported verdict,
and this build searches for no rung. The exception is scoped to the slice-1 fixture: the chain
cutter has no border lock (ADR 0033 "no vertex/edge locks"), so independently collapsed segments
could open their shared boundary loop, and the canopy ships one card density (A12). Whether the
forest may stand on LOD0 is the runtime lane's frame-cost measurement to make; a certificate with
no chain renders at source detail (ADR 0035's benign case) until then.

Exit is non-zero exactly when a stage refused.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import struct
import subprocess
import sys
import tempfile
from typing import Dict, List, Optional, Set, Tuple

_SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _SCRIPTS)

import ron  # noqa: E402  — the paths above are what make these importable
import toolchain  # noqa: E402

MODES = ("build", "verify", "lint")
EXPORT = os.path.join("scripts", "prop", "export_tree.py")
SPECIES_TABLE = os.path.join("assets", "trees", "species.ron")
REGISTRY = os.path.join("assets", "materials", "materials.ron")
CERTIFICATE_FIELDS = ("blend_digest", "view_glb_sha", "sim_ron_sha", "mesh_count", "chains")
PARTS = ("trunk", "cap_bottom", "cap_top", "canopy")

#: The wire word's cut index is a byte below the segment count (contract A3).
MAX_SEGMENTS = 255

#: The fields of a species row the generator reads — `plan.sample` and `plan.whorls`, the two
#: readers `gen_tree.build` hands the row to — and so the fields the blend and its ron are a
#: function of, beside the seed. `test_prop.py` holds the generator to exactly this set.
GENERATOR_FIELDS = ("height_m", "dbh_m", "crown_base_frac", "crown_width_frac", "whorl_spacing_m")

#: What can move either artifact, hashed into `blend_digest` beside the inputs. Relative to this
#: directory; the hook suite reads the list by text to check the shared-surface predicate covers it.
PIPELINE_SOURCES = (
    "build.py",
    "export_tree.py",
    "gen_tree.py",
    "plan.py",
    "ron.py",
    "../toolchain.py",
)

#: How far a stored metre may sit from the ron's: the glb stores float32 and the ron four decimals.
TOL_M = 1e-4

#: How far a cap's unit normal may sit from its axis, per component.
NORMAL_TOL = 1e-3

REBUILD = ("run `scripts/prop/build.py build` and commit the trio — the tracked artifacts are not "
           "what this source and this pipeline produce")

_GLB_MAGIC = 0x46546C67
_CHUNK_JSON = 0x4E4F534A
_CHUNK_BIN = 0x004E4942
_COMPONENT = {5120: ("b", 1), 5121: ("B", 1), 5122: ("h", 2), 5123: ("H", 2), 5125: ("I", 4),
              5126: ("f", 4)}
_ARITY = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT2": 4, "MAT3": 9, "MAT4": 16}


class Refused(Exception):
    def __init__(self, stage: str, failures: List[str]):
        super().__init__(stage)
        self.stage = stage
        self.failures = failures


# ── reading ──────────────────────────────────────────────────────────────────────────────────────

def parse_glb(blob: bytes) -> Tuple[dict, bytes]:
    """`(json, bin)` of a binary glTF whose container is well framed: the declared length is the
    file's, the chunks are aligned and inside it, the JSON chunk comes first and once, the BIN
    chunk at most once, and nothing follows the last chunk. Refuses anything else by name."""
    if len(blob) < 20:
        raise ValueError("not a glb: {} byte(s)".format(len(blob)))
    magic, version, total = struct.unpack_from("<III", blob, 0)
    if magic != _GLB_MAGIC or version != 2:
        raise ValueError("not a glb 2 (magic {:#x}, version {})".format(magic, version))
    if total != len(blob):
        raise ValueError("the header declares {} byte(s) and the file is {}".format(
            total, len(blob)))
    at, js, binary, seen = 12, None, None, 0
    while at < len(blob):
        if at + 8 > len(blob):
            raise ValueError("{} byte(s) after the last chunk, too few for a chunk header".format(
                len(blob) - at))
        length, kind = struct.unpack_from("<II", blob, at)
        if length % 4 or at + 8 + length > len(blob):
            raise ValueError("chunk {} at byte {} declares {} byte(s); chunks are 4-aligned and "
                             "inside the file".format(seen, at, length))
        chunk = blob[at + 8:at + 8 + length]
        if kind == _CHUNK_JSON:
            if seen != 0:
                raise ValueError("the JSON chunk is chunk {}; it is the first".format(seen))
            js = json.loads(chunk.decode("utf-8"))
        elif kind == _CHUNK_BIN:
            if seen != 1 or binary is not None:
                raise ValueError("a BIN chunk at chunk {}; at most one, directly after the "
                                 "JSON chunk".format(seen))
            binary = chunk
        else:
            raise ValueError("chunk {} has type {:#x}, neither JSON nor BIN".format(seen, kind))
        at += 8 + length
        seen += 1
    if js is None:
        raise ValueError("the glb has no JSON chunk")
    return js, binary if binary is not None else b""


def accessor(js: dict, binary: bytes, index, what: str) -> List[tuple]:
    """The decoded elements of one accessor, each a tuple of its components. `ValueError` names
    what makes it unreadable: a reference past the arrays, a sparse or strided-too-tightly
    layout, a type outside the subset, or bytes the BIN chunk does not hold."""
    accessors = js.get("accessors", [])
    if not _index(index, len(accessors)):
        raise ValueError("{} names accessor {!r}; the glb holds {}".format(what, index,
                                                                           len(accessors)))
    acc = accessors[index]
    if "sparse" in acc or "bufferView" not in acc:
        raise ValueError("{} (accessor {}) is sparse or has no bufferView".format(what, index))
    component = _COMPONENT.get(acc.get("componentType"))
    arity = _ARITY.get(acc.get("type"))
    count = acc.get("count")
    if component is None or arity is None or not _index(count, None):
        raise ValueError("{} (accessor {}) is {!r} × {!r} count {!r}, outside the subset".format(
            what, index, acc.get("type"), acc.get("componentType"), count))
    views = js.get("bufferViews", [])
    if not _index(acc["bufferView"], len(views)):
        raise ValueError("{} (accessor {}) names bufferView {!r}; the glb holds {}".format(
            what, index, acc["bufferView"], len(views)))
    view = views[acc["bufferView"]]
    fmt, size = component
    element = arity * size
    stride = view.get("byteStride", element)
    if view.get("buffer", 0) != 0 or stride < element:
        raise ValueError("{} (accessor {}) reads buffer {!r} at stride {}; a glb's geometry is "
                         "buffer 0, strided no tighter than its {}-byte element".format(
                             what, index, view.get("buffer", 0), stride, element))
    start = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    end = start + (count - 1) * stride + element if count else start
    view_end = view.get("byteOffset", 0) + view.get("byteLength", 0)
    if end > view_end or view_end > len(binary):
        raise ValueError("{} (accessor {}) reads bytes {}..{}; its bufferView ends at {} and the "
                         "BIN chunk holds {} byte(s)".format(what, index, start, end, view_end,
                                                              len(binary)))
    return [struct.unpack_from("<" + fmt * arity, binary, start + i * stride) for i in range(count)]


def _index(value, bound: Optional[int]) -> bool:
    return (isinstance(value, int) and not isinstance(value, bool) and value >= 0
            and (bound is None or value < bound))


def sha256_bytes(blob: bytes) -> str:
    return hashlib.sha256(blob).hexdigest()


def read(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read()


def repo_root() -> str:
    return os.path.dirname(_SCRIPTS)


def sources_digest() -> str:
    here = os.path.dirname(os.path.abspath(__file__))
    digest = hashlib.sha256()
    for name in PIPELINE_SOURCES:
        digest.update(name.encode())
        digest.update(read(os.path.join(here, name)))
    return digest.hexdigest()


def blend_digest(blend: str, tree_ron: str, table: str, registry: str) -> str:
    """The staleness field: the source, the ron, what the build reads of the species table, the
    pipeline, the pins.

    The `.tree.ron` is hashed WHOLE. It is the sim artifact (the sim is built from it at spawn,
    ADR 0014), and every field in it is read: `species` picks the row, each segment's three
    dimensions are held against the glb here and read at load; the header is the generator's
    record of the draw. The table is read through `species_projection`, so an edit to it outside
    what the build reads of this tree's row leaves the trio current. The registry reaches the
    digest only as the projection's verdict on the substance.
    """
    digest = hashlib.sha256()
    for path in (blend, tree_ron):
        digest.update(sha256_bytes(read(path)).encode())
    projection = species_projection(table, species_of(tree_ron), substances(registry))
    digest.update(sha256_bytes(projection).encode())
    digest.update(sources_digest().encode())
    for _key, value in toolchain.ENVIRONMENT:
        digest.update(value.encode())
    return digest.hexdigest()


def species_of(tree_ron: str):
    """The species key a `.tree.ron` names, or None when it names none; `Refused("ron")` when the
    ron cannot be read. The contracts refuse a ron that names no species or no row; this only has to
    be total, so the staleness field is cut before them."""
    try:
        tree = ron.load(tree_ron)
    except (OSError, ron.RonError) as error:
        raise Refused("ron", ["{}: {}".format(tree_ron, error)]) from error
    return tree.get("species") if isinstance(tree, dict) else None


def substances(registry: str) -> Set[str]:
    """The substance keys of the registry; `Refused` when it cannot be read as one."""
    try:
        document = ron.load(registry)
    except (OSError, ron.RonError) as error:
        raise Refused("registry", ["{}: {}".format(registry, error)]) from error
    if not isinstance(document, dict) or not isinstance(document.get("substances"), dict):
        raise Refused("registry", ["{} has no substances map".format(registry)])
    return set(document["substances"])


# ── the node contract ────────────────────────────────────────────────────────────────────────────

def _finite(value) -> bool:
    return (isinstance(value, (int, float)) and not isinstance(value, bool)
            and math.isfinite(value))


def _finite_positive(value) -> bool:
    return _finite(value) and value > 0.0


def _translation(node: dict) -> List[float]:
    return [float(v) for v in node.get("translation", [0.0, 0.0, 0.0])]


def transform_failures(node: dict, name: str) -> List[str]:
    """A tree node's transform is a finite translation and nothing else: no matrix, the identity
    rotation, unit scale, each array the size the format gives it."""
    failures = []
    if "matrix" in node:
        failures.append("node {!r} carries a matrix; a tree node is a translation only".format(name))
    for key, identity in (("translation", (0.0, 0.0, 0.0)), ("rotation", (0.0, 0.0, 0.0, 1.0)),
                          ("scale", (1.0, 1.0, 1.0))):
        values = node.get(key, list(identity))
        if (not isinstance(values, list) or len(values) != len(identity)
                or not all(_finite(v) for v in values)):
            failures.append("node {!r} {} is {!r}, not {} finite numbers".format(
                name, key, values, len(identity)))
        elif key != "translation" and any(abs(a - b) > 1e-6 for a, b in zip(values, identity)):
            failures.append("node {!r} carries a {}; a tree node is a translation only, so a fall "
                            "is a rotation about its origin".format(name, key))
    return failures


def tree_nodes(js: dict) -> Dict[str, Dict[int, int]]:
    """Node index per (part, segment), read from extras: `{"trunk": {0: 3, 1: 7}, ...}`."""
    out: Dict[str, Dict[int, int]] = {part: {} for part in PARTS}
    for index, node in enumerate(js.get("nodes", [])):
        extras = node.get("extras")
        if not isinstance(extras, dict) or extras.get("tree_part") not in PARTS:
            continue
        segment = extras.get("tree_segment")
        if isinstance(segment, int) and not isinstance(segment, bool):
            out[extras["tree_part"]].setdefault(segment, index)
    return out


def root_node(js: dict) -> int:
    return js.get("scenes", [{}])[js.get("scene", 0)].get("nodes", [None])[0]


def contract_failures(js: dict, stem: str) -> List[str]:
    """Every law of the slice contract's node tree (§2) the document breaks. Empty is the pass."""
    failures = []
    nodes = js.get("nodes", [])
    scene = js.get("scenes", [{}])[js.get("scene", 0)]
    roots = scene.get("nodes", [])
    if len(roots) != 1 or nodes[roots[0]].get("name") != stem:
        failures.append("the scene has {} root node(s) {}; the contract has one, named {!r}".format(
            len(roots), [nodes[i].get("name") for i in roots], stem))
        return failures
    root = roots[0]
    failures += transform_failures(nodes[root], stem)
    if any(abs(v) > 1e-6 for v in _translation(nodes[root])):
        failures.append("the root sits at {}; the contract places it at the origin".format(
            _translation(nodes[root])))
    parent = {}
    for index, node in enumerate(nodes):
        for child in node.get("children", []):
            parent[child] = index

    for index, node in enumerate(nodes):
        if index == root:
            continue
        extras = node.get("extras")
        name = node.get("name", "#{}".format(index))
        if not isinstance(extras, dict):
            failures.append("node {!r} carries no extras".format(name))
            continue
        segment, part = extras.get("tree_segment"), extras.get("tree_part")
        if part not in PARTS:
            failures.append("node {!r} has tree_part {!r}; the parts are {}".format(
                name, part, ", ".join(PARTS)))
        if not _index(segment, None):
            failures.append("node {!r} has tree_segment {!r}, not a non-negative integer".format(
                name, segment))
        if "mesh" not in node:
            failures.append("node {!r} references no mesh".format(name))
        wrong = transform_failures(node, name)
        failures += wrong
        if not wrong:
            translation = _translation(node)
            if abs(translation[0]) > 1e-6 or abs(translation[2]) > 1e-6:
                failures.append("node {!r} sits off its parent's axis at {}".format(
                    name, translation))
    if failures:
        return failures

    by_part = tree_nodes(js)
    trunks = by_part["trunk"]
    count = len(trunks)
    if count == 0:
        failures.append("the document has no trunk node")
        return failures
    if sorted(trunks) != list(range(count)):
        failures.append("trunk segments are {}; the contract numbers them 0..{} without a "
                        "gap".format(sorted(trunks), count - 1))
        return failures
    for segment in range(count):
        index = trunks[segment]
        name = nodes[index].get("name")
        expected_parent = root if segment == 0 else trunks[segment - 1]
        if parent.get(index) != expected_parent:
            failures.append("trunk {} ({!r}) is a child of {!r}; the chain parents it to {!r}".format(
                segment, name, nodes[parent[index]].get("name") if index in parent else None,
                nodes[expected_parent].get("name")))
        offset = _translation(nodes[index])[1]
        if segment == 0 and abs(offset) > 1e-6:
            failures.append("trunk 0 ({!r}) sits {} m up the root; segment 0's origin is the "
                            "root's".format(name, offset))
        if segment > 0 and offset <= 0.0:
            failures.append("trunk {} ({!r}) sits {} m up its parent; a segment is placed its "
                            "parent's height up the +Y axis".format(segment, name, offset))
        for part in ("cap_bottom", "cap_top"):
            cap = by_part[part].get(segment)
            if cap is None or parent.get(cap) != index:
                failures.append("trunk {} has no {} child; the runtime reveals the pair at a "
                                "cut".format(segment, part))
        canopy = by_part["canopy"].get(segment)
        if canopy is not None and parent.get(canopy) != index:
            failures.append("canopy {} is not a child of trunk {}".format(segment, segment))
    for part in PARTS:
        for segment in by_part[part]:
            if segment >= count:
                failures.append("{} node for segment {} names a segment the trunk does not "
                                "have".format(part, segment))
    for segment, index in by_part["cap_bottom"].items():
        if abs(_translation(nodes[index])[1]) > 1e-6:
            failures.append("cap_bottom {} is not at its segment's origin".format(segment))
    seen = {}
    for index, node in enumerate(nodes):
        extras = node.get("extras") if index != root else None
        if isinstance(extras, dict):
            key = (extras.get("tree_part"), extras.get("tree_segment"))
            if key in seen:
                failures.append("two nodes carry extras {}: {!r} and {!r}".format(
                    key, nodes[seen[key]].get("name"), node.get("name")))
            seen[key] = index
    return failures


# ── the ron, against the glb and the tables ──────────────────────────────────────────────────────

def _pair_value(pair):
    """The value of a `(value, Kind("why"))` provenance pair, or the bare value."""
    return pair[0] if isinstance(pair, tuple) and len(pair) == 2 else pair


def species_failures(key: str, row, registry: Set[str]) -> List[str]:
    """What the runtime reads off a species row, checked before it is built from: the two
    thresholds positive (the loader refuses a non-positive one), `hp` an integer pool, the
    substance a key of the registry."""
    if not isinstance(row, dict):
        return ["species {!r} is not a Species row".format(key)]
    failures = []
    fell_k = _pair_value(row.get("fell_k"))
    if not _finite_positive(fell_k):
        failures.append("species {!r} fell_k is {!r}, not a positive finite number".format(
            key, fell_k))
    hp = _pair_value(row.get("hp"))
    if not (isinstance(hp, int) and not isinstance(hp, bool) and hp > 0):
        failures.append("species {!r} hp is {!r}, not a positive integer".format(key, hp))
    substance = row.get("substance")
    if not isinstance(substance, str) or substance not in registry:
        failures.append("species {!r} names substance {!r}, which is not a key of the substance "
                        "registry ({})".format(key, substance, ", ".join(sorted(registry))))
    return failures


def _canonical(value):
    """A parsed RON value with every number a float, so a respelled literal is one input."""
    if isinstance(value, bool) or value is None or isinstance(value, str):
        return value
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, (list, tuple)):
        return [_canonical(item) for item in value]
    if isinstance(value, dict):
        return {key: _canonical(item) for key, item in value.items()}
    return {"__call__": value.name, "args": [_canonical(item) for item in value.args]}


def species_projection(table: str, key, registry: Set[str]) -> bytes:
    """What the build reads of the species table for one tree, and nothing else of it.

    The row under `key` reaches the artifacts through two readers: the generator, which draws the
    tree from `GENERATOR_FIELDS`, and `species_failures`, whose verdict the build refuses on — the
    thresholds `fell_k` and `hp` and the substance against the registry's keys are the runtime's to
    read at spawn, and the build reads only that the verdict is empty. So the projection is those
    fields by name, the substance, and the verdict: a threshold's value, its provenance, a comment
    or a respelled literal is not in it. JSON with sorted keys, compact separators, no NaN, and
    every number a float in Python's shortest round-trip `repr`, so `9.0e5` and `900000.0` are one
    input. A row the table lacks projects as its verdict; `Refused("ron")` when the table cannot be
    read. The release staleness check (#131) reads this same projection.
    """
    try:
        species = ron.load(table)
    except (OSError, ron.RonError) as error:
        raise Refused("ron", ["{}: {}".format(table, error)]) from error
    row = species.get(key) if isinstance(species, dict) else None
    fields = row if isinstance(row, dict) else {}
    document = {
        "generator": {field: _canonical(fields.get(field)) for field in GENERATOR_FIELDS},
        "substance": _canonical(fields.get("substance")),
        "verdict": species_failures(key, row, registry),
    }
    return json.dumps(document, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def ron_failures(tree, js: dict, species: dict, registry: Set[str]) -> List[str]:
    """Every claim the `.tree.ron` makes that the glb, the species table or the substance registry
    does not bear out."""
    failures = []
    if not isinstance(tree, dict) or tree.get("__struct__") != "TreeAsset":
        return ["the ron is not a TreeAsset"]
    for field in ("species", "segments"):
        if field not in tree:
            failures.append("TreeAsset has no {}".format(field))
    if "dbh_m" in tree:
        failures.append("TreeAsset carries dbh_m, a field the contract dropped (A6): DBH is derived "
                        "at load from the taper at 1.3 m")
    if failures:
        return failures
    if tree["species"] not in species:
        failures.append("species {!r} is not in the species table ({})".format(
            tree["species"], ", ".join(sorted(species))))
    else:
        failures += species_failures(tree["species"], species[tree["species"]], registry)
    segments = tree["segments"]
    if not isinstance(segments, list) or not segments:
        return failures + ["segments is empty; a trunk is at least one segment"]
    if len(segments) > MAX_SEGMENTS:
        failures.append("the ron has {} segments; the wire word's cut index allows {} (A3)".format(
            len(segments), MAX_SEGMENTS))
    for index, segment in enumerate(segments):
        if not isinstance(segment, dict) or segment.get("__struct__") != "Segment":
            failures.append("segment {} is not a Segment".format(index))
            continue
        for field in ("height_m", "r_bottom_m", "r_top_m"):
            if not _finite_positive(segment.get(field)):
                failures.append("segment {} {} is {!r}, not a positive finite number of "
                                "metres".format(index, field, segment.get(field)))
        if index > 0 and isinstance(segments[index - 1], dict):
            below = segments[index - 1].get("r_top_m")
            if _finite_positive(below) and _finite_positive(segment.get("r_bottom_m")) \
                    and abs(below - segment["r_bottom_m"]) > TOL_M:
                failures.append("segment {} r_bottom_m {} does not meet segment {} r_top_m {}; "
                                "the caps at a boundary are one disc".format(
                                    index, segment["r_bottom_m"], index - 1, below))
    if failures:
        return failures

    nodes = js.get("nodes", [])
    by_part = tree_nodes(js)
    trunks = by_part["trunk"]
    if sorted(trunks) != list(range(len(segments))):
        failures.append("the ron has {} segment(s) and the glb {} trunk node(s)".format(
            len(segments), len(trunks)))
        return failures
    for index, segment in enumerate(segments):
        if index > 0:
            offset = _translation(nodes[trunks[index]])[1]
            if abs(offset - segments[index - 1]["height_m"]) > TOL_M:
                failures.append("trunk {} sits {} m up its parent and the ron gives segment {} "
                                "height_m {}".format(index, offset, index - 1,
                                                     segments[index - 1]["height_m"]))
        top = by_part["cap_top"].get(index)
        if top is not None:
            offset = _translation(nodes[top])[1]
            if abs(offset - segment["height_m"]) > TOL_M:
                failures.append("cap_top {} sits {} m up its segment and the ron gives height_m "
                                "{}".format(index, offset, segment["height_m"]))
    recorded = (nodes[root_node(js)].get("extras") or {}).get("tree_height_m")
    total = sum(segment["height_m"] for segment in segments)
    if isinstance(recorded, (int, float)) and abs(total - recorded) > 1e-3:
        failures.append("the segment heights sum to {} m and the root records a {} m tree".format(
            round(total, 4), recorded))
    return failures


# ── the geometry, against the ron ────────────────────────────────────────────────────────────────

def node_geometry(js: dict, binary: bytes, index: int, name: str):
    """`(positions, normals, triangles)` of one node's mesh, every primitive merged: positions
    as `(x, y, z)` tuples, normals likewise or `None` when absent, triangles as index triples
    into the positions. `ValueError` names what makes the mesh unusable."""
    node = js["nodes"][index]
    meshes = js.get("meshes", [])
    if not _index(node.get("mesh"), len(meshes)):
        raise ValueError("node {!r} references mesh {!r}; the glb holds {}".format(
            name, node.get("mesh"), len(meshes)))
    primitives = meshes[node["mesh"]].get("primitives", [])
    if not primitives:
        raise ValueError("node {!r}'s mesh has no primitive".format(name))
    positions, normals, triangles, have_normals = [], [], [], True
    for number, primitive in enumerate(primitives):
        what = "primitive {} of node {!r}".format(number, name)
        if primitive.get("mode", 4) != 4:
            raise ValueError("{} is mode {}; a tree part is triangles (mode 4)".format(
                what, primitive.get("mode")))
        attributes = primitive.get("attributes", {})
        if "POSITION" not in attributes or "indices" not in primitive:
            raise ValueError("{} has no POSITION or no indices".format(what))
        acc = js["accessors"][attributes["POSITION"]] \
            if _index(attributes["POSITION"], len(js.get("accessors", []))) else {}
        if acc.get("type") != "VEC3" or acc.get("componentType") != 5126:
            raise ValueError("{} POSITION is {!r} × {!r}; a position is a VEC3 of float32".format(
                what, acc.get("type"), acc.get("componentType")))
        base = len(positions)
        points = accessor(js, binary, attributes["POSITION"], what + " POSITION")
        indices = [i[0] for i in accessor(js, binary, primitive["indices"], what + " indices")]
        if js["accessors"][primitive["indices"]].get("componentType") not in (5121, 5123, 5125):
            raise ValueError("{} indices are not unsigned integers".format(what))
        if not points or not indices:
            raise ValueError("{} is empty: {} vertex(es) and {} index(es)".format(
                what, len(points), len(indices)))
        if len(indices) % 3:
            raise ValueError("{} has {} indices, not a whole number of triangles".format(
                what, len(indices)))
        for i, point in enumerate(points):
            if not all(math.isfinite(c) for c in point):
                raise ValueError("vertex {} of {} is {}, not finite".format(i, what, point))
        for i, vertex in enumerate(indices):
            if vertex >= len(points):
                raise ValueError("index {} of {} is {}, past its {} vertices".format(
                    i, what, vertex, len(points)))
        positions += points
        triangles += [(base + indices[i], base + indices[i + 1], base + indices[i + 2])
                      for i in range(0, len(indices), 3)]
        if "NORMAL" in attributes and have_normals:
            normals += accessor(js, binary, attributes["NORMAL"], what + " NORMAL")
        else:
            have_normals = False
    return positions, normals if have_normals else None, triangles


def _ring_twins(ring_a, ring_b) -> int:
    """How many vertices of `ring_a` have no vertex of `ring_b` within `TOL_M` in the xz plane."""
    return sum(1 for (xa, _, za) in ring_a
               if all(math.hypot(xa - xb, za - zb) > TOL_M for (xb, _, zb) in ring_b))


def geometry_failures(js: dict, binary: bytes, segments: List[dict]) -> List[str]:
    """Every way the stored vertices disagree with the ron's trunk, run once the two other
    contracts hold (it reads the segments as validated). Empty is the pass."""
    failures = []
    for number, buffer in enumerate(js.get("buffers", [])):
        if "uri" not in buffer and buffer.get("byteLength", 0) > len(binary):
            failures.append("buffer {} declares {} byte(s) and the BIN chunk holds {}".format(
                number, buffer.get("byteLength"), len(binary)))
    if failures:
        return failures
    nodes = js["nodes"]
    by_part = tree_nodes(js)
    height = sum(segment["height_m"] for segment in segments)
    rings: Dict[int, Tuple[list, list]] = {}
    for part in PARTS:
        for segment, index in sorted(by_part[part].items()):
            name = nodes[index].get("name", "#{}".format(index))
            try:
                positions, normals, _triangles = node_geometry(js, binary, index, name)
            except ValueError as error:
                failures.append(str(error))
                continue
            far = max(math.sqrt(x * x + y * y + z * z) for x, y, z in positions)
            if far > height + TOL_M:
                failures.append("a vertex of {!r} is {:.3f} m from its origin; no part of a "
                                "{:.3f} m tree reaches further".format(name, far, height))
            row = segments[segment] if segment < len(segments) else None
            if row is None:
                continue
            if part == "trunk":
                failures += _trunk_failures(segment, row, positions, rings)
            elif part in ("cap_bottom", "cap_top"):
                failures += _cap_failures(part, segment, row, positions, normals)
    return failures


def _trunk_failures(segment: int, row: dict, positions, rings) -> List[str]:
    failures = []
    bottom = [p for p in positions if abs(p[1]) <= TOL_M]
    top = [p for p in positions if abs(p[1] - row["height_m"]) <= TOL_M]
    stray = [p for p in positions if p not in bottom and p not in top]
    if stray:
        failures.append("trunk {} has {} vertex(es) off its ring planes y = 0 and y = {}, the "
                        "first at y = {:.4f}".format(segment, len(stray), row["height_m"],
                                                     stray[0][1]))
    for ring, radius, which in ((bottom, row["r_bottom_m"], "bottom"), (top, row["r_top_m"], "top")):
        if len(ring) < 3:
            failures.append("trunk {}'s {} ring has {} vertex(es); a ring is at least three".format(
                segment, which, len(ring)))
            continue
        worst = max(abs(math.hypot(x, z) - radius) for x, _, z in ring)
        if worst > TOL_M:
            failures.append("trunk {}'s {} ring is {:.4f} m off the ron's r_{}_m {} at its worst "
                            "vertex".format(segment, which, worst, which, radius))
    rings[segment] = (bottom, top)
    below = rings.get(segment - 1)
    if below is not None and len(bottom) >= 3 and len(below[1]) >= 3:
        unmatched = _ring_twins(below[1], bottom) + _ring_twins(bottom, below[1])
        if unmatched:
            failures.append("trunk {}'s top ring and trunk {}'s bottom ring are not one loop: {} "
                            "vertex(es) have no twin within {} m".format(
                                segment - 1, segment, unmatched, TOL_M))
    return failures


def _cap_failures(part: str, segment: int, row: dict, positions, normals) -> List[str]:
    failures = []
    radius = row["r_bottom_m"] if part == "cap_bottom" else row["r_top_m"]
    axis = -1.0 if part == "cap_bottom" else 1.0
    off_plane = max(abs(p[1]) for p in positions)
    if off_plane > TOL_M:
        failures.append("{} {} is not flat on its plane: a vertex sits {:.4f} m off it".format(
            part, segment, off_plane))
    rim = max(math.hypot(x, z) for x, _, z in positions)
    if abs(rim - radius) > TOL_M:
        failures.append("{} {} reaches {:.4f} m and the ron's ring radius is {}".format(
            part, segment, rim, radius))
    if normals is None:
        failures.append("{} {} has no normals; the disc's facing is read from them".format(
            part, segment))
    else:
        tilt = max(max(abs(nx), abs(ny - axis), abs(nz)) for nx, ny, nz in normals)
        if tilt > NORMAL_TOL:
            failures.append("{} {} does not face {}Y: a normal is {:.3f} off the axis".format(
                part, segment, "-" if axis < 0 else "+", tilt))
    return failures


# ── the certificate ──────────────────────────────────────────────────────────────────────────────

def certificate(digest: str, view_blob: bytes, ron_blob: bytes, mesh_count: int) -> dict:
    return {
        "blend_digest": digest,
        "view_glb_sha": sha256_bytes(view_blob),
        "sim_ron_sha": sha256_bytes(ron_blob),
        "mesh_count": mesh_count,
        "chains": {},
    }


def certificate_bytes(cert: dict) -> bytes:
    return (json.dumps(cert, indent=2, sort_keys=False, allow_nan=False) + "\n").encode()


def coherence(cert, view_blob: bytes, ron_blob: bytes, digest: Optional[str]) -> List[str]:
    """Every claim the certificate makes about the bytes beside it, as named failures."""
    failures = []
    if not isinstance(cert, dict):
        return ["the certificate is not an object"]
    missing = [field for field in CERTIFICATE_FIELDS if field not in cert]
    extra = [field for field in cert if field not in CERTIFICATE_FIELDS]
    if missing:
        failures.append("the certificate has no {}".format(", ".join(missing)))
    if extra:
        failures.append("the certificate carries {}, which is not one of its five fields".format(
            ", ".join(extra)))
    if missing:
        return failures
    for field, blob, what in (("view_glb_sha", view_blob, "view glb"),
                              ("sim_ron_sha", ron_blob, "tree ron")):
        actual = sha256_bytes(blob)
        if cert[field] != actual:
            failures.append("{} is {!r} and the {} hashes to {!r} — the trio is incoherent".format(
                field, cert[field], what, actual))
    if digest is not None and cert["blend_digest"] != digest:
        failures.append("blend_digest is {!r} and this source and pipeline digest to {!r} — the "
                        "trio is stale".format(cert["blend_digest"], digest))
    try:
        js, _ = parse_glb(view_blob)
    except ValueError as error:
        return failures + ["the view glb cannot be read: {}".format(error)]
    held = len(js.get("meshes", []))
    if cert["mesh_count"] != held:
        failures.append("mesh_count is {!r} and the view glb holds {} mesh(es)".format(
            cert["mesh_count"], held))
    if cert["chains"] != {}:
        failures.append("chains is {!r}; this build certifies no rung, so a chain here is one "
                        "nothing measured".format(cert["chains"]))
    return failures


def land(path: str, blob: bytes) -> None:
    """One file at its tracked path, by a rename within its own directory — the tank build's
    `trio._land`, which the two lanes do not yet share."""
    directory = os.path.dirname(path) or "."
    handle, staging = tempfile.mkstemp(prefix="." + os.path.basename(path) + ".", dir=directory)
    try:
        with os.fdopen(handle, "wb") as target:
            target.write(blob)
        mask = os.umask(0)
        os.umask(mask)
        os.chmod(staging, 0o666 & ~mask)
        os.replace(staging, path)
    except BaseException:
        if os.path.exists(staging):
            os.remove(staging)
        raise


# ── the stages ───────────────────────────────────────────────────────────────────────────────────

def paths(blend: str):
    stem = blend[:-len(".blend")]
    return stem + ".glb", stem + ".tree.ron", stem + ".lod.json"


def shown(path: str, root: str) -> str:
    """A path as a report names it: relative to the repository when inside it, else as given."""
    relative = os.path.relpath(path, root)
    return path if relative.startswith("..") else relative


def export(blend: str, root: str, work: str, blender: str) -> bytes:
    candidate = os.path.join(work, os.path.basename(blend)[:-len(".blend")] + ".glb")
    command = [blender, "--background", "--factory-startup", blend,
               "--python", os.path.join(root, EXPORT), "--", "--out", candidate]
    completed = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if completed.returncode or not os.path.isfile(candidate):
        tail = completed.stdout.decode(errors="replace").strip().splitlines()[-12:]
        raise Refused("export", ["export_tree.py exited {}".format(completed.returncode)] + tail)
    return read(candidate)


def contracts(view_blob: bytes, tree_ron: str, table: str, registry: str, stem: str) -> dict:
    """The three contracts over one candidate; the parsed glb JSON when they hold."""
    try:
        js, binary = parse_glb(view_blob)
    except ValueError as error:
        raise Refused("contract", [str(error)]) from error
    failures = contract_failures(js, stem)
    if failures:
        raise Refused("contract", failures)
    try:
        tree = ron.load(tree_ron)
        species = ron.load(table)
    except (OSError, ron.RonError) as error:
        raise Refused("ron", ["{}: {}".format(tree_ron, error)]) from error
    failures = ron_failures(tree, js, species, substances(registry))
    if failures:
        raise Refused("ron", failures)
    failures = geometry_failures(js, binary, tree["segments"])
    if failures:
        raise Refused("geometry", failures)
    return js


def build(blend: str, root: str, work: str, blender: str, registry: str, publish: bool) -> int:
    glb, tree_ron, cert_path = paths(blend)
    stem = os.path.basename(blend)[:-len(".blend")]
    table = os.path.join(root, SPECIES_TABLE)
    if not os.path.isfile(tree_ron):
        raise Refused("ron", ["{} is not beside the blend; the generator writes it".format(
            shown(tree_ron, root))])
    print("build ▸ export", flush=True)
    view_blob = export(blend, root, work, blender)
    print("build ▸ contracts", flush=True)
    js = contracts(view_blob, tree_ron, table, registry, stem)
    by_part = tree_nodes(js)
    print("build ▸ {} segment(s), {} canopy node(s), {} mesh(es), {} triangle-bearing "
          "primitive(s)".format(len(by_part["trunk"]), len(by_part["canopy"]),
                                len(js.get("meshes", [])),
                                sum(len(m["primitives"]) for m in js.get("meshes", []))),
          flush=True)
    if not publish:
        print("build ▸ lint certified", flush=True)
        return 0
    cert = certificate(blend_digest(blend, tree_ron, table, registry), view_blob, read(tree_ron),
                       len(js.get("meshes", [])))
    land(glb, view_blob)
    land(cert_path, certificate_bytes(cert))
    for path in (glb, tree_ron, cert_path):
        print("build ▸ published {} — {:.2f} MB, sha256 {}".format(
            shown(path, root), os.path.getsize(path) / 1e6, sha256_bytes(read(path))),
            flush=True)
    print("build ▸ LOD0 only: chains is empty, the scoped exception the module doc states",
          flush=True)
    return 0


def verify(blend: str, root: str, work: str, blender: str, registry: str) -> int:
    glb, tree_ron, cert_path = paths(blend)
    stem = os.path.basename(blend)[:-len(".blend")]
    table = os.path.join(root, SPECIES_TABLE)
    missing = [shown(p, root) for p in (glb, tree_ron, cert_path) if not os.path.isfile(p)]
    if missing:
        raise Refused("trio", ["the trio is incomplete: {} not here".format(", ".join(missing))])
    view_blob, ron_blob = read(glb), read(tree_ron)
    try:
        cert = json.loads(read(cert_path).decode("utf-8"))
    except ValueError as error:
        raise Refused("certificate", ["{}: {}".format(cert_path, error)]) from error
    failures = coherence(cert, view_blob, ron_blob, blend_digest(blend, tree_ron, table, registry))
    if failures:
        raise Refused("certificate", failures + [REBUILD])
    contracts(view_blob, tree_ron, table, registry, stem)
    print("build ▸ re-export", flush=True)
    candidate = export(blend, root, work, blender)
    if candidate != view_blob:
        at = next((i for i, (a, b) in enumerate(zip(candidate, view_blob)) if a != b),
                  min(len(candidate), len(view_blob)))
        raise Refused("compare", [
            "{} is {} byte(s) and the blend exports to {} byte(s), first divergence at byte "
            "{}".format(shown(glb, root), len(view_blob), len(candidate), at),
            REBUILD,
        ])
    print("build ▸ verify certified — {} coherent, the blend's own export, LOD0 only".format(
        shown(glb, root)), flush=True)
    return 0


def parse(argv: Optional[List[str]] = None):
    parser = argparse.ArgumentParser(prog="build.py", allow_abbrev=False)
    parser.add_argument("mode", choices=MODES)
    parser.add_argument("blend", help="assets/trees/<id>/<id>.blend — the generator's output")
    parser.add_argument("--registry", default=None,
                        help="the substance registry a species' substance must be a key of; "
                             "assets/materials/materials.ron by default. A lane verifying another "
                             "revision passes that revision's.")
    return parser.parse_args(argv)


def run(mode: str, blend: str, registry: Optional[str] = None) -> int:
    blend = os.path.abspath(blend)
    if not blend.endswith(".blend"):
        print("build ▸ {} is not a .blend".format(blend), flush=True)
        return 1
    root = repo_root()
    registry = os.path.abspath(registry) if registry else os.path.join(root, REGISTRY)
    try:
        program = toolchain.blender()
        wrong = toolchain.finding(program)
        if wrong is not None:
            raise Refused("toolchain", [wrong.evidence, wrong.repair])
        with tempfile.TemporaryDirectory(prefix="prop-build-") as work:
            if mode == "verify":
                return verify(blend, root, work, program.binary, registry)
            return build(blend, root, work, program.binary, registry, publish=mode == "build")
    except Refused as refusal:
        for line in refusal.failures:
            print("  - {}".format(line), flush=True)
        print("build ▸ {} refused at {}{}".format(
            mode, refusal.stage, " — the tracked trio is unchanged" if mode == "build" else ""),
            flush=True)
        return 1


def main() -> int:
    arguments = parse()
    return run(arguments.mode, arguments.blend, arguments.registry)


if __name__ == "__main__":
    sys.exit(main())
