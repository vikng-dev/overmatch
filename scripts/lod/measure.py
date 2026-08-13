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

THE SECOND BOUND, AND IT IS THE ONE THAT MADE THIS AFFORDABLE (ADR 0036 §6). The covering radius
prices a proof at target^-2 — showing a maximum is under `e` forces every patch on the whole surface
below `e`'s length scale, whether or not the two surfaces are anywhere near each other there. But
distance to a FIXED triangle is convex, so over a patch its maximum sits at a corner, and the BVH
already returns WHICH triangle it hit:

    max_{p in S} d(p) <= min_k max_j dist(v_j, tri_k)

Nine point-triangle distances, no query, no subdivision — and on a near-coplanar region it collapses
to the largest corner distance and proves the patch outright. `patch_bounds` takes the minimum of
the two, so it can only tighten, and BOTH consumers below use it.

TWO CONSUMERS, TWO QUESTIONS (ADR 0036 §1). CERTIFICATION wants the number, and runs the bracket
above. The SEARCH only ever wanted a Boolean — "is this candidate inside this rung?" — and paying
for a bracket to answer it is what froze the old rung scan on a 2 784-triangle mesh. `fits_target`
answers the Boolean directly, with a declared node budget and three values: PROVEN_FAIL at the
first sampled witness over the target, PROVEN_PASS when every live bound is under it, UNDECIDED
when the budget runs out. UNDECIDED counts as FAIL at every caller: it may cost triangles, never
honesty.

WHAT DEVIATION CANNOT SEE, and therefore what else lives here: a vanished component (near-zero
Hausdorff), a duplicated face, a NaN and a flipped winding. Those are counted, not estimated.
"""

import heapq
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

def glb_chunks_from_bytes(blob, path="<bytes>"):
    """The JSON dict and the BIN blob of a glb. Stdlib + a struct unpack, no importer involved."""
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
    return Surface(positions, indices, normals[indices], uvs[indices], name)


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

    def __init__(self, verts, tri_v, corner_n, corner_uv, name):
        self.name = name
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
    def validity(self):
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

        return {
            "tris": self.tri_count,
            "verts": self.vert_count,
            "components": self.components(),
            "duplicate_faces": duplicates,
            "nonfinite_attrs": nonfinite,
            "orientation_flips": flips,
            # NON-EMPTY, as a counter so it sits in the same table as every other gate. A collapse
            # that reached zero triangles is not a level, and every number beside it would be a
            # statement about nothing.
            "empty_surfaces": 0 if self.tri_count else 1,
            # DIAGNOSTICS, gated by nothing here (ADR 0036 §4): manifoldness is the armour
            # pipeline's law, and the deviation bound is what polices decimator misbehaviour in
            # this lane. Recorded per level and re-derived from the shipped bytes by the verifier,
            # so a regression is legible.
            "boundary_edges": int((edge_counts == 1).sum()),
            "nonmanifold_edges": int((edge_counts > 2).sum()),
            "min_tri_area_mm2": float(self.tri_area.min() * 1e6) if self.tri_count else 0.0,
            "bbox_mm": [round(float(v) * 1000.0, 4) for v in (self.bbox_max - self.bbox_min)],
            "radius_m": round(self.radius, 6),
            "origin_radius_m": round(self.origin_radius, 6),
        }


# ── the patch bounds, shared by the certificate and the verdict ──────────────────────────────────

def point_triangle_distances(points, a, b, c):
    """Distance from each of `points` to the triangle (a, b, c), elementwise. Shapes (N, 3).

    Ericson's region test, written branchlessly so a whole seed pass is one numpy call. The regions
    are applied in REVERSE priority order (interior, BC, AC, C, AB, B, A) because the reference
    algorithm returns from the first region that matches, and a later `where` would otherwise
    overwrite an earlier decision.

    SOUNDNESS DOES NOT DEPEND ON THE REGION LOGIC BEING RIGHT, only on the point being ON the
    triangle. Every branch returns |p - q| for some q in the convex hull of a, b, c, so the answer
    is never BELOW the true point-triangle distance — and `convex_patch_bounds` only ever needs an
    upper bound. A region bug would cost tightness, never a false certificate. `test_refusals`
    pins the tightness against a brute-force reference anyway.

    IT FAILS CLOSED. A needle triangle can drive the barycentric denominators to zero and produce a
    non-finite closest point, which would be an UNDER-report — the one direction that is not safe.
    Any non-finite answer becomes infinity, so the caller's `min` discards the convex bound and the
    covering-radius subdivision decides the patch.
    """
    ab, ac = b - a, c - a
    ap = points - a
    d1 = (ab * ap).sum(-1)
    d2 = (ac * ap).sum(-1)
    bp = points - b
    d3 = (ab * bp).sum(-1)
    d4 = (ac * bp).sum(-1)
    cp = points - c
    d5 = (ab * cp).sum(-1)
    d6 = (ac * cp).sum(-1)
    va = d3 * d6 - d5 * d4
    vb = d5 * d2 - d1 * d6
    vc = d1 * d4 - d3 * d2

    def _ratio(numerator, denominator):
        safe = np.where(denominator == 0.0, 1.0, denominator)
        return np.clip(numerator / safe, 0.0, 1.0)

    # Interior of the face.
    denominator = va + vb + vc
    v = _ratio(vb, denominator)
    w = _ratio(vc, denominator)
    closest = a + v[..., None] * ab + w[..., None] * ac
    # Edge BC, then AC, then C, then AB, then B, then A — increasing priority.
    mask = (va <= 0) & ((d4 - d3) >= 0) & ((d5 - d6) >= 0)
    t = _ratio(d4 - d3, (d4 - d3) + (d5 - d6))
    closest = np.where(mask[..., None], b + t[..., None] * (c - b), closest)
    mask = (vb <= 0) & (d2 >= 0) & (d6 <= 0)
    t = _ratio(d2, d2 - d6)
    closest = np.where(mask[..., None], a + t[..., None] * ac, closest)
    mask = (d6 >= 0) & (d5 <= d6)
    closest = np.where(mask[..., None], c, closest)
    mask = (vc <= 0) & (d1 >= 0) & (d3 <= 0)
    t = _ratio(d1, d1 - d3)
    closest = np.where(mask[..., None], a + t[..., None] * ab, closest)
    mask = (d3 >= 0) & (d4 <= d3)
    closest = np.where(mask[..., None], b, closest)
    mask = (d1 <= 0) & (d2 <= 0)
    closest = np.where(mask[..., None], a, closest)
    out = np.linalg.norm(points - closest, axis=-1)
    return np.where(np.isfinite(out), out, np.inf)


def covering_patch_bounds(corners, distances):
    """The covering-radius bound over N patches at once: min_k ( d(v_k) + max_j |v_k - v_j| ).

    Valid because d is 1-Lipschitz and |p - v_k| is convex, so its maximum over the triangle is
    attained at another CORNER — a triangle being the convex hull of its three vertices. Batched
    because it is priced four children at a time inside the loop and a whole surface at a time at
    the seeds.
    """
    spans = np.linalg.norm(corners[:, :, None, :] - corners[:, None, :, :], axis=-1)
    return (distances + spans.max(axis=2)).min(axis=1)


def convex_patch_bounds(corners, hits, dst):
    """THE CONVEX BOUND (ADR 0036 §6): min_k max_j dist(v_j, tri_k), over N patches at once.

    `hits[n][k]` is the index of the triangle of `dst` that the BVH returned as NEAREST to corner k
    of patch n, or -1 when the query hit nothing.

    WHY IT IS AN UPPER BOUND. For a FIXED triangle T, p -> dist(p, T) is convex, so its maximum over
    a patch — the convex hull of three corners — is attained at a corner. And dist(p, dst) <=
    dist(p, T) for every T in dst. So for each corner's nearest triangle tri_k,

        max_{p in S} dist(p, dst) <= max_{p in S} dist(p, tri_k) = max_j dist(v_j, tri_k)

    and the minimum over k of those three numbers is the tightest of three valid bounds. Nine
    point-triangle distances, no BVH query, and no subdivision.

    WHY IT MATTERS, MEASURED. The covering-radius bound prices an ACCEPTANCE at target^-2 — proving
    a maximum is under `e` forces every patch on the whole surface below `e`'s length scale, whether
    or not the surfaces are anywhere near each other there. This one does not: on a near-coplanar
    region all three corners see the same plane, `max_j dist(v_j, tri_k)` collapses to the largest
    corner distance, and the patch is proven with zero subdivision. The caller takes the MINIMUM of
    the two, so it can only ever tighten.
    """
    count = len(corners)
    best = np.full(count, np.inf)
    for k in range(3):
        index = hits[:, k]
        present = index >= 0
        safe = np.where(present, index, 0)
        a, b, c = dst.p0[safe], dst.p1[safe], dst.p2[safe]
        worst = np.zeros(count)
        for j in range(3):
            worst = np.maximum(worst, point_triangle_distances(corners[:, j], a, b, c))
        best = np.minimum(best, np.where(present, worst, np.inf))
    return best


def patch_bounds(corners, distances, hits, dst, target):
    """Both bounds, minimised — and the expensive one skipped where the cheap one already closed."""
    bounds = covering_patch_bounds(corners, distances)
    open_patches = np.flatnonzero(bounds > target)
    if len(open_patches):
        bounds[open_patches] = np.minimum(
            bounds[open_patches],
            convex_patch_bounds(corners[open_patches], hits[open_patches], dst),
        )
    return bounds


# ── the certified deviation ──────────────────────────────────────────────────────────────────────

def branch_and_bound(corners, distances, tags, probe, tol, max_nodes,
                     target=None, rel_tol=0.0, bound_of=None):
    """Bracket max over the seed patches of a 1-Lipschitz distance field. Returns (lower, upper).

    `probe(key, point) -> (distance, tag)` is injected rather than reached for, so the loop can be
    regression-tested against a synthetic field with a KNOWN interior maximum, without Blender and
    without a BVH. That is not a convenience: an earlier version of this function returned an "upper
    bound" that was not one, and no test could have caught it because the only way to call it was
    through a mesh whose true worst case nobody knew independently.

    THE TAG IS WHATEVER THE BOUND NEEDS, and the loop never reads it — it only carries it from the
    probe that produced a corner to the `bound_of` that consumes it. For a BVH probe it is the index
    of the nearest triangle, which is what the convex bound is computed from.

    `bound_of(corners, distances, tags, ceiling) -> bounds` prices a BATCH of patches; the default
    is the covering-radius bound alone, which needs nothing but the patch. `ceiling` is the value
    the caller is about to compare against, so a bound function may skip its expensive half where
    the cheap half has already closed.

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
    if bound_of is None:
        def bound_of(patch_corners, patch_distances, _tags, _ceiling):
            return covering_patch_bounds(patch_corners, patch_distances)

    corners = np.asarray(corners, dtype=np.float64)
    distances = np.asarray(distances, dtype=np.float64)
    tags = (np.zeros(distances.shape, dtype=np.int64) if tags is None
            else np.asarray(tags, dtype=np.int64))

    # THE LOWER END IS A WITNESS AND ONLY MEASUREMENTS ARE WITNESSES. A corner the probe could not
    # answer for carries `UNKNOWN_DISTANCE`; it belongs in the bound, which the heap keeps open on
    # it, and nowhere near `best`, which the stopping rule closes against.
    finite = distances[np.isfinite(distances)]
    best = float(finite.max()) if finite.size else 0.0
    observed = bool(finite.size)
    bounds = bound_of(corners, distances, tags, best)
    heap = []
    counter = 0
    for t in range(len(corners)):
        counter += 1
        heapq.heappush(heap, (-float(bounds[t]), counter, corners[t], distances[t], tags[t]))

    pruned = 0.0
    nodes = 0
    while heap:
        live = -heap[0][0]
        slack = max(tol, rel_tol * best)
        if live <= best + slack or nodes >= max_nodes:
            break
        if target is not None and (live <= target or best > target):
            break
        # A DEAD QUERY CANNOT BE SUBDIVIDED INTO A LIVE ONE. When nothing has answered yet — an
        # empty destination tree, or one built from non-finite coordinates, both of which Blender
        # reports as "no hit" at EVERY point (measured) — every child of every patch is unknown too,
        # and expanding them would spend the whole node budget growing a heap of infinities. Stop
        # and hand back the infinite upper bound the caller must refuse anyway.
        #
        # The test is "nothing has answered YET", not "this patch is unknown", because one usable
        # corner anywhere is enough for subdivision to recover: the covering bound holds for each
        # corner independently, so a live tree resolves an unknown patch at its midpoints.
        if not observed and not math.isfinite(live):
            break
        _, _, patch, patch_d, patch_t = heapq.heappop(heap)
        nodes += 1
        a, b, c = patch
        ab, bc, ca = 0.5 * (a + b), 0.5 * (b + c), 0.5 * (c + a)
        d_ab, t_ab = probe(("m", tuple(ab)), ab)
        d_bc, t_bc = probe(("m", tuple(bc)), bc)
        d_ca, t_ca = probe(("m", tuple(ca)), ca)
        best = sampled_max(best, d_ab, d_bc, d_ca)
        observed = observed or any(math.isfinite(d) for d in (d_ab, d_bc, d_ca))
        slack = max(tol, rel_tol * best)
        children = np.array([(a, ab, ca), (ab, b, bc), (ca, bc, c), (ab, bc, ca)])
        child_d = np.array([
            (patch_d[0], d_ab, d_ca), (d_ab, patch_d[1], d_bc),
            (d_ca, d_bc, patch_d[2]), (d_ab, d_bc, d_ca),
        ])
        child_t = np.array([
            (patch_t[0], t_ab, t_ca), (t_ab, patch_t[1], t_bc),
            (t_ca, t_bc, patch_t[2]), (t_ab, t_bc, t_ca),
        ])
        child_bounds = bound_of(children, child_d, child_t, best + slack)
        for index in range(4):
            sub = float(child_bounds[index])
            if sub > best + slack:
                counter += 1
                heapq.heappush(heap, (
                    -sub, counter, children[index], child_d[index], child_t[index],
                ))
            else:
                pruned = max(pruned, sub)

    live = -heap[0][0] if heap else 0.0
    return best, max(best, pruned, live)


#: What a probe answers when it CANNOT answer. Infinity, never zero, and the difference is the whole
#: of this constant's reason to exist.
#:
#: A `find_nearest` that returns no hit, or a hit whose distance is NaN, is not the statement "this
#: point is on the surface" — it is "this query failed". Mapping it to 0.0 said the first, and a
#: zero is the most TIGHTENING value there is: it drives the covering bound down, it can become the
#: `best` a bracket closes against, and `NaN > target` is False so it silently passes an acceptance
#: check. Infinity says the second, and every consumer already handles it correctly by construction
#: — `min_k` over the corners ignores it (each corner bounds the patch on its own, so one usable
#: corner is enough), the live heap can never close on it, and a sampled value that is not a number
#: is not a sample.
UNKNOWN_DISTANCE = math.inf


def bvh_probe(dst):
    """`(distance, nearest triangle index)` against `dst`, memoized. Blender-only (mathutils).

    FAIL-CLOSED, the same law `point_triangle_distances` follows: an answer that is not a finite
    distance becomes `UNKNOWN_DISTANCE` with no triangle, so it can only ever force subdivision or
    refusal. It can never tighten a bound and it can never be mistaken for a measurement.

    REACHABILITY, stated because a guard nobody can trigger reads as superstition. The no-hit branch
    is effectively unreachable against a NON-EMPTY tree at finite coordinates: Blender's default
    search radius is ~1.8e19, so something is always inside it. It IS reachable two ways — a
    destination surface with no triangles at all (a collapse that reached zero, which the
    `max_empty_surfaces` gate refuses immediately AFTER the deviation is measured, not before), and
    a NON-FINITE coordinate in either surface, which reaches this through `certify` because
    `certified_deviation` runs before `max_nonfinite` looks at the shipped bytes. Both are real
    orderings in the lane today, and both now end in a refusal rather than in a number.
    """
    from mathutils import Vector

    find = dst.bvh.find_nearest
    cache = {}

    def probe(key, point):
        value = cache.get(key)
        if value is None:
            hit = find(Vector(point))
            distance = hit[3] if hit[0] is not None else None
            if distance is None or not math.isfinite(distance):
                value = (UNKNOWN_DISTANCE, -1)
            else:
                value = (float(distance), int(hit[2]))
            cache[key] = value
        return value

    return probe


def sampled_max(current, *values):
    """The largest value ACTUALLY MEASURED, ignoring the ones that are not measurements.

    A lower bound is a witness — "the surface attains at least this" — and `UNKNOWN_DISTANCE` is
    the absence of a witness rather than an enormous one. Folding it in would report an infinite
    deviation as if it had been observed; dropping it leaves the lower end honest while the bound
    side, which is where the infinity belongs, keeps the bracket open.
    """
    for value in values:
        if math.isfinite(value) and value > current:
            current = value
    return current


def seed_patches(src, probe):
    """Every triangle of `src` as a seed patch: (corners, corner distances, corner tags).

    One probe per REFERENCED vertex — an unreferenced vertex is not on the surface, and a bound or a
    witness taken from one would be a statement about a point the mesh does not have.
    """
    corner_d = np.zeros(src.vert_count, dtype=np.float64)
    corner_t = np.full(src.vert_count, -1, dtype=np.int64)
    for index in np.unique(src.tri_v):
        distance, tag = probe(int(index), src.verts[index])
        corner_d[index] = distance
        corner_t[index] = tag
    corners = np.stack([src.p0, src.p1, src.p2], axis=1)
    return corners, corner_d[src.tri_v], corner_t[src.tri_v]


def _one_way(src, dst, tol, max_nodes, target=None, rel_tol=0.0):
    """Certified max over `src`'s surface of the distance to `dst`. Returns (lower, upper) metres.

    THE CONVEX BOUND IS APPLIED HERE TOO, on the same law as the search side (ADR 0036 §6), and it
    was the rebuild's hard success gate that forced it. Leaving certification on the covering-radius
    bound alone left the lane INTERNALLY INCONSISTENT: on `Turret_Decor` the directed search proved
    a 1 330-triangle candidate inside the 3.890 mm rung and this function, node-capped at 1.5 M,
    returned 4.8383 mm and refused the level it had just been handed. Measured, the tightened bound
    turns that refusal into a complete three-level chain and takes the full-tank projection from
    71+ minutes to inside the ruling.

    WHAT DID NOT CHANGE, and it is the part that matters: this still measures the DECODED SHIPPED
    BYTES, still returns a sound two-way bracket at the certification tolerance, and the caller
    still gates on the UPPER end. Only the bound got tighter, and a tighter upper bound can move a
    certificate one way — down. Nothing is admitted that was not admissible before.
    """
    probe = bvh_probe(dst)
    corners, distances, tags = seed_patches(src, probe)

    def bound_of(patch_corners, patch_distances, patch_tags, ceiling):
        return patch_bounds(patch_corners, patch_distances, patch_tags, dst, ceiling)

    return branch_and_bound(corners, distances, tags, probe, tol, max_nodes,
                            target, rel_tol, bound_of)


# ── the budgeted Boolean verdict (ADR 0036 §1) ───────────────────────────────────────────────────

PROVEN_PASS = "PROVEN_PASS"
PROVEN_FAIL = "PROVEN_FAIL"
UNDECIDED = "UNDECIDED"


def one_way_fits(src, dst, target_m, node_budget):
    """Does every point of `src` lie within `target_m` of `dst`? Returns (verdict, witness, nodes).

    THREE DIFFERENCES FROM `_one_way`, all of them about answering the caller's actual question
    instead of bracketing a number nobody reads.

    WITNESS FIRST. The corner distances are swept in ascending vertex index and the FIRST one over
    the target returns immediately. A rejection is one BVH query deep in the good case and never
    builds a heap at all. (Only vertices a triangle references are swept: an unreferenced vertex is
    not on the surface, and rejecting on one would be a lie.)

    LAZY SEED CONSTRUCTION. Every source triangle's bound is computed in one vectorised pass off the
    corner distances already in hand, and the heap receives ONLY the triangles whose bound exceeds
    the target. Every other triangle is already decided — its whole patch is proven under the target
    — so it is not a node, not a heap entry and not a memory cost. When the live set is empty the
    answer is PROVEN_PASS before a single subdivision.

    TARGET-ONLY PRUNING. `branch_and_bound` subdivides until a bracket closes to `rel_tol`, which is
    the mechanism that froze on a 2 784-triangle mesh: the bracket had to shrink by ~58 % to answer
    a question a single sample could have answered. Here a child is queued only if its bound is over
    the target, and the loop ends when the heap empties (PASS), a sample exceeds the target (FAIL),
    or the node budget is spent (UNDECIDED).

    SOUNDNESS is the same as the bracket's: every live bound under the target proves the maximum is
    under it, and a SAMPLED point over the target is a witness that it is not. Neither statement
    needs the bracket to close.

    THE BOUND IS THE ONE `_one_way` CERTIFIES WITH — `patch_bounds`, both halves, one expression.
    What differs between the two callers is only the question and the stopping rule.
    """
    probe = bvh_probe(dst)

    # WITNESS FIRST, and it is why the corner sweep is not `seed_patches`: a rejection must return
    # at the first sample over the target rather than after probing the whole surface.
    witness = 0.0
    corner_d = np.zeros(src.vert_count, dtype=np.float64)
    corner_t = np.full(src.vert_count, -1, dtype=np.int64)
    for index in np.unique(src.tri_v):
        distance, tag = probe(int(index), src.verts[index])
        corner_d[index] = distance
        corner_t[index] = tag
        # UNMEASURABLE IS UNACCEPTABLE, and it is checked before the comparison rather than through
        # it: `UNKNOWN_DISTANCE > target` happens to be True, but the NaN it stands in for is not,
        # and a predicate that only works because of which sentinel was picked is a predicate
        # waiting to be silently inverted. A candidate whose distance to the source cannot be
        # measured cannot be proven inside a rung, so it fails.
        if not math.isfinite(distance):
            return PROVEN_FAIL, witness, 0
        if distance > witness:
            witness = distance
        if distance > target_m:
            return PROVEN_FAIL, witness, 0

    corners = np.stack([src.p0, src.p1, src.p2], axis=1)
    distances = corner_d[src.tri_v]
    hits = corner_t[src.tri_v]
    bounds = patch_bounds(corners, distances, hits, dst, target_m)
    live = np.flatnonzero(bounds > target_m)
    if not len(live):
        return PROVEN_PASS, witness, 0

    heap = []
    counter = 0
    for t in live:
        counter += 1
        heapq.heappush(heap, (-float(bounds[t]), counter, corners[t], distances[t], hits[t]))

    nodes = 0
    while heap:
        if nodes >= node_budget:
            return UNDECIDED, witness, nodes
        _, _, patch, patch_d, patch_h = heapq.heappop(heap)
        nodes += 1
        a, b, c = patch
        ab, bc, ca = 0.5 * (a + b), 0.5 * (b + c), 0.5 * (c + a)
        d_ab, t_ab = probe(("m", tuple(ab)), ab)
        d_bc, t_bc = probe(("m", tuple(bc)), bc)
        d_ca, t_ca = probe(("m", tuple(ca)), ca)
        if not all(math.isfinite(d) for d in (d_ab, d_bc, d_ca)):
            return PROVEN_FAIL, witness, nodes
        witness = sampled_max(witness, d_ab, d_bc, d_ca)
        if witness > target_m:
            return PROVEN_FAIL, witness, nodes
        children = np.array([
            (a, ab, ca), (ab, b, bc), (ca, bc, c), (ab, bc, ca),
        ])
        child_d = np.array([
            (patch_d[0], d_ab, d_ca), (d_ab, patch_d[1], d_bc),
            (d_ca, d_bc, patch_d[2]), (d_ab, d_bc, d_ca),
        ])
        child_h = np.array([
            (patch_h[0], t_ab, t_ca), (t_ab, patch_h[1], t_bc),
            (t_ca, t_bc, patch_h[2]), (t_ab, t_bc, t_ca),
        ])
        child_bounds = patch_bounds(children, child_d, child_h, dst, target_m)
        for index in range(4):
            if child_bounds[index] > target_m:
                counter += 1
                heapq.heappush(heap, (
                    -float(child_bounds[index]), counter,
                    children[index], child_d[index], child_h[index],
                ))
    return PROVEN_PASS, witness, nodes


def fits_target(source, candidate, target_mm, node_budget):
    """TWO-WAY budgeted verdict: is the deviation between these surfaces within `target_mm`?

    Both directions, because one-way misses a hole — the same reason `certified_deviation` runs
    both. The source->candidate direction runs first (a deleted boss is visible only from it, and it
    is the cheaper rejection in practice). A FAIL in either direction is a FAIL; UNDECIDED in
    either, with no FAIL, is UNDECIDED. The budget is PER DIRECTION, so a verdict costs at most
    `2 * node_budget` nodes.

    THE WITNESS IS THE DEVIATION. Measured on the shipped corpus, the largest sampled point of a
    PROVEN_PASS reproduces the certified `dev_source_mm` to four decimals — the Boolean recovers the
    number as a by-product, without ever bracketing it. It is returned for logging and never gated
    on: the shipped bound comes from `certify`, on the decoded bytes, at the certification
    tolerance.
    """
    if source.digest() == candidate.digest():
        return {"verdict": PROVEN_PASS, "witness_mm": 0.0, "nodes": 0}
    target_m = target_mm / 1000.0
    verdict_a, witness_a, nodes_a = one_way_fits(source, candidate, target_m, node_budget)
    if verdict_a == PROVEN_FAIL:
        return {"verdict": PROVEN_FAIL, "witness_mm": witness_a * 1000.0, "nodes": nodes_a}
    verdict_b, witness_b, nodes_b = one_way_fits(candidate, source, target_m, node_budget)
    witness = max(witness_a, witness_b) * 1000.0
    nodes = nodes_a + nodes_b
    if verdict_b == PROVEN_FAIL:
        return {"verdict": PROVEN_FAIL, "witness_mm": witness, "nodes": nodes}
    if UNDECIDED in (verdict_a, verdict_b):
        return {"verdict": UNDECIDED, "witness_mm": witness, "nodes": nodes}
    return {"verdict": PROVEN_PASS, "witness_mm": witness, "nodes": nodes}


#: A rung the SEARCH could not decide inside its node budget. Named because two files act on it —
#: `rung_lost_to` writes it and `scripts/tank/chains.py` announces it — and a string spelled twice is
#: a vocabulary that drifts. Its sibling is `trio.LOST_TO_BRACKET`.
LOST_TO_BUDGET = "verdict_node_budget"

#: The only three things a rung can be lost to, in the order they outrank each other.
SKIP_LOST_TO = (LOST_TO_BUDGET, "geometry", "skip_fraction")


def rung_lost_to(undecided, otherwise):
    """What a lost rung is lost TO. Any UNDECIDED verdict outranks every other explanation.

    ONE DECLARATION, TWO CALLERS, for the same reason the gate list has one: generation writes this
    field and verification validates it, and when the rule lived in both places they disagreed. The
    writer filed a rung under `skip_fraction` whenever the sparse-chain rule had fired, even if the
    search had abstained on a candidate at that rung — and the verifier's fidelity warning, which
    only fires on `verdict_node_budget`, went quiet on exactly the rungs that earned one.

    THE PRECEDENCE IS NOT A PREFERENCE. If the search could not decide a candidate, the winner it
    settled on won partly by default, so the shed fraction that failed `SKIP_FRACTION` was measured
    against a field that was never fully judged. "Lost to the budget" is the true statement; "lost
    to the skip rule" is a true statement about a comparison that should not have been the last
    word. Nothing unproven ships either way — what is at stake is whether the trade is legible.
    """
    return LOST_TO_BUDGET if undecided else otherwise


def directed_rung_search(floor_tris, ceiling_tris, probe):
    """The cheapest candidate a rung can find, by bisection over integer triangle BUDGETS.

    `probe(budget) -> (reached, verdict)`, where `reached` is the count the DECIMATOR landed on
    (None below the topology floor) and `verdict` is one of the three above. Returns the winning
    budget, or None.

    THE PROBE ORDER IS FIXED AND INTEGER. `mid = (lo + hi) // 2`, no floats, no randomness, no
    clock — the same asset gives the same probes on any machine, which is what makes two runs
    comparable field for field. On PROVEN_PASS the search moves below the count the decimator
    REACHED rather than below the budget it was asked for: every budget in [reached, mid] realizes
    the same mesh, so `reached - 1` is the next distinct question (`bisect_to_budget` is where that
    step's soundness is established).

    UNDECIDED IS TREATED AS FAIL, and so is a structurally invalid candidate and a budget below the
    floor: all four move the search RICHER. An undecidable candidate may cost triangles; it may
    never cost honesty (ADR 0036 §1).

    WHAT THIS GIVES UP, DELIBERATELY. The exhaustive staircase proved "no valid output with fewer
    triangles meets this rung". A bisection over a predicate that is not monotone cannot prove that
    — it finds a low-triangle valid candidate deterministically. ADR 0036 §2 retires the minimality
    claim on the record: what the player depends on is the certified bound at the switch distance,
    which is untouched.

    It lives here, away from `bpy`, so the loop can be tested against a synthetic probe — including
    the one that says UNDECIDED to everything, which must select nothing.
    """
    low, high = floor_tris, ceiling_tris
    best = None
    while low <= high:
        middle = (low + high) // 2
        reached, verdict = probe(middle)
        if verdict == PROVEN_PASS:
            best = middle
            high = reached - 1
        else:
            low = middle + 1
    return best


class DecimatorContractError(Exception):
    """The decimation oracle did not behave like the monotone step function it is taken to be."""


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

    THIS IS WHERE THE DECIMATOR'S CONTRACT IS ESTABLISHED, and it is deliberately a pure function of
    an injected `evaluate` so it can be checked against synthetic staircases whose answer is known
    independently — see `test_refusals.BisectionTests`. It matters to `directed_rung_search`, whose
    step from an accepted candidate to the next distinct question is `reached - 1`: that step is
    sound only if `reached` really is the greatest realizable count at or below the budget. A search
    built on top cannot establish it — it can only ask the oracle, and an oracle that lies
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
    raise DecimatorContractError(
        f"the budget bisection did not exhaust its bracket in {max_steps} steps — the evaluator "
        f"is not behaving like a monotone step function"
    )


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
    the normal map and the lighting, none of which this lane measures — the rendered comparison
    that once did is deleted (ADR 0036 §3) and the eye is what judges appearance now. This number
    is reported so a regression is legible, never so a build fails on it.

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


def validity_gate_failures(validity, source_validity, gates):
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
    # NON-DEGENERATE, which the gate list has claimed since ADR 0036 §4 re-scoped it and which
    # nothing enforced: the smallest triangle area was measured, recorded, compared against nothing.
    #
    # THE THRESHOLD IS NOT A NEW NUMBER. It is the cleanup pass's own coincidence distance, squared:
    # a face whose area is at or below `(cleanup_dissolve_frac_of_diag * diagonal)**2` has BOTH its
    # extents at the scale this lane already treats as one point, so it has no meaningful normal.
    # Scale-free, and derived rather than declared twice.
    #
    # IT IS DELIBERATELY NOT THE SLIVER TEST. A needle — 50 mm long, microns thick, real area — is
    # far above this floor, and it is `generate.cleanup` that dissolves that class, before export
    # and by construction. Setting the floor high enough to catch slivers would refuse geometry the
    # decimator legitimately produces. This gate names exact degeneracy and says so.
    floor_mm2 = (gates["cleanup_dissolve_frac_of_diag"]
                 * math.sqrt(sum(float(v) * float(v) for v in validity["bbox_mm"]))) ** 2
    if validity["min_tri_area_mm2"] <= floor_mm2:
        failures.append(
            f"smallest triangle is {validity['min_tri_area_mm2']:.6g} mm^2, at or under the "
            f"{floor_mm2:.6g} mm^2 degeneracy floor for a mesh this size — a face that small has "
            f"no consistent normal, and every quantity derived from one is noise"
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
#: RE-SCOPED BY ADR 0036 §4. What left: the UV-area and baked-tangent counters (they served the
#: deleted rendered-difference gate, and untextured physics-vocabulary meshes are legal), and the
#: non-manifold-edge gate (manifoldness is the armour pipeline's law; here the deviation bound
#: polices the decimator by construction — a mangled region deviates and fails, an undeviating one
#: is invisible by definition). Both counters are still measured and recorded per level.
STRUCTURAL_GATES = (
        ("empty_surfaces", "max_empty_surfaces", "empty surface(s) — no triangles at all"),
        ("duplicate_faces", "max_duplicate_faces", "duplicate face(s)"),
        ("nonfinite_attrs", "max_nonfinite", "non-finite attribute component(s)"),
        ("orientation_flips", "max_orientation_flips",
         "edge(s) traversed the same way by both their faces — inconsistent winding"),
)
