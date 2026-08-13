"""The lane's math and its refusals: every named refusal fires, and every declared number holds.

    python3 scripts/lod/test_refusals.py

"Nothing silently passes" (ADR 0033 §10) is a claim about code that almost never runs — the refusal
paths only fire on an asset nobody has authored yet. So they are exercised here against synthetic
GLBs built byte by byte, rather than trusted because they are written down. numpy only; no Blender,
which is why this suite runs on every push (the BVH-backed deviation search does need Blender and is
exercised by the tank build itself).

`ProjectionTests` at the end of this file holds what the ladder's arithmetic owes, independent of
any one asset (ADR 0035).
"""

import json
import math
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
        report = self.surface("clean.glb", SQUARE_POS, SQUARE_IDX, SQUARE_UV).validity()
        self.assertEqual(report["duplicate_faces"], 0)
        self.assertEqual(report["orientation_flips"], 0)
        self.assertEqual(report["nonfinite_attrs"], 0)
        self.assertEqual(report["empty_surfaces"], 0)
        self.assertEqual(report["components"], 1)

    def test_a_duplicate_face_is_counted(self):
        report = self.surface(
            "dup.glb", SQUARE_POS, SQUARE_IDX + [0, 1, 2], SQUARE_UV
        ).validity()
        self.assertEqual(report["duplicate_faces"], 1)

    def test_a_flipped_winding_is_counted(self):
        """The two triangles share edge 0-2; reversing one makes both traverse it the same way."""
        report = self.surface(
            "flip.glb", SQUARE_POS, [0, 1, 2, 2, 0, 3][:3] + [0, 3, 2], SQUARE_UV
        ).validity()
        self.assertEqual(report["orientation_flips"], 1)

    def test_two_disjoint_squares_count_as_two_components(self):
        positions = SQUARE_POS + [[10, 0, 0], [11, 0, 0], [11, 1, 0], [10, 1, 0]]
        indices = SQUARE_IDX + [4, 5, 6, 4, 6, 7]
        uvs = SQUARE_UV + SQUARE_UV
        self.assertEqual(
            self.surface("two.glb", positions, indices, uvs).validity()["components"], 2
        )

    def test_a_nonfinite_position_is_counted(self):
        positions = [[0, 0, 0], [1, 0, 0], [float("nan"), 1, 0], [0, 1, 0]]
        report = self.surface("nan.glb", positions, SQUARE_IDX, SQUARE_UV).validity()
        self.assertGreater(report["nonfinite_attrs"], 0)

    def test_three_faces_on_one_edge_are_counted_but_no_longer_gated(self):
        """A T-junction of three faces, counted as a DIAGNOSTIC (ADR 0036 §4).

        Manifoldness is the armour pipeline's law, not this lane's: here the deviation bound polices
        the decimator by construction. The counter stays because the armour lane cares and because a
        regression should be legible; what left is the refusal built on it.
        """
        positions = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1]]
        indices = [0, 1, 2, 0, 1, 3, 0, 1, 4]
        uvs = [[0, 0], [1, 0], [1, 1], [0, 1], [0.5, 0.5]]
        surface = self.surface("nonmanifold.glb", positions, indices, uvs)
        report = surface.validity()
        self.assertEqual(report["nonmanifold_edges"], 1)
        self.assertNotIn("max_nonmanifold_edges", self.gates)
        self.assertEqual(
            M.validity_gate_failures(report, report, self.gates), [],
            "a non-manifold edge is recorded and no longer refuses a level",
        )

    def test_an_empty_surface_is_refused(self):
        """Non-empty is the one gate ADR 0036 §4 ADDED: a collapse to nothing is not a level."""
        report = dict(self.surface("clean.glb", SQUARE_POS, SQUARE_IDX, SQUARE_UV).validity())
        report["empty_surfaces"] = 1
        failures = M.validity_gate_failures(report, report, self.gates)
        self.assertTrue(any("empty surface" in f for f in failures), failures)

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
            surface.validity()["origin_radius_m"], 5.0, places=6
        )

    def test_the_digest_separates_meshes_that_differ_only_in_uv(self):
        """Codex's counterexample: same positions and topology, one with a collapsed UV.

        The candidate key must be at least as sharp as every test that could be applied to what it
        keys, INCLUDING tests not yet written — which is why nothing in it is quantised. Two meshes
        that differ only in a UV are two meshes; keying them together discards one arbitrarily.
        """
        good = self.surface("uv_good.glb", SQUARE_POS, SQUARE_IDX, SQUARE_UV)
        collapsed = self.surface(
            "uv_collapsed.glb", SQUARE_POS, SQUARE_IDX, [[0, 0], [0, 0], [0, 0], [0, 0]]
        )
        self.assertNotEqual(good.digest(), collapsed.digest())

    def test_the_digest_separates_meshes_that_differ_only_in_winding(self):
        """Same points, one triangle wound the other way: `orientation_flips` 0 against 1."""
        good = self.surface("wind_good.glb", SQUARE_POS, [0, 1, 2, 0, 2, 3], SQUARE_UV)
        flipped = self.surface("wind_flipped.glb", SQUARE_POS, [0, 1, 2, 0, 3, 2], SQUARE_UV)
        self.assertEqual(good.validity()["orientation_flips"], 0)
        self.assertEqual(flipped.validity()["orientation_flips"], 1)
        self.assertNotEqual(good.digest(), flipped.digest())

    def test_the_digest_separates_meshes_that_differ_only_in_normals(self):
        normals = [[0, 0, 1]] * 4
        tilted = [[0, 0.5, 0.866]] * 4
        a = M.from_glb(build_glb(os.path.join(self.dir, "n_a.glb"), SQUARE_POS, SQUARE_IDX,
                                 normals=normals, uvs=SQUARE_UV))
        b = M.from_glb(build_glb(os.path.join(self.dir, "n_b.glb"), SQUARE_POS, SQUARE_IDX,
                                 normals=tilted, uvs=SQUARE_UV))
        self.assertNotEqual(a.digest(), b.digest())

    def test_the_digest_is_not_quantised_below_any_epsilon_a_gate_could_use(self):
        """UVs a picometre apart are two candidates, and the key has to say so.

        Executed counterexample from when the digest rounded UVs to 1e-7: two meshes whose UV areas
        were 5e-13 and 1.5e-12 hashed identically, and one was discarded as a duplicate. Keeping the
        bits is the cheapest way to stay sharper than gates nobody has written yet.
        """
        below = [[0, 0], [1e-6, 0], [0, 1e-6]]
        above = [[0, 0], [3e-6, 0], [0, 1e-6]]
        positions = [[0, 0, 0], [1, 0, 0], [0, 1, 0]]
        a = M.from_glb(build_glb(os.path.join(self.dir, "uv_below.glb"), positions, [0, 1, 2],
                                 uvs=below))
        b = M.from_glb(build_glb(os.path.join(self.dir, "uv_above.glb"), positions, [0, 1, 2],
                                 uvs=above))
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
        """d(p) = max(0, height - |p - peak|). 1-Lipschitz, maximum `height`, attained only at peak.

        Returns the loop's `(distance, tag)` pair. The tag is what a BVH probe would answer with the
        triangle it hit; a synthetic field has no triangles, so it answers -1 and the default
        covering-radius bound is the only one available — which is exactly the configuration that
        makes this test a test OF THE LOOP rather than of the bound beside it.
        """
        peak = np.asarray(peak, dtype=np.float64)

        def probe(_key, point):
            return max(0.0, height - float(np.linalg.norm(np.asarray(point) - peak))), -1

        return probe

    def seeds_for(self, corners, probe):
        return (
            np.array([corners]),
            np.array([[probe(i, corners[i])[0] for i in range(3)]]),
            np.array([[-1, -1, -1]]),
        )

    def test_the_upper_bound_brackets_an_interior_maximum(self):
        corners = (
            np.array([0.0, 0.0, 0.0]), np.array([1.0, 0.0, 0.0]), np.array([0.0, 1.0, 0.0])
        )
        truth = 0.4
        probe = self.cone((0.3, 0.3, 0.0), truth)
        lower, upper = M.branch_and_bound(
            *self.seeds_for(corners, probe), probe,
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
        probe = self.cone((0.3, 0.3, 0.0), truth)
        lower, upper = M.branch_and_bound(
            *self.seeds_for(corners, probe), probe,
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
        probe = self.cone((0.3, 0.3, 0.0), truth)
        lower, upper = M.branch_and_bound(
            *self.seeds_for(corners, probe), probe,
            tol=1.0, max_nodes=20000, rel_tol=0.0,
        )
        self.assertGreaterEqual(upper, truth)
        self.assertGreater(upper, lower, "a pruned search cannot claim a closed bracket")

    def test_a_converged_bracket_finds_the_interior_maximum(self):
        corners = (
            np.array([0.0, 0.0, 0.0]), np.array([1.0, 0.0, 0.0]), np.array([0.0, 1.0, 0.0])
        )
        truth = 0.4
        probe = self.cone((0.3, 0.3, 0.0), truth)
        lower, upper = M.branch_and_bound(
            *self.seeds_for(corners, probe), probe,
            tol=1e-3, max_nodes=200000, rel_tol=0.0,
        )
        self.assertAlmostEqual(lower, truth, places=2)
        self.assertLessEqual(upper - lower, 0.05)


class GateParityTests(unittest.TestCase):
    """Every caller enforces the SAME gates, and every declared limit gates something.

    Twice a gate existed at one caller and was simply absent from another — `components_must_match`
    was compared when a level was cut and never again, and the checks were re-derived against a
    threshold the corpus supplied for itself. Both are the same bug: two lists that were supposed to
    agree.

    There is one list now. `measure.validity_gate_failures` is what the search's pre-filter calls
    and what certification calls, so parity holds by construction; these tests hold the remaining
    edge — that the list actually consults every limit the configuration declares, and that every
    caller really is calling it.
    """

    @staticmethod
    def clean_validity():
        return {
            "tris": 10, "verts": 30, "components": 1, "duplicate_faces": 0, "nonfinite_attrs": 0,
            "orientation_flips": 0, "empty_surfaces": 0, "nonmanifold_edges": 0,
            "boundary_edges": 0, "min_tri_area_mm2": 1.0,
            "origin_radius_m": 0.4, "radius_m": 0.4, "bbox_mm": [1.0, 1.0, 1.0],
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

    def test_the_degeneracy_gate_the_config_claims_actually_fires(self):
        """The gate list has claimed "non-degenerate" since ADR 0036 §4 and enforced nothing.

        `min_tri_area_mm2` was measured on every level, recorded in every manifest, and compared
        against no threshold at all — the exact shape of "a counter nothing consults is decoration"
        that this file's own gate table exists to prevent. The floor is the cleanup pass's
        coincidence distance squared, so it is scale-free and is not a second declared number.
        """
        validity = self.clean_validity()
        diagonal_mm = math.sqrt(sum(v * v for v in validity["bbox_mm"]))
        floor = (CONFIG.GATES["cleanup_dissolve_frac_of_diag"] * diagonal_mm) ** 2

        validity["min_tri_area_mm2"] = floor * 0.5
        failures = M.validity_gate_failures(validity, self.clean_validity(), CONFIG.GATES)
        self.assertTrue(any("degeneracy floor" in f for f in failures), failures)

        validity["min_tri_area_mm2"] = floor
        self.assertTrue(
            M.validity_gate_failures(validity, self.clean_validity(), CONFIG.GATES),
            "a face exactly AT the floor is degenerate — the comparison is inclusive",
        )

        validity["min_tri_area_mm2"] = floor * 1.001
        self.assertEqual(
            M.validity_gate_failures(validity, self.clean_validity(), CONFIG.GATES), [],
            "and a face above it is not",
        )

    def test_the_degeneracy_floor_is_scale_free(self):
        """A metre-wide part and a kilometre-wide one are held to the same RELATIVE flatness.

        An absolute area floor would refuse small parts and wave large ones through, which is the
        failure the whole ladder is built to avoid — every threshold in this lane is a fraction of
        the mesh's own diagonal.
        """
        small = self.clean_validity()
        small["bbox_mm"] = [10.0, 10.0, 10.0]
        large = self.clean_validity()
        large["bbox_mm"] = [10000.0, 10000.0, 10000.0]
        eps = CONFIG.GATES["cleanup_dissolve_frac_of_diag"]
        for record in (small, large):
            diagonal_mm = math.sqrt(sum(v * v for v in record["bbox_mm"]))
            record["min_tri_area_mm2"] = ((eps * diagonal_mm) ** 2) * 4.0
            self.assertEqual(
                M.validity_gate_failures(record, record, CONFIG.GATES), [],
                f"a face four times the floor passes at every scale: {record['bbox_mm']}",
            )
            record["min_tri_area_mm2"] = ((eps * diagonal_mm) ** 2) * 0.25
            self.assertTrue(
                M.validity_gate_failures(record, record, CONFIG.GATES),
                f"and a quarter of it fails at every scale: {record['bbox_mm']}",
            )

    def test_the_gates_the_adr_retired_are_gone_from_the_table(self):
        """ADR 0036 §4, held as a fact rather than as prose: these counters no longer refuse.

        A deleted gate that quietly comes back is the same class of drift as a gate that quietly
        goes away — both change what the corpus means without anyone deciding. The counters
        themselves are still measured, which is why this asserts against the GATE table.
        """
        consulted = {counter for counter, _limit, _description in M.STRUCTURAL_GATES}
        for retired in ("nonmanifold_edges", "tangent_default_faces", "tangent_default_verts",
                        "degenerate_tangents", "baked_tangents"):
            self.assertNotIn(retired, consulted)
        for retired in ("max_nonmanifold_edges", "max_tangent_default_faces",
                        "max_tangent_default_verts", "max_degenerate_tangents",
                        "tangent_min_length", "uv_area_eps"):
            self.assertNotIn(retired, CONFIG.GATES)

    def test_no_caller_keeps_a_private_gate_list(self):
        """A TEXT TRIPWIRE, said plainly — the name is a claim this mechanism cannot prove alone.

        WHAT IT CHECKS: every caller really does call the shared function, generation keeps no
        `validity_failures` of its own, and NO FILE SPELLS a `max_*` limit's name at all, in either
        quote style. Quoting the key is how every ordinary reading is written: `GATES["x"]`,
        `GATES['x']`, `.get("x")`, `CONFIG.GATES["x"]`, an alias `g["x"]` — so refusing the name
        outright covers all of them at once, and the reading it enforces is that a limit is only
        ever consulted inside `measure.validity_gate_failures`, which is handed the whole dict.

        THE CALLERS ARE THE TWO HALVES OF ONE SEARCH: `generate.py`'s pre-filter, which admits a
        candidate before it can cost a verdict, and `scripts/tank/chains.py`'s certification of the
        decoded shipped bytes.

        WHAT EVADES IT, stated because a heuristic that hides its holes reads as a proof: a key
        assembled at runtime (`GATES["max_" + name]`), a name held in a variable or built by an
        f-string, or a second gate table constructed from `CONFIG.GATES` without spelling any key.
        A text scan cannot decide those.
        """
        directory = os.path.dirname(os.path.abspath(M.__file__))
        sources = {
            name: open(os.path.join(directory, name), encoding="utf-8").read()
            for name in ("generate.py", os.path.join("..", "tank", "chains.py"))
        }
        for name, source in sources.items():
            self.assertIn(
                "M.validity_gate_failures(", source, f"{name} does not call the shared gate list"
            )
        self.assertNotIn("def validity_failures(", sources["generate.py"])
        for limit in (key for key in CONFIG.GATES if key.startswith("max_")):
            for name, source in sources.items():
                for spelling in (f'"{limit}"', f"'{limit}'"):
                    self.assertNotIn(
                        spelling, source,
                        f"{name} names the limit {limit} directly ({spelling}) — the shared gate "
                        f"list is the only place a limit may be read, or there are two lists again",
                    )


class BisectionTests(unittest.TestCase):
    """WHERE THE DECIMATOR'S CONTRACT IS ACTUALLY ESTABLISHED.

    A search cannot prove its oracle honest — it can only ask it, and a consistently wrong oracle
    answers consistently. So the property "returns the greatest realizable count at or below the
    budget" is proven HERE, where it is arithmetic rather than a conversation: over synthetic
    staircases whose answer is computed independently from the staircase's own definition. The
    directed search's `reached - 1` step is only sound because of it.
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


class ConvexBoundTests(unittest.TestCase):
    """The acceptance bound ADR 0036 §6 adds, and the two properties everything downstream needs.

    SOUND: it never claims a patch is closer to the target surface than it is, so a certificate can
    never be bought with it. TIGHTER: it is never LOOSER than the covering-radius bound on the same
    inputs, because the caller takes the minimum of the two — which is what makes it a wall-time
    change and not a corpus change.
    """

    @staticmethod
    def brute_force_max(corners, dst_p0, dst_p1, dst_p2, samples=60):
        """max over the patch of the true distance to the destination surface, densely sampled.

        A LOWER bound on the truth (a finite sample can only miss the maximum), which is exactly the
        direction that makes it useful here: anything the bound returns must be at least this.
        """
        weights = []
        for i in range(samples + 1):
            for j in range(samples + 1 - i):
                weights.append((i / samples, j / samples))
        weights = np.array(weights)
        bary = np.stack([1.0 - weights[:, 0] - weights[:, 1], weights[:, 0], weights[:, 1]], axis=1)
        points = bary @ np.asarray(corners)
        best = np.full(len(points), np.inf)
        for t in range(len(dst_p0)):
            best = np.minimum(best, M.point_triangle_distances(
                points,
                np.tile(dst_p0[t], (len(points), 1)),
                np.tile(dst_p1[t], (len(points), 1)),
                np.tile(dst_p2[t], (len(points), 1)),
            ))
        return float(best.max())

    class FakeSurface:
        def __init__(self, triangles):
            triangles = np.asarray(triangles, dtype=np.float64)
            self.p0, self.p1, self.p2 = triangles[:, 0], triangles[:, 1], triangles[:, 2]

    def nearest(self, dst, point):
        """(distance, triangle index) — the BVH's answer, computed by exhaustion here."""
        point = np.asarray(point, dtype=np.float64)[None, :]
        distances = [
            float(M.point_triangle_distances(point, dst.p0[t:t + 1], dst.p1[t:t + 1],
                                             dst.p2[t:t + 1])[0])
            for t in range(len(dst.p0))
        ]
        index = int(np.argmin(distances))
        return distances[index], index

    def test_the_point_triangle_distance_is_exact(self):
        """Against a dense barycentric sweep of the triangle, over random configurations.

        The sweep can only OVER-report (it samples), so `computed <= sampled` everywhere is the
        soundness half and closeness is the tightness half.
        """
        rng = np.random.default_rng(20260812)
        count = 500
        a, b, c = (rng.normal(size=(count, 3)) for _ in range(3))
        points = rng.normal(size=(count, 3)) * 2.0
        computed = M.point_triangle_distances(points, a, b, c)
        step = 0.02
        grid = np.array([
            (i * step, j * step)
            for i in range(int(1 / step) + 1)
            for j in range(int(1 / step) + 1 - i)
        ])
        bary = np.stack([1.0 - grid[:, 0] - grid[:, 1], grid[:, 0], grid[:, 1]], axis=1)
        sampled = np.full(count, np.inf)
        for row in bary:
            candidate = row[0] * a + row[1] * b + row[2] * c
            sampled = np.minimum(sampled, np.linalg.norm(candidate - points, axis=1))
        self.assertTrue(np.all(computed <= sampled + 1e-12), "it under-reports nothing")
        self.assertLess(float((sampled - computed).max()), 0.02, "and it is the real minimum")

    def test_a_degenerate_triangle_never_under_reports(self):
        """A needle and a point-triangle: the barycentric denominators go to zero here.

        The fail-closed rule is what makes this safe — a non-finite result becomes infinity, so the
        convex bound is discarded and the covering-radius subdivision decides the patch.
        """
        points = np.array([[0.0, 0.0, 1.0], [5.0, 5.0, 5.0]])
        needle_a = np.array([[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]])
        needle_b = np.array([[1.0, 0.0, 0.0], [0.0, 0.0, 0.0]])
        needle_c = np.array([[1.0, 1e-18, 0.0], [0.0, 0.0, 0.0]])
        got = M.point_triangle_distances(points, needle_a, needle_b, needle_c)
        self.assertTrue(np.all(np.isfinite(got) | np.isinf(got)))
        self.assertGreaterEqual(float(got[0]), 1.0 - 1e-9)
        self.assertGreaterEqual(float(got[1]), float(np.sqrt(75.0)) - 1e-9)

    def test_the_bound_holds_over_the_whole_patch(self):
        """Random patches against a random two-triangle surface: the bound is never under the truth."""
        rng = np.random.default_rng(7)
        for _ in range(200):
            dst = self.FakeSurface(rng.normal(size=(3, 3, 3)))
            corners = rng.normal(size=(3, 3)) + np.array([0.0, 0.0, 2.0])
            hits = [self.nearest(dst, corner) for corner in corners]
            bound = float(M.convex_patch_bounds(
                corners[None, :, :], np.array([[h[1] for h in hits]]), dst
            )[0])
            truth = self.brute_force_max(corners, dst.p0, dst.p1, dst.p2, samples=24)
            self.assertGreaterEqual(
                bound, truth - 1e-9,
                "the convex bound must never be below a distance the surface actually attains",
            )

    def test_the_convex_bound_never_certifies_looser_than_the_subdivision_bound(self):
        """THE MUTANT THE BRIEF ASKS FOR, and it is a property of the combination rather than luck.

        `patch_bounds` returns `min(covering, convex)`, so the answer is at most the covering-radius
        bound on identical inputs — the bound the shipped corpus was cut with. A convex bound that
        came out LARGER cannot loosen anything; one that comes out smaller only saves nodes.
        """
        rng = np.random.default_rng(99)
        for _ in range(200):
            dst = self.FakeSurface(rng.normal(size=(2, 3, 3)))
            corners = rng.normal(size=(1, 3, 3))
            hits = np.array([[self.nearest(dst, corner)[1] for corner in corners[0]]])
            distances = np.array([[self.nearest(dst, corner)[0] for corner in corners[0]]])
            covering = float(M.covering_patch_bounds(corners, distances)[0])
            combined = float(M.patch_bounds(corners, distances, hits, dst, target=-1.0)[0])
            self.assertLessEqual(combined, covering + 1e-12)

    def certify_both_ways(self, dst, triangles, tol=1e-4, max_nodes=250, rel_tol=0.0):
        """The CERTIFY path — `branch_and_bound` — run with each bound over identical inputs.

        Same seeds, same probe, same stopping rule; only `bound_of` differs. That is the whole of
        what the fix changed, so it is the whole of what these compare.
        """
        cache = {}

        def probe(key, point):
            if key not in cache:
                cache[key] = self.nearest(dst, point)
            return cache[key]

        corners = np.asarray(triangles, dtype=np.float64)
        answers = [[probe(("v", index, j), corners[index][j]) for j in range(3)]
                   for index in range(len(corners))]
        distances = np.array([[a[0] for a in row] for row in answers])
        tags = np.array([[a[1] for a in row] for row in answers])
        subdivision = M.branch_and_bound(
            corners, distances, tags, probe, tol, max_nodes, None, rel_tol,
        )
        convex = M.branch_and_bound(
            corners, distances, tags, probe, tol, max_nodes, None, rel_tol,
            bound_of=lambda c, d, t, ceiling: M.patch_bounds(c, d, t, dst, ceiling),
        )
        return subdivision, convex

    def test_the_certified_bound_never_loosens_when_the_convex_bound_is_added(self):
        """MUTANT, on the CERTIFY path: the fix may only move a certificate DOWN.

        ADR 0036 §6's bound went into acceptance first and certification second, and the second half
        is the one that touches shipped numbers — every recorded `dev_source_mm_upper` and every
        switch distance derived from it. So the property is asserted where it matters: over identical
        seeds and an identical stopping rule, the convex-bounded bracket's UPPER end is never above
        the subdivision-only one's.

        THE LOWER END MAY MOVE DOWN, and that is the tightening working rather than a regression. It
        is the largest point SAMPLED, so a bracket that closes in fewer nodes has sampled fewer
        points and carries a weaker witness. The guarantee the corpus rests on is the upper end, and
        a witness is sound at any density; what is asserted here is that the pair is still a
        bracket.
        """
        rng = np.random.default_rng(4242)
        for _ in range(25):
            dst = self.FakeSurface(rng.normal(size=(3, 3, 3)))
            triangles = rng.normal(size=(2, 3, 3)) + np.array([0.0, 0.0, 1.5])
            (_lo_a, up_a), (lo_b, up_b) = self.certify_both_ways(dst, triangles)
            self.assertLessEqual(
                up_b, up_a + 1e-12,
                "the convex bound certified LOOSER than the subdivision it is minimised against",
            )
            self.assertLessEqual(lo_b, up_b + 1e-12, "a lower end above its own upper end")

    def test_a_starved_certification_is_still_above_every_attained_distance(self):
        """MUTANT, at certification tolerance: a tighter bound must still BE a bound.

        Deliberately node-starved, because that is the mode the shipped corpus meets — the certify
        bracket is capped at 1.5 M nodes and a large mesh spends it. A starved bracket must be wide
        and honest, never narrow and wrong, and the tightening must not have cost that.
        """
        rng = np.random.default_rng(31337)
        for _ in range(25):
            dst = self.FakeSurface(rng.normal(size=(3, 3, 3)))
            triangles = rng.normal(size=(2, 3, 3)) + np.array([0.0, 0.0, 1.5])
            _subdivision, (_lower, upper) = self.certify_both_ways(
                dst, triangles, tol=1e-6, max_nodes=8,
            )
            attained = max(
                self.brute_force_max(patch, dst.p0, dst.p1, dst.p2, samples=20)
                for patch in triangles
            )
            self.assertGreaterEqual(
                upper, attained - 1e-9,
                "a certified upper bound below a distance the surface actually attains is not a "
                "bound at all — this is the exact defect that survived before the loop was made "
                "testable without a BVH",
            )

    def test_a_coplanar_patch_is_proven_with_no_subdivision(self):
        """The mechanism the ADR bought: on a flat region the bound collapses to the corner maximum.

        The covering-radius bound cannot do this — it adds a whole covering radius to every corner
        distance, so proving a maximum under `e` forces every patch below `e`'s length scale. That
        is the target^-2 acceptance cost the rebuild exists to attack.
        """
        dst = self.FakeSurface([
            [[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [10.0, 10.0, 0.0]],
            [[-10.0, -10.0, 0.0], [10.0, 10.0, 0.0], [-10.0, 10.0, 0.0]],
        ])
        # Wholly inside the first triangle (the half-plane y <= x), so all three corners see the
        # same plane — which is the case the bound exists for.
        corners = np.array([[[5.0, 0.0, 0.1], [7.0, 0.0, 0.1], [6.0, -2.0, 0.1]]])
        hits = np.array([[self.nearest(dst, corner)[1] for corner in corners[0]]])
        distances = np.array([[self.nearest(dst, corner)[0] for corner in corners[0]]])
        covering = float(M.covering_patch_bounds(corners, distances)[0])
        convex = float(M.convex_patch_bounds(corners, hits, dst)[0])
        self.assertAlmostEqual(convex, 0.1, places=9)
        self.assertGreater(covering, 2.0, "the covering radius bound is dominated by the patch size")


class ExceptionalProbeTests(unittest.TestCase):
    """A BVH that cannot answer must never tighten anything. Fail-closed, in every consumer.

    THE DEFECT THIS PINS, found by review: `bvh_probe` mapped a no-hit to `(0.0, -1)` and passed a
    NaN hit distance straight through. Zero is the most TIGHTENING value a distance can take — it
    pulls the covering bound down, it can become the `best` a bracket closes against, and it makes
    a patch look proven. NaN is worse, because `NaN > target` is False and the acceptance check
    reads that as "inside the rung".

    Both now become `UNKNOWN_DISTANCE`, which every consumer already handles by construction: the
    bound's `min_k` ignores it (each corner bounds the whole patch on its own, so one usable corner
    is enough), the live heap can never close on it, and it is not a sample so it never reaches a
    lower bound.
    """

    #: A plane at z = 0, and a SMALL patch one metre above it. The true maximum distance over that
    #: patch is 1.0, and the patch is small so that a corner answering ZERO drags the covering bound
    #: to nearly nothing — which is exactly how a tightening sentinel becomes an unsound bound.
    FLAT = [
        [[-5.0, -5.0, 0.0], [5.0, -5.0, 0.0], [5.0, 5.0, 0.0]],
        [[-5.0, -5.0, 0.0], [5.0, 5.0, 0.0], [-5.0, 5.0, 0.0]],
    ]
    PATCH = np.array([[[0.0, 0.0, 1.0], [1.0e-3, 0.0, 1.0], [0.0, 1.0e-3, 1.0]]])
    TRUTH = 1.0

    def bracket_under(self, answer):
        """`branch_and_bound` over `PATCH` against a probe that can only ever answer `answer`.

        That is the fully-failed query — an empty destination tree, or one poisoned by a non-finite
        coordinate — which is the reachable shape of this defect and the sharpest form of it.
        """
        dst = ConvexBoundTests.FakeSurface(self.FLAT)
        distances = np.full((1, 3), answer[0], dtype=np.float64)
        tags = np.full((1, 3), answer[1], dtype=np.int64)
        return M.branch_and_bound(
            self.PATCH, distances, tags, lambda _k, _p: answer,
            tol=1e-6, max_nodes=200, rel_tol=0.0,
            bound_of=lambda c, d, t, ceiling: M.patch_bounds(c, d, t, dst, ceiling),
        )

    def test_the_zero_a_failed_query_used_to_answer_certifies_a_false_bound(self):
        """THE DEFECT, executed. `(0.0, -1)` is what `bvh_probe` returned for a no-hit.

        Every corner answers zero, so the covering bound is the patch's own span — a millimetre —
        and the bracket closes there. The surface is a METRE away. This is not a loose bound, it is
        a wrong one, and nothing downstream could have known.
        """
        _lower, upper = self.bracket_under((0.0, -1))
        self.assertLess(
            upper, self.TRUTH,
            "this test exists to demonstrate an UNSOUND bound; if it no longer is one, the "
            "demonstration has rotted and the guard below is testing nothing",
        )

    def test_an_unknown_distance_keeps_the_bracket_open_instead(self):
        """MUTANT. The same query, answered honestly: nothing tightens, so nothing closes."""
        for answer in ((M.UNKNOWN_DISTANCE, -1), (float("nan"), -1)):
            with self.subTest(answer=answer[0]):
                # A NaN reaches the bound as a NaN only if the probe wrapper let it through; the
                # wrapper maps it to UNKNOWN_DISTANCE, and this pins the wrapper's law.
                mapped = (M.UNKNOWN_DISTANCE, -1) if not math.isfinite(answer[0]) else answer
                lower, upper = self.bracket_under(mapped)
                self.assertGreaterEqual(
                    upper, self.TRUTH,
                    "an unanswerable query produced an upper bound below a distance the surface "
                    "actually attains — the failure is open, not closed",
                )
                self.assertTrue(
                    math.isfinite(lower),
                    "a non-sample became a sampled lower bound, so an unmeasured deviation would "
                    "be reported as an observed one",
                )

    def test_a_dead_query_stops_instead_of_spending_the_whole_budget(self):
        """Fail-closed must not mean fail-expensively.

        Nothing answers, so no subdivision can ever produce a finite bound — and without a stop the
        loop would run the full certify budget (1.5 M nodes) pushing four infinite children per
        node, which is an unbounded heap on a defective input. The upper bound it hands back is the
        same infinity either way; only the price differs.
        """
        dst = ConvexBoundTests.FakeSurface(self.FLAT)
        distances = np.full((1, 3), M.UNKNOWN_DISTANCE, dtype=np.float64)
        tags = np.full((1, 3), -1, dtype=np.int64)
        probes = []

        def probe(_k, _p):
            probes.append(1)
            return (M.UNKNOWN_DISTANCE, -1)

        _lower, upper = M.branch_and_bound(
            self.PATCH, distances, tags, probe, tol=1e-9, max_nodes=1_500_000, rel_tol=0.0,
            bound_of=lambda c, d, t, ceiling: M.patch_bounds(c, d, t, dst, ceiling),
        )
        self.assertFalse(math.isfinite(upper), "the bracket must stay open")
        self.assertLess(len(probes), 16, f"it spent {len(probes)} probes proving nothing")

    def test_a_measured_corner_still_bounds_a_patch_whose_neighbour_is_unknown(self):
        """FAIL-CLOSED IS NOT FAIL-USELESS: one usable corner is enough, and that is why.

        The covering bound holds for EACH corner independently, so `min_k` skipping an unknown one
        is a tightening the geometry permits rather than a hole. Subdivision therefore recovers: the
        midpoints get real answers and the patch decides. Without this the fix would trade a wrong
        bound for a lane that refuses everything near a defect.
        """
        dst = ConvexBoundTests.FakeSurface(self.FLAT)
        honest = ConvexBoundTests().nearest
        distances = np.array([[M.UNKNOWN_DISTANCE,
                               honest(dst, self.PATCH[0][1])[0],
                               honest(dst, self.PATCH[0][2])[0]]])
        tags = np.array([[-1, honest(dst, self.PATCH[0][1])[1], honest(dst, self.PATCH[0][2])[1]]])
        bounds = M.patch_bounds(self.PATCH, distances, tags, dst, self.TRUTH)
        self.assertTrue(np.isfinite(bounds[0]), "one measured corner bounds the whole patch")
        self.assertGreaterEqual(float(bounds[0]), self.TRUTH - 1e-9)

    def test_an_unknown_is_never_a_witness(self):
        """The lower end is a WITNESS, and an unanswered query is not one.

        Folding `UNKNOWN_DISTANCE` into `best` would report an infinite deviation as if the surface
        had been observed to attain it — and would let the stopping rule close instantly on
        `inf <= inf + slack` and return that as a measurement.
        """
        self.assertEqual(M.sampled_max(0.5, M.UNKNOWN_DISTANCE), 0.5)
        self.assertEqual(M.sampled_max(0.5, float("nan")), 0.5)
        self.assertEqual(M.sampled_max(0.5, 0.9, M.UNKNOWN_DISTANCE), 0.9)

    def test_the_acceptance_check_cannot_rest_on_the_comparison_alone(self):
        """MUTANT, on the acceptance side, and the reason the finiteness test comes FIRST.

        `one_way_fits` rejects a corner over the rung target with `distance > target_m`. A NaN loses
        that comparison — silently, and in the direction that ACCEPTS — so a probe answer that is
        not a number would have carried an unmeasurable candidate toward a PASS. The guard is a
        finiteness check ahead of the comparison, and this pins why it cannot be folded into it.
        """
        target = 0.001
        self.assertFalse(float("nan") > target, "a NaN is not caught by the target comparison")
        self.assertTrue(M.UNKNOWN_DISTANCE > target)
        for answer in (float("nan"), M.UNKNOWN_DISTANCE):
            self.assertFalse(math.isfinite(answer), "both are caught by the finiteness check")


class DirectedSearchTests(unittest.TestCase):
    """`measure.directed_rung_search` — the loop, against synthetic probes with known answers."""

    @staticmethod
    def probe_from(realizable, feasible, log=None):
        """A probe over a declared staircase, with a declared feasibility set.

        `realizable` is the sorted list of counts the decimator can produce; the greatest one at or
        below the budget is what a probe returns, which is `bisect_to_budget`'s contract.
        """
        def probe(budget):
            if log is not None:
                log.append(budget)
            reached = [r for r in realizable if r <= budget]
            if not reached:
                return None, M.PROVEN_FAIL
            count = reached[-1]
            return count, (M.PROVEN_PASS if count in feasible else M.PROVEN_FAIL)

        return probe

    def test_it_finds_a_feasible_candidate_and_returns_a_realizing_budget(self):
        realizable = [10, 40, 90, 140, 200]
        probe = self.probe_from(realizable, feasible={140, 200})
        budget = M.directed_rung_search(10, 200, probe)
        self.assertIsNotNone(budget)
        reached = [r for r in realizable if r <= budget][-1]
        self.assertEqual(reached, 140, "the cheapest feasible count the bisection reaches")

    def test_no_feasible_candidate_returns_nothing(self):
        probe = self.probe_from([10, 40, 90], feasible=set())
        self.assertIsNone(M.directed_rung_search(10, 90, probe))

    def test_an_undecided_verdict_never_selects_a_candidate(self):
        """MUTANT. UNDECIDED is FAIL at the search, or the ladder ships an unproven level.

        Every probe here is undecidable — the pathology `Turret_Decor`'s two finest rungs hit — and
        the only acceptable outcome is an empty rung. A search that treated UNDECIDED as anything
        but a failure would select the first candidate it could not decide.
        """
        def probe(budget):
            return min(budget, 200), M.UNDECIDED

        self.assertIsNone(M.directed_rung_search(10, 200, probe))

    def test_a_mixture_of_undecided_and_pass_takes_only_the_proven_one(self):
        """The undecidable band is stepped over, richer — it can cost triangles, never honesty."""
        realizable = [10, 40, 90, 140, 200]

        def probe(budget):
            reached = [r for r in realizable if r <= budget]
            if not reached:
                return None, M.PROVEN_FAIL
            count = reached[-1]
            if count in (40, 90):
                return count, M.UNDECIDED
            return count, (M.PROVEN_PASS if count >= 140 else M.PROVEN_FAIL)

        budget = M.directed_rung_search(10, 200, probe)
        reached = [r for r in realizable if r <= budget][-1]
        self.assertEqual(reached, 140)

    def test_below_the_floor_moves_the_search_richer_rather_than_ending_it(self):
        probe = self.probe_from([120, 300], feasible={120, 300})
        budget = M.directed_rung_search(1, 300, probe)
        self.assertIsNotNone(budget)
        self.assertGreaterEqual(budget, 120)

    def test_the_probe_order_is_identical_across_two_runs(self):
        """MUTANT. The corpus is a function of the geometry, so the SEARCH must be too.

        Integer midpoints, no clock and no randomness — the same asset asks the same questions on
        any machine, which is what makes two runs comparable field for field.
        """
        realizable = list(range(2, 1521, 3))
        first, second = [], []
        M.directed_rung_search(2, 1520, self.probe_from(realizable, {700, 1000}, log=first))
        M.directed_rung_search(2, 1520, self.probe_from(realizable, {700, 1000}, log=second))
        self.assertEqual(first, second)
        self.assertTrue(first, "the search must actually probe something")

    def test_an_undecided_rung_is_lost_to_the_budget_whatever_else_is_true_of_it(self):
        """The WRITER'S half of the misattribution, at the one declaration both halves read.

        `generate` picks this field and `chain` validates it, and while the rule lived in both
        places they disagreed: the writer filed a rung under `skip_fraction` whenever the
        sparse-chain rule had fired, even if the search had abstained on a candidate at that rung —
        and the verifier's fidelity warning, which only fires on `verdict_node_budget`, went quiet
        on exactly the rungs that had earned one.
        """
        self.assertEqual(M.rung_lost_to(0, "skip_fraction"), "skip_fraction")
        self.assertEqual(M.rung_lost_to(0, "geometry"), "geometry")
        for otherwise in ("skip_fraction", "geometry"):
            self.assertEqual(
                M.rung_lost_to(1, otherwise), "verdict_node_budget",
                "one abstention outranks every other explanation for losing the rung",
            )
            self.assertEqual(M.rung_lost_to(37, otherwise), "verdict_node_budget")
        for value in M.SKIP_LOST_TO:
            self.assertIn(value, ("verdict_node_budget", "geometry", "skip_fraction"))

    def test_the_budget_law_reads_one_number_on_both_sides(self):
        """MUTANT for a split brain: two readers of one budget must not round differently.

        Generation used to compute the budget from the evaluated source's FULL-PRECISION diagonal
        while the verifier recomputed it from the recorded box rounded to four decimals, and then
        demanded exact integer equality. `verdict_node_budget` TRUNCATES, so the two agreed only
        while no asset landed near an integer boundary — and the failure would have been a valid
        corpus refused for a number nobody edited.

        The straddle is HUNTED rather than asserted: this walks a box outward in tenths of a micron
        until it finds one where the two readings genuinely truncate to different integers, which is
        the proof that the hazard was real rather than theoretical. It takes a few dozen steps,
        because a 4th-decimal rounding moves this budget by about a twentieth of a node and the
        boundary is hit whenever that crosses an integer.
        """
        target = CONFIG.E1_MM
        straddle = None
        for step in range(200000):
            raw = 700.0 + step * 1.7e-5
            bbox = [round(raw, 4), 180.0, 180.0]
            from_recorded = CONFIG.verdict_node_budget(CONFIG.diagonal_mm_from_bbox(bbox), target)
            from_full = CONFIG.verdict_node_budget(
                math.sqrt(raw * raw + 180.0 * 180.0 + 180.0 * 180.0), target
            )
            if from_recorded != from_full:
                straddle = (bbox, from_recorded, from_full)
                break
        self.assertIsNotNone(
            straddle,
            "no box in the swept range makes the two readings disagree — if that is now true in "
            "general the hazard is gone, but it is far likelier that this sweep stopped covering "
            "the boundary",
        )
        bbox, from_recorded, from_full = straddle
        self.assertNotEqual(
            from_recorded, from_full,
            "the two readings of one box differ by a whole node here — which is exactly what a "
            "verifier demanding exact integer equality would have refused",
        )
        # Both sides now read the recorded box, so agreement is by construction rather than by
        # this asset happening to sit away from a boundary.
        self.assertEqual(
            CONFIG.verdict_node_budget(CONFIG.diagonal_mm_from_bbox(bbox), target),
            CONFIG.verdict_node_budget(CONFIG.diagonal_mm_from_bbox(list(bbox)), target),
        )

    def test_the_node_budget_is_scale_free_deterministic_and_capped(self):
        """The declared budget law, held against the three properties ADR 0036 §6 asks of it."""
        # Scale-free: four times the target is sixteen times cheaper.
        fine = CONFIG.verdict_node_budget(769.0, 3.89)
        coarse = CONFIG.verdict_node_budget(769.0, 4 * 3.89)
        self.assertAlmostEqual(fine / coarse, 16.0, delta=0.1)
        # A bigger mesh at the same rung gets a bigger budget, until the cap.
        self.assertGreater(
            CONFIG.verdict_node_budget(1600.0, 15.56),
            CONFIG.verdict_node_budget(769.0, 15.56),
        )
        self.assertEqual(CONFIG.verdict_node_budget(100000.0, 3.89), CONFIG.VERDICT_NODES_CAP)
        # Deterministic and integral.
        self.assertEqual(fine, CONFIG.verdict_node_budget(769.0, 3.89))
        self.assertIsInstance(fine, int)


class ProjectionTests(unittest.TestCase):
    """The one projection, and the world it is quoted in.

    Nobody writes a switch distance down — the runtime derives it from the certificate's deviations
    and the active view — but the ladder still reads this projection to decide where a chain STOPS
    (`RIGHT_WALL_M`), so the arithmetic is load bearing and is held here.
    """

    #: The reference view the doctrine quotes, spelled out so a `config` that moved is a failure
    #: rather than a silently different question.
    VIEW = {"name": "gunner optic", "vfov_rad": 0.12, "height_px": 2160.0, "budget_px": 1.0}

    def test_the_projection_is_exact_not_small_angle(self):
        """The shortcut the drifted ledger used differs measurably at a wide FOV."""
        dev_mm = 18.641  # the deviation the drifted ledger was quoting
        view = self.VIEW
        exact = CONFIG.switch_distance_m(dev_mm, 0.0, view)
        shortcut = (dev_mm / 1000.0) * view["height_px"] / (view["vfov_rad"] * view["budget_px"])
        # 335.1 exact against 335.5 small-angle: the historic "335.5 m derived" was the shortcut,
        # and at the optic the two are close enough that nobody would have noticed.
        self.assertAlmostEqual(exact, 335.135, places=2)
        self.assertAlmostEqual(shortcut, 335.538, places=2)
        self.assertLess(abs(exact - shortcut) / exact, 0.002, "optic hides the error")

        wide = dict(view, vfov_rad=0.785)
        exact_wide = CONFIG.switch_distance_m(dev_mm, 0.0, wide)
        shortcut_wide = (dev_mm / 1000.0) * wide["height_px"] / (wide["vfov_rad"] * 1.0)
        self.assertGreater(
            abs(exact_wide - shortcut_wide) / exact_wide, 0.03,
            "the small-angle shortcut must be visibly wrong at the commander FOV",
        )

    def test_the_declared_reference_view_is_the_one_quoted_here(self):
        """`config.REFERENCE_VIEW` is what every number above was computed in."""
        for key, value in self.VIEW.items():
            self.assertEqual(CONFIG.REFERENCE_VIEW[key], value)

    def test_the_octave_grid_doubles_the_switch_distance(self):
        base = CONFIG.switch_distance_m(CONFIG.E1_MM, 0.0, self.VIEW)
        for rung, target in CONFIG.rungs()[:5]:
            self.assertAlmostEqual(
                CONFIG.switch_distance_m(target, 0.0, self.VIEW), base * 2 ** (rung - 1), places=6
            )

    def test_the_bounding_radius_slack_is_conservative(self):
        near = CONFIG.switch_distance_m(10.0, 0.0, self.VIEW)
        with_radius = CONFIG.switch_distance_m(10.0, 0.383, self.VIEW)
        self.assertAlmostEqual(with_radius - near, 0.383, places=9)

    def test_one_millimetre_goes_subpixel_at_eighteen_metres(self):
        """The optic reference the doctrine quotes, re-derived rather than remembered."""
        self.assertAlmostEqual(CONFIG.switch_distance_m(1.0, 0.0, self.VIEW), 17.98, places=1)

    def test_the_right_wall_is_the_default_maps_diagonal(self):
        """The wall read here independently of `config`'s own parse.

        The number this asserts is not written down anywhere in this file: it comes out of
        `assets/maps/<id>/level.json`, which is the file `crate::map` builds the grid from. So a map
        that grows moves both sides of this equality together and the test keeps passing — what it
        catches is `config` deriving the wall from something OTHER than the world the game loads.
        """
        root = CONFIG.repo_root()
        level = os.path.join(root, "assets", "maps", CONFIG.default_map_id(root), "level.json")
        with open(level, encoding="utf-8") as handle:
            extent = json.load(handle)["terrain"]["heightmap"]["world_extent_xz"]
        side = extent["maximum"][0] - extent["minimum"][0]
        self.assertAlmostEqual(CONFIG.WORLD_SIZE_M, side, places=6)
        self.assertAlmostEqual(CONFIG.RIGHT_WALL_M, side * math.sqrt(2.0), places=6)

    def test_the_pins_are_the_toolchains(self):
        """One home per pin: `config` names `scripts/toolchain.py`'s values rather than copying
        them, and the tank build hashes `toolchain.py`'s bytes into every certificate."""
        sys.path.insert(0, os.path.join(CONFIG.repo_root(), "scripts"))
        import toolchain  # noqa: PLC0415 — the pins' one home, only needed for this claim

        self.assertIs(CONFIG.EXPECTED_BLENDER, toolchain.BLENDER_VERSION)
        self.assertIs(CONFIG.EXPECTED_BLENDER_BUILD, toolchain.BLENDER_BUILD)
        self.assertIs(CONFIG.EXPECTED_GLTF_EXPORTER, toolchain.GLTF_EXPORTER_VERSION)


if __name__ == "__main__":
    unittest.main(verbosity=2)
