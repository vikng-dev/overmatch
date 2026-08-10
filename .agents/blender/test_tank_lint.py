"""test_tank_lint.py — synthetic fixtures for the L1 source pass.

    /Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \\
      --python .agents/blender/test_tank_lint.py

Every case is a MUTATION. It builds a clean tank in a fresh scene, asserts the check under test is
silent, then introduces exactly one defect and asserts the check fires with the severity compiled
into it. Presence alone would not separate a check from one that fires on everything.

Fixtures are built with `bpy` in this process; nothing under `assets/` is read or written. The
only files touched are a temporary `assets/<id>/<id>.tank.ron` and a temporary library blend,
which exist so the path-shaped and link-shaped laws have something real to measure.
"""

import dataclasses
import json
import math
import os
import sys
import tempfile
import traceback

import bpy

_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.dirname(os.path.dirname(_HERE))
sys.path.insert(0, _HERE)
sys.path.insert(0, os.path.join(_ROOT, "scripts", "tank"))

import export_tank  # noqa: E402
import report  # noqa: E402
from report import Severity  # noqa: E402


# ── fixture scaffolding ──────────────────────────────────────────────────────────────────────────

_WORK = tempfile.mkdtemp(prefix="tank-lint-test-")

#: A stored path with the shape L1.SAVED_SOURCE requires, and a real sibling spec beside it. The
#: blend itself is never written: the law measures the path and the sibling, not the bytes.
_ASSET_DIR = os.path.join(_WORK, "assets", "testbed")
os.makedirs(_ASSET_DIR, exist_ok=True)
BLEND_PATH = os.path.join(_ASSET_DIR, "testbed.blend")
with open(os.path.join(_ASSET_DIR, "testbed.tank.ron"), "w", encoding="utf-8") as _handle:
    _handle.write("()\n")


def purge():
    """Empty this .blend of everything a case can have left behind, libraries first — linked
    datablocks are freed by their library, not one at a time."""
    for library in list(bpy.data.libraries):
        bpy.data.libraries.remove(library)
    for scene in list(bpy.data.scenes):
        if scene is not bpy.context.window.scene:
            bpy.data.scenes.remove(scene)
    for collection in (
        bpy.data.objects, bpy.data.meshes, bpy.data.materials,
        bpy.data.actions, bpy.data.armatures,
    ):
        for datablock in list(collection):
            collection.remove(datablock)


def triangle_mesh(name):
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata([(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)], [], [(0, 1, 2)])
    mesh.update()
    return mesh


def clean_scene():
    """A tank shaped like the laws want one: a parented MESH/EMPTY hierarchy, unit transforms, no
    modifiers, no animation, and not one stock name."""
    purge()
    scene = bpy.context.window.scene
    hull = bpy.data.objects.new("Hull", triangle_mesh("Hull"))
    hull.data.materials.append(bpy.data.materials.new("Steel"))
    turret = bpy.data.objects.new("Turret", triangle_mesh("Turret"))
    muzzle = bpy.data.objects.new("Muzzle", None)
    for obj in (hull, turret, muzzle):
        scene.collection.objects.link(obj)
    turret.parent = hull
    muzzle.parent = turret
    return scene


def source_of(scene=None, filepath=BLEND_PATH, is_dirty=False):
    """A `Source` read off the live blend through the same path the door uses, with the two
    filesystem facts a headless fixture cannot set overridden."""
    live = export_tank.Source.live(scene or bpy.context.window.scene)
    return dataclasses.replace(live, filepath=filepath, is_dirty=is_dirty)


def write_library(name):
    """Write a one-object library blend. Purges first: `bpy.data.objects.new` appends `.001` to a
    name another local datablock already holds, and the donor has to keep the name exactly."""
    purge()
    donor = bpy.data.objects.new(name, triangle_mesh(name))
    assert donor.name == name, "the donor was renamed to {}".format(donor.name)
    path = os.path.join(_WORK, "library-{}.blend".format(name))
    bpy.data.libraries.write(path, {donor}, fake_user=True)
    return path


def link_object(path, name):
    """Link one object out of a library blend into the live scene. The only way to get linked model
    data, or two export-bound objects under one name, into a scene."""
    with bpy.data.libraries.load(path, link=True) as (_source, target):
        target.objects = [name]
    linked = target.objects[0]
    assert linked is not None, "{} does not hold objects[{}]".format(path, name)
    bpy.context.window.scene.collection.objects.link(linked)
    return linked


# ── assertions ───────────────────────────────────────────────────────────────────────────────────

def of(findings, check_id):
    return [finding for finding in findings if finding.check.id == check_id]


def assert_silent(findings, check_id):
    hits = of(findings, check_id)
    assert not hits, "{} fired on the clean fixture: {}".format(check_id, hits[0].evidence)


def assert_fires(findings, check_id, severity):
    hits = of(findings, check_id)
    assert hits, "{} did not fire; report held {}".format(
        check_id, sorted({finding.check.id for finding in findings}) or "nothing"
    )
    for finding in hits:
        assert finding.check.severity == severity, "{} came back {}, not {}".format(
            check_id, finding.check.severity.label, severity.label
        )
        assert finding.evidence and finding.repair, "{} came back without evidence or repair".format(
            check_id
        )


def assert_exit(findings, code):
    assert report.exit_code(findings) == code, "exit {} for {}".format(
        report.exit_code(findings), report.summary(findings)
    )


CASES = []


def case(function):
    CASES.append(function)
    return function


# ── the clean fixture is clean ───────────────────────────────────────────────────────────────────

@case
def clean_source_has_no_findings():
    findings = export_tank.lint(source_of(clean_scene()))
    assert not findings, "the clean fixture reported {}".format(report.render_text(findings))
    assert_exit(findings, 0)


# ── L1.SAVED_SOURCE ──────────────────────────────────────────────────────────────────────────────

@case
def saved_source_unsaved_blend():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.SAVED_SOURCE")
    findings = export_tank.lint(source_of(scene, filepath=""))
    assert_fires(findings, "L1.SAVED_SOURCE", Severity.ERROR)
    assert_exit(findings, 1)


@case
def saved_source_wrong_layout():
    scene = clean_scene()
    stray = os.path.join(_WORK, "scratch", "testbed.blend")
    os.makedirs(os.path.dirname(stray), exist_ok=True)
    assert_silent(export_tank.lint(source_of(scene)), "L1.SAVED_SOURCE")
    assert_fires(export_tank.lint(source_of(scene, filepath=stray)), "L1.SAVED_SOURCE", Severity.ERROR)


@case
def saved_source_unsaved_changes():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.SAVED_SOURCE")
    assert_fires(
        export_tank.lint(source_of(scene, is_dirty=True)), "L1.SAVED_SOURCE", Severity.ERROR
    )


@case
def saved_source_missing_spec():
    scene = clean_scene()
    lonely = os.path.join(_WORK, "assets", "lonely", "lonely.blend")
    os.makedirs(os.path.dirname(lonely), exist_ok=True)
    assert_silent(export_tank.lint(source_of(scene)), "L1.SAVED_SOURCE")
    assert_fires(
        export_tank.lint(source_of(scene, filepath=lonely)), "L1.SAVED_SOURCE", Severity.ERROR
    )


# ── L1.EXPORT_SCOPE ──────────────────────────────────────────────────────────────────────────────

@case
def export_scope_empty_scene():
    clean_scene()
    assert_silent(export_tank.lint(source_of()), "L1.EXPORT_SCOPE")
    empty = bpy.data.scenes.new("Workbench")
    assert_fires(export_tank.lint(source_of(empty)), "L1.EXPORT_SCOPE", Severity.ERROR)


@case
def export_scope_unscoped_parent():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.EXPORT_SCOPE")
    scene.collection.objects.unlink(bpy.data.objects["Hull"])
    assert_fires(export_tank.lint(source_of(scene)), "L1.EXPORT_SCOPE", Severity.ERROR)


@case
def export_scope_foreign_object_type():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.EXPORT_SCOPE")
    scene.collection.objects.link(
        bpy.data.objects.new("Headlight", bpy.data.lights.new("Headlight", type="POINT"))
    )
    assert_fires(export_tank.lint(source_of(scene)), "L1.EXPORT_SCOPE", Severity.ERROR)


# ── L1.LOCAL_MODEL_DATA ──────────────────────────────────────────────────────────────────────────

@case
def local_model_data_linked_object():
    library = write_library("Sponson")
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.LOCAL_MODEL_DATA")
    link_object(library, "Sponson")
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.LOCAL_MODEL_DATA", Severity.ERROR)
    kinds = {finding.subject.kind for finding in of(findings, "L1.LOCAL_MODEL_DATA")}
    assert report.SubjectKind.OBJECT in kinds and report.SubjectKind.MESH in kinds, (
        "the linked object and its linked mesh are two subjects; got {}".format(kinds)
    )


# ── L1.MODIFIER_STACK ────────────────────────────────────────────────────────────────────────────

@case
def modifier_stack_any_modifier():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.MODIFIER_STACK")
    modifier = bpy.data.objects["Hull"].modifiers.new(name="Smooth by Angle", type="NODES")
    modifier.show_viewport = False
    modifier.show_render = False
    assert_fires(export_tank.lint(source_of(scene)), "L1.MODIFIER_STACK", Severity.ERROR)


# ── L1.DEFORMATION ───────────────────────────────────────────────────────────────────────────────

@case
def deformation_shape_keys():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.DEFORMATION")
    bpy.data.objects["Hull"].shape_key_add(name="Basis")
    assert_fires(export_tank.lint(source_of(scene)), "L1.DEFORMATION", Severity.ERROR)


@case
def deformation_armature_binding():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.DEFORMATION")
    rig = bpy.data.objects.new("Rig", bpy.data.armatures.new("Rig"))
    bpy.data.objects["Hull"].modifiers.new(name="Armature", type="ARMATURE").object = rig
    assert_fires(export_tank.lint(source_of(scene)), "L1.DEFORMATION", Severity.ERROR)


# ── L1.ANIMATION ─────────────────────────────────────────────────────────────────────────────────

@case
def animation_object_action():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.ANIMATION")
    bpy.data.objects["Hull"].animation_data_create().action = bpy.data.actions.new("Recoil")
    assert_fires(export_tank.lint(source_of(scene)), "L1.ANIMATION", Severity.ERROR)


@case
def animation_mesh_nla_strip():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.ANIMATION")
    anim = bpy.data.meshes["Hull"].animation_data_create()
    anim.nla_tracks.new().strips.new("Recoil", 1, bpy.data.actions.new("Recoil"))
    assert_fires(export_tank.lint(source_of(scene)), "L1.ANIMATION", Severity.ERROR)


@case
def animation_shape_key_driver():
    scene = clean_scene()
    bpy.data.objects["Hull"].shape_key_add(name="Basis")
    bpy.data.objects["Hull"].shape_key_add(name="Dent")
    keys = bpy.data.meshes["Hull"].shape_keys
    assert_silent(export_tank.lint(source_of(scene)), "L1.ANIMATION")
    keys.driver_add('key_blocks["Dent"].value')
    assert_fires(export_tank.lint(source_of(scene)), "L1.ANIMATION", Severity.ERROR)


@case
def animation_empty_animdata_is_clean():
    """The law's own exemption: AnimData with no action, strip or driver is not animation."""
    scene = clean_scene()
    bpy.data.objects["Hull"].animation_data_create()
    bpy.data.meshes["Hull"].animation_data_create()
    findings = export_tank.lint(source_of(scene))
    assert_silent(findings, "L1.ANIMATION")
    assert_exit(findings, 0)


# ── L1.TRANSFORM_FINITE ──────────────────────────────────────────────────────────────────────────

@case
def transform_finite_nan_translation():
    """NaN, not infinity: Blender clamps a written infinity to FLT_MAX (measured, 5.1.2), so NaN is
    the only non-finite value a transform channel can actually hold."""
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.TRANSFORM_FINITE")
    bpy.data.objects["Turret"].location[0] = float("nan")
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.TRANSFORM_FINITE", Severity.ERROR)
    channels = {finding.subject.element for finding in of(findings, "L1.TRANSFORM_FINITE")}
    assert "channel `location`" in channels, "the authored channel is unnamed: {}".format(channels)
    assert "channel `matrix_local`" in channels, (
        "the composed matrix did not carry the NaN — view_layer.update() did not run: {}".format(
            channels
        )
    )


# ── L1.HANDEDNESS ────────────────────────────────────────────────────────────────────────────────

@case
def handedness_mirrored_node():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.HANDEDNESS")
    bpy.data.objects["Turret"].scale = (-1.0, 1.0, 1.0)
    assert_fires(export_tank.lint(source_of(scene)), "L1.HANDEDNESS", Severity.ERROR)


@case
def handedness_collapsed_node():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.HANDEDNESS")
    bpy.data.objects["Turret"].scale = (0.0, 1.0, 1.0)
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.HANDEDNESS", Severity.ERROR)
    assert math.isclose(
        bpy.data.objects["Turret"].matrix_local.to_3x3().determinant(), 0.0, abs_tol=0.0
    ), "the fixture is not the zero-determinant case"


# ── L1.UNAPPLIED_SCALE ───────────────────────────────────────────────────────────────────────────

@case
def unapplied_scale_is_a_warning_that_does_not_fail():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.UNAPPLIED_SCALE")
    bpy.data.objects["Turret"].scale = (2.0, 2.0, 2.0)
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.UNAPPLIED_SCALE", Severity.WARNING)
    assert_exit(findings, 0)


@case
def unapplied_scale_reads_delta_scale():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.UNAPPLIED_SCALE")
    bpy.data.objects["Turret"].delta_scale = (1.0, 1.0, 2.0)
    assert_fires(export_tank.lint(source_of(scene)), "L1.UNAPPLIED_SCALE", Severity.WARNING)


# ── L1.UNIQUE_NAMES ──────────────────────────────────────────────────────────────────────────────

@case
def unique_names_duplicate_across_a_library():
    library = write_library("Hull")
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.UNIQUE_NAMES")
    linked = link_object(library, "Hull")
    assert linked.name == "Hull", "the fixture did not produce a shared name: {}".format(linked.name)
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.UNIQUE_NAMES", Severity.ERROR)
    assert len(of(findings, "L1.UNIQUE_NAMES")) == 1, "one shared name is one finding"


@case
def unique_names_empty_name():
    """`obj.name = ""` yields `Object.001` (measured, 5.1.2), so no bpy call reaches this branch —
    only a blend another writer produced. The check reads nothing but `.name`, so the fixture is a
    subject that has one."""
    class Nameless:
        name = ""

    scene = clean_scene()
    clean = export_tank.check_unique_names(source_of(scene))
    assert not clean, "the clean fixture named something empty"
    findings = report.sorted_findings(
        export_tank.check_unique_names(dataclasses.replace(source_of(scene), objects=[Nameless()]))
    )
    assert_fires(findings, "L1.UNIQUE_NAMES", Severity.ERROR)
    assert_exit(findings, 1)


# ── L1.DEFAULT_NAMES ─────────────────────────────────────────────────────────────────────────────

@case
def default_names_object():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.DEFAULT_NAMES")
    bpy.data.objects["Hull"].name = "Cube"
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.DEFAULT_NAMES", Severity.WARNING)
    assert_exit(findings, 0)


@case
def default_names_survive_the_copy_suffix():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.DEFAULT_NAMES")
    bpy.data.objects["Hull"].name = "Suzanne.007"
    assert_fires(export_tank.lint(source_of(scene)), "L1.DEFAULT_NAMES", Severity.WARNING)


@case
def default_names_mesh_and_material():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.DEFAULT_NAMES")
    bpy.data.meshes["Hull"].name = "Mesh"
    bpy.data.materials["Steel"].name = "Material"
    findings = of(export_tank.lint(source_of(scene)), "L1.DEFAULT_NAMES")
    kinds = {finding.subject.kind for finding in findings}
    assert kinds == {report.SubjectKind.MESH, report.SubjectKind.MATERIAL}, (
        "expected the mesh and the material, got {}".format(kinds)
    )


@case
def default_names_leave_a_deliberate_name_alone():
    """Exact case only. `hull_cube` and `cube` are somebody's decision, not Blender's."""
    scene = clean_scene()
    bpy.data.objects["Hull"].name = "cube"
    bpy.data.meshes["Hull"].name = "Hull_Cube"
    assert_silent(export_tank.lint(source_of(scene)), "L1.DEFAULT_NAMES")


# ── the door's own modes ─────────────────────────────────────────────────────────────────────────

@case
def unimplemented_modes_refuse_by_name():
    for mode in ("export", "verify"):
        findings = export_tank.run(mode)
        assert len(findings) == 1, "{} reported {} findings".format(mode, len(findings))
        assert_fires(findings, "door.mode-unimplemented", Severity.ERROR)
        assert findings[0].subject.name == mode
        assert_exit(findings, 1)


@case
def lint_mode_reads_the_live_blend():
    """`run('lint')` builds its Source from the open blend rather than from a fixture, and this
    process never saved one — so a clean scene reports L1.SAVED_SOURCE and nothing else."""
    clean_scene()
    findings = export_tank.run("lint")
    assert [finding.check.id for finding in findings] == ["L1.SAVED_SOURCE"], (
        "run('lint') reported {}".format([finding.check.id for finding in findings])
    )
    assert_exit(findings, 1)


# ── the report shape ─────────────────────────────────────────────────────────────────────────────

@case
def report_sorts_errors_before_warnings_and_renders_both_ways():
    scene = clean_scene()
    bpy.data.objects["Turret"].scale = (2.0, 2.0, 2.0)          # warning
    bpy.data.objects["Hull"].modifiers.new(name="Bevel", type="BEVEL")  # error
    findings = export_tank.lint(source_of(scene))
    severities = [finding.check.severity for finding in findings]
    assert severities == sorted(severities), "the report is not in severity order: {}".format(
        [severity.label for severity in severities]
    )
    assert_exit(findings, 1)

    rendered = json.loads(report.render_json(findings))["findings"]
    assert [row["check"] for row in rendered] == [finding.check.id for finding in findings], (
        "the JSON rendering holds different rows in a different order than the text one"
    )
    text = report.render_text(findings)
    for finding in findings:
        assert finding.check.law in text and finding.repair in text, (
            "{} lost its law or repair in the text rendering".format(finding.check.id)
        )


@case
def report_sort_is_independent_of_discovery_order():
    scene = clean_scene()
    bpy.data.objects["Turret"].scale = (2.0, 2.0, 2.0)
    bpy.data.objects["Hull"].modifiers.new(name="Bevel", type="BEVEL")
    findings = export_tank.lint(source_of(scene))
    shuffled = report.sorted_findings(list(reversed(findings)))
    assert shuffled == findings, "the sort does not settle: {} vs {}".format(
        [finding.check.id for finding in shuffled], [finding.check.id for finding in findings]
    )


# ── runner ───────────────────────────────────────────────────────────────────────────────────────

def run_cases():
    failed = []
    for function in CASES:
        try:
            function()
        except Exception:  # noqa: BLE001 — a failed case is reported, never fatal to the run
            failed.append(function.__name__)
            print("FAIL  {}".format(function.__name__))
            print(traceback.format_exc())
        else:
            print("ok    {}".format(function.__name__))
    print("\ntest_tank_lint ▸ {} cases, {} passed, {} failed".format(
        len(CASES), len(CASES) - len(failed), len(failed)
    ))
    if failed:
        print("failed: {}".format(", ".join(failed)))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(run_cases())
