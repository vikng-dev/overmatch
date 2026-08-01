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
              primitives=None, targets=None, mode=4, indexed=True, mesh_count=1):
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
        "nodes": [{"name": f"Node{i}", "mesh": i} for i in range(mesh_count)],
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

    def test_the_digest_ignores_index_order_but_not_geometry(self):
        """The cache key that collapses a decimation plateau — it must key on the MESH, not the file."""
        a = self.surface("d1.glb", SQUARE_POS, SQUARE_IDX, SQUARE_UV)
        b = self.surface("d2.glb", SQUARE_POS, [2, 0, 1, 0, 2, 3], SQUARE_UV)
        moved = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0.05]]
        c = self.surface("d3.glb", moved, SQUARE_IDX, SQUARE_UV)
        self.assertEqual(a.digest(), b.digest())
        self.assertNotEqual(a.digest(), c.digest())


if __name__ == "__main__":
    unittest.main(verbosity=2)
