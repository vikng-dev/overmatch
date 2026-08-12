"""The three artifacts of ADR 0035, as document surgery: what a build assembles and what a
verification takes apart.

    <id>.glb       the view artifact — scene, textures, and every rung as an additional mesh record
    <id>.sim.glb   the sim artifact — rung-0 geometry and material names, nothing else
    <id>.lod.json  the certificate — five fields, nothing derivable

RUNG RECORDS ARE APPENDED, NEVER INTERLEAVED. Every rung mesh, accessor and bufferView lands after
the last one the door wrote, so the door-owned prefix of the view glb keeps its indices, its
bufferView byteOffsets and its bytes. `strip_rungs` is the inverse, and the door's own
section-by-section comparison runs against what it returns.

THE SIM ARTIFACT COPIES ACCESSOR PAYLOADS VERBATIM. Its POSITION, NORMAL and index bytes are the
view glb's bytes moved to new offsets, so both sides walk one surface by construction rather than by
convention.

Stdlib and numpy only — no `bpy`, so every law here is testable without Blender.
"""

from __future__ import annotations

import copy
import hashlib
import json
import math
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "lod"))

import glb_ktx2  # noqa: E402  — the paths above are what make these importable
import measure  # noqa: E402

#: The view glb's attributes that survive into the sim artifact. TEXCOORD_*, TANGENT and COLOR_*
#: are texture vocabulary; the ballistic walk reads position, winding and material.
SIM_ATTRIBUTES = ("POSITION", "NORMAL")

#: The certificate's whole schema (ADR 0035). A field outside this set is a second answer to a
#: question one of these already answers.
CERTIFICATE_FIELDS = ("blend_digest", "view_glb_sha", "sim_glb_sha", "mesh_count", "chains")

#: What a certificate is named, relative to the tracked model.
CERTIFICATE_SUFFIX = ".lod.json"
SIM_SUFFIX = ".sim.glb"


class TrioError(Exception):
    """A named refusal: the documents do not have the shape this law is stated over."""


# ── reading a primitive as a surface ─────────────────────────────────────────────────────────────

def chain_name(gltf, mesh_index, primitive_index):
    """The certificate's key for one source primitive: the glTF mesh name and the primitive index.

    Mesh names must be unique in a document this addresses — `census` refuses a duplicate, because
    two chains under one key is a certificate that describes neither.
    """
    name = gltf["meshes"][mesh_index].get("name")
    if not name:
        raise TrioError(f"mesh {mesh_index} has no name; a certificate keys chains by mesh name")
    return f"{name}#{primitive_index}"


def primitive_surface(gltf, binary, mesh_index, primitive_index, name):
    """One primitive of a multi-mesh document as a `measure.Surface`, in Blender coordinates.

    `measure.surface_from_bytes` addresses a single-mesh level or a named node; the seam is the
    PRIMITIVE (ADR 0035), so the mesh and primitive indices are addressed directly and every other
    refusal that decode makes is restated over the same attributes.
    """
    import numpy as np

    primitive = gltf["meshes"][mesh_index]["primitives"][primitive_index]
    attributes = primitive["attributes"]
    if primitive.get("mode", 4) != 4:
        raise measure.Refusal("not-triangles", f"{name} primitive mode {primitive.get('mode')}")
    if "indices" not in primitive:
        raise measure.Refusal("non-indexed-primitive", name)
    for required in ("POSITION", "NORMAL", "TEXCOORD_0"):
        if required not in attributes:
            raise measure.Refusal("missing-attribute", f"{name} has no {required}")
    for banned, reason in (("JOINTS_0", "skinned-mesh"), ("WEIGHTS_0", "skinned-mesh")):
        if banned in attributes:
            raise measure.Refusal(reason, f"{name} carries {banned}")
    if primitive.get("targets"):
        raise measure.Refusal("morph-mesh", f"{name} carries {len(primitive['targets'])} targets")

    read = measure._accessor  # noqa: SLF001 — the one decode every gate measures through
    indices = read(gltf, binary, primitive["indices"]).astype(np.int64).reshape(-1, 3)
    positions = measure.gltf_to_blender(
        read(gltf, binary, attributes["POSITION"]).astype(np.float64)
    )
    normals = measure.gltf_to_blender(read(gltf, binary, attributes["NORMAL"]).astype(np.float64))
    uvs = read(gltf, binary, attributes["TEXCOORD_0"]).astype(np.float64)
    return measure.Surface(positions, indices, normals[indices], uvs[indices], name)


def census(gltf, binary):
    """Every primitive of a document, as records ordered by (mesh index, primitive index).

    Each record carries the chain key, the geometry digest that DEDUPS chains, the triangle count
    and how many scene nodes reference the mesh. A primitive this lane cannot certify carries its
    refusal instead of a digest, and no chain is cut for it.
    """
    referenced = {}
    for node in gltf.get("nodes", []):
        if "mesh" in node:
            referenced[node["mesh"]] = referenced.get(node["mesh"], 0) + 1
    seen = set()
    rows = []
    for mesh_index, mesh in enumerate(gltf.get("meshes", [])):
        name = mesh.get("name")
        if name in seen:
            raise TrioError(
                f"two meshes are called {name!r}; a certificate keys chains by mesh name and would "
                f"describe neither"
            )
        seen.add(name)
        for primitive_index in range(len(mesh["primitives"])):
            key = chain_name(gltf, mesh_index, primitive_index)
            row = {
                "chain": key, "mesh_index": mesh_index, "primitive": primitive_index,
                "nodes": referenced.get(mesh_index, 0),
            }
            try:
                surface = primitive_surface(gltf, binary, mesh_index, primitive_index, key)
            except measure.Refusal as refusal:
                row["refusal"] = f"{refusal.reason}: {refusal.detail}"
            else:
                row["digest"] = surface.digest()
                row["tris"] = surface.tri_count
                row["diagonal_mm"] = surface.diagonal * 1000.0
                row["origin_radius_m"] = surface.origin_radius
            rows.append(row)
    return rows


def chains_by_digest(rows):
    """Digest -> the chain keys that share that source geometry, each list sorted.

    The 8 road-wheel nodes reference one mesh, so they are one key already; two meshes that hold
    the same geometry (the six smoke launchers) are one SEARCH and one set of rung records under
    two keys. The representative — the key the rung meshes are named after — is the smallest.
    """
    groups = {}
    for row in rows:
        if "digest" not in row:
            continue
        groups.setdefault(row["digest"], []).append(row["chain"])
    return {digest: sorted(keys) for digest, keys in groups.items()}


# ── embedding the rungs ──────────────────────────────────────────────────────────────────────────

def _accessor_payload(js, binary, index):
    """The bytes of one accessor, and the bufferView `target` they were written under.

    Refuses an interleaved or sparse accessor: the exporter writes one tightly packed bufferView per
    accessor, and a stride this copy did not honour would be silently wrong bytes.
    """
    accessor = js["accessors"][index]
    if "sparse" in accessor:
        raise TrioError(f"accessor {index} is sparse")
    view = js["bufferViews"][accessor["bufferView"]]
    if view.get("byteStride") is not None:
        raise TrioError(f"accessor {index} is interleaved (byteStride {view['byteStride']})")
    size = glb_ktx2.element_bytes(accessor) * accessor["count"]
    start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
    if start + size > view.get("byteOffset", 0) + view["byteLength"]:
        raise TrioError(f"accessor {index} runs past its bufferView")
    return binary[start:start + size], view.get("target")


def _append_accessor(js, binary, source_js, source_bin, index):
    """Copy one accessor's bytes to the end of `binary` and return the new accessor index."""
    payload, target = _accessor_payload(source_js, source_bin, index)
    binary += b"\0" * (-len(binary) % 4)
    view = {"buffer": 0, "byteOffset": len(binary), "byteLength": len(payload)}
    if target is not None:
        view["target"] = target
    binary += payload
    js["bufferViews"].append(view)
    accessor = copy.deepcopy(source_js["accessors"][index])
    accessor.pop("byteOffset", None)
    accessor["bufferView"] = len(js["bufferViews"]) - 1
    js["accessors"].append(accessor)
    return len(js["accessors"]) - 1, binary


def embed_rung(js, binary, rung_bytes, mesh_name):
    """Append one exported rung — a single-mesh, single-primitive glb — as a mesh record.

    NO MATERIAL. A rung wears the source primitive's material at bind time, which is what lets one
    set of rung records serve every chain key that shares the geometry.

    The attribute order is sorted and the indices accessor lands last, so the same rungs assemble
    into the same bytes on every run.
    """
    source_js, source_bin = glb_ktx2.parse_glb(rung_bytes, mesh_name)
    meshes = source_js.get("meshes", [])
    if len(meshes) != 1 or len(meshes[0]["primitives"]) != 1:
        raise TrioError(f"{mesh_name}: a rung is one mesh of one primitive")
    primitive = meshes[0]["primitives"][0]
    attributes = {}
    for key in sorted(primitive["attributes"]):
        attributes[key], binary = _append_accessor(
            js, binary, source_js, source_bin, primitive["attributes"][key]
        )
    indices, binary = _append_accessor(js, binary, source_js, source_bin, primitive["indices"])
    js["meshes"].append({
        "name": mesh_name,
        "primitives": [{"attributes": attributes, "indices": indices, "mode": 4}],
    })
    return js, binary


def embed_rungs(view_bytes, rungs):
    """The view artifact: the door's baked candidate with every rung appended, in the given order.

    `rungs` is a sequence of `(mesh_name, glb bytes)`. Returns `(bytes, mesh_count)`, `mesh_count`
    being the number of meshes the door wrote — where the rung records begin.
    """
    js, binary = glb_ktx2.parse_glb(view_bytes, "<view>")
    mesh_count = len(js.get("meshes", []))
    binary = bytearray(binary)
    for mesh_name, blob in rungs:
        js, binary = embed_rung(js, binary, blob, mesh_name)
    js["buffers"][0]["byteLength"] = len(binary)
    return glb_ktx2.glb_bytes(js, bytes(binary)), mesh_count


def strip_rungs(view_bytes, mesh_count):
    """The door-owned prefix of a view glb, serialized canonically. The inverse of `embed_rungs`.

    Every mesh, accessor and bufferView the rungs added is dropped, the buffer shrinks back to the
    door's own span, and what remains must BE the door's candidate byte for byte — which is what
    the door's comparison is then run against.
    """
    js, binary = glb_ktx2.parse_glb(view_bytes, "<view>")
    if mesh_count > len(js.get("meshes", [])):
        raise TrioError(
            f"the certificate declares {mesh_count} source mesh(es) and this glb holds "
            f"{len(js.get('meshes', []))}"
        )
    for node in js.get("nodes", []):
        if node.get("mesh", 0) >= mesh_count:
            raise TrioError(
                f"node {node.get('name')!r} references mesh {node['mesh']}, which the certificate "
                f"declares a rung record; rungs are referenced by no scene node"
            )
    kept_accessors, kept_views = set(), set()
    for mesh in js["meshes"][:mesh_count]:
        for primitive in mesh["primitives"]:
            for index in list(primitive["attributes"].values()) + [primitive.get("indices")]:
                if index is not None:
                    kept_accessors.add(index)
    for index in kept_accessors:
        kept_views.add(js["accessors"][index]["bufferView"])
    for image in js.get("images", []):
        if "bufferView" in image:
            kept_views.add(image["bufferView"])
    if kept_accessors and max(kept_accessors) >= len(js["accessors"]):
        raise TrioError("an accessor index is out of range")
    accessor_count = max(kept_accessors) + 1 if kept_accessors else 0
    view_count = max(kept_views) + 1 if kept_views else 0
    js["accessors"] = js["accessors"][:accessor_count]
    js["bufferViews"] = js["bufferViews"][:view_count]
    js["meshes"] = js["meshes"][:mesh_count]
    # The buffer shrinks back to the last byte a KEPT view reaches, which is what the exporter
    # declares (MEASURED on the tiger and on every shipped rung: byteLength == max view end). The
    # chunk's own 4-byte padding is `glb_bytes`'s and is not part of that number.
    span = max(
        (view.get("byteOffset", 0) + view["byteLength"] for view in js["bufferViews"]), default=0
    )
    js["buffers"][0]["byteLength"] = span
    return glb_ktx2.glb_bytes(js, bytes(binary[:span]))


# ── the sim artifact ─────────────────────────────────────────────────────────────────────────────

def sim_bytes(view_bytes, mesh_count):
    """The sim artifact: rung-0 geometry and material names, and nothing a renderer needs.

    Textures, samplers, extensions, UVs, tangents and every rung record are gone; what survives is
    the scene graph, the meshes the scene nodes reference, and the material NAME each primitive
    wears — material is membership (ADR 0007), and a name is the whole of it here.
    """
    js, binary = glb_ktx2.parse_glb(view_bytes, "<view>")
    out = {"asset": copy.deepcopy(js["asset"]), "accessors": [], "bufferViews": [], "meshes": []}
    payload = bytearray()
    for mesh in js["meshes"][:mesh_count]:
        primitives = []
        for primitive in mesh["primitives"]:
            attributes = {}
            for key in SIM_ATTRIBUTES:
                if key not in primitive["attributes"]:
                    raise TrioError(f"{mesh.get('name')!r} has no {key}")
                attributes[key], payload = _append_accessor(
                    out, payload, js, binary, primitive["attributes"][key]
                )
            indices, payload = _append_accessor(out, payload, js, binary, primitive["indices"])
            kept = {"attributes": attributes, "indices": indices, "mode": 4}
            if "material" in primitive:
                kept["material"] = primitive["material"]
            primitives.append(kept)
        out["meshes"].append({"name": mesh.get("name"), "primitives": primitives})
    out["materials"] = [{"name": material.get("name")} for material in js.get("materials", [])]
    out["nodes"] = copy.deepcopy(js.get("nodes", []))
    out["scenes"] = copy.deepcopy(js.get("scenes", []))
    out["scene"] = js.get("scene", 0)
    out["buffers"] = [{"byteLength": len(payload) + (-len(payload) % 4)}]
    return glb_ktx2.glb_bytes(out, bytes(payload))


def geometry_payloads(blob, mesh_count):
    """Every rung-0 geometry accessor's bytes, keyed by (mesh name, primitive, attribute).

    The equality the sim artifact is built to satisfy: these bytes, taken from the view glb and from
    the sim glb, are the same bytes at different offsets.
    """
    js, binary = glb_ktx2.parse_glb(blob, "<glb>")
    out = {}
    for mesh_index, mesh in enumerate(js["meshes"][:mesh_count]):
        for index, primitive in enumerate(mesh["primitives"]):
            for key in list(SIM_ATTRIBUTES) + ["indices"]:
                accessor = (primitive["attributes"].get(key) if key != "indices"
                            else primitive["indices"])
                if accessor is None:
                    continue
                out[(mesh.get("name") or mesh_index, index, key)] = _accessor_payload(
                    js, binary, accessor
                )[0]
    return out


# ── the certificate ──────────────────────────────────────────────────────────────────────────────

def sha256_bytes(blob):
    return hashlib.sha256(blob).hexdigest()


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _finite(value):
    """A real number, and not a NaN or an infinity.

    NaN is why this is a function rather than a comparison. Every ordering test is False against it
    — `b <= a` included — so a NaN deviation slides through an ascending check that is written the
    obvious way, and `json.dumps` then emits a bare `NaN` token no strict reader will parse.
    """
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def chain_failures(name, chain):
    """Every CONSTRUCTIVE law one chain row must satisfy, as named failures. Empty is the pass.

    ONE IMPLEMENTATION, TWO CALLERS. `certificate` refuses a row that breaks these when the trio is
    built; `coherence` re-applies them to the certificate ON DISK, because a hand-edited
    `deviation_mm`, a descending ladder or a bogus radius passes every hash the trio carries — the
    hashes cover the BINARIES, and the certificate is the file nobody was checking.
    """
    failures = []
    if not isinstance(chain, dict):
        failures.append(f"chain {name!r} is {type(chain).__name__}, not an object")
        return failures
    extra = sorted(set(chain) - {"radius_m", "rungs"})
    if extra:
        failures.append(
            f"chain {name!r} carries {', '.join(extra)}, which a chain row has no field for"
        )
    radius = chain.get("radius_m")
    if not _finite(radius) or radius <= 0.0:
        failures.append(f"chain {name!r} has radius_m {radius!r}, which is not a positive finite "
                        f"number of metres")
    rungs = chain.get("rungs")
    if not isinstance(rungs, list) or not rungs:
        failures.append(f"chain {name!r} has no rungs; a chain with none is not a row")
        return failures
    previous = None
    seen = set()
    for index, rung in enumerate(rungs):
        if not isinstance(rung, dict) or sorted(rung) != ["deviation_mm", "mesh"]:
            failures.append(f"chain {name!r} rung {index} is not a {{mesh, deviation_mm}} pair")
            continue
        if not isinstance(rung["mesh"], str) or not rung["mesh"]:
            failures.append(f"chain {name!r} rung {index} names no mesh")
        elif rung["mesh"] in seen:
            failures.append(f"chain {name!r} names rung mesh {rung['mesh']!r} twice")
        else:
            seen.add(rung["mesh"])
        deviation = rung["deviation_mm"]
        if not _finite(deviation) or deviation <= 0.0:
            failures.append(f"chain {name!r} rung {index} has deviation_mm {deviation!r}, which is "
                            f"not a positive finite number of millimetres")
        elif previous is not None and deviation <= previous:
            failures.append(
                f"chain {name!r} deviations do not strictly ascend: rung {index} is {deviation} "
                f"after {previous}. The runtime picks a level by comparing a derived distance "
                f"against the next one, and an unordered ladder makes that comparison meaningless"
            )
        elif previous is None or deviation > previous:
            previous = deviation
    return failures


def certificate(blend_digest, view_sha, sim_sha, mesh_count, chains):
    """The five fields, in a fixed order, every chain row satisfying `chain_failures`."""
    ordered = {}
    for name in sorted(chains):
        rungs = chains[name]["rungs"]
        if not rungs:
            continue
        row = {
            "radius_m": chains[name]["radius_m"],
            "rungs": [{"mesh": rung["mesh"], "deviation_mm": rung["deviation_mm"]}
                      for rung in rungs],
        }
        failures = chain_failures(name, row)
        if failures:
            raise TrioError("; ".join(failures))
        ordered[name] = row
    return {
        "blend_digest": blend_digest,
        "view_glb_sha": view_sha,
        "sim_glb_sha": sim_sha,
        "mesh_count": mesh_count,
        "chains": ordered,
    }


def certificate_bytes(cert):
    """The certificate's one serialization. `allow_nan=False` refuses to WRITE a bare `NaN` or
    `Infinity` token, which is JavaScript and not JSON — a reader that rejects it would be right,
    and one that accepts it would read a distance nothing can compare."""
    return (json.dumps(cert, indent=2, sort_keys=False, allow_nan=False) + "\n").encode()


def coherence(cert, view_blob, sim_blob, blend_digest=None, source_mesh_count=None):
    """Every claim the certificate makes about the bytes beside it, as a list of named failures.

    HASH-LOUD, and that is what makes the staged publish safe: binaries land first and the
    certificate last, so an interrupted publish leaves a certificate whose `view_glb_sha` names
    bytes that are no longer there, and this says so.

    AND THE CERTIFICATE'S OWN BODY, which the hashes do not reach: they cover the two BINARIES, so
    an edited `deviation_mm`, a ladder that descends, a radius that is not a number, or a chain key
    naming a primitive the scene does not hold all read as a certified trio. What is re-applied here
    is every CONSTRUCTIVE law — shape, ordering, key form, coverage — and none of the measurements:
    re-deriving a deviation needs the search, which is the build's job and not a verifier's.
    """
    failures = []
    missing = [field for field in CERTIFICATE_FIELDS if field not in cert]
    extra = [field for field in cert if field not in CERTIFICATE_FIELDS]
    if missing:
        failures.append(f"the certificate has no {', '.join(missing)}")
    if extra:
        failures.append(f"the certificate carries {', '.join(extra)}, which is not one of its five "
                        f"fields")
    if missing:
        return failures
    for field, blob, what in (("view_glb_sha", view_blob, "view"),
                              ("sim_glb_sha", sim_blob, "sim")):
        actual = sha256_bytes(blob)
        if cert[field] != actual:
            failures.append(
                f"{field} is {cert[field]!r} and the {what} artifact hashes to {actual!r} — the "
                f"trio is incoherent, which is what an interrupted publish looks like"
            )
    if blend_digest is not None and cert["blend_digest"] != blend_digest:
        failures.append(
            f"blend_digest is {cert['blend_digest']!r} and this source and configuration digest to "
            f"{blend_digest!r} — the trio is stale"
        )
    if source_mesh_count is not None and cert["mesh_count"] != source_mesh_count:
        failures.append(
            f"mesh_count is {cert['mesh_count']} and the source produces {source_mesh_count} mesh"
            f"(es) — the certificate does not cover this model"
        )
    try:
        view_js, _ = glb_ktx2.parse_glb(view_blob, "<view>")
    except SystemExit as error:
        failures.append(f"the view artifact cannot be read: {error}")
        return failures
    if not isinstance(cert["mesh_count"], int) or isinstance(cert["mesh_count"], bool) \
            or not 0 <= cert["mesh_count"] <= len(view_js.get("meshes", [])):
        failures.append(
            f"mesh_count is {cert['mesh_count']!r} and the view artifact holds "
            f"{len(view_js.get('meshes', []))} mesh(es)"
        )
        return failures
    if not isinstance(cert["chains"], dict):
        failures.append("chains is not an object")
        return failures

    # THE CERTIFICATE'S OWN BODY. Its two hashes cover the BINARIES; nothing covered the file they
    # sit in, so an edited deviation, a descending ladder or a chain key naming no primitive read
    # as a certified trio. Re-deriving a MEASUREMENT needs the search and belongs to the build;
    # shape, ordering and coverage are free and are asked here.
    for name in sorted(cert["chains"]):
        failures.extend(chain_failures(name, cert["chains"][name]))

    source = {mesh.get("name"): mesh for mesh in view_js.get("meshes", [])[:cert["mesh_count"]]}
    for name in sorted(cert["chains"]):
        mesh_name, _, index = name.rpartition("#")
        if not mesh_name or not index.isdigit():
            failures.append(
                f"chain key {name!r} is not `<meshName>#<primitiveIndex>`, which is the only form "
                f"a consumer can resolve against the scene"
            )
        elif mesh_name not in source:
            failures.append(
                f"chain key {name!r} names mesh {mesh_name!r}, which is not one of the "
                f"{cert['mesh_count']} the certificate declares are the source's"
            )
        elif int(index) >= len(source[mesh_name]["primitives"]):
            failures.append(
                f"chain key {name!r} names primitive {index} of a mesh that has "
                f"{len(source[mesh_name]['primitives'])}"
            )

    # COVERAGE, BOTH WAYS. A rung record no chain names is geometry that ships and is never
    # selected; a chain naming a record the file does not hold is a ladder with a hole in it.
    # Several chains may name ONE record — that is the source-geometry dedup ADR 0035 asks for.
    held = {mesh.get("name") for mesh in view_js.get("meshes", [])[cert["mesh_count"]:]}
    named = {rung["mesh"] for chain in cert["chains"].values()
             for rung in chain.get("rungs", []) if isinstance(rung, dict) and "mesh" in rung}
    for missing_name in sorted(named - held):
        failures.append(
            f"a chain names rung mesh {missing_name!r}, which the view artifact does not hold"
        )
    for orphan in sorted(name for name in held - named if name is not None):
        failures.append(
            f"the view artifact holds rung mesh {orphan!r}, which no chain names — it ships and "
            f"nothing can select it"
        )
    for node in view_js.get("nodes", []):
        if node.get("mesh", 0) >= cert["mesh_count"]:
            failures.append(
                f"node {node.get('name')!r} references mesh {node['mesh']}, at or past the "
                f"{cert['mesh_count']} the certificate declares are the source's"
            )
    return failures


# ── what a certify miss means ────────────────────────────────────────────────────────────────────

#: The three verdicts a certified rung can carry.
ACCEPT = "accept"
LOST_TO_BRACKET = "certification_bracket"
FAILS_THE_RUN = "fails"


def rung_certification(deviation, structural, target_mm):
    """Which of the two causes a certify miss has. `deviation` is `measure.certified_deviation`'s.

    THE SEARCH AND THE CERTIFICATE ASK DIFFERENT QUESTIONS. The directed search answers a budgeted
    BOOLEAN (ADR 0036 §1); certification BRACKETS the number to `deviation_rel_tol_certify` and
    gates on the UPPER end. So a candidate the search proved under a rung can carry a certified
    upper bound just above it while every point sampled on the shipped bytes is under — the bracket
    did not close, which ADR 0033 §6 prices in triangles and never in honesty. MEASURED on the
    tiger: 7.7909 mm against a 7.7800 mm rung, a 0.14 % bracket, on three rungs of 180.

    A SAMPLED WITNESS OVER THE TARGET is the other cause, and it is not the same fact: the shipped
    bytes deviate, so they are not the surface the search measured, and that fails the run
    (ADR 0033 §10). Any structural failure fails the run for the same reason. The two are separated
    by the LOWER end of the one bracket.

    A NON-FINITE bound fails the run: `NaN > target` is False, so an unguarded comparison reads an
    unanswerable measurement as a pass.

    It lives here, away from `bpy`, so BOTH sides of the boundary can be driven by a test.
    """
    if structural:
        return FAILS_THE_RUN
    lower, upper = deviation.get("mm"), deviation.get("mm_upper")
    if not _finite(lower) or not _finite(upper) or not _finite(target_mm):
        return FAILS_THE_RUN
    if lower > target_mm:
        return FAILS_THE_RUN
    if upper > target_mm:
        return LOST_TO_BRACKET
    return ACCEPT


# ── the staged publish ───────────────────────────────────────────────────────────────────────────

def paths(glb):
    """The trio's three paths, derived from the tracked model's own."""
    stem = glb[:-len(".glb")] if glb.endswith(".glb") else glb
    return glb, stem + SIM_SUFFIX, stem + CERTIFICATE_SUFFIX


def _land(path, blob):
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


def publish(glb, view_blob, sim_blob, cert, after_binaries=None):
    """The trio, binaries first and the certificate last. Returns the three paths.

    `after_binaries` is called once both binaries have landed and before the certificate does — the
    seam an interruption test opens, and the only reason this parameter exists.
    """
    view_path, sim_path, cert_path = paths(glb)
    _land(view_path, view_blob)
    _land(sim_path, sim_blob)
    if after_binaries is not None:
        after_binaries()
    _land(cert_path, certificate_bytes(cert))
    return view_path, sim_path, cert_path
