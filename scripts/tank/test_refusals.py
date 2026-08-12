"""The coverage table: every check the door can print, and the case that drives it to the report.

    python3 scripts/tank/test_refusals.py

WHAT THIS IS
------------
The door's vocabulary is four producers in three languages — the Blender source pass, the Rust
consumer contract, the derivation verifier and the wrapper's own rows — and its suites are as many.
Nothing until now said that the union of the first equals the union of the second: a check could be
declared, compiled into a stage, and never once driven to a report by anything.

`COVERAGE` below is that statement, one row per check id, naming the file and the case that drives
it END TO END — through the stage's real entry point, over real bytes, with the severity compiled
into it and the exit status the report then carries. It is a table rather than a paragraph because
a table can be checked: this suite harvests every `Check` declared anywhere in the door and refuses
a check with no row, a row with no check, and a row naming a case its file does not hold.

WHAT IT IS NOT
--------------
It re-drives nothing. Each named case owns its law and is where a mutation of that law is caught;
this file owns only the claim that the set of them is complete. A check that needs a new case is a
failure HERE and a new case THERE.
"""

from __future__ import annotations

import ast
import os
import re
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import asset_door  # noqa: E402

ROOT = asset_door.repo_root()

#: The producers, by the file that declares their checks.
PRODUCERS = (
    os.path.join(".agents", "blender", "export_tank.py"),
    os.path.join("scripts", "tank", "glb_ktx2.py"),
    os.path.join("scripts", "tank", "asset_door.py"),
    os.path.join("scripts", "tank", "build.py"),
    os.path.join("scripts", "toolchain.py"),
    os.path.join("src", "bake.rs"),
)

LINT = os.path.join(".agents", "blender", "test_tank_lint.py")
CONTRACT = os.path.join("src", "bake.rs")
DERIVATION = os.path.join("scripts", "tank", "test_glb_ktx2.py")
DOOR = os.path.join("scripts", "tank", "test_asset_door.py")
BUILD = os.path.join("scripts", "tank", "test_build.py")

#: check id → (file, case) — the one case that drives this check's refusal through the real stage.
#: Several checks may share a case where one fixture is one defect that names them both.
COVERAGE = {
    # ── L1, the .blend source pass ───────────────────────────────────────────────────────────────
    "L1.SAVED_SOURCE": (LINT, "saved_source_unsaved_blend"),
    "L1.EXPORT_SCOPE": (LINT, "export_scope_empty_scene"),
    "L1.LOCAL_MODEL_DATA": (LINT, "local_model_data_linked_object"),
    "L1.MODIFIER_STACK": (LINT, "modifier_stack_any_modifier"),
    "L1.DEFORMATION": (LINT, "deformation_shape_keys"),
    "L1.ANIMATION": (LINT, "animation_object_action"),
    "L1.TRANSFORM_FINITE": (LINT, "transform_finite_nan_translation"),
    "L1.HANDEDNESS": (LINT, "handedness_mirrored_node"),
    "L1.AUTHORED_SCALE": (
        LINT, "authored_scale_is_an_error_and_the_composition_it_lands_in_is_a_warning"
    ),
    "L1.UNAPPLIED_SCALE": (
        LINT, "authored_scale_is_an_error_and_the_composition_it_lands_in_is_a_warning"
    ),
    "L1.COMPENSATED_SCALE": (LINT, "compensated_scale_channels_that_undo_each_other"),
    "L1.UNIQUE_NAMES": (LINT, "unique_names_duplicate_across_a_library"),
    "L1.DEFAULT_NAMES": (LINT, "default_names_object"),
    "L1.SPEC_REFERENCES": (LINT, "spec_references_a_node_the_scene_does_not_hold"),
    "L1.NONEMPTY_MESH": (LINT, "nonempty_mesh_without_polygons"),
    "L1.FINITE_MESH_DATA": (LINT, "finite_mesh_data_nan_position"),
    "L1.ZERO_AREA_TRIANGLE": (LINT, "zero_area_triangle_coincident_vertices"),
    "L1.DUPLICATE_TRIANGLE": (LINT, "duplicate_triangle_reversed_copy_on_welded_positions"),
    "L1.LOOSE_GEOMETRY": (LINT, "loose_geometry_is_a_warning_that_does_not_fail"),
    "L1.TEXTURE_UV_SOURCE": (LINT, "texture_uv_source_named_layer_the_mesh_does_not_carry"),
    "L1.UV_FINITE": (LINT, "uv_finite_nan_in_a_sampled_layer"),
    "L1.ZERO_AREA_UV": (LINT, "zero_area_uv_is_a_warning_that_does_not_fail"),
    "L1.SUBSTANCE_IDENTITY": (LINT, "substance_identity_a_local_counterfeit"),
    "L1.TEXTURE_SOURCE": (LINT, "texture_source_file_that_is_not_there"),
    "L1.SOURCE_CENSUS": (LINT, "source_census_without_a_baseline_is_neither_pass_nor_fail"),

    # ── L2, the shared consumer contract, every row driven from a written glb ────────────────────
    "L2.SPEC": (CONTRACT, "an_unreadable_sheet_or_an_unresolved_reference_is_refused"),
    "L2.DOCUMENT": (CONTRACT, "a_primitive_the_sim_cannot_read_is_refused"),
    "L2.ROLE_COHERENCE": (CONTRACT, "a_node_that_cannot_play_its_declared_role_is_refused"),
    "L2.PRIMITIVE_FORM": (CONTRACT, "a_primitive_the_sim_cannot_read_is_refused"),
    "L2.UNIT_SCALE": (CONTRACT, "a_scaled_node_the_sim_composes_through_is_refused"),
    "L2.CERTIFIED_RANGE": (CONTRACT, "a_coordinate_outside_the_certified_range_is_refused"),
    "L2.EXACT_DEGENERACY": (CONTRACT, "a_zero_area_face_is_refused"),
    "L2.MANIFOLD_WINDING": (CONTRACT, "open_inverted_and_doubled_shells_are_refused"),
    "L2.POSITIVE_SHELL_VOLUME": (CONTRACT, "open_inverted_and_doubled_shells_are_refused"),
    "L2.SHELL_EMBEDDING": (CONTRACT, "a_shell_that_passes_through_itself_is_refused"),

    # ── D, the closed GLB derivation inventory ──────────────────────────────────────────────────
    "D.STRUCTURAL_DERIVATION": (DERIVATION, "test_mutated_non_texture_json_refuses"),
    "D.TANGENTS": (DERIVATION, "test_a_normal_mapped_primitive_without_tangents_refuses"),
    "D.KTX2_MIPS": (DERIVATION, "test_a_dropped_mip_level_is_an_incomplete_chain"),

    # ── the door's own rows: what only the wrapper and the pass around it can see ────────────────
    "door.toolchain": (DOOR, "test_a_toolchain_mismatch_refuses_before_the_chain"),
    "door.canon-missing": (LINT, "a_missing_canon_is_one_refusal_of_the_door_s_own"),
    "door.unresolved-library": (DOOR, "test_an_unresolved_library_refuses_every_mode"),
    "door.mode-unimplemented": (DOOR, "test_a_mode_with_a_chain_refuses_without_a_candidate_path"),
    "door.raw-export": (
        LINT, "an_exporter_that_writes_no_candidate_is_one_refusal_of_the_door_s_own"
    ),
    "door.registry": (CONTRACT, "the_registry_the_door_supplies_is_the_one_the_contract_reads"),
    "door.stage-failed": (DOOR, "test_an_encoder_failure_refuses"),
    "door.candidate-mismatch": (
        DOOR, "test_a_mesh_bufferview_byte_flip_is_a_mismatch_naming_the_section"
    ),

    # ── the build's own rows: what only the trio's assembler can see (ADR 0035) ──────────────────
    "build.trio-incoherent": (
        BUILD, "test_a_tampered_certificate_digest_refuses_at_the_certificate"
    ),
    "build.sim-not-derived": (
        BUILD, "test_a_sim_artifact_that_hashes_right_and_is_not_the_strip_refuses"
    ),
    "build.cache-corrupt": (
        BUILD, "test_a_rung_whose_bytes_are_not_the_ones_the_record_measured_is_refused"
    ),
}

#: `id: "L2.SOMETHING"` in a Rust `Check { … }` initialiser.
RUST_CHECK = re.compile(r'^\s*id:\s*"([^"]+)"\s*,\s*$', re.MULTILINE)

#: `def <name>` in Python, `fn <name>` in Rust — the two spellings of "this file holds that case".
CASE = re.compile(r"^\s*(?:def|fn)\s+(\w+)\s*\(", re.MULTILINE)


def contents(relative_path):
    with open(os.path.join(ROOT, relative_path), encoding="utf-8") as handle:
        return handle.read()


def declared(relative_path):
    """Every check id declared in one producer, however that language spells a declaration."""
    text = contents(relative_path)
    if relative_path.endswith(".rs"):
        return set(RUST_CHECK.findall(text))
    found = set()
    for node in ast.walk(ast.parse(text)):
        if not isinstance(node, ast.Call):
            continue
        name = node.func.id if isinstance(node.func, ast.Name) else None
        if name != "Check":
            continue
        for keyword in node.keywords:
            if keyword.arg == "id" and isinstance(keyword.value, ast.Constant):
                found.add(keyword.value.value)
    return found


class TheTableIsTheVocabulary(unittest.TestCase):
    """The set the door can print, and the set the suites drive, are one set."""

    def test_every_declared_check_has_a_driving_case(self):
        declarations = {}
        for producer in PRODUCERS:
            for check in declared(producer):
                declarations.setdefault(check, producer)
        self.assertTrue(declarations, "no check was harvested — the harvester is broken")
        self.assertEqual(
            sorted(set(declarations) - set(COVERAGE)), [],
            "declared with no case driving it to a report",
        )
        self.assertEqual(
            sorted(set(COVERAGE) - set(declarations)), [],
            "a row for a check no producer declares",
        )

    def test_every_row_names_a_case_its_file_holds(self):
        cases = {}
        for _, (path, _) in COVERAGE.items():
            if path not in cases:
                cases[path] = set(CASE.findall(contents(path)))
        for check, (path, case) in sorted(COVERAGE.items()):
            self.assertIn(case, cases[path], "{}: {} holds no `{}`".format(check, path, case))


if __name__ == "__main__":
    unittest.main(verbosity=2)
