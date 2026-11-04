"""Build and certify a generated prop's common construction source and foliage.

build/lint/verify take assets/trees/<id>/<id>.blend and its .tree.ron generator
source. The GLB is an inspection artifact: one closed trunk and a canopy at the
origin. Simulation uses only the common construction bake and chunk-owned dressing.
The certificate fingerprints the source, view and cutter input; verify also re-exports
Blender and compares bytes. LOD chains remain unsupported for this inspection view.
The construction bake owns partition, mass/inertia, attachment and runtime admission.
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
from typing import Dict, List, Optional, Tuple

_SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _SCRIPTS)

import plan  # noqa: E402  — the paths above are what make these importable
import ron  # noqa: E402
import toolchain  # noqa: E402

MODES = ("build", "verify", "lint")
EXPORT = os.path.join("scripts", "prop", "export_tree.py")
SPECIES_TABLE = os.path.join("assets", "trees", "species.ron")
CERTIFICATE_FIELDS = ("view_glb_sha", "source_ron_sha", "construction_sha", "mesh_count", "chains")
PARTS = ("trunk", "canopy")

#: Where a tree's construction goes, under the repository root: the construction bake's spec folder.
CONSTRUCTIONS = os.path.join("tools", "constructions")

#: Where a tree's view dressing goes, under the repository root: beside its baked construction,
#: as the game's construction library reads it.
DRESSINGS = os.path.join("assets", "constructions")

#: What can move either artifact — the build's declared dependency list, relative to this
#: directory. Read only by `scripts/hooks/test_pushed_assets.sh`, by text: its coverage row holds
#: the CI lane's surface (`assets_shared_lanes`, `scripts/hooks/pushed_assets.sh`) to re-verify
#: every tree trio when an entry moves.
PIPELINE_SOURCES = (
    "build.py",
    "export_tree.py",
    "gen_tree.py",
    "plan.py",
    "ron.py",
    "../toolchain.py",
)

#: Resolution: float32 view-coordinate allowance, metres; below visible silhouette changes.
TOL_M = 1e-4

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
            failures.append("node {!r} carries a {}; a source node has identity rotation and scale".format(name, key))
    return failures


def tree_nodes(js: dict) -> Dict[str, int]:
    return {n.get("extras", {}).get("construction_part"): i
            for i, n in enumerate(js.get("nodes", []))
            if n.get("extras", {}).get("construction_part") in PARTS}


def root_node(js: dict) -> int:
    return js.get("scenes", [{}])[js.get("scene", 0)].get("nodes", [None])[0]


def contract_failures(js: dict, stem: str) -> List[str]:
    """The inspection view has one identity root with one trunk and canopy child."""
    nodes = js.get("nodes", [])
    roots = js.get("scenes", [{}])[js.get("scene", 0)].get("nodes", [])
    if len(roots) != 1 or not _index(roots[0], len(nodes)) or nodes[roots[0]].get("name") != stem:
        return ["the scene must have one root named {!r}".format(stem)]
    root = roots[0]
    failures, parts = [], []
    for index, node in enumerate(nodes):
        name = node.get("name", str(index))
        wrong = transform_failures(node, name)
        failures += wrong
        if not wrong and any(abs(v) > 1e-6 for v in _translation(node)):
            failures.append("node {!r} must be at the source origin".format(name))
        if index == root:
            continue
        extras = node.get("extras", {})
        part = extras.get("construction_part")
        parts.append(part)
        if part not in PARTS:
            failures.append("node {!r} has no recognized construction_part".format(name))
        if index not in nodes[root].get("children", []) or node.get("children"):
            failures.append("node {!r} must be a direct leaf child of the root".format(name))
        if "mesh" not in node:
            failures.append("node {!r} references no mesh".format(name))
    if sorted(str(p) for p in parts) != sorted(PARTS):
        failures.append("the source must have exactly one trunk and canopy")
    return failures


# ── the ron, against the glb and the tables ──────────────────────────────────────────────────────

#: The jobs a cut parameter may state (`tools/src/construction/spec.rs`, `Job`): a look or a
#: resolution, never a strength.
CUT_JOBS = ("Taste", "Resolution")


def _provenance(pair) -> bool:
    """Whether `pair` is a positive `(value, Job("why"))` of a job in `CUT_JOBS` with a non-blank
    reason."""
    if not (isinstance(pair, tuple) and len(pair) == 2 and _finite_positive(pair[0])):
        return False
    job = pair[1]
    return (isinstance(job, ron.Call) and job.name in CUT_JOBS and len(job.args) == 1
            and isinstance(job.args[0], str) and job.args[0].strip() != "")


def species_failures(key: str, row) -> List[str]:
    """The row a tree's species names, checked before it is built from: a `Species` row whose
    `trunk` names its material, anchor and cells, each cell parameter a checked provenance pair
    (`_provenance`). Their values are the bake's to refuse."""
    if not isinstance(row, dict) or row.get("__struct__") != "Species":
        return ["species {!r} is not a Species row".format(key)]
    expected = {"__struct__", "height_m", "dbh_m", "crown_base_frac", "crown_width_frac",
                "whorl_spacing_m", "trunk"}
    if set(row) != expected:
        return ["species {!r}: author exactly the generation fields {}; regenerate".format(key, sorted(expected))]
    for field in ("height_m", "dbh_m"):
        bounds = row[field]
        if not (isinstance(bounds, tuple) and len(bounds) == 2
                and all(_finite_positive(v) for v in bounds) and bounds[0] < bounds[1]):
            return ["species {!r}.{}: author an ordered positive range".format(key, field)]
    for field in ("crown_base_frac", "crown_width_frac", "whorl_spacing_m"):
        if not _finite_positive(row[field]):
            return ["species {!r}.{}: author a finite positive foliage parameter".format(key, field)]
    if row["crown_base_frac"] >= 1:
        return ["species {!r}.crown_base_frac: author a fraction below one".format(key)]
    trunk = row.get("trunk")
    if not isinstance(trunk, dict) or trunk.get("__struct__") != "Trunk":
        return ["species {!r} has no Trunk: its construction names a material, an anchor and "
                "cells".format(key)]
    if set(trunk) != {"__struct__", "material", "anchor", "cells", "shape"}:
        return ["species {!r}.trunk: author only material, anchor, shape and cells".format(key)]
    failures = ["species {!r} trunk {} is {!r}, not a key".format(key, field, trunk.get(field))
                for field in ("material", "anchor") if not isinstance(trunk.get(field), str) or not trunk[field].strip()]
    cells = trunk.get("cells")
    for field in ("size_m", "stretch"):
        pair = cells.get(field) if isinstance(cells, dict) else None
        if not _provenance(pair):
            failures.append("{}: species {!r} trunk.cells.{} is {!r}: author a positive (value, "
                            "Taste(\"why\")) or (value, Resolution(\"why\")) pair with a non-blank "
                            "reason".format(SPECIES_TABLE, key, field, pair))
    if not isinstance(cells, dict) or set(cells) != {"size_m", "stretch"}:
        failures.append("trunk.cells requires exactly size_m and stretch")
    shape = trunk.get("shape")
    if not isinstance(shape, dict):
        return failures + ["species {!r} trunk.shape is required; author its taper data".format(key)]
    if set(shape) != {"breast_height_m", "exponent", "radial_error_m"}:
        failures.append("trunk.shape requires exactly breast_height_m, exponent and radial_error_m")
    for field in ("breast_height_m", "exponent", "radial_error_m"):
        pair = shape.get(field)
        if not (isinstance(pair, tuple) and len(pair) == 2 and _finite_positive(pair[0])
                and isinstance(pair[1], ron.Call) and len(pair[1].args) == 1
                and pair[1].name in ("Measured", "Derived", "Unknown", "Taste", "Resolution")
                and isinstance(pair[1].args[0], str) and pair[1].args[0].strip()):
            failures.append("species {!r} trunk.shape.{} requires positive sourced data".format(key, field))
    if not failures and shape["radial_error_m"][1].name != "Resolution":
        failures.append("trunk.shape.radial_error_m requires Resolution with its geometric reason")
    if not failures and shape["exponent"][0] > 1:
        failures.append("trunk.shape.exponent must be <= 1 for a convex taper")
    if not failures and row["height_m"][0] <= shape["breast_height_m"][0]:
        failures.append("height_m must be above trunk.shape.breast_height_m")
    return failures


def ron_failures(tree, js: dict, species: dict) -> List[str]:
    fields = {"__struct__", "species", "seed", "height_m", "dbh_m"}
    if not isinstance(tree, dict) or tree.get("__struct__") != "TreeSource" or set(tree) != fields:
        return ["source must be TreeSource(species, seed, height_m, dbh_m); regenerate"]
    if tree["species"] not in species:
        return ["source species {!r} is not in the species table".format(tree["species"])]
    failures = species_failures(tree["species"], species[tree["species"]])
    for field in ("height_m", "dbh_m"):
        if not _finite_positive(tree[field]):
            failures.append("source {} must be finite and positive".format(field))
    if not isinstance(tree["seed"], int) or isinstance(tree["seed"], bool):
        failures.append("source seed must be an integer")
    if failures:
        return failures
    sampled = plan.sample(species[tree["species"]], tree["seed"])
    extras = js["nodes"][root_node(js)].get("extras", {})
    for field in ("species", "seed", "height_m", "dbh_m"):
        if extras.get("tree_"+field) != tree[field]:
            failures.append("root tree_{} disagrees with source; regenerate".format(field))
    for field in ("height_m", "dbh_m"):
        if sampled[field] != tree[field]:
            failures.append("source {} disagrees with species and seed; regenerate".format(field))
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


def source_solid(tree: dict, species: dict) -> tuple:
    return plan.solid(plan.rings(tree["height_m"], tree["dbh_m"],
                                 species[tree["species"]]["trunk"]["shape"]))


def geometry_failures(js: dict, binary: bytes, tree: dict, species: dict) -> List[str]:
    """The exported trunk tiles the source's exterior with outward triangles."""
    failures = []
    for part, index in tree_nodes(js).items():
        try:
            positions, _, triangles = node_geometry(js, binary, index, part)
        except ValueError as error:
            failures.append(str(error))
            continue
        if part != "trunk":
            continue
        corners, faces = source_solid(tree, species)
        planes = _planes(corners, faces)
        for vertex in positions:
            if all(math.dist(vertex, c) > TOL_M for c in corners):
                failures.append("trunk vertex {} is no source corner".format(vertex))
        for corner in corners:
            if all(math.dist(corner, p) > TOL_M for p in positions):
                failures.append("trunk lacks source corner {}".format(corner))
        # Exported normals split vertex indices; topology is over source corner identities.
        corner_ids = [next((i for i,c in enumerate(corners) if math.dist(p,c) <= TOL_M), None)
                      for p in positions]
        edges = {}
        for triangle in triangles:
            ids = [corner_ids[i] for i in triangle]
            if None in ids:
                continue  # The unknown-corner refusal above already names this input fault.
            for i,j in zip(ids, ids[1:]+ids[:1]):
                edges[(i,j)] = edges.get((i,j),0)+1
        if any(count != 1 or edges.get((j,i)) != 1 for (i,j),count in edges.items()):
            failures.append("trunk triangles must form a closed, oriented surface without duplicates")
        areas = [0.0]*len(faces)
        for triangle in triangles:
            a, b, c = [positions[i] for i in triangle]
            u, v = [b[k]-a[k] for k in range(3)], [c[k]-a[k] for k in range(3)]
            cross = (u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0])
            matches = [i for i, (n, d) in enumerate(planes)
                       if all(abs(sum(n[k]*p[k] for k in range(3))-d) <= TOL_M for p in (a,b,c))]
            if len(matches) != 1:
                failures.append("trunk triangle must lie on exactly one source face")
                continue
            i = matches[0]
            signed_area = sum(cross[k]*planes[i][0][k] for k in range(3))/2
            if signed_area <= 0:
                failures.append("trunk triangle must face outward with positive area")
            areas[i] += signed_area
        for i, face in enumerate(faces):
            points = [corners[j] for j in face]
            n = planes[i][0]
            expected = 0.0
            for b, c in zip(points[1:], points[2:]):
                u, v = [b[k]-points[0][k] for k in range(3)], [c[k]-points[0][k] for k in range(3)]
                expected += sum(n[k]*(u[(k+1)%3]*v[(k+2)%3]-u[(k+2)%3]*v[(k+1)%3]) for k in range(3))/2
            if abs(areas[i]-expected) > TOL_M*expected:
                failures.append("trunk face {} is not completely tiled".format(i))
    return failures


def trunk_vertices(js: dict, binary: bytes) -> List[tuple]:
    return node_geometry(js, binary, tree_nodes(js)["trunk"], "trunk")[0]


def construction_text(tree: dict, species: dict, stem: str) -> str:
    """The trunk's construction (module doc) from the ron's rings, its species row's `trunk` and
    the plan's section. Reads inputs the three contracts passed."""
    header = ("Generated by scripts/prop/build.py from assets/trees/{0}/{0}.tree.ron, its species "
              "row's trunk and scripts/prop/plan.py's section. Regenerate rather than "
              "edit.").format(stem)
    return plan.construction_ron(stem, species[tree["species"]]["trunk"], plan.rings(tree["height_m"], tree["dbh_m"], species[tree["species"]]["trunk"]["shape"]), header)


def dressing_text(js: dict, table: str, stem: str) -> str:
    """The trunk's view dressing (`plan.dressing_ron`): its cards replanned from the species row
    and the seed the glb's root records, the plan the generator built the canopy from. Refuses a
    root whose recorded height is not the replanned one: the cards would not be the canopy's."""
    extras = js["nodes"][root_node(js)].get("extras") or {}
    species = extras.get("tree_species")
    rows = ron.load(table)
    if species not in rows or not isinstance(extras.get("tree_seed"), int):
        raise Refused("dressing", ["the glb's root records species {!r} and seed {!r}; the "
                                   "generator writes both".format(species, extras.get("tree_seed"))])
    sampled = plan.sample(rows[species], extras["tree_seed"])
    if sampled["height_m"] != extras.get("tree_height_m"):
        raise Refused("dressing", ["the glb's root records a {} m tree and its seed plans {} m; "
                                   "regenerate the blend".format(extras.get("tree_height_m"),
                                                                 sampled["height_m"])])
    whorl_rows = plan.whorls(sampled["height_m"], rows[species], sampled["rng"])
    header = ("Generated by scripts/prop/build.py from {0}.glb's root (species {1}, seed {2}) and "
              "scripts/prop/plan.py's canopy. Regenerate rather than edit.").format(
                  stem, species, extras["tree_seed"])
    return plan.dressing_ron(whorl_rows, header)


def canopy_vertices(js: dict, binary: bytes) -> List[tuple]:
    return node_geometry(js, binary, tree_nodes(js)["canopy"], "canopy")[0]


def dressing_failures(dressing: str, js: dict, binary: bytes) -> List[str]:
    """Every way the dressing's cards and the view's canopy are not one set of cards: a card
    corner that is no canopy vertex within `TOL_M`, or a canopy vertex that is no card's corner
    (a card the dressing lost). Reads the cards back from the text."""
    try:
        cards = [[tuple(float(c) for c in corner) for corner in card["corners"]]
                 for card in ron.parse(dressing)["cards"]]
    except (ron.RonError, KeyError, IndexError, TypeError, ValueError) as error:
        return ["the dressing cannot be read: {}".format(error)]
    view = canopy_vertices(js, binary)
    corners = [corner for card in cards for corner in card]
    failures = []
    for card, outline in enumerate(cards):
        for index, corner in enumerate(outline):
            if all(math.dist(corner, vertex) > TOL_M for vertex in view):
                failures.append("card {} corner {} at {} is no vertex of the view's canopy".format(
                    card, index, tuple(round(c, 4) for c in corner)))
    for index, vertex in enumerate(view):
        if all(math.dist(corner, vertex) > TOL_M for corner in corners):
            failures.append("view canopy vertex {} at {} is no card's corner".format(
                index, tuple(round(c, 4) for c in vertex)))
    return failures


def dressing(view_blob: bytes, table: str, stem: str) -> str:
    """The dressing the candidate's root gives, refused unless its cards are the view's canopy."""
    js, binary = parse_glb(view_blob)
    text = dressing_text(js, table, stem)
    failures = dressing_failures(text, js, binary)
    if failures:
        raise Refused("dressing", failures)
    return text


def dressing_path(root: str, stem: str) -> str:
    return os.path.join(root, DRESSINGS, stem, stem + ".dressing.ron")


def _planes(corners: List[tuple], faces: List[List[int]]) -> List[tuple]:
    """Each face's outward unit normal and offset, by Newell's sum about the face's mean."""
    out = []
    for face in faces:
        points = [corners[i] for i in face]
        mean = [sum(p[k] for p in points) / len(points) for k in range(3)]
        n = [0.0, 0.0, 0.0]
        for p, q in zip(points, points[1:] + points[:1]):
            a = [p[k] - mean[k] for k in range(3)]
            b = [q[k] - mean[k] for k in range(3)]
            n = [n[0] + a[1] * b[2] - a[2] * b[1], n[1] + a[2] * b[0] - a[0] * b[2],
                 n[2] + a[0] * b[1] - a[1] * b[0]]
        size = math.sqrt(sum(c * c for c in n))
        n = [c / size for c in n]
        out.append((n, sum(n[k] * mean[k] for k in range(3))))
    return out


def identity_failures(construction: str, js: dict, binary: bytes) -> List[str]:
    """The one convex source element and the inspection trunk have identical corners."""
    try:
        pieces = [([tuple(float(c) for c in corner) for corner in piece["corners"]],
                   piece["faces"]) for piece in ron.parse(construction)["pieces"]]
    except (ron.RonError, KeyError, IndexError, TypeError, ValueError) as error:
        return ["the construction cannot be read: {}".format(error)]
    if len(pieces) != 1:
        return ["construction must have one source piece; the pattern alone cuts its interior"]
    view = trunk_vertices(js, binary)
    failures = []
    for piece, (corners, _) in enumerate(pieces):
        for index, corner in enumerate(corners):
            if all(math.dist(corner, vertex) > TOL_M for vertex in view):
                failures.append("piece {} corner {} at {} is no vertex of the view's trunk".format(
                    piece, index, tuple(round(c, 4) for c in corner)))
    planes = [_planes(corners, faces) for corners, faces in pieces]
    for index, vertex in enumerate(view):
        outside = min(max(sum(n[k] * vertex[k] for k in range(3)) - offset for n, offset in solid)
                      for solid in planes)
        if outside > TOL_M:
            failures.append("view trunk vertex {} at {} lies {:.4f} m outside the source solid".format(
                index, tuple(round(c, 4) for c in vertex), outside))
    return failures


def construction_path(root: str, stem: str) -> str:
    return os.path.join(root, CONSTRUCTIONS, stem + ".pieces.ron")


# ── the certificate ──────────────────────────────────────────────────────────────────────────────

def certificate(view_blob: bytes, ron_blob: bytes, construction_blob: bytes,
                mesh_count: int) -> dict:
    return {
        "view_glb_sha": sha256_bytes(view_blob),
        "source_ron_sha": sha256_bytes(ron_blob),
        "construction_sha": sha256_bytes(construction_blob),
        "mesh_count": mesh_count,
        "chains": {},
    }


def certificate_bytes(cert: dict) -> bytes:
    return (json.dumps(cert, indent=2, sort_keys=False, allow_nan=False) + "\n").encode()


def coherence(cert, view_blob: bytes, ron_blob: bytes, construction_blob: bytes) -> List[str]:
    """Every claim the certificate makes about the bytes beside it, as named failures."""
    failures = []
    if not isinstance(cert, dict):
        return ["the certificate is not an object"]
    missing = [field for field in CERTIFICATE_FIELDS if field not in cert]
    extra = [field for field in cert if field not in CERTIFICATE_FIELDS]
    if missing:
        failures.append("the certificate has no {}".format(", ".join(missing)))
    if extra:
        failures.append("the certificate carries {}, which is not one of its {} fields".format(
            ", ".join(extra), len(CERTIFICATE_FIELDS)))
    if missing:
        return failures
    for field, blob, what in (("view_glb_sha", view_blob, "view glb"),
                              ("source_ron_sha", ron_blob, "tree ron"),
                              ("construction_sha", construction_blob, "construction")):
        actual = sha256_bytes(blob)
        if cert[field] != actual:
            failures.append("{} is {!r} and the {} hashes to {!r} — the trio is incoherent".format(
                field, cert[field], what, actual))
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


def construction(view_blob: bytes, tree_ron: str, table: str, stem: str) -> str:
    """The three contracts over one candidate, then the construction they give and its identity
    with the view; the construction's text when they all hold."""
    js = contracts(view_blob, tree_ron, table, stem)
    _, binary = parse_glb(view_blob)
    text = construction_text(ron.load(tree_ron), ron.load(table), stem)
    failures = identity_failures(text, js, binary)
    if failures:
        raise Refused("identity", failures)
    return text


def stale_failures(view_blob: bytes, tree_ron: str, table: str, stem: str,
                   spec_blob: bytes) -> List[str]:
    """`spec_blob` against the construction the tree, its species row and its view give."""
    if construction(view_blob, tree_ron, table, stem).encode() == spec_blob:
        return []
    return ["{}.construction.ron is not what the tree, its species row and its view give".format(
        stem)]


def contracts(view_blob: bytes, tree_ron: str, table: str, stem: str) -> dict:
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
    failures = ron_failures(tree, js, species)
    if failures:
        raise Refused("ron", failures)
    try:
        failures = geometry_failures(js, binary, tree, species)
    except ValueError as error:
        raise Refused("geometry", ["{}: species {!r}: {}".format(table, tree["species"], error)]) from error
    if failures:
        raise Refused("geometry", failures)
    return js


def build(blend: str, root: str, work: str, blender: str, publish: bool) -> int:
    glb, tree_ron, cert_path = paths(blend)
    stem = os.path.basename(blend)[:-len(".blend")]
    table = os.path.join(root, SPECIES_TABLE)
    if not os.path.isfile(tree_ron):
        raise Refused("ron", ["{} is not beside the blend; the generator writes it".format(
            shown(tree_ron, root))])
    print("build ▸ export", flush=True)
    view_blob = export(blend, root, work, blender)
    print("build ▸ contracts and construction", flush=True)
    text = construction(view_blob, tree_ron, table, stem)
    js, _ = parse_glb(view_blob)
    print("build ▸ one convex source, {} mesh(es)".format(len(js.get("meshes", []))), flush=True)
    dressed_blob = dressing(view_blob, table, stem).encode()
    if not publish:
        print("build ▸ lint certified", flush=True)
        return 0
    spec = construction_path(root, stem)
    dressed = dressing_path(root, stem)
    cert = certificate(view_blob, read(tree_ron), text.encode(), len(js.get("meshes", [])))
    for path in (glb, spec, dressed, cert_path):
        os.makedirs(os.path.dirname(path), exist_ok=True)
    land(glb, view_blob)
    land(spec, text.encode())
    land(dressed, dressed_blob)
    land(cert_path, certificate_bytes(cert))
    print("build ▸ run `bake-constructions` (tools/) to cut {}".format(shown(spec, root)), flush=True)
    for path in (glb, tree_ron, spec, dressed, cert_path):
        print("build ▸ published {} — {:.2f} MB, sha256 {}".format(
            shown(path, root), os.path.getsize(path) / 1e6, sha256_bytes(read(path))),
            flush=True)
    print("build ▸ inspection view: source detail only, no LOD chains",
          flush=True)
    return 0


def verify(blend: str, root: str, work: str, blender: str) -> int:
    glb, tree_ron, cert_path = paths(blend)
    stem = os.path.basename(blend)[:-len(".blend")]
    table = os.path.join(root, SPECIES_TABLE)
    spec = construction_path(root, stem)
    missing = [shown(p, root) for p in (glb, tree_ron, spec, cert_path) if not os.path.isfile(p)]
    if missing:
        raise Refused("trio", ["the trio is incomplete: {} not here".format(", ".join(missing))])
    view_blob, ron_blob, spec_blob = read(glb), read(tree_ron), read(spec)
    try:
        cert = json.loads(read(cert_path).decode("utf-8"))
    except ValueError as error:
        raise Refused("certificate", ["{}: {}".format(cert_path, error)]) from error
    failures = coherence(cert, view_blob, ron_blob, spec_blob)
    if failures:
        raise Refused("certificate", failures + [REBUILD])
    failures = stale_failures(view_blob, tree_ron, table, stem, spec_blob)
    if failures:
        raise Refused("construction", failures + [REBUILD])
    dressed = dressing_path(root, stem)
    if not os.path.isfile(dressed) or read(dressed) != dressing(view_blob, table, stem).encode():
        raise Refused("dressing", ["{} is not what the glb's root and the plan give".format(
            shown(dressed, root)), REBUILD])
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
    return parser.parse_args(argv)


def run(mode: str, blend: str) -> int:
    blend = os.path.abspath(blend)
    if not blend.endswith(".blend"):
        print("build ▸ {} is not a .blend".format(blend), flush=True)
        return 1
    root = repo_root()
    try:
        program = toolchain.blender()
        wrong = toolchain.finding(program)
        if wrong is not None:
            raise Refused("toolchain", [wrong.evidence, wrong.repair])
        with tempfile.TemporaryDirectory(prefix="prop-build-") as work:
            if mode == "verify":
                return verify(blend, root, work, program.binary)
            return build(blend, root, work, program.binary, publish=mode == "build")
    except OSError as error:
        print("build ▸ {} refused at filesystem: {} — publication may be incomplete; "
              "fix the filesystem and rebuild".format(mode, error), flush=True)
        return 1
    except Refused as refusal:
        for line in refusal.failures:
            print("  - {}".format(line), flush=True)
        print("build ▸ {} refused at {}{}".format(
            mode, refusal.stage, " — the tracked trio is unchanged" if mode == "build" else ""),
            flush=True)
        return 1


def main() -> int:
    arguments = parse()
    return run(arguments.mode, arguments.blend)


if __name__ == "__main__":
    sys.exit(main())
