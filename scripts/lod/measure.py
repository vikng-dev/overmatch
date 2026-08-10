"""Every measurement the ladder gates on — taken on decoded GLB bytes, never on a Blender datablock.

CERTIFICATION ORDER IS SACRED (ADR 0033 §6): generate -> cleanup -> export -> DECODE THE SHIPPED
GLB -> measure. A number taken off the mesh that went INTO the exporter certifies the exporter's
input, not the asset; float32 quantisation, vertex splitting, index re-ordering and axis conversion
all happen after it. So the only Blender datablock this module reads is the SOURCE (which is the
artist's mesh by definition), and everything it is compared against arrives through `from_glb`.

WHY THE WORST CASE IS PROVEN AND NOT SAMPLED. `certified_deviation` is a branch-and-bound over
sub-triangles that exploits d(p) = distance(p, other surface) being 1-Lipschitz: over a patch S with
corners v0,v1,v2,

    max_{p in S} d(p) <= min_k ( d(v_k) + max_j |v_k - v_j| )

— the covering-radius bound. The queue expands the largest upper bound first and stops when the best
bound is within `tol` of the best sample seen, which BRACKETS the true worst case. The caller gates
on the UPPER end, so a bracket that failed to close costs triangles and never honesty. Sampling, at
any density anyone can afford, proves nothing about the spike that pops.

WHAT DEVIATION CANNOT SEE, and therefore what else lives here: a vanished component (near-zero
Hausdorff), a needle triangle whose interpolated normal is noise, a duplicated face, a NaN, a
flipped winding, and a UV-degenerate face whose tangent bevy will default. Those are counted, not
estimated.
"""

import json
import math
import struct

import numpy as np

# glTF componentType enum -> numpy dtype. Covers indices and vertex attributes alike.
_COMPONENT = {5120: np.int8, 5121: np.uint8, 5122: np.int16, 5123: np.uint16,
              5125: np.uint32, 5126: np.float32}
_NCOMP = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


class Refusal(Exception):
    """A named, loud refusal: the asset is outside what this pipeline certifies (ADR 0033 §10)."""

    def __init__(self, reason, detail):
        super().__init__(f"{reason}: {detail}")
        self.reason = reason
        self.detail = detail


# ── decoding the shipped bytes ───────────────────────────────────────────────────────────────────

def glb_chunks(path):
    """The JSON dict and the BIN blob of a glb. Stdlib + a struct unpack, no importer involved."""
    with open(path, "rb") as handle:
        return glb_chunks_from_bytes(handle.read(), path)


def glb_chunks_from_bytes(blob, path="<bytes>"):
    magic, _version, length = struct.unpack_from("<4sII", blob, 0)
    if magic != b"glTF":
        raise Refusal("not-a-glb", path)
    offset, gltf, binary = 12, None, None
    while offset < min(length, len(blob)):
        chunk_length, chunk_type = struct.unpack_from("<II", blob, offset)
        payload = blob[offset + 8: offset + 8 + chunk_length]
        if chunk_type == 0x4E4F534A:
            gltf = json.loads(payload)
        elif chunk_type == 0x004E4942:
            binary = payload
        offset += 8 + chunk_length + (-chunk_length % 4)
    if gltf is None:
        raise Refusal("no-json-chunk", path)
    return gltf, binary


def _accessor(gltf, binary, index):
    """One accessor as an (count, ncomp) array, honouring byteStride. Sparse accessors refused."""
    accessor = gltf["accessors"][index]
    if "sparse" in accessor:
        raise Refusal("sparse-accessor", f"accessor {index}")
    ncomp = _NCOMP[accessor["type"]]
    dtype = _COMPONENT[accessor["componentType"]]
    count = accessor["count"]
    if "bufferView" not in accessor:
        return np.zeros((count, ncomp), dtype=dtype)
    view = gltf["bufferViews"][accessor["bufferView"]]
    base = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
    stride = view.get("byteStride") or np.dtype(dtype).itemsize * ncomp
    raw = np.frombuffer(binary, dtype=np.uint8, count=stride * count, offset=base)
    raw = raw.reshape(count, stride)[:, : np.dtype(dtype).itemsize * ncomp]
    return np.ascontiguousarray(raw).view(dtype).reshape(count, ncomp)


#: glTF is Y-up and right-handed; Blender is Z-up. The exporter rotates -90 deg about X on the way
#: out, so a shipped position (x, y, z) is the Blender position (x, -z, y). Deviation is
#: rotation-invariant, but the source surface lives in Blender coordinates and the two must be
#: compared in ONE frame — measuring across the conversion would silently compare a mesh with a
#: rotated copy of itself and report a hull-sized "deviation".
def gltf_to_blender(points):
    out = np.empty_like(points)
    out[:, 0] = points[:, 0]
    out[:, 1] = -points[:, 2]
    out[:, 2] = points[:, 1]
    return out


def primitive_of(gltf, node_name=None):
    """The single mesh primitive a chain level (or a named node) must be. Refuses anything else."""
    meshes = gltf.get("meshes", [])
    if node_name is None:
        if len(meshes) != 1:
            raise Refusal("multi-mesh-glb", f"{len(meshes)} meshes; a chain level holds exactly 1")
        mesh = meshes[0]
    else:
        nodes = [n for n in gltf.get("nodes", []) if n.get("name") == node_name]
        if len(nodes) != 1 or nodes[0].get("mesh") is None:
            raise Refusal("node-not-found", f"{node_name!r} is not a single mesh node")
        mesh = meshes[nodes[0]["mesh"]]
    primitives = mesh["primitives"]
    if len(primitives) != 1:
        raise Refusal(
            "multi-primitive-mesh",
            f"mesh {mesh.get('name')!r} splits into {len(primitives)} primitives; the chain loader "
            f"reads primitive 0 only, so the rest would never be drawn",
        )
    return primitives[0]


def from_glb(path, node_name=None, name=None):
    """A `Surface` built from the bytes on disk. THE decode every gate measures through."""
    with open(path, "rb") as handle:
        return surface_from_bytes(handle.read(), node_name, name or path)


def surface_from_bytes(blob, node_name=None, name=None):
    """The same decode, from bytes already in hand.

    Split out so the VERIFIER can re-derive a level's record from the shipped bytes without a file
    path — it resolves LFS pointers to their objects itself. Verification that compares a recorded
    count against another recorded count proves the manifest is self-consistent and nothing about
    the asset; this is the entry point that makes it about the asset.
    """
    gltf, binary = glb_chunks_from_bytes(blob, name or "<bytes>")
    primitive = primitive_of(gltf, node_name)
    attributes = primitive["attributes"]
    if primitive.get("mode", 4) != 4:
        raise Refusal("not-triangles", f"{name} primitive mode {primitive.get('mode')}")
    if "indices" not in primitive:
        raise Refusal("non-indexed-primitive", name)
    for required in ("POSITION", "NORMAL", "TEXCOORD_0"):
        if required not in attributes:
            raise Refusal("missing-attribute", f"{name} has no {required}")
    for banned, reason in (("JOINTS_0", "skinned-mesh"), ("WEIGHTS_0", "skinned-mesh")):
        if banned in attributes:
            raise Refusal(reason, f"{name} carries {banned}")
    if primitive.get("targets"):
        raise Refusal("morph-mesh", f"{name} carries {len(primitive['targets'])} morph targets")

    indices = _accessor(gltf, binary, primitive["indices"]).astype(np.int64).reshape(-1, 3)
    positions = gltf_to_blender(_accessor(gltf, binary, attributes["POSITION"]).astype(np.float64))
    normals = gltf_to_blender(_accessor(gltf, binary, attributes["NORMAL"]).astype(np.float64))
    uvs = _accessor(gltf, binary, attributes["TEXCOORD_0"]).astype(np.float64)
    # TANGENT is vec4: xyz is the tangent, w the bitangent sign. Decoded when present because the
    # loader USES it verbatim rather than generating one — so it is part of what ships, and the
    # gate measures it rather than a proxy for it.
    tangents = None
    if "TANGENT" in attributes:
        raw = _accessor(gltf, binary, attributes["TANGENT"]).astype(np.float64)
        tangents = np.concatenate(
            [gltf_to_blender(raw[:, :3]), raw[:, 3:4]], axis=1
        )
    return Surface(positions, indices, normals[indices], uvs[indices], name, tangents)


def from_bpy_mesh(mesh, matrix=None, name="source"):
    """A `Surface` from an evaluated Blender mesh — used for the SOURCE only.

    `matrix` bakes world rotation and scale (never translation) into the coordinates, so every
    distance downstream is a true world distance in metres.
    """
    import bpy  # only importable inside Blender; this path is Blender-only

    work = mesh.copy()
    if matrix is not None:
        work.transform(matrix)
    work.calc_loop_triangles()
    triangles = work.loop_triangles
    verts = np.empty((len(work.vertices), 3), dtype=np.float64)
    flat = np.empty(len(work.vertices) * 3, dtype=np.float32)
    work.vertices.foreach_get("co", flat)
    verts[:] = flat.reshape(-1, 3)

    tri_v = np.array([tuple(t.vertices) for t in triangles], dtype=np.int64)
    tri_loops = np.array([tuple(t.loops) for t in triangles], dtype=np.int64)

    corner = np.zeros(len(work.loops) * 3, dtype=np.float32)
    work.corner_normals.foreach_get("vector", corner)
    corner_n = corner.reshape(-1, 3).astype(np.float64)[tri_loops]

    layer = work.uv_layers.active
    if layer is None:
        raise Refusal("no-uv-layer", name)
    flat_uv = np.zeros(len(work.loops) * 2, dtype=np.float32)
    layer.uv.foreach_get("vector", flat_uv)
    corner_uv = flat_uv.reshape(-1, 2).astype(np.float64)[tri_loops]

    bpy.data.meshes.remove(work)
    return Surface(verts, tri_v, corner_n, corner_uv, name)


# ── the surface ──────────────────────────────────────────────────────────────────────────────────

class Surface:
    """Positions, triangles, per-corner normals and UVs, plus every derived quality counter."""

    def __init__(self, verts, tri_v, corner_n, corner_uv, name, tangents=None):
        self.name = name
        #: Per-vertex vec4 tangents AS SHIPPED, or None when the file carries none (in which
        #: case the loader generates them and this pipeline can only gate a proxy).
        self.tangents = None if tangents is None else np.ascontiguousarray(tangents)
        self.verts = np.ascontiguousarray(verts, dtype=np.float64)
        self.tri_v = np.ascontiguousarray(tri_v, dtype=np.int64)
        self.corner_n = np.ascontiguousarray(corner_n, dtype=np.float64)
        self.corner_uv = np.ascontiguousarray(corner_uv, dtype=np.float64)

        self.p0 = self.verts[self.tri_v[:, 0]]
        self.p1 = self.verts[self.tri_v[:, 1]]
        self.p2 = self.verts[self.tri_v[:, 2]]
        cross = np.cross(self.p1 - self.p0, self.p2 - self.p0)
        self.tri_area = 0.5 * np.linalg.norm(cross, axis=1)
        self.tri_count = int(len(self.tri_v))
        self.vert_count = int(len(self.verts))

        uv0, uv1, uv2 = self.corner_uv[:, 0], self.corner_uv[:, 1], self.corner_uv[:, 2]
        a, b = uv1 - uv0, uv2 - uv0
        self.uv_area = 0.5 * np.abs(a[:, 0] * b[:, 1] - a[:, 1] * b[:, 0])

        lo, hi = self.verts.min(axis=0), self.verts.max(axis=0)
        self.bbox_min, self.bbox_max = lo, hi
        self.diagonal = float(np.linalg.norm(hi - lo))
        #: Half the AABB diagonal. A SHAPE measure — use it for scale-relative thresholds, never
        #: for the runtime slack: it bounds distance from the box CENTRE, and bevy's abrupt
        #: `VisibilityRange` measures to the entity ORIGIN, which is somewhere else entirely.
        self.radius = 0.5 * self.diagonal
        #: Farthest vertex from the object ORIGIN — the honest slack for origin-anchored selection.
        #: On the shipped Link this is 0.400124 m against a half-diagonal of 0.384004 m, so the old
        #: slack switched every level 16 mm early.
        self.origin_radius = (
            float(np.linalg.norm(self.verts, axis=1).max()) if len(self.verts) else 0.0
        )
        self._bvh = None

    # -- geometry helpers ------------------------------------------------------------------------
    @property
    def bvh(self):
        if self._bvh is None:
            from mathutils.bvhtree import BVHTree

            self._bvh = BVHTree.FromPolygons(
                [tuple(v) for v in self.verts],
                [tuple(int(i) for i in t) for t in self.tri_v],
                all_triangles=True,
            )
        return self._bvh

    def digest(self):
        """A stable hash of EVERYTHING THAT SHIPS — the candidate identity, and a plateau's key.

        It hashed positions and SORTED triangle indices, which threw away exactly the attributes the
        gates measure: sorting each triangle discards its winding, and normals and UVs were not in
        it at all. Two meshes differing only in winding, or only in a collapsed UV, hashed
        identically — so one of them was silently dropped as a duplicate candidate, and it could be
        the one with a defect (or the only one that met a rung).

        What goes in: positions, per-corner normals and per-corner UVs AT FULL PRECISION, and each
        triangle's corner ORDER. What stays out: only the arbitrary choices a serializer makes —
        which corner is listed first (each triangle is rotated to start at its lowest vertex index,
        which preserves winding while ignoring the rotation) and the order triangles appear in
        (rows are sorted).

        NOTHING IS QUANTISED ANY MORE, and that is the point. Rounding UVs to 1e-7 was coarser than
        `uv_area_eps = 1e-12`, the finest epsilon a gate consumes — so two candidates with UV areas
        of 5e-13 and 1.5e-12, which differ in `tangent_default_faces`, hashed identically and one of
        them was discarded as a duplicate. A key used to decide which candidates exist must be at
        least as sharp as every test applied to them; the cheapest way to guarantee that against
        gates not yet written is to keep the bits.
        """
        import hashlib

        # Rotate each triangle to start at its smallest index: winding-preserving, listing-agnostic.
        starts = np.argmin(self.tri_v, axis=1)
        columns = (starts[:, None] + np.arange(3)[None, :]) % 3
        rows = np.take_along_axis(self.tri_v, columns, axis=1)
        normals = np.take_along_axis(
            self.corner_n, columns[:, :, None].repeat(3, axis=2), axis=1
        )
        uvs = np.take_along_axis(self.corner_uv, columns[:, :, None].repeat(2, axis=2), axis=1)

        # Sort by the triangle rows only; the attributes ride along so equal geometry with
        # different attributes still separates.
        order = np.lexsort(rows.T[::-1])
        h = hashlib.sha256()
        h.update(np.ascontiguousarray(self.verts).tobytes())
        h.update(np.ascontiguousarray(rows[order]).tobytes())
        h.update(np.ascontiguousarray(normals[order]).tobytes())
        h.update(np.ascontiguousarray(uvs[order]).tobytes())
        return h.hexdigest()

    def welded_digest(self, tol=1e-9):
        """A hash of the WELDED surface: coincident positions merged, triangles as a set.

        Invariant to how a serializer splits corners — the same shoe is 815 vertices in Blender and
        4 747 in the shipped glb, and this gives both the same digest — and sensitive to any change
        in where the surface is or how it is joined. That combination is what lets generation record
        a geometry fingerprint the VERIFIER can re-derive from decoded bytes without Blender.

        Not a substitute for `digest`, which keys candidates and must separate meshes differing only
        in an attribute. This one is about geometric identity across a re-encode.
        """
        import hashlib

        clean = np.nan_to_num(self.verts, nan=0.0, posinf=0.0, neginf=0.0)
        keys = np.round(clean / tol).astype(np.int64)
        unique, inverse = np.unique(keys, axis=0, return_inverse=True)
        triangles = np.unique(np.sort(inverse.reshape(-1)[self.tri_v], axis=1), axis=0)
        h = hashlib.sha256()
        h.update(unique.tobytes())
        h.update(triangles.tobytes())
        return h.hexdigest()

    def welded(self, tol=1e-9):
        """Vertex indices remapped so coincident positions share one index (glTF splits corners).

        Non-finite coordinates are folded to zero for the weld only. They are a defect in their own
        right and `validity` counts them; letting a NaN through to the integer cast here would raise
        a warning and make the topology counters meaningless on top of the real failure.
        """
        clean = np.nan_to_num(self.verts, nan=0.0, posinf=0.0, neginf=0.0)
        keys = np.round(clean / tol).astype(np.int64)
        _, inverse = np.unique(keys, axis=0, return_inverse=True)
        return inverse.reshape(-1)

    def components(self):
        """Connected components over welded positions. A vanished part shows here and nowhere else."""
        weld = self.welded()
        n = int(weld.max()) + 1 if len(weld) else 0
        parent = np.arange(n)

        def find(x):
            while parent[x] != x:
                parent[x] = parent[parent[x]]
                x = parent[x]
            return x

        for tri in weld[self.tri_v]:
            a = find(int(tri[0]))
            for other in tri[1:]:
                b = find(int(other))
                if a != b:
                    parent[b] = a
        used = np.unique(weld[self.tri_v])
        return len({find(int(v)) for v in used})

    # -- the validity gates ----------------------------------------------------------------------
    def validity(self, gates):
        """Every structural check, as plain counts. The caller compares them with the thresholds."""
        weld = self.welded()
        faces = np.sort(weld[self.tri_v], axis=1)
        _, counts = np.unique(faces, axis=0, return_counts=True)
        duplicates = int((counts - 1).sum())

        nonfinite = int(
            (~np.isfinite(self.verts)).sum()
            + (~np.isfinite(self.corner_n)).sum()
            + (~np.isfinite(self.corner_uv)).sum()
        )

        # Orientation: every edge shared by exactly two faces must be traversed once in each
        # direction. A flipped winding shows as the same directed edge appearing twice.
        directed = np.concatenate([
            np.stack([weld[self.tri_v[:, 0]], weld[self.tri_v[:, 1]]], axis=1),
            np.stack([weld[self.tri_v[:, 1]], weld[self.tri_v[:, 2]]], axis=1),
            np.stack([weld[self.tri_v[:, 2]], weld[self.tri_v[:, 0]]], axis=1),
        ])
        undirected = np.sort(directed, axis=1)
        _, inverse, edge_counts = np.unique(
            undirected, axis=0, return_inverse=True, return_counts=True
        )
        inverse = inverse.reshape(-1)
        interior = edge_counts[inverse] == 2
        forward = directed[:, 0] < directed[:, 1]
        order = np.argsort(inverse[interior], kind="stable")
        pairs = forward[interior][order].reshape(-1, 2)
        flips = int((pairs[:, 0] == pairs[:, 1]).sum())

        bad_uv = self.uv_area < gates["uv_area_eps"]
        touched = np.zeros(self.vert_count, dtype=np.int64)
        bad_touched = np.zeros(self.vert_count, dtype=np.int64)
        np.add.at(touched, self.tri_v.reshape(-1), 1)
        np.add.at(bad_touched, self.tri_v[bad_uv].reshape(-1), 1)

        # THE TANGENTS THAT SHIP. `tangent_default_faces` above is a necessary condition on the
        # UVs; this is the thing itself. mikktspace declines a corner for reasons a UV-area test
        # cannot see — measured on this corpus, three levels had zero UV-degenerate faces and one
        # give-up tangent each.
        if self.tangents is None:
            # Absent is not clean. The caller decides whether a level is allowed to lack them;
            # this only reports what is there, and zero baked tangents is a fact, not a pass.
            baked, degenerate, worst = 0, 0, 0.0
        else:
            lengths = np.linalg.norm(self.tangents[:, :3], axis=1)
            signs = np.abs(self.tangents[:, 3])
            bad = (
                (lengths < gates["tangent_min_length"])
                | (~np.isfinite(self.tangents).all(axis=1))
                | (np.abs(signs - 1.0) > 1e-3)
            )
            baked, degenerate = int(len(lengths)), int(bad.sum())
            worst = float(lengths.min())

        return {
            "tris": self.tri_count,
            "verts": self.vert_count,
            "baked_tangents": baked,
            "degenerate_tangents": degenerate,
            "min_tangent_length": round(worst, 9),
            "components": self.components(),
            "duplicate_faces": duplicates,
            "nonfinite_attrs": nonfinite,
            "orientation_flips": flips,
            "boundary_edges": int((edge_counts == 1).sum()),
            "nonmanifold_edges": int((edge_counts > 2).sum()),
            "min_tri_area_mm2": float(self.tri_area.min() * 1e6) if self.tri_count else 0.0,
            "tangent_default_faces": int(bad_uv.sum()),
            "tangent_default_verts": int(((touched > 0) & (touched == bad_touched)).sum()),
            "bbox_mm": [round(float(v) * 1000.0, 4) for v in (self.bbox_max - self.bbox_min)],
            "radius_m": round(self.radius, 6),
            "origin_radius_m": round(self.origin_radius, 6),
        }


# ── the certified deviation ──────────────────────────────────────────────────────────────────────

def _patch_bound(corners, distances):
    """The covering-radius bound over one sub-triangle: min_k ( d(v_k) + max_j |v_k - v_j| ).

    Valid because d is 1-Lipschitz and |p - v_k| is convex, so its maximum over the triangle is
    attained at another CORNER — a triangle being the convex hull of its three vertices.
    """
    return min(
        distances[k] + max(float(np.linalg.norm(corners[k] - corners[j])) for j in range(3))
        for k in range(3)
    )


def branch_and_bound(seeds, distance_at, tol, max_nodes, target=None, rel_tol=0.0):
    """Bracket max over the seed patches of a 1-Lipschitz `distance_at`. Returns (lower, upper).

    `distance_at(key, point) -> float` is injected rather than reached for, so the bound itself can
    be regression-tested against a synthetic field with a KNOWN interior maximum, without Blender
    and without a BVH. That is not a convenience: the previous version of this function returned an
    "upper bound" that was not one, and no test could have caught it because the only way to call
    it was through a mesh whose true worst case nobody knew independently.

    THE PRUNED PATCHES ARE PART OF THE ANSWER. A child whose bound is already within tolerance is
    not queued — expanding it cannot improve the lower bound enough to matter — but its bound still
    BOUNDS a region of the surface, and that region may hold a maximum above every point sampled so
    far. Dropping it on the floor and then reporting `max(best, live_heap_top)` reports a number
    that is not an upper bound at all. Every pruned bound is folded into `pruned` and into the
    returned upper endpoint, which is what makes the endpoint's name true.

    STOPPING. `tol`/`rel_tol` close the bracket; `target` stops as soon as the caller's actual
    question is decided — upper already under it (accept) or a SAMPLED point already over it
    (reject). Both are sound one-directional facts, which is why a caller may apply them per
    direction of a two-way measurement.
    """
    import heapq

    best = 0.0
    pruned = 0.0
    heap = []
    counter = 0
    for corners, distances in seeds:
        best = max(best, max(distances))
        counter += 1
        heapq.heappush(heap, (-_patch_bound(corners, distances), counter, corners, distances))

    nodes = 0
    while heap:
        live = -heap[0][0]
        slack = max(tol, rel_tol * best)
        if live <= best + slack or nodes >= max_nodes:
            break
        if target is not None and (live <= target or best > target):
            break
        _, _, corners, distances = heapq.heappop(heap)
        nodes += 1
        a, b, c = corners
        ab, bc, ca = 0.5 * (a + b), 0.5 * (b + c), 0.5 * (c + a)
        d_ab = distance_at(("m", tuple(ab)), ab)
        d_bc = distance_at(("m", tuple(bc)), bc)
        d_ca = distance_at(("m", tuple(ca)), ca)
        best = max(best, d_ab, d_bc, d_ca)
        slack = max(tol, rel_tol * best)
        for patch, patch_d in (
            ((a, ab, ca), (distances[0], d_ab, d_ca)),
            ((ab, b, bc), (d_ab, distances[1], d_bc)),
            ((ca, bc, c), (d_ca, d_bc, distances[2])),
            ((ab, bc, ca), (d_ab, d_bc, d_ca)),
        ):
            sub = _patch_bound(patch, patch_d)
            if sub > best + slack:
                counter += 1
                heapq.heappush(heap, (-sub, counter, patch, patch_d))
            else:
                pruned = max(pruned, sub)

    live = -heap[0][0] if heap else 0.0
    return best, max(best, pruned, live)


def _one_way(src, dst, tol, max_nodes, target=None, rel_tol=0.0):
    """Certified max over `src`'s surface of the distance to `dst`. Returns (lower, upper) metres."""
    from mathutils import Vector

    find = dst.bvh.find_nearest
    cache = {}

    def distance_at(key, point):
        value = cache.get(key)
        if value is None:
            hit = find(Vector(point))
            value = hit[3] if hit[0] is not None else 0.0
            cache[key] = value
        return value

    seeds = []
    for t in range(src.tri_count):
        corners = (src.p0[t], src.p1[t], src.p2[t])
        ids = src.tri_v[t]
        seeds.append(
            (corners, tuple(distance_at(int(ids[k]), corners[k]) for k in range(3)))
        )
    return branch_and_bound(seeds, distance_at, tol, max_nodes, target, rel_tol)


class EnumerationError(Exception):
    """The decimator's realizable outputs could not be enumerated completely. Never degrade."""


#: Hard cap on bisection steps. The ordinary case exhausts its bracket in ~60; the worst case
#: walks `high` down through the denormals and needs ~1075. This is a runaway guard, not the
#: stopping rule — the stopping rule is that the bracket holds no representable ratio.
BISECTION_MAX_STEPS = 1100


def bisect_to_budget(evaluate, budget, max_steps=BISECTION_MAX_STEPS):
    """The GREATEST count `evaluate` can reach at or below `budget`. Returns (count, ratio).

    `evaluate(ratio) -> count` must be monotone non-decreasing — for the Blender decimate modifier
    it is, since a larger ratio keeps more triangles. Given only that, this is exact: it runs until
    the bracket is EXHAUSTED at f64 resolution (the midpoint stops being strictly between the ends),
    so no plateau can hide inside the final interval however narrow it is.

    IT USED TO RUN A FIXED 28 HALVINGS, and that made the claim conditional on a minimum plateau
    width nobody had established. Executed counterexample: a staircase realizing 100, 999 and 2000
    where the 999 plateau is 1e-10 wide returns 100 at a budget of 1000 — 28 halvings cannot see it,
    32 can. Rather than assume Blender's ratio quantisation is coarse enough (it may well be; nobody
    measured it), the loop now exhausts the bracket and the precondition disappears.

    THIS IS WHERE THE ENUMERATION'S CONTRACT IS ESTABLISHED, and it is deliberately a pure function
    of an injected `evaluate` so it can be checked against synthetic staircases whose answer is
    known independently — see `test_refusals.BisectionTests`. That matters because the enumeration
    built on top cannot establish it: an enumerator can only ask the oracle, and an oracle that lies
    consistently answers every question consistently. The honesty has to come from here.
    """
    # BOTH ENDPOINTS ARE EVALUATED, and neither is reachable by halving an open interval.
    #
    # Ratio 0 is the floor: by monotonicity nothing below it exists, so a budget under it is
    # unreachable and there is nothing to bisect. Without it the "everything exceeds the budget"
    # case walks `high` down through a thousand denormals.
    #
    # Ratio 1 is the ceiling, and it was the hole. `(low + high) / 2` never produces 1.0, so an
    # evaluator whose top step exists ONLY at exactly 1.0 was invisible: `evaluate(r) = 2000 if
    # r == 1.0 else 100` returned 100 for a budget of 2000. By monotonicity `evaluate(1.0)` is the
    # largest count there is, so if it fits the budget it IS the answer and no search is needed.
    floor_count = evaluate(0.0)
    if floor_count > budget:
        return None, None
    ceiling_count = evaluate(1.0)
    if ceiling_count <= budget:
        return ceiling_count, 1.0

    low, high, best = 0.0, 1.0, (floor_count, 0.0)
    for _ in range(max_steps):
        middle = (low + high) / 2.0
        if middle <= low or middle >= high:
            return best                 # the bracket holds no representable ratio: exhausted
        count = evaluate(middle)
        if count <= budget:
            best = (count, middle)
            low = middle
        else:
            high = middle
    raise EnumerationError(
        f"the budget bisection did not exhaust its bracket in {max_steps} steps — the evaluator "
        f"is not behaving like a monotone step function"
    )


def enumerate_staircase(floor_tris, ceiling_tris, probe, spot_checks=0, seed=0, max_outputs=None):
    """Every mesh the decimator realizes in [floor, ceiling]. Returns `{key: step_count}`.

    `probe(budget)` returns `(step_count, key)` or `None`:

      * `step_count` is what the DECIMATOR produced, and it is the domain the walk steps in.
      * `key` identifies the mesh that would actually SHIP — a GEOMETRY HASH, not a triangle count.
        Two different cleaned meshes can carry the same count, and keying by count silently threw
        one of them away, possibly the only one that met a rung.

    THE TWO ARE DIFFERENT THINGS AND MUST NOT BE INTERCHANGED. An earlier version stepped on the raw
    count, cached candidates under the cleaned count, and looked candidates up by the raw one:
    correct only until cleanup dissolved a face, then a `KeyError` or a lost candidate.

    ── WHAT THIS FUNCTION PROVES, AND WHAT IT TAKES ON TRUST ────────────────────────────────────

    IT IS EXHAUSTIVE **GIVEN** AN ORACLE THAT MEETS ITS CONTRACT — `probe(B)` returning the greatest
    realizable count at or below `B`. Given that, walking `budget <- step_count - 1` from the ceiling
    visits every realizable output exactly once, because nothing realizable lies in the interval the
    jump skips.

    IT CANNOT VERIFY THAT CONTRACT, and does not claim to. An enumeration interrogating its own
    oracle is circular: `lambda b: (min(b, 5), min(b, 5))` answers every question this function can
    ask, perfectly consistently, while concealing everything above 5. No number of probes finds
    that, because the probes are addressed to the liar. `test_refusals` pins this as a KNOWN,
    UNDETECTABLE case so nobody re-derives a stronger claim from a green suite.

    The contract is established instead where it is a mathematical property rather than a
    conversation: `bisect_to_budget` runs to convergence over a monotone evaluator, and is proven
    on synthetic staircases whose answers are known by construction.

    THE SPOT CHECKS ARE REGRESSION DEFENCE, NOT PROOF. They catch an oracle that becomes internally
    INCONSISTENT — the 1 %-early-stop class, where `probe(r)` stops agreeing with `r` for a value
    the walk itself found realizable, and the `f(B) = B - 1` class. That is the failure this
    pipeline actually shipped once, so it is worth catching cheaply. It is not the same as proof.
    """
    rng = np.random.default_rng(seed)
    by_key = {}
    incumbents = {}
    skipped = []
    budget = ceiling_tris
    while budget >= floor_tris:
        answer = probe(budget)
        if answer is None:
            break
        step_count, key = answer
        if step_count > budget:
            raise EnumerationError(
                f"the decimation oracle returned {step_count} for a budget of {budget} — it must "
                f"return the greatest realizable count AT OR BELOW the budget"
            )
        by_key.setdefault(key, step_count)
        incumbents.setdefault(step_count, step_count)
        if max_outputs is not None and len(by_key) > max_outputs:
            raise EnumerationError(
                f"more than {max_outputs} realizable outputs below {ceiling_tris} triangles; "
                f"refusing to hold them all. Raise "
                f"config.SEARCH_LIMITS['max_enumerated_outputs'] deliberately, having thought "
                f"about the memory and the certification time it buys"
            )
        if step_count + 1 <= budget:
            skipped.append((step_count + 1, budget, step_count))
        if step_count <= floor_tris:
            break
        budget = step_count - 1

    _check_oracle_consistency(probe, sorted(incumbents), skipped, spot_checks, rng)
    return by_key


def _check_oracle_consistency(probe, realizable, skipped, spot_checks, rng):
    """Catch an oracle that contradicts ITSELF. Not a proof of honesty — see `enumerate_staircase`.

    Both checks below are implications of the contract, so a violation is decisive: the oracle is
    broken. Neither can detect an oracle that is wrong CONSISTENTLY, because both ask that same
    oracle. They exist because the two failures this pipeline actually had — a bisection stopping
    within 1 % of its budget, and the `B - 1` shape — are both self-contradictory, and cheap to
    catch on real geometry every run.
    """
    for value in realizable:
        answer = probe(value)
        if answer is None or answer[0] != value:
            got = "nothing" if answer is None else answer[0]
            raise EnumerationError(
                f"the oracle realizes {value} triangles, but asked for a budget of exactly {value} "
                f"it returns {got} — it contradicts itself, so the walk's jumps stepped over "
                f"outputs and the candidate set is incomplete"
            )
    for _ in range(spot_checks):
        if not skipped:
            break
        low, high, incumbent = skipped[int(rng.integers(0, len(skipped)))]
        budget = int(rng.integers(low, high + 1))
        answer = probe(budget)
        got = None if answer is None else answer[0]
        if got != incumbent:
            raise EnumerationError(
                f"budget {budget} realizes {got} triangles, but the walk skipped that interval "
                f"having been told its incumbent was {incumbent} — the enumeration is missing "
                f"whatever lies between them"
            )


def pareto_minimal(outputs, deviation_for, target_mm):
    """The FEWEST-triangle realized output whose certified upper bound clears `target_mm`.

    `outputs` is every candidate the decimator can produce, ASCENDING BY TRIANGLE COUNT and
    identified by an opaque key (a geometry digest — two distinct meshes can share a count);
    `deviation_for(key, target_mm)` returns that candidate's bracket as `{"lo_mm", "up_mm"}`.

    PROVEN MINIMAL BY EXHAUSTION, which is the whole point. The previous search bisected
    feasible/infeasible budgets and walked down until the first failure — assuming the monotonicity
    this pipeline's own doctrine says does not hold, so a lower feasible ISLAND below an infeasible
    step was invisible and the winner was not minimal. Scanning ascending and returning the first
    feasible output means everything smaller has been measured and rejected, with no assumption
    about the shape of the curve at all.

    It lives here, away from `bpy`, so the logic can be tested against a synthetic non-monotone
    feasibility function without Blender.
    """
    for key in outputs:
        entry = deviation_for(key, target_mm)
        if entry is not None and entry["up_mm"] <= target_mm:
            return entry
    return None


def same_surface(a, b, tol=1e-9):
    """Are these two decodes the SAME surface? Returns (verdict, reason).

    L0 is the source (ADR 0033 §1) and that has to be PROVEN, not sampled. Vertex distance plus a
    triangle count does not prove it: the exporter splits corners by (position, normal, uv), so the
    same shoe decodes to 3 888 vertices out of an 815-vertex Blender mesh, and a mesh could match at
    every vertex and still differ in how the vertices are joined — an interior face flipped,
    re-triangulated across the other diagonal, or a hole punched between coincident corners.

    So the comparison is on the CANONICAL TOPOLOGY: weld coincident positions, then compare the
    welded position set and the welded triangle set as sets. That is invariant to the exporter's
    vertex splitting and index ordering, and it is not invariant to anything that changes the
    surface. Positions are compared at `tol` (a nanometre) because the shipped buffer is float32.
    """
    if a.tri_count != b.tri_count:
        return False, f"triangle counts differ: {a.tri_count} against {b.tri_count}"
    keys_a = np.round(a.verts / tol).astype(np.int64)
    keys_b = np.round(b.verts / tol).astype(np.int64)
    uniq_a, inv_a = np.unique(keys_a, axis=0, return_inverse=True)
    uniq_b, inv_b = np.unique(keys_b, axis=0, return_inverse=True)
    if uniq_a.shape != uniq_b.shape or not np.array_equal(uniq_a, uniq_b):
        return False, (
            f"welded position sets differ: {len(uniq_a)} against {len(uniq_b)} distinct positions"
        )
    # `np.unique` returns the welded positions sorted, so equal position sets means the welded
    # INDEX spaces already agree and the triangle sets are directly comparable.
    tris_a = np.unique(np.sort(inv_a.reshape(-1)[a.tri_v], axis=1), axis=0)
    tris_b = np.unique(np.sort(inv_b.reshape(-1)[b.tri_v], axis=1), axis=0)
    if tris_a.shape != tris_b.shape or not np.array_equal(tris_a, tris_b):
        return False, "welded triangle sets differ — same points, joined differently"
    return True, "identical welded topology and positions"


def vertex_deviation(a, b):
    """Two-way max distance from each surface's VERTICES to the other surface, in millimetres.

    The cheap check, and the only honest one for an IDENTITY comparison. Branch-and-bound cannot
    close a bracket on two identical surfaces: the true worst case is zero, so the stop test
    `bound <= best + tol` demands every patch be subdivided until its own covering radius is under
    `tol` — a quarter-million sub-triangles per triangle, for a number everybody already knows. For
    "did these bytes ship the mesh I measured", vertices plus matched triangle counts is the
    statement worth making, and it costs one BVH query per vertex.
    """
    from mathutils import Vector

    worst = 0.0
    for source, target in ((a, b), (b, a)):
        for point in source.verts:
            hit = target.bvh.find_nearest(Vector(point))
            if hit[0] is not None:
                worst = max(worst, hit[3])
    return worst * 1000.0


def certified_deviation(a, b, tol, max_nodes, target_mm=None, rel_tol=0.0):
    """TWO-WAY certified deviation between surfaces, in millimetres.

    One-way misses holes: a level that deleted a boss is arbitrarily close to the source measured
    from the level, and only the source->level direction sees the missing metal. Both directions are
    run and the worst of the two is the deviation; both upper bounds are kept so the caller can gate
    on a proven ceiling.

    IDENTICAL SURFACES SHORT-CIRCUIT. Two meshes with the same geometry digest ARE the same mesh,
    and the branch-and-bound is at its very worst on them: the true answer is zero, so no patch is
    ever small enough to stop on and the search runs to `max_nodes` to say what the digest says in
    microseconds. The search's own ceiling probe is exactly this case.
    """
    if a.digest() == b.digest():
        return {"mm": 0.0, "mm_upper": 0.0, "a_to_b_mm": 0.0, "b_to_a_mm": 0.0,
                "bracket_mm": 0.0, "identical": True}
    target = None if target_mm is None else target_mm / 1000.0
    lo_a, hi_a = _one_way(a, b, tol, max_nodes, target, rel_tol)
    lo_b, hi_b = _one_way(b, a, tol, max_nodes, target, rel_tol)
    return {
        "mm": max(lo_a, lo_b) * 1000.0,
        "mm_upper": max(hi_a, hi_b) * 1000.0,
        "a_to_b_mm": lo_a * 1000.0,
        "b_to_a_mm": lo_b * 1000.0,
        "bracket_mm": (max(hi_a, hi_b) - max(lo_a, lo_b)) * 1000.0,
    }


# ── the diagnostic (never a gate) ────────────────────────────────────────────────────────────────

def normal_angle_diagnostic(source, level, samples, seed=12345):
    """Worst / p99 shading-normal angle between the pair, in degrees. DIAGNOSTIC ONLY (ADR 0033 §7).

    No fixed angle can honestly gate shading: how visible a normal error is depends on roughness,
    the normal map and the lighting, which is exactly what the rendered-difference gate measures
    instead. This number is reported so a regression is legible, never so a build fails on it.

    READ IT WITH ITS COMPANION `backface_corr_frac`. The correspondence is the nearest point, and on
    a shoe with 3 mm walls the nearest point routinely lands on the FAR side of a wall — a surface
    facing the other way, whose normal is ~180 deg off and has nothing to do with any shading error.
    Those samples are excluded from the angle statistics and counted separately, because their
    fraction is itself the signal (it rises as thin features are collapsed away). The angles below
    are therefore maxima over surviving correspondences, not over the whole surface.
    """
    from mathutils import Vector

    rng = np.random.default_rng(seed)
    weights = source.tri_area / source.tri_area.sum()
    picks = rng.choice(source.tri_count, size=samples, p=weights)
    r1, r2 = rng.random(samples), rng.random(samples)
    s = np.sqrt(r1)
    b0, b1, b2 = (1.0 - s)[:, None], (s * (1.0 - r2))[:, None], (s * r2)[:, None]
    points = b0 * source.p0[picks] + b1 * source.p1[picks] + b2 * source.p2[picks]
    normals = (
        b0 * source.corner_n[picks, 0]
        + b1 * source.corner_n[picks, 1]
        + b2 * source.corner_n[picks, 2]
    )
    normals /= np.linalg.norm(normals, axis=1, keepdims=True) + 1e-30

    angles = []
    backfacing = 0
    face_normals = np.cross(level.p1 - level.p0, level.p2 - level.p0)
    face_normals /= np.linalg.norm(face_normals, axis=1, keepdims=True) + 1e-30
    for point, normal in zip(points, normals):
        hit = level.bvh.find_nearest(Vector(point))
        if hit[0] is None:
            continue
        cosine = float(np.clip(normal @ face_normals[hit[2]], -1.0, 1.0))
        if cosine <= 0.0:
            backfacing += 1  # nearest point is through a wall; not a shading statement
            continue
        angles.append(math.degrees(math.acos(cosine)))
    total = len(angles) + backfacing
    if not angles:
        return {"max_deg": 0.0, "p99_deg": 0.0, "p95_deg": 0.0,
                "backface_corr_frac": 1.0 if total else 0.0, "samples": total}
    array = np.array(angles)
    return {
        "max_deg": round(float(array.max()), 3),
        "p99_deg": round(float(np.percentile(array, 99)), 3),
        "p95_deg": round(float(np.percentile(array, 95)), 3),
        "backface_corr_frac": round(backfacing / total, 5),
        "samples": total,
    }


def validity_gate_failures(validity, source_validity, gates, require_baked_tangents=True):
    """The structural gates, as a list of named failures. Empty means clean.

    ONE IMPLEMENTATION, TWO CALLERS — generation and verification — and that is the point of it
    living here rather than in either of them. Twice now a gate existed at generation and was simply
    absent from the verifier: `components_must_match` was compared when a level was cut and never
    again, so a manifest describing a level that had split into two pieces verified clean as long as
    every recorded number honestly described the broken bytes. Parity enforced by a test would have
    needed writing and remembering; parity by construction cannot drift, because there is only one
    list and both sides read it.

    The caller supplies the values. Generation measures them from what it just built; verification
    RE-DERIVES them from the decoded shipped bytes, including `source_validity` — which is decoded
    L0, not a number the manifest asserted about itself.
    """
    failures = []
    if gates["components_must_match"] and validity["components"] != source_validity["components"]:
        failures.append(
            f"component count {validity['components']} != source {source_validity['components']} "
            f"— a part vanished, and a vanished part has near-zero Hausdorff distance"
        )
    # PRESENCE FIRST. `degenerate_tangents` counts bad tangents among those that EXIST, so a level
    # that shipped none at all scored a clean zero and passed — and would have gone back to
    # loader-generated, uncertified tangents without a word. A generated level carries one tangent
    # per vertex or it does not ship.
    if require_baked_tangents and validity["baked_tangents"] != validity["verts"]:
        failures.append(
            f"{validity['baked_tangents']} baked tangents for {validity['verts']} vertices — a "
            f"generated level must BAKE a tangent per vertex, or bevy generates them at load and "
            f"nothing here certified what renders"
        )
    for key, limit_key, description in STRUCTURAL_GATES:
        if validity[key] > gates[limit_key]:
            failures.append(f"{validity[key]} {description}")
    return failures


#: The structural gate table: (validity counter, config limit, what a violation means).
#:
#: DECLARED, so it can be ENUMERATED. `test_refusals` walks it against `config.GATES` and demands
#: every declared `max_*` limit actually gate something — a limit nobody consults is a threshold
#: that reads as protection and is not. And because generation and verification both call the one
#: function above, a gate added here is enforced on both sides at once; parity is by construction
#: rather than by a test that has to be remembered.
STRUCTURAL_GATES = (
        ("duplicate_faces", "max_duplicate_faces", "duplicate face(s)"),
        ("nonfinite_attrs", "max_nonfinite", "non-finite attribute component(s)"),
        ("orientation_flips", "max_orientation_flips",
         "edge(s) traversed the same way by both their faces — inconsistent winding"),
        ("nonmanifold_edges", "max_nonmanifold_edges",
         "edge(s) shared by more than two faces — no consistent normal or tangent frame there, "
         "and a non-watertight volume bakes to zero armour silently"),
        ("tangent_default_faces", "max_tangent_default_faces",
         "face(s) with degenerate UV area would take a DEFAULTED tangent at bind"),
        ("tangent_default_verts", "max_tangent_default_verts",
         "vertex/vertices whose every incident face has degenerate UV area"),
        ("degenerate_tangents", "max_degenerate_tangents",
         "BAKED tangent(s) the loader would use verbatim that are zero-length, non-finite, or "
         "carry a bitangent sign that is not +/-1 — mikktspace gave up on them, and unlike the UV "
         "test above this is the thing itself rather than a necessary condition on it"),
)
