"""build.py — the prop build: one command per tree, the trio of the slice contract.

    python3 scripts/prop/build.py build  assets/trees/<id>/<id>.blend
    python3 scripts/prop/build.py verify assets/trees/<id>/<id>.blend
    python3 scripts/prop/build.py lint   assets/trees/<id>/<id>.blend

A `.blend` (and the `<id>.tree.ron` the generator wrote beside it) goes in; `<id>.glb` and
`<id>.lod.json` come out and the `.tree.ron` is carried through: checked against the glb and
hashed into the certificate. The pattern is the tank build's (`scripts/tank/build.py`, ADR 0035)
cut down to what a tree has.

WHAT `build` DOES, IN ORDER
1. the toolchain preflight (`scripts/toolchain.py` pins Blender);
2. the export, `scripts/prop/export_tree.py` inside Blender, into this invocation's work
   directory — custom properties as glTF extras;
3. the NODE CONTRACT on the candidate (`contract_failures`): one root, a trunk chain, two caps per
   segment, an optional canopy, extras on every node, origins on the parent's axis;
4. the `.tree.ron` held against the candidate (`ron_failures`): its species is in the table, its
   segment heights are the chain's node offsets and the cap_top offsets, adjacent radii meet, and
   the heights sum to the height the generator recorded on the root;
5. the certificate, in the tank's five-field shape;
6. a STAGED publish: the glb lands, the certificate last.

THE CERTIFICATE. The tank's five fields with `sim_ron_sha` in `sim_glb_sha`'s place (contract
amendment A13): a tree has no sim glb, its sim artifact is the `.tree.ron` the sim is built from at
spawn (ADR 0014), and the field hashes those bytes. `chains` is EMPTY: this build ships LOD0 only.
The trunk's segments are a few dozen triangles each, under any rung's shed fraction, and the chain
cutter has no border lock (ADR 0033 "no vertex/edge locks"), so independently collapsed segments
could open their shared boundary loop; the canopy ships one card density (A12, the card ladder is
deferred). A certificate with no chain renders at source detail (ADR 0035's benign case).

THE RON carries no `dbh_m` (A6): the loader derives DBH from the taper at 1.3 m, so a second
statement of it here would be one that could disagree. The caps' meaning is the runtime's (A5): an
interior cut k reveals `cap_top_{k-1}` and `cap_bottom_k`, cut 0 reveals `cap_bottom_0` only; this
build only guarantees both discs exist per segment, flat, at its two ends.

WHAT `verify` DOES: the trio's coherence (both hashes, the staleness digest, the five fields) and
the two contracts over the TRACKED glb and ron, with no Blender. It writes nothing.

WHAT `lint` DOES: the export into a work directory and the two contracts, publishing nothing.

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
from typing import Dict, List, Optional, Tuple

_SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _SCRIPTS)

import ron  # noqa: E402  — the paths above are what make these importable
import toolchain  # noqa: E402

MODES = ("build", "verify", "lint")
EXPORT = os.path.join("scripts", "prop", "export_tree.py")
SPECIES_TABLE = os.path.join("assets", "trees", "species.ron")
CERTIFICATE_FIELDS = ("blend_digest", "view_glb_sha", "sim_ron_sha", "mesh_count", "chains")
PARTS = ("trunk", "cap_bottom", "cap_top", "canopy")

#: What can move either artifact, hashed into `blend_digest` beside the inputs.
PIPELINE_SOURCES = ("build.py", "export_tree.py", "gen_tree.py", "plan.py", "ron.py",
                    "../toolchain.py")

#: How far a node offset may sit from the ron's metres: the glb stores float32 and the ron four
#: decimals.
OFFSET_TOL_M = 1e-4

REBUILD = ("run `scripts/prop/build.py build` and commit the trio — the tracked artifacts are not "
           "what this source and this pipeline produce")


class Refused(Exception):
    def __init__(self, stage: str, failures: List[str]):
        super().__init__(stage)
        self.stage = stage
        self.failures = failures


# ── reading ──────────────────────────────────────────────────────────────────────────────────────

def parse_glb(blob: bytes) -> Tuple[dict, bytes]:
    """`(json, bin)` of a binary glTF; refuses anything that is not one."""
    if len(blob) < 12:
        raise ValueError("not a glb: {} byte(s)".format(len(blob)))
    magic, version, _total = struct.unpack_from("<III", blob, 0)
    if magic != 0x46546C67 or version != 2:
        raise ValueError("not a glb 2 (magic {:#x}, version {})".format(magic, version))
    at, js, binary = 12, None, b""
    while at + 8 <= len(blob):
        length, kind = struct.unpack_from("<II", blob, at)
        chunk = blob[at + 8:at + 8 + length]
        if kind == 0x4E4F534A:
            js = json.loads(chunk.decode("utf-8"))
        elif kind == 0x004E4942:
            binary = chunk
        at += 8 + length
    if js is None:
        raise ValueError("the glb has no JSON chunk")
    return js, binary


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


def blend_digest(blend: str, tree_ron: str, table: str) -> str:
    """The staleness field: the source, the ron, the species table, the pipeline, the pins."""
    digest = hashlib.sha256()
    for path in (blend, tree_ron, table):
        digest.update(sha256_bytes(read(path)).encode())
    digest.update(sources_digest().encode())
    for _key, value in toolchain.ENVIRONMENT:
        digest.update(value.encode())
    return digest.hexdigest()


# ── the node contract ────────────────────────────────────────────────────────────────────────────

def _translation(node: dict) -> List[float]:
    return [float(v) for v in node.get("translation", [0.0, 0.0, 0.0])]


def _untransformed(node: dict) -> bool:
    rotation = node.get("rotation", [0.0, 0.0, 0.0, 1.0])
    scale = node.get("scale", [1.0, 1.0, 1.0])
    return ("matrix" not in node
            and all(abs(a - b) < 1e-6 for a, b in zip(rotation, (0.0, 0.0, 0.0, 1.0)))
            and all(abs(s - 1.0) < 1e-6 for s in scale))


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
        if not isinstance(segment, int) or isinstance(segment, bool) or segment < 0:
            failures.append("node {!r} has tree_segment {!r}, not a non-negative integer".format(
                name, segment))
        if "mesh" not in node:
            failures.append("node {!r} references no mesh".format(name))
        if not _untransformed(node):
            failures.append("node {!r} carries a rotation, scale or matrix; a tree node is a "
                            "translation only, so a fall is a rotation about its origin".format(name))
        translation = _translation(node)
        if abs(translation[0]) > 1e-6 or abs(translation[2]) > 1e-6:
            failures.append("node {!r} sits off its parent's axis at {}".format(name, translation))
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
        if segment > 0 and _translation(nodes[index])[1] <= 0.0:
            failures.append("trunk {} ({!r}) sits {} m up its parent; a segment is placed its "
                            "parent's height up the +Y axis".format(
                                segment, name, _translation(nodes[index])[1]))
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


# ── the ron, against the glb ─────────────────────────────────────────────────────────────────────

def _finite_positive(value) -> bool:
    return (isinstance(value, (int, float)) and not isinstance(value, bool)
            and math.isfinite(value) and value > 0.0)


def ron_failures(tree, js: dict, species: dict) -> List[str]:
    """Every claim the `.tree.ron` makes that the glb or the species table does not bear out."""
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
    segments = tree["segments"]
    if not isinstance(segments, list) or not segments:
        return failures + ["segments is empty; a trunk is at least one segment"]
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
                    and abs(below - segment["r_bottom_m"]) > OFFSET_TOL_M:
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
            if abs(offset - segments[index - 1]["height_m"]) > OFFSET_TOL_M:
                failures.append("trunk {} sits {} m up its parent and the ron gives segment {} "
                                "height_m {}".format(index, offset, index - 1,
                                                     segments[index - 1]["height_m"]))
        top = by_part["cap_top"].get(index)
        if top is not None:
            offset = _translation(nodes[top])[1]
            if abs(offset - segment["height_m"]) > OFFSET_TOL_M:
                failures.append("cap_top {} sits {} m up its segment and the ron gives height_m "
                                "{}".format(index, offset, segment["height_m"]))
    root = nodes[js.get("scenes", [{}])[js.get("scene", 0)]["nodes"][0]]
    recorded = (root.get("extras") or {}).get("tree_height_m")
    total = sum(segment["height_m"] for segment in segments)
    if isinstance(recorded, (int, float)) and abs(total - recorded) > 1e-3:
        failures.append("the segment heights sum to {} m and the root records a {} m tree".format(
            round(total, 4), recorded))
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
    """One file at its tracked path, by a rename within its own directory."""
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


def export(blend: str, root: str, work: str, blender: str) -> bytes:
    candidate = os.path.join(work, os.path.basename(blend)[:-len(".blend")] + ".glb")
    command = [blender, "--background", "--factory-startup", blend,
               "--python", os.path.join(root, EXPORT), "--", "--out", candidate]
    completed = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if completed.returncode or not os.path.isfile(candidate):
        tail = completed.stdout.decode(errors="replace").strip().splitlines()[-12:]
        raise Refused("export", ["export_tree.py exited {}".format(completed.returncode)] + tail)
    return read(candidate)


def contracts(view_blob: bytes, tree_ron: str, table: str, stem: str) -> dict:
    """Both contracts over one candidate; the parsed glb JSON when they hold."""
    try:
        js, _ = parse_glb(view_blob)
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
    return js


def build(blend: str, root: str, work: str, blender: str, publish: bool) -> int:
    glb, tree_ron, cert_path = paths(blend)
    stem = os.path.basename(blend)[:-len(".blend")]
    table = os.path.join(root, SPECIES_TABLE)
    if not os.path.isfile(tree_ron):
        raise Refused("ron", ["{} is not beside the blend; the generator writes it".format(
            os.path.relpath(tree_ron, root))])
    print("build ▸ export", flush=True)
    view_blob = export(blend, root, work, blender)
    print("build ▸ contracts", flush=True)
    js = contracts(view_blob, tree_ron, table, stem)
    by_part = tree_nodes(js)
    print("build ▸ {} segment(s), {} canopy node(s), {} mesh(es), {} triangle-bearing "
          "primitive(s)".format(len(by_part["trunk"]), len(by_part["canopy"]),
                                len(js.get("meshes", [])),
                                sum(len(m["primitives"]) for m in js.get("meshes", []))),
          flush=True)
    if not publish:
        print("build ▸ lint certified", flush=True)
        return 0
    cert = certificate(blend_digest(blend, tree_ron, table), view_blob, read(tree_ron),
                       len(js.get("meshes", [])))
    land(glb, view_blob)
    land(cert_path, certificate_bytes(cert))
    for path in (glb, tree_ron, cert_path):
        print("build ▸ published {} — {:.2f} MB, sha256 {}".format(
            os.path.relpath(path, root), os.path.getsize(path) / 1e6, sha256_bytes(read(path))),
            flush=True)
    print("build ▸ LOD0 only: chains is empty by design (see the module doc)", flush=True)
    return 0


def verify(blend: str, root: str) -> int:
    glb, tree_ron, cert_path = paths(blend)
    stem = os.path.basename(blend)[:-len(".blend")]
    table = os.path.join(root, SPECIES_TABLE)
    missing = [os.path.relpath(p, root) for p in (glb, tree_ron, cert_path) if not os.path.isfile(p)]
    if missing:
        raise Refused("trio", ["the trio is incomplete: {} not here".format(", ".join(missing))])
    view_blob, ron_blob = read(glb), read(tree_ron)
    try:
        cert = json.loads(read(cert_path).decode("utf-8"))
    except ValueError as error:
        raise Refused("certificate", ["{}: {}".format(cert_path, error)]) from error
    failures = coherence(cert, view_blob, ron_blob, blend_digest(blend, tree_ron, table))
    if failures:
        raise Refused("certificate", failures + [REBUILD])
    contracts(view_blob, tree_ron, table, stem)
    print("build ▸ verify certified — {} coherent, LOD0 only".format(os.path.relpath(glb, root)),
          flush=True)
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
        if mode == "verify":
            return verify(blend, root)
        program = toolchain.blender()
        wrong = toolchain.finding(program)
        if wrong is not None:
            raise Refused("toolchain", [wrong.evidence, wrong.repair])
        with tempfile.TemporaryDirectory(prefix="prop-build-") as work:
            return build(blend, root, work, program.binary, publish=mode == "build")
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
