"""Proof that every named refusal fires, and that the byte-level gates count what they claim.

    python3 scripts/lod/test_refusals.py

"Nothing silently passes" (ADR 0033 §10) is a claim about code that almost never runs — the refusal
paths only fire on an asset nobody has authored yet. So they are exercised here against synthetic
GLBs built byte by byte, rather than trusted because they are written down. numpy only; no Blender,
which is why these can run in a hook alongside `test_chain.py` (the BVH-backed deviation search does
need Blender and is exercised by generation itself).
"""

import json
import os
import struct
import sys
import tempfile
import unittest

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import config as CONFIG  # noqa: E402
import measure as M  # noqa: E402


def build_glb(path, positions, indices, normals=None, uvs=None, extra_attrs=None,
              primitives=None, targets=None, mode=4, indexed=True, mesh_count=1,
              tangents=None, node_name=None):
    """A minimal, valid glb carrying exactly what the caller asked for. Handwritten on purpose."""
    positions = np.asarray(positions, dtype=np.float32)
    indices = np.asarray(indices, dtype=np.uint32).reshape(-1)
    normals = np.asarray(
        normals if normals is not None else np.tile([0.0, 0.0, 1.0], (len(positions), 1)),
        dtype=np.float32,
    )
    uvs = np.asarray(
        uvs if uvs is not None else np.tile([0.0, 0.0], (len(positions), 1)), dtype=np.float32
    )

    blobs, views, accessors = [], [], []
    offset = 0

    def add(array, kind, component, count, minmax=False):
        nonlocal offset
        raw = array.tobytes()
        pad = (-len(raw)) % 4
        blobs.append(raw + b"\x00" * pad)
        views.append({"buffer": 0, "byteOffset": offset, "byteLength": len(raw)})
        offset += len(raw) + pad
        accessor = {"bufferView": len(views) - 1, "componentType": component,
                    "count": count, "type": kind}
        if minmax:
            accessor["min"] = [float(v) for v in array.min(axis=0)]
            accessor["max"] = [float(v) for v in array.max(axis=0)]
        accessors.append(accessor)
        return len(accessors) - 1

    index_accessor = add(indices, "SCALAR", 5125, len(indices))
    position_accessor = add(positions, "VEC3", 5126, len(positions), minmax=True)
    normal_accessor = add(normals, "VEC3", 5126, len(normals))
    uv_accessor = add(uvs, "VEC2", 5126, len(uvs))

    attributes = {"POSITION": position_accessor, "NORMAL": normal_accessor,
                  "TEXCOORD_0": uv_accessor}
    if tangents is not None:
        tangents = np.asarray(tangents, dtype=np.float32)
        attributes["TANGENT"] = add(tangents, "VEC4", 5126, len(tangents))
    if extra_attrs:
        for name in extra_attrs:
            attributes[name] = uv_accessor
    primitive = {"attributes": attributes, "mode": mode}
    if indexed:
        primitive["indices"] = index_accessor
    if targets:
        primitive["targets"] = targets
    prims = primitives if primitives is not None else [primitive]

    meshes = [{"name": f"M{i}", "primitives": prims} for i in range(mesh_count)]
    gltf = {
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": list(range(mesh_count))}],
        "nodes": [{"name": node_name or f"Node{i}", "mesh": i} for i in range(mesh_count)],
        "meshes": meshes,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": offset}],
    }
    binary = b"".join(blobs)
    text = json.dumps(gltf).encode()
    text += b" " * ((-len(text)) % 4)
    total = 12 + 8 + len(text) + 8 + len(binary)
    with open(path, "wb") as handle:
        handle.write(struct.pack("<4sII", b"glTF", 2, total))
        handle.write(struct.pack("<II", len(text), 0x4E4F534A))
        handle.write(text)
        handle.write(struct.pack("<II", len(binary), 0x004E4942))
        handle.write(binary)
    return path


#: A unit square as two triangles, in the XY plane, with a sane UV layout.
SQUARE_POS = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0]]
SQUARE_IDX = [0, 1, 2, 0, 2, 3]
SQUARE_UV = [[0, 0], [1, 0], [1, 1], [0, 1]]


class RefusalTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="lod-refusal-")

    def path(self, name):
        return os.path.join(self.dir, name)

    def refusal_from(self, path):
        with self.assertRaises(M.Refusal) as caught:
            M.from_glb(path)
        return caught.exception.reason

    def test_a_skinned_mesh_is_refused_by_name(self):
        p = build_glb(self.path("skin.glb"), SQUARE_POS, SQUARE_IDX, uvs=SQUARE_UV,
                      extra_attrs=["JOINTS_0"])
        self.assertEqual(self.refusal_from(p), "skinned-mesh")

    def test_a_morph_target_mesh_is_refused_by_name(self):
        p = build_glb(self.path("morph.glb"), SQUARE_POS, SQUARE_IDX, uvs=SQUARE_UV,
                      targets=[{"POSITION": 1}])
        self.assertEqual(self.refusal_from(p), "morph-mesh")

    def test_a_multi_primitive_mesh_is_refused_by_name(self):
        base = {"attributes": {"POSITION": 1, "NORMAL": 2, "TEXCOORD_0": 3},
                "indices": 0, "mode": 4}
        p = build_glb(self.path("multi.glb"), SQUARE_POS, SQUARE_IDX, uvs=SQUARE_UV,
                      primitives=[dict(base), dict(base)])
        self.assertEqual(self.refusal_from(p), "multi-primitive-mesh")

    def test_a_multi_mesh_glb_is_refused_by_name(self):
        p = build_glb(self.path("many.glb"), SQUARE_POS, SQUARE_IDX, uvs=SQUARE_UV, mesh_count=2)
        self.assertEqual(self.refusal_from(p), "multi-mesh-glb")

    def test_a_non_indexed_primitive_is_refused_by_name(self):
        p = build_glb(self.path("noidx.glb"), SQUARE_POS, SQUARE_IDX, uvs=SQUARE_UV, indexed=False)
        self.assertEqual(self.refusal_from(p), "non-indexed-primitive")

    def test_a_non_triangle_mode_is_refused_by_name(self):
        p = build_glb(self.path("strip.glb"), SQUARE_POS, SQUARE_IDX, uvs=SQUARE_UV, mode=5)
        self.assertEqual(self.refusal_from(p), "not-triangles")

    def test_a_file_that_is_not_a_glb_is_refused_by_name(self):
        p = self.path("nope.glb")
        with open(p, "wb") as handle:
            handle.write(b"not a glb at all, really")
        self.assertEqual(self.refusal_from(p), "not-a-glb")


class DecodeTests(unittest.TestCase):
    """The decode itself — the step every gate's number depends on being right."""

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="lod-decode-")

    def test_the_decode_undoes_the_y_up_conversion(self):
        """glTF is Y-up; the source lives in Blender's Z-up. Measuring across that is a hull-sized lie."""
        blender = np.array([[1.0, 2.0, 3.0]])
        gltf = np.array([[1.0, 3.0, -2.0]])  # what the exporter writes for that point
        np.testing.assert_allclose(M.gltf_to_blender(gltf), blender)

    def test_positions_survive_the_round_trip(self):
        path = build_glb(os.path.join(self.dir, "sq.glb"), SQUARE_POS, SQUARE_IDX, uvs=SQUARE_UV)
        surface = M.from_glb(path)
        self.assertEqual(surface.tri_count, 2)
        self.assertEqual(surface.vert_count, 4)
        # the square is 1 x 1 in glTF XY, which is Blender XZ after the conversion
        self.assertAlmostEqual(surface.diagonal, float(np.sqrt(2.0)), places=6)


class ValidityTests(unittest.TestCase):
    """Each structural gate, shown counting the defect it exists for and nothing else."""

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="lod-validity-")
        self.gates = dict(CONFIG.GATES)

    def surface(self, name, positions, indices, uvs):
        return M.from_glb(build_glb(os.path.join(self.dir, name), positions, indices, uvs=uvs))

    def test_a_clean_square_passes_everything(self):
        report = self.surface("clean.glb", SQUARE_POS, SQUARE_IDX, SQUARE_UV).validity(self.gates)
        self.assertEqual(report["duplicate_faces"], 0)
        self.assertEqual(report["orientation_flips"], 0)
        self.assertEqual(report["tangent_default_faces"], 0)
        self.assertEqual(report["nonfinite_attrs"], 0)
        self.assertEqual(report["components"], 1)
        self.assertEqual(report["slivers_below_floor"], 0)

    def test_a_duplicate_face_is_counted(self):
        report = self.surface(
            "dup.glb", SQUARE_POS, SQUARE_IDX + [0, 1, 2], SQUARE_UV
        ).validity(self.gates)
        self.assertEqual(report["duplicate_faces"], 1)

    def test_a_flipped_winding_is_counted(self):
        """The two triangles share edge 0-2; reversing one makes both traverse it the same way."""
        report = self.surface(
            "flip.glb", SQUARE_POS, [0, 1, 2, 2, 0, 3][:3] + [0, 3, 2], SQUARE_UV
        ).validity(self.gates)
        self.assertEqual(report["orientation_flips"], 1)

    def test_a_zero_uv_face_is_counted_as_a_defaulted_tangent(self):
        """One triangle whose UV corners coincide; mikktspace divides by that area and defaults.

        Two disjoint triangles rather than a shared-edge pair, because any two triangles sharing a
        vertex whose UV is collapsed drag each other into the count — which is correct behaviour and
        makes a poor demonstration of counting ONE.
        """
        positions = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [5, 0, 0], [6, 0, 0], [5, 1, 0]]
        uvs = [[0, 0], [0, 0], [0, 0], [0, 0], [1, 0], [0, 1]]
        report = self.surface("uv0.glb", positions, [0, 1, 2, 3, 4, 5], uvs).validity(self.gates)
        self.assertEqual(report["tangent_default_faces"], 1)
        self.assertEqual(report["tangent_default_verts"], 3)

    def test_two_disjoint_squares_count_as_two_components(self):
        positions = SQUARE_POS + [[10, 0, 0], [11, 0, 0], [11, 1, 0], [10, 1, 0]]
        indices = SQUARE_IDX + [4, 5, 6, 4, 6, 7]
        uvs = SQUARE_UV + SQUARE_UV
        self.assertEqual(
            self.surface("two.glb", positions, indices, uvs).validity(self.gates)["components"], 2
        )

    def test_a_needle_triangle_falls_below_the_altitude_floor(self):
        """A collapse leaves needles; the gate is an altitude, not an area, so length cannot hide it."""
        positions = [[0, 0, 0], [1, 0, 0], [0.5, 1e-9, 0]]
        surface = self.surface("needle.glb", positions, [0, 1, 2], [[0, 0], [1, 0], [0.5, 1]])
        report = surface.validity(self.gates)
        self.assertEqual(report["slivers_below_floor"], 1)
        self.assertLess(report["min_altitude_m"], report["min_altitude_floor_m"])

    def test_a_nonfinite_position_is_counted(self):
        positions = [[0, 0, 0], [1, 0, 0], [float("nan"), 1, 0], [0, 1, 0]]
        report = self.surface("nan.glb", positions, SQUARE_IDX, SQUARE_UV).validity(self.gates)
        self.assertGreater(report["nonfinite_attrs"], 0)

    def test_three_faces_on_one_edge_are_counted_as_non_manifold(self):
        """A T-junction of three faces: no consistent normal or tangent frame along that edge.

        Reported but never gated for a while, which is its own lesson — a counter nothing compares
        against is decoration. A non-watertight volume also bakes to ZERO armour, silently.
        """
        positions = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1]]
        indices = [0, 1, 2, 0, 1, 3, 0, 1, 4]
        uvs = [[0, 0], [1, 0], [1, 1], [0, 1], [0.5, 0.5]]
        report = self.surface("nonmanifold.glb", positions, indices, uvs).validity(self.gates)
        self.assertEqual(report["nonmanifold_edges"], 1)
        self.assertGreater(report["nonmanifold_edges"], self.gates["max_nonmanifold_edges"])

    def test_the_origin_radius_is_measured_from_the_origin_not_the_box_centre(self):
        """The runtime slack the switch distances use — and the two are genuinely different points.

        `VisibilityRange` measures to the entity ORIGIN. A half-AABB-diagonal bounds distance from
        the box CENTRE, which on an off-centre asset is somewhere else entirely: the shipped Link
        decodes to 0.400124 m from its origin against a 0.384004 m half-diagonal, so every switch
        was landing 16 mm early.
        """
        positions = [[1, 0, 0], [3, 0, 0], [3, 4, 0], [1, 4, 0]]
        surface = self.surface("offcentre.glb", positions, SQUARE_IDX, SQUARE_UV)
        # glTF (x, y, z) decodes to Blender (x, -z, y): the far corner is (3, 0, 4), |.| = 5.
        self.assertAlmostEqual(surface.origin_radius, 5.0, places=6)
        self.assertAlmostEqual(surface.radius, 0.5 * float(np.sqrt(4 + 16)), places=6)
        self.assertGreater(
            surface.origin_radius, surface.radius,
            "an off-centre asset is farther from its origin than from its own box centre",
        )
        self.assertAlmostEqual(
            surface.validity(self.gates)["origin_radius_m"], 5.0, places=6
        )

    def test_the_digest_separates_meshes_that_differ_only_in_uv(self):
        """Codex's counterexample: same positions and topology, one with a collapsed UV.

        Their validity differs — `tangent_default_faces` 0 against 2 — so treating them as one
        candidate can discard the good one and keep the broken one, or the reverse. Identity has to
        cover every attribute the gates measure.
        """
        good = self.surface("uv_good.glb", SQUARE_POS, SQUARE_IDX, SQUARE_UV)
        collapsed = self.surface(
            "uv_collapsed.glb", SQUARE_POS, SQUARE_IDX, [[0, 0], [0, 0], [0, 0], [0, 0]]
        )
        self.assertEqual(good.validity(self.gates)["tangent_default_faces"], 0)
        self.assertEqual(collapsed.validity(self.gates)["tangent_default_faces"], 2)
        self.assertNotEqual(good.digest(), collapsed.digest())

    def test_the_digest_separates_meshes_that_differ_only_in_winding(self):
        """Same points, one triangle wound the other way: `orientation_flips` 0 against 1."""
        good = self.surface("wind_good.glb", SQUARE_POS, [0, 1, 2, 0, 2, 3], SQUARE_UV)
        flipped = self.surface("wind_flipped.glb", SQUARE_POS, [0, 1, 2, 0, 3, 2], SQUARE_UV)
        self.assertEqual(good.validity(self.gates)["orientation_flips"], 0)
        self.assertEqual(flipped.validity(self.gates)["orientation_flips"], 1)
        self.assertNotEqual(good.digest(), flipped.digest())

    def test_the_digest_separates_meshes_that_differ_only_in_normals(self):
        normals = [[0, 0, 1]] * 4
        tilted = [[0, 0.5, 0.866]] * 4
        a = M.from_glb(build_glb(os.path.join(self.dir, "n_a.glb"), SQUARE_POS, SQUARE_IDX,
                                 normals=normals, uvs=SQUARE_UV))
        b = M.from_glb(build_glb(os.path.join(self.dir, "n_b.glb"), SQUARE_POS, SQUARE_IDX,
                                 normals=tilted, uvs=SQUARE_UV))
        self.assertNotEqual(a.digest(), b.digest())

    def test_the_digest_separates_uv_areas_either_side_of_the_gate_epsilon(self):
        """The key must be at least as sharp as the finest test applied to what it keys.

        Executed counterexample: UV areas of 5e-13 and 1.5e-12 straddle `uv_area_eps = 1e-12`, so
        they differ in `tangent_default_faces` — and the digest, which rounded UVs to 1e-7, gave
        them the same key. One of the two was then discarded as a duplicate, arbitrarily.
        """
        below = [[0, 0], [1e-6, 0], [0, 1e-6]]        # uv area 5e-13
        above = [[0, 0], [3e-6, 0], [0, 1e-6]]        # uv area 1.5e-12
        positions = [[0, 0, 0], [1, 0, 0], [0, 1, 0]]
        a = M.from_glb(build_glb(os.path.join(self.dir, "uv_below.glb"), positions, [0, 1, 2],
                                 uvs=below))
        b = M.from_glb(build_glb(os.path.join(self.dir, "uv_above.glb"), positions, [0, 1, 2],
                                 uvs=above))
        self.assertEqual(a.validity(self.gates)["tangent_default_faces"], 1)
        self.assertEqual(b.validity(self.gates)["tangent_default_faces"], 0)
        self.assertNotEqual(a.digest(), b.digest())

    def test_the_digest_ignores_index_order_but_not_geometry(self):
        """The cache key that collapses a decimation plateau — it must key on the MESH, not the file."""
        a = self.surface("d1.glb", SQUARE_POS, SQUARE_IDX, SQUARE_UV)
        b = self.surface("d2.glb", SQUARE_POS, [2, 0, 1, 0, 2, 3], SQUARE_UV)
        moved = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0.05]]
        c = self.surface("d3.glb", moved, SQUARE_IDX, SQUARE_UV)
        self.assertEqual(a.digest(), b.digest())
        self.assertNotEqual(a.digest(), c.digest())


class SurfaceIdentityTests(unittest.TestCase):
    """`same_surface`, the proof that the shipped L0 IS the source rather than merely near it."""

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="lod-identity-")

    def surface(self, name, positions, indices, uvs, normals=None):
        return M.from_glb(build_glb(
            os.path.join(self.dir, name), positions, indices, uvs=uvs, normals=normals
        ))

    def test_the_same_mesh_split_differently_is_the_same_surface(self):
        """The exporter splits corners by (position, normal, uv); that must not read as a change."""
        split_positions = [SQUARE_POS[i] for i in SQUARE_IDX]
        split_uvs = [SQUARE_UV[i] for i in SQUARE_IDX]
        a = self.surface("welded.glb", SQUARE_POS, SQUARE_IDX, SQUARE_UV)
        b = self.surface("split.glb", split_positions, list(range(6)), split_uvs)
        self.assertEqual(b.vert_count, 6)
        self.assertEqual(a.vert_count, 4)
        verdict, reason = M.same_surface(a, b)
        self.assertTrue(verdict, reason)

    def test_the_same_points_joined_differently_are_not_the_same_surface(self):
        """The case a vertex-distance check cannot see: identical points, the other diagonal."""
        a = self.surface("diag1.glb", SQUARE_POS, [0, 1, 2, 0, 2, 3], SQUARE_UV)
        b = self.surface("diag2.glb", SQUARE_POS, [0, 1, 3, 1, 2, 3], SQUARE_UV)
        # Both meshes carry exactly the same points and the same triangle count, so the vertex
        # distance a vertex-only check computes is zero in both directions. Only the topology
        # differs, and only a topology comparison can see it.
        np.testing.assert_allclose(np.sort(a.verts, axis=0), np.sort(b.verts, axis=0))
        self.assertEqual(a.tri_count, b.tri_count)
        verdict, reason = M.same_surface(a, b)
        self.assertFalse(verdict, "re-triangulating across the other diagonal is a different mesh")
        self.assertIn("joined differently", reason)

    def test_a_moved_vertex_is_not_the_same_surface(self):
        moved = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0.05]]
        a = self.surface("base.glb", SQUARE_POS, SQUARE_IDX, SQUARE_UV)
        b = self.surface("moved.glb", moved, SQUARE_IDX, SQUARE_UV)
        verdict, reason = M.same_surface(a, b)
        self.assertFalse(verdict, reason)

    def test_a_different_triangle_count_is_not_the_same_surface(self):
        a = self.surface("two.glb", SQUARE_POS, SQUARE_IDX, SQUARE_UV)
        b = self.surface("one.glb", SQUARE_POS, [0, 1, 2], SQUARE_UV)
        verdict, reason = M.same_surface(a, b)
        self.assertFalse(verdict)
        self.assertIn("triangle counts differ", reason)


class BranchAndBoundTests(unittest.TestCase):
    """The bracket, driven with a synthetic field whose true maximum is known independently.

    THIS IS THE TEST THAT WAS MISSING, and its absence is why the bug survived: the only way to call
    the bound was through a mesh pair whose true worst case nobody knew, so "the upper bound" could
    be anything and every number still looked plausible. An adversarial review found it by probing
    with a 1-Lipschitz field that has an INTERIOR maximum — a maximum at no vertex, which is exactly
    the case a corner-sampled search can miss and the covering-radius bound exists to cover.
    """

    @staticmethod
    def cone(peak, height):
        """d(p) = max(0, height - |p - peak|). 1-Lipschitz, maximum `height`, attained only at peak."""
        peak = np.asarray(peak, dtype=np.float64)

        def distance_at(_key, point):
            return max(0.0, height - float(np.linalg.norm(np.asarray(point) - peak)))

        return distance_at

    def seeds_for(self, corners, distance_at):
        return [(corners, tuple(distance_at(i, corners[i]) for i in range(3)))]

    def test_the_upper_bound_brackets_an_interior_maximum(self):
        corners = (
            np.array([0.0, 0.0, 0.0]), np.array([1.0, 0.0, 0.0]), np.array([0.0, 1.0, 0.0])
        )
        truth = 0.4
        distance_at = self.cone((0.3, 0.3, 0.0), truth)
        lower, upper = M.branch_and_bound(
            self.seeds_for(corners, distance_at), distance_at,
            tol=1e-4, max_nodes=20000, rel_tol=0.0,
        )
        self.assertLessEqual(lower, truth + 1e-9, "the sampled lower bound cannot exceed the truth")
        self.assertGreaterEqual(
            upper, truth - 1e-9,
            f"upper bound {upper} is BELOW the true maximum {truth} — it is not an upper bound",
        )

    def test_a_starved_search_still_returns_a_true_upper_bound(self):
        """Cut off at one node, the bracket must be wide and honest rather than narrow and wrong."""
        corners = (
            np.array([0.0, 0.0, 0.0]), np.array([1.0, 0.0, 0.0]), np.array([0.0, 1.0, 0.0])
        )
        truth = 0.4
        distance_at = self.cone((0.3, 0.3, 0.0), truth)
        lower, upper = M.branch_and_bound(
            self.seeds_for(corners, distance_at), distance_at,
            tol=1e-4, max_nodes=1, rel_tol=0.0,
        )
        self.assertLessEqual(lower, truth)
        self.assertGreaterEqual(upper, truth)

    def test_pruned_patches_are_folded_into_the_upper_bound(self):
        """A tolerance that prunes everything must widen the bound, not silently discard it.

        With a tolerance far larger than the answer, every child is pruned on its first expansion.
        The returned upper bound must still cover the truth — the old code returned `best` here,
        which is the sampled maximum and nothing more.
        """
        corners = (
            np.array([0.0, 0.0, 0.0]), np.array([1.0, 0.0, 0.0]), np.array([0.0, 1.0, 0.0])
        )
        truth = 0.4
        distance_at = self.cone((0.3, 0.3, 0.0), truth)
        lower, upper = M.branch_and_bound(
            self.seeds_for(corners, distance_at), distance_at,
            tol=1.0, max_nodes=20000, rel_tol=0.0,
        )
        self.assertGreaterEqual(upper, truth)
        self.assertGreater(upper, lower, "a pruned search cannot claim a closed bracket")

    def test_a_converged_bracket_finds_the_interior_maximum(self):
        corners = (
            np.array([0.0, 0.0, 0.0]), np.array([1.0, 0.0, 0.0]), np.array([0.0, 1.0, 0.0])
        )
        truth = 0.4
        distance_at = self.cone((0.3, 0.3, 0.0), truth)
        lower, upper = M.branch_and_bound(
            self.seeds_for(corners, distance_at), distance_at,
            tol=1e-3, max_nodes=200000, rel_tol=0.0,
        )
        self.assertAlmostEqual(lower, truth, places=2)
        self.assertLessEqual(upper - lower, 0.05)


class GateParityTests(unittest.TestCase):
    """Generation and verification enforce the SAME gates, and every declared limit gates something.

    Twice a gate existed at generation and was simply absent from the verifier — `components_must_
    match` was compared when a level was cut and never again, and the sliver floor was re-derived
    against a threshold the manifest supplied for itself. Both are the same bug: two lists that were
    supposed to agree.

    There is one list now. `measure.validity_gate_failures` is what generation calls and what
    verification calls, so parity holds by construction; these tests hold the remaining edge — that
    the list actually consults every limit the configuration declares, and that both callers really
    are calling it.
    """

    @staticmethod
    def clean_validity():
        return {
            "tris": 10, "verts": 30, "components": 1, "duplicate_faces": 0, "nonfinite_attrs": 0,
            "orientation_flips": 0, "nonmanifold_edges": 0, "boundary_edges": 0,
            "slivers_below_floor": 0, "tangent_default_faces": 0, "tangent_default_verts": 0,
            "min_altitude_m": 0.01, "min_altitude_floor_m": 0.001, "min_tri_area_mm2": 1.0,
            "origin_radius_m": 0.4, "bbox_mm": [1.0, 1.0, 1.0],
            "baked_tangents": 30, "degenerate_tangents": 0, "min_tangent_length": 1.0,
        }

    def test_every_declared_limit_gates_something(self):
        """A `max_*` limit nobody consults is a threshold that reads as protection and is not."""
        declared = {key for key in CONFIG.GATES if key.startswith("max_")}
        consulted = {limit for _counter, limit, _description in M.STRUCTURAL_GATES}
        self.assertEqual(
            declared - consulted, set(),
            "these limits are declared in config.GATES and consulted by nothing",
        )

    def test_each_gate_in_the_table_actually_fires(self):
        for counter, limit, _description in M.STRUCTURAL_GATES:
            with self.subTest(gate=counter):
                validity = self.clean_validity()
                validity[counter] = CONFIG.GATES[limit] + 1
                failures = M.validity_gate_failures(validity, self.clean_validity(), CONFIG.GATES)
                self.assertTrue(failures, f"{counter} over its limit produced no failure")

    def test_the_component_gate_fires(self):
        validity = self.clean_validity()
        validity["components"] = 2
        failures = M.validity_gate_failures(validity, self.clean_validity(), CONFIG.GATES)
        self.assertTrue(any("component count" in f for f in failures), failures)

    def test_the_sliver_gate_fires(self):
        validity = self.clean_validity()
        validity["slivers_below_floor"] = 1
        failures = M.validity_gate_failures(validity, self.clean_validity(), CONFIG.GATES)
        self.assertTrue(any("altitude floor" in f for f in failures), failures)

    def test_the_tangent_presence_gate_fires(self):
        validity = self.clean_validity()
        validity["baked_tangents"] = 0
        failures = M.validity_gate_failures(validity, self.clean_validity(), CONFIG.GATES)
        self.assertTrue(any("baked tangents" in f for f in failures), failures)

    def test_the_verifier_and_generator_share_the_gate_function(self):
        import inspect

        import chain
        source = inspect.getsource(chain)
        self.assertIn("M.validity_gate_failures(", source,
                      "the verifier must call the shared gate list")
        generate_source = open(
            os.path.join(os.path.dirname(os.path.abspath(M.__file__)), "generate.py")
        ).read()
        self.assertIn("M.validity_gate_failures(", generate_source,
                      "generation must call the shared gate list")
        self.assertNotIn("def validity_failures(", generate_source,
                         "generation must not keep a private copy of the gate list")


class BisectionTests(unittest.TestCase):
    """WHERE THE ENUMERATION'S CONTRACT IS ACTUALLY ESTABLISHED.

    The enumeration cannot prove its oracle honest — it can only ask it, and a consistently wrong
    oracle answers consistently. So the property "returns the greatest realizable count at or below
    the budget" is proven HERE, where it is arithmetic rather than a conversation: over synthetic
    staircases whose answer is computed independently from the staircase's own definition.
    """

    @staticmethod
    def staircase(steps):
        """A monotone non-decreasing count(ratio), as the decimate modifier's is.

        `steps` is a list of (ratio_threshold, count), ascending. The realizable set is the counts.
        """
        ordered = sorted(steps)

        def evaluate(ratio):
            count = ordered[0][1]
            for threshold, value in ordered:
                if ratio >= threshold:
                    count = value
            return count

        return evaluate, [value for _t, value in ordered]

    def truth(self, realizable, budget):
        under = [value for value in realizable if value <= budget]
        return max(under) if under else None

    def test_the_bisection_returns_the_greatest_count_under_every_budget(self):
        """EVERY budget in the range, not a sample of them — the name has to be true."""
        evaluate, realizable = self.staircase(
            [(0.0, 12), (0.1, 40), (0.25, 41), (0.4, 190), (0.55, 191), (0.7, 640), (0.9, 1661)]
        )
        for budget in range(0, 1700):
            count, _ratio = M.bisect_to_budget(evaluate, budget)
            self.assertEqual(count, self.truth(realizable, budget), f"at budget {budget}")

    def test_a_plateau_narrower_than_a_fixed_halving_count_is_still_found(self):
        """The executed counterexample against the retired fixed-28-halving bisection.

        A 1e-10-wide plateau at 999 is invisible to 28 halvings, which return 100 for a budget of
        1000. Exhausting the bracket removes the precondition rather than assuming Blender's ratio
        quantisation is coarse enough to make it safe — nobody had measured that.
        """
        # 0.3, not 0.5: a plateau sitting ON a dyadic rational is found by the first probe,
        # which would make the counterexample accidental rather than structural.
        evaluate, realizable = self.staircase([(0.0, 100), (0.3, 999), (0.3 + 1e-10, 2000)])
        self.assertEqual(self.truth(realizable, 1000), 999)
        self.assertEqual(M.bisect_to_budget(evaluate, 1000)[0], 999)

        def fixed_halvings(budget, halvings):
            low, high, best = 0.0, 1.0, None
            for _ in range(halvings):
                middle = (low + high) / 2.0
                count = evaluate(middle)
                if count <= budget:
                    best = (count, middle)
                    low = middle
                else:
                    high = middle
            return best[0] if best else None

        self.assertEqual(fixed_halvings(1000, 28), 100, "28 halvings miss the narrow plateau")
        self.assertEqual(fixed_halvings(1000, 40), 999, "more halvings find it — hence no fixed N")

    def test_adjacent_steps_are_resolved(self):
        """Steps one triangle apart — the case a coarse bracket would merge."""
        evaluate, realizable = self.staircase([(0.0, 100), (0.5, 101), (0.5000001, 102)])
        for budget, expected in ((100, 100), (101, 101), (102, 102), (150, 102)):
            with self.subTest(budget=budget):
                self.assertEqual(M.bisect_to_budget(evaluate, budget)[0], expected)

    def test_the_top_step_at_exactly_ratio_one_is_found(self):
        """`(low + high) / 2` never produces 1.0, so a step living only there was invisible.

        Executed counterexample: `evaluate(r) = 2000 if r == 1.0 else 100` returned 100 for a budget
        of 2000. Monotonicity makes `evaluate(1.0)` the largest count there is — if it fits the
        budget it IS the answer, and the search never had to start.
        """
        seen = []

        def evaluate(ratio):
            seen.append(ratio)
            return 2000 if ratio == 1.0 else 100

        count, ratio = M.bisect_to_budget(evaluate, 2000)
        self.assertEqual(count, 2000)
        self.assertEqual(ratio, 1.0)
        self.assertIn(1.0, seen, "the ceiling must actually be evaluated")

    def test_both_endpoints_are_evaluated(self):
        seen = []

        def evaluate(ratio):
            seen.append(ratio)
            return int(ratio * 1000)

        M.bisect_to_budget(evaluate, 500)
        self.assertIn(0.0, seen)
        self.assertIn(1.0, seen)

    def test_a_budget_below_the_floor_returns_nothing(self):
        evaluate, _ = self.staircase([(0.0, 194), (0.5, 800)])
        self.assertEqual(M.bisect_to_budget(evaluate, 100), (None, None))

    def test_the_returned_ratio_reproduces_the_returned_count(self):
        evaluate, _ = self.staircase([(0.0, 12), (0.3, 300), (0.6, 900), (0.85, 1661)])
        count, ratio = M.bisect_to_budget(evaluate, 700)
        self.assertEqual(count, 300)
        self.assertEqual(evaluate(ratio), count)

    def test_a_one_percent_early_stop_would_break_the_property(self):
        """The regression this pipeline actually shipped, shown breaking the contract.

        Reproduces the retired early stop against the same staircase the honest bisection is held
        to, so the difference between "about this big" and "the greatest at or below" is a measured
        fact in the suite rather than a remembered story.
        """
        # Two realizable counts inside 1 % of each other: the early stop takes the first one it
        # lands on and never looks up. That is precisely how outputs went missing.
        evaluate, realizable = self.staircase(
            [(0.0, 100), (0.2, 995), (0.9, 999), (0.95, 2000)]
        )

        def early_stopping(budget):
            low, high, best = 0.0, 1.0, None
            for _ in range(28):
                middle = (low + high) / 2.0
                count = evaluate(middle)
                if count <= budget:
                    best = (count, middle)
                    low = middle
                else:
                    high = middle
                if best and budget * 0.99 <= best[0] <= budget:
                    break
            return best if best else (None, None)

        disagreements = [
            budget for budget in range(100, 2001)
            if early_stopping(budget)[0] != self.truth(realizable, budget)
        ]
        self.assertTrue(
            disagreements,
            "the early stop must be shown to violate the property the enumeration relies on",
        )
        self.assertIn(1000, disagreements, "budget 1000 realizes 999, the early stop returns 995")
        for budget in range(100, 2001):
            self.assertEqual(
                M.bisect_to_budget(evaluate, budget)[0], self.truth(realizable, budget),
                f"the converged bisection must hold at budget {budget}",
            )


class StaircaseEnumerationTests(unittest.TestCase):
    """The enumeration walk, against synthetic decimators whose realizable set is known.

    THE WALK IS THE PART THAT WAS WRONG, twice. First the oracle under it stopped within 1 % of its
    budget, so the `reached - 1` jump stepped over outputs. Then the guard meant to catch that was
    itself unsound: it accepted any count already in the output set, which `f(B) = B - 1` satisfies
    while enumerating half the staircase. The oracle is injected here so both the walk and its
    guard are driven directly, with no Blender.
    """

    @staticmethod
    def exact(realizable, calls=None, cleaned=None):
        """The contract: the greatest realizable count at or below the budget.

        Returns `(step_count, shipped_key)`. `cleaned` maps a step count to what would ship after
        the cleanup pass, so the two-domain handling can be exercised.
        """
        ordered = sorted(realizable)

        def probe(budget):
            if calls is not None:
                calls.append(budget)
            under = [value for value in ordered if value <= budget]
            if not under:
                return None
            step = under[-1]
            return step, (cleaned or {}).get(step, step)

        return probe

    def test_every_realizable_output_is_found(self):
        realizable = {194, 200, 231, 316, 400, 583, 700, 855, 1000, 1661}
        outputs = sorted(M.enumerate_staircase(194, 1661, self.exact(realizable)))
        self.assertEqual(outputs, sorted(realizable))

    def test_one_decimation_per_output_plus_the_contract_checks(self):
        """The `reached - 1` jump is what makes exhaustive affordable; hold it to that."""
        realizable = {100, 250, 400, 620, 900, 1500}
        calls = []
        outputs = sorted(M.enumerate_staircase(100, 1500, self.exact(realizable, calls)))
        self.assertEqual(outputs, sorted(realizable))
        # One call per output for the walk, plus one per output for the idempotence check.
        self.assertEqual(len(calls), 2 * len(realizable))

    def test_the_off_by_one_oracle_is_caught(self):
        """Codex's counterexample: `f(B) = max(1, B-1)` over 1..10 enumerated only [1,3,5,7,9].

        Every answer it gives IS in the set it built, so a membership guard passes it. Asking
        `probe(9) == 9` — the idempotence the contract implies for any realizable value — refuses
        it immediately.
        """
        def off_by_one(budget):
            step = max(1, budget - 1)
            return step, step

        with self.assertRaises(M.EnumerationError) as caught:
            M.enumerate_staircase(1, 10, off_by_one, spot_checks=48, seed=1)
        self.assertIn("asked for a budget of exactly", str(caught.exception))

    def test_a_one_percent_stop_is_caught(self):
        """The original bug: a bisection that lands NEAR the greatest realizable count."""
        realizable = sorted({100, 300, 305, 310, 500, 505, 900, 1500})

        def sloppy(budget):
            under = [value for value in realizable if value <= budget]
            if not under:
                return None
            step = under[0] if len(under) > 1 and budget > 400 else under[-1]
            return step, step

        with self.assertRaises(M.EnumerationError):
            M.enumerate_staircase(100, 1500, sloppy, spot_checks=64, seed=7)

    def test_an_oracle_that_overshoots_its_budget_is_refused(self):
        with self.assertRaises(M.EnumerationError):
            M.enumerate_staircase(100, 500, lambda budget: (budget + 10, budget + 10))

    def test_the_output_limit_refuses_rather_than_filling_memory(self):
        realizable = set(range(100, 900))
        with self.assertRaises(M.EnumerationError) as caught:
            M.enumerate_staircase(100, 899, self.exact(realizable), max_outputs=50)
        message = str(caught.exception)
        self.assertIn("refusing to hold them all", message)
        # The remediation must name a setting that EXISTS.
        # The remediation must name a setting that EXISTS — it named `config.MAX_ENUMERATED_OUTPUTS`,
        # which never has.
        self.assertIn("max_enumerated_outputs", message)
        self.assertIn("SEARCH_LIMITS", message)
        self.assertNotIn("config.MAX_ENUMERATED_OUTPUTS", message)
        self.assertIn("max_enumerated_outputs", CONFIG.SEARCH_LIMITS)

    def test_a_floor_only_staircase_terminates(self):
        outputs = sorted(M.enumerate_staircase(194, 1661, self.exact({194})))
        self.assertEqual(outputs, [194])

    def test_a_consistently_capped_oracle_is_NOT_detected(self):
        """THE TRUST BOUNDARY, pinned as a test so nobody re-derives a stronger claim from green.

        `lambda b: (min(b, 5), min(b, 5))` answers every question this enumeration can ask, exactly
        as the contract requires, while concealing everything above 5. It passes — and it SHOULD
        pass here, because an enumeration interrogating its own oracle is circular and no amount of
        probing changes that. The contract is established in `BisectionTests`, over staircases whose
        answers are known independently; the spot checks below only catch an oracle contradicting
        ITSELF. If this test ever starts failing, the claim has quietly grown and the docstrings
        need re-reading.
        """
        capped = M.enumerate_staircase(
            1, 10, lambda b: (min(b, 5), min(b, 5)), spot_checks=48, seed=20260802
        )
        self.assertEqual(sorted(capped), [1, 2, 3, 4, 5])

    def test_two_meshes_with_the_same_triangle_count_are_both_kept(self):
        """Candidates are keyed by GEOMETRY, not by triangle count.

        Keying by count assumed one mesh per count. Two cleaned meshes can carry the same count and
        different geometry, and the loser was dropped silently — possibly the only one that met a
        rung. Here two decimator steps clean down to the same 400 triangles with different shapes.
        """
        realizable = sorted({100, 402, 405, 900})
        # Both 402 and 405 clean to 400 triangles, but they are different meshes.
        keys = {100: "d-100", 402: "d-400-a", 405: "d-400-b", 900: "d-900"}

        def probe(budget):
            under = [value for value in realizable if value <= budget]
            if not under:
                return None
            step = under[-1]
            return step, keys[step]

        by_key = M.enumerate_staircase(100, 900, probe, spot_checks=16, seed=11)
        self.assertEqual(sorted(by_key), ["d-100", "d-400-a", "d-400-b", "d-900"])
        self.assertEqual(len(by_key), 4, "neither same-sized mesh may be discarded")

    def test_cleanup_that_removes_a_face_does_not_lose_the_candidate(self):
        """The two count domains, kept apart.

        The walk steps on the DECIMATOR's count; the cache is keyed by what SHIPS after cleanup.
        They differ the moment cleanup dissolves a degenerate face, and the earlier code stepped on
        one while looking candidates up by the other — a `KeyError`, or a silently dropped
        candidate, on that day. Here cleanup takes a face off two of the four outputs.
        """
        realizable = {100, 250, 400, 900}
        cleaned = {250: 249, 900: 898}
        by_key = M.enumerate_staircase(
            100, 900, self.exact(realizable, cleaned=cleaned), spot_checks=16, seed=5
        )
        outputs = sorted(by_key)
        self.assertEqual(outputs, [100, 249, 400, 898])
        # Every key the search will index by must resolve — this is the KeyError, caught.
        for key in outputs:
            self.assertIn(key, by_key)
        table = {key: 1.0 for key in outputs}
        best = M.pareto_minimal(outputs, lambda tris, _t: {"lo_mm": table[tris],
                                                          "up_mm": table[tris]}, 2.0)
        self.assertEqual(best["up_mm"], 1.0)


class ParetoSearchTests(unittest.TestCase):
    """The search, against a feasibility curve with a hole in it."""

    @staticmethod
    def brackets(table):
        def deviation_for(tris, _target_mm):
            value = table[tris]
            return {"lo_mm": value, "up_mm": value}

        return deviation_for

    def test_a_lower_feasible_island_is_found_through_the_real_enumeration(self):
        """End to end: the walk discovers the outputs, the scan picks the minimum.

        Non-monotone by construction — 300 clears the target, 400 and 500 do not, 600 does. A
        bisection lands on 600, walks down, fails at 500 and stops, shipping 600 triangles and
        comparing the WRONG incumbent against the sparse-chain shed threshold. Nothing here is
        pre-supplied: the output set comes out of `enumerate_staircase` driving a synthetic
        decimator, which is the seam the first version of this test skipped over.
        """
        table = {300: 5.0, 400: 12.0, 500: 11.0, 600: 6.0, 800: 4.0}
        outputs = sorted(M.enumerate_staircase(
            300, 800, StaircaseEnumerationTests.exact(set(table)), spot_checks=32, seed=3
        ))
        self.assertEqual(outputs, [300, 400, 500, 600, 800])
        best = M.pareto_minimal(outputs, self.brackets(table), 8.0)
        self.assertIsNotNone(best)
        self.assertEqual(best["up_mm"], 5.0)

    def test_the_minimum_is_returned_when_the_curve_is_ordinary(self):
        table = {200: 40.0, 400: 20.0, 800: 5.0, 1600: 1.0}
        best = M.pareto_minimal(sorted(table), self.brackets(table), 6.0)
        self.assertEqual(best["up_mm"], 5.0)

    def test_no_feasible_output_returns_none(self):
        table = {200: 40.0, 400: 20.0}
        self.assertIsNone(M.pareto_minimal(sorted(table), self.brackets(table), 1.0))

    def test_acceptance_is_on_the_upper_bound_not_the_sample(self):
        """A candidate whose lower bound clears the rung but whose bracket does not is refused."""
        def deviation_for(tris, _target):
            return {200: {"lo_mm": 3.0, "up_mm": 9.0}, 400: {"lo_mm": 2.0, "up_mm": 5.0}}[tris]

        best = M.pareto_minimal([200, 400], deviation_for, 6.0)
        self.assertEqual(best["up_mm"], 5.0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
