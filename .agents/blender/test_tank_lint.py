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

import contextlib
import dataclasses
import io
import json
import math
import os
import struct
import subprocess
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
        bpy.data.actions, bpy.data.armatures, bpy.data.images,
    ):
        for datablock in list(collection):
            collection.remove(datablock)


def triangle_mesh(name, positions=((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)),
                  edges=(), faces=((0, 1, 2),)):
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(list(positions), list(edges), list(faces))
    mesh.update()
    return mesh


def reshape(name, **geometry):
    """Give an existing object a differently shaped mesh under the name it already had, so a case
    that mutates topology still reads as one defect on one part."""
    obj = bpy.data.objects[name]
    materials = [slot.material for slot in obj.material_slots]
    previous = obj.data
    # Reassign before removing: a mesh datablock takes the objects that use it with it.
    obj.data = triangle_mesh(name + ".reshaped", **geometry)
    bpy.data.meshes.remove(previous)
    obj.data.name = name
    for material in materials:
        obj.data.materials.append(material)
    return obj.data


def unwrap(mesh, coordinates=((0.0, 0.0), (1.0, 0.0), (0.0, 1.0)), name="UVMap"):
    """One UV layer holding a real triangle. A layer Blender just made is all zeroes, which is the
    collapsed case, not the clean one."""
    layer = mesh.uv_layers.new(name=name)
    for index, uv in enumerate(coordinates):
        layer.uv[index].vector = uv
    return layer


def stored_image(name):
    """An 8x8 PNG written beside the fixtures and loaded back. Not `Image.pack()`: a generated
    image reports packed nothing (measured, 5.1.2), and L1.TEXTURE_SOURCE would then read it as the
    defect it is."""
    image = bpy.data.images.new(name, 8, 8)
    image.filepath_raw = os.path.join(_WORK, name + ".png")
    image.file_format = "PNG"
    image.save()
    return image


def textured_material(name, uv_map=None, coordinate=None):
    """A material whose Principled BSDF samples one stored image. `uv_map` drives the texture from
    a UV Map node naming that layer; `coordinate` drives it from a Texture Coordinate output.
    Neither, and the texture falls back the way Blender does: the active-render UV layer."""
    material = bpy.data.materials.new(name)
    tree = material.node_tree
    tree.nodes.clear()
    output = tree.nodes.new("ShaderNodeOutputMaterial")
    shader = tree.nodes.new("ShaderNodeBsdfPrincipled")
    texture = tree.nodes.new("ShaderNodeTexImage")
    texture.image = stored_image(name)
    tree.links.new(shader.outputs["BSDF"], output.inputs["Surface"])
    tree.links.new(texture.outputs["Color"], shader.inputs["Base Color"])
    if uv_map is not None:
        node = tree.nodes.new("ShaderNodeUVMap")
        node.uv_map = uv_map
        tree.links.new(node.outputs["UV"], texture.inputs["Vector"])
    elif coordinate is not None:
        node = tree.nodes.new("ShaderNodeTexCoord")
        tree.links.new(node.outputs[coordinate], texture.inputs["Vector"])
    return material


def sampled_hull(unwrapped=True, **material):
    """The clean tank with its hull unwrapped and wearing a texture-sampling material — what the
    sampled-UV laws need in front of them before they measure anything at all."""
    scene = clean_scene()
    mesh = bpy.data.meshes["Hull"]
    if unwrapped:
        unwrap(mesh)
    mesh.materials[0] = textured_material("Painted", **material)
    return scene


def git(directory, *arguments):
    subprocess.run(("git",) + arguments, cwd=directory, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def commit_blend(name, contents=None):
    """A git worktree holding one committed `assets/<name>/<name>.blend` — the baseline
    L1.SOURCE_CENSUS resolves from HEAD. `contents` replaces the blend with literal bytes, which is
    how the LFS-pointer case gets a baseline whose object this clone does not hold."""
    top = os.path.join(_WORK, "repo-" + name)
    directory = os.path.join(top, "assets", name)
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, name + ".blend")
    with open(os.path.join(directory, name + ".tank.ron"), "w", encoding="utf-8") as handle:
        handle.write("()\n")
    if contents is None:
        # `copy=True`: the fixture writes a blend without becoming it, so the rest of the run still
        # has the never-saved session `lint_mode_reads_the_live_blend` measures.
        bpy.ops.wm.save_as_mainfile(filepath=path, copy=True)
    else:
        with open(path, "wb") as handle:
            handle.write(contents)
    git(top, "init", "-q")
    git(top, "config", "user.email", "lint@overmatch.test")
    git(top, "config", "user.name", "tank lint")
    git(top, "add", "-A")
    git(top, "commit", "-q", "-m", "baseline")
    return path


def census_rows(findings):
    """The census as `{row label: measured}` — the rendering a human reads, keyed."""
    return {finding.subject.element: finding.evidence for finding in of(findings, "L1.SOURCE_CENSUS")}


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


#: A stand-in registry vocabulary. WHICH names are keys is the Rust generator's business and is
#: pinned there; what the source pass does with them is this file's.
CANON_KEYS = ("RHA", "MildSteel", "Rubber")


def canon(*node_references, substance_keys=CANON_KEYS):
    """A canon file's contents, as `Canon.read` would return them."""
    return export_tank.Canon(tuple(node_references), frozenset(substance_keys))


#: The canon a case that is not about the canon gets: no reference to resolve, the vocabulary above.
CANON = canon()


def source_of(scene=None, filepath=BLEND_PATH, canonical=CANON):
    """A `Source` read off the live blend through the same path the door uses, with the two facts a
    headless fixture cannot set — the stored path and the canon file — overridden."""
    live = export_tank.Source.live(scene or bpy.context.window.scene)
    return dataclasses.replace(live, filepath=filepath, canon=canonical)


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


#: The one path the canonical material library is identified by, built where a repository would hold
#: it. Nothing under the real `assets/` is read: the laws measure the path relationship, so a
#: fixture library standing at the same relationship IS the canonical library to them.
MATERIAL_LIBRARY = os.path.join(_WORK, "assets", "materials", "materials.blend")


def write_material_library(*names, path=MATERIAL_LIBRARY):
    """Write a library blend holding one material per name. Purges first, for `write_library`'s
    reason: a local datablock already holding the name would push the donor to `.001`."""
    purge()
    donors = set()
    for name in names:
        material = bpy.data.materials.new(name)
        assert material.name == name, "the donor was renamed to {}".format(material.name)
        donors.add(material)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    bpy.data.libraries.write(path, donors, fake_user=True)
    return path


def link_material(name, path=MATERIAL_LIBRARY):
    """Link one material out of a library blend. A linked material's name is read-only, which is
    exactly the identity the substance law rests on."""
    with bpy.data.libraries.load(path, link=True) as (_source, target):
        target.materials = [name]
    linked = target.materials[0]
    assert linked is not None, "{} does not hold materials[{}]".format(path, name)
    assert linked.library is not None, "{} came back local".format(name)
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
def clean_source_reports_nothing_but_its_census():
    """The census is INFO and always present, so a clean source is a report holding only census
    rows — never an empty one."""
    findings = export_tank.lint(source_of(clean_scene()))
    other = [finding for finding in findings if finding.check.id != "L1.SOURCE_CENSUS"]
    assert not other, "the clean fixture reported {}".format(report.render_text(other))
    assert findings, "the census did not report the clean fixture at all"
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


# ── L1.SPEC_REFERENCES ───────────────────────────────────────────────────────────────────────────

@case
def spec_references_resolve_to_one_object_each():
    """Every field the canon file can carry, resolving. Python knows none of these names — they
    arrive in the document."""
    scene = clean_scene()
    resolving = canon(("volumes", "Hull"), ("servos", "Turret"), ("views.node", "Muzzle"))
    assert_silent(export_tank.lint(source_of(scene, canonical=resolving)), "L1.SPEC_REFERENCES")


@case
def spec_references_a_node_the_scene_does_not_hold():
    scene = clean_scene()
    absent = canon(("volumes", "Hull"), ("volumes", "Sponson"))
    findings = export_tank.lint(source_of(scene, canonical=absent))
    assert_fires(findings, "L1.SPEC_REFERENCES", Severity.ERROR)
    hits = of(findings, "L1.SPEC_REFERENCES")
    assert len(hits) == 1, "only the unresolved reference is a finding: {}".format(hits)
    assert hits[0].subject.name == "Sponson", hits[0].subject
    assert hits[0].subject.element == "declared in `volumes`", hits[0].subject
    assert "0 export-bound object(s)" in hits[0].evidence, hits[0].evidence
    assert_exit(findings, 1)


@case
def spec_references_a_name_two_objects_carry():
    """A reference to a shared name addresses whichever node the exporter writes second, which is
    nobody's decision."""
    library = write_library("Hull")
    scene = clean_scene()
    link_object(library, "Hull")
    findings = export_tank.lint(source_of(scene, canonical=canon(("volumes", "Hull"))))
    assert_fires(findings, "L1.SPEC_REFERENCES", Severity.ERROR)
    assert "2 export-bound object(s)" in of(findings, "L1.SPEC_REFERENCES")[0].evidence


# ── L1.NONEMPTY_MESH ─────────────────────────────────────────────────────────────────────────────

@case
def nonempty_mesh_without_polygons():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.NONEMPTY_MESH")
    reshape("Hull", edges=((0, 1), (1, 2)), faces=())
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.NONEMPTY_MESH", Severity.ERROR)
    assert_exit(findings, 1)


# ── L1.FINITE_MESH_DATA ──────────────────────────────────────────────────────────────────────────

@case
def finite_mesh_data_nan_position():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.FINITE_MESH_DATA")
    bpy.data.meshes["Hull"].vertices[1].co[0] = float("nan")
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.FINITE_MESH_DATA", Severity.ERROR)
    named = [finding.subject.element for finding in of(findings, "L1.FINITE_MESH_DATA")]
    assert any("vertex positions" in element for element in named), (
        "the attribute that holds the NaN is unnamed: {}".format(named)
    )
    # No corner-normal fixture exists, and this records why: 5.1.2 sanitizes derived normals. A
    # face on a NaN position comes back (0,0,1), and a custom split normal written as NaN comes
    # back as the face normal. The check still reads them, because the law names them and a Blender
    # that stops sanitizing would otherwise ship the NaN into the accessor unseen.
    assert all(math.isfinite(value) for normal in bpy.data.meshes["Hull"].corner_normals
               for value in normal.vector), (
        "5.1.2 no longer sanitizes derived normals — this case can now pin the corner-normal read"
    )
    assert_exit(findings, 1)


@case
def finite_mesh_data_nan_colour_attribute():
    scene = clean_scene()
    colours = bpy.data.meshes["Hull"].color_attributes.new(
        name="Weathering", type="FLOAT_COLOR", domain="CORNER"
    )
    assert_silent(export_tank.lint(source_of(scene)), "L1.FINITE_MESH_DATA")
    colours.data[0].color = (float("nan"), 0.0, 0.0, 1.0)
    assert_fires(export_tank.lint(source_of(scene)), "L1.FINITE_MESH_DATA", Severity.ERROR)


@case
def finite_mesh_data_reads_a_layer_no_texture_samples():
    """The two UV laws have different scopes: this one reads every layer export writes, and
    L1.UV_FINITE only the layers a texture actually samples."""
    scene = clean_scene()
    layer = unwrap(bpy.data.meshes["Hull"])
    assert_silent(export_tank.lint(source_of(scene)), "L1.FINITE_MESH_DATA")
    layer.uv[1].vector = (float("nan"), 0.0)
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.FINITE_MESH_DATA", Severity.ERROR)
    assert_silent(findings, "L1.UV_FINITE")


# ── L1.ZERO_AREA_TRIANGLE ────────────────────────────────────────────────────────────────────────

@case
def zero_area_triangle_coincident_vertices():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.ZERO_AREA_TRIANGLE")
    reshape("Hull", positions=((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (1.0, 0.0, 0.0)))
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.ZERO_AREA_TRIANGLE", Severity.ERROR)
    assert_exit(findings, 1)


@case
def zero_area_triangle_has_no_tolerance():
    """A triangle whose cross product underflows float32 but not float64 is not zero. The law says
    exactly non-zero in stored coordinates, so the arithmetic has to be wider than the storage."""
    scene = clean_scene()
    tiny = 1.0e-25
    reshape("Hull", positions=((0.0, 0.0, 0.0), (tiny, 0.0, 0.0), (0.0, tiny, 0.0)))
    assert_silent(export_tank.lint(source_of(scene)), "L1.ZERO_AREA_TRIANGLE")


# ── L1.DUPLICATE_TRIANGLE ────────────────────────────────────────────────────────────────────────

@case
def duplicate_triangle_reversed_copy_on_welded_positions():
    """Distinct vertices at the same coordinates, wound the other way — still one surface twice."""
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.DUPLICATE_TRIANGLE")
    reshape(
        "Hull",
        positions=((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0),
                   (0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)),
        faces=((0, 1, 2), (5, 4, 3)),
    )
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.DUPLICATE_TRIANGLE", Severity.ERROR)
    assert_exit(findings, 1)


# ── L1.LOOSE_GEOMETRY ────────────────────────────────────────────────────────────────────────────

@case
def loose_geometry_is_a_warning_that_does_not_fail():
    """A vertex no polygon uses, and no loose edge at all — half the law on its own."""
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.LOOSE_GEOMETRY")
    reshape(
        "Hull",
        positions=((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0)),
    )
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.LOOSE_GEOMETRY", Severity.WARNING)
    assert "1 of 4 vertices and 0 of 3 edges" in of(findings, "L1.LOOSE_GEOMETRY")[0].evidence
    assert_exit(findings, 0)


@case
def loose_geometry_reads_an_edge_no_polygon_uses():
    """The other half, and the shape the live tank is actually in: an edge bridging two faces,
    belonging to neither, with not one loose vertex to give it away."""
    scene = clean_scene()
    reshape(
        "Hull",
        positions=((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0),
                   (2.0, 0.0, 0.0), (3.0, 0.0, 0.0), (2.0, 1.0, 0.0)),
        edges=((0, 3),),
        faces=((0, 1, 2), (3, 4, 5)),
    )
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.LOOSE_GEOMETRY", Severity.WARNING)
    assert "0 of 6 vertices and 1 of 7 edges" in of(findings, "L1.LOOSE_GEOMETRY")[0].evidence


# ── L1.TEXTURE_UV_SOURCE ─────────────────────────────────────────────────────────────────────────

@case
def texture_uv_source_named_layer_the_mesh_does_not_carry():
    assert_silent(export_tank.lint(source_of(sampled_hull())), "L1.TEXTURE_UV_SOURCE")
    findings = export_tank.lint(source_of(sampled_hull(uv_map="Decals")))
    assert_fires(findings, "L1.TEXTURE_UV_SOURCE", Severity.ERROR)
    assert_exit(findings, 1)


@case
def texture_uv_source_named_layer_the_mesh_does_carry():
    """The same UV Map node, naming a layer that exists, is silent — the law is about resolution,
    not about which node drives the texture."""
    assert_silent(export_tank.lint(source_of(sampled_hull(uv_map="UVMap"))), "L1.TEXTURE_UV_SOURCE")


@case
def texture_uv_source_non_uv_coordinate():
    findings = export_tank.lint(source_of(sampled_hull(coordinate="Generated")))
    assert_fires(findings, "L1.TEXTURE_UV_SOURCE", Severity.ERROR)


@case
def texture_uv_source_no_uv_layer_at_all():
    findings = export_tank.lint(source_of(sampled_hull(unwrapped=False)))
    assert_fires(findings, "L1.TEXTURE_UV_SOURCE", Severity.ERROR)


# ── L1.UV_FINITE ─────────────────────────────────────────────────────────────────────────────────

@case
def uv_finite_nan_in_a_sampled_layer():
    scene = sampled_hull()
    assert_silent(export_tank.lint(source_of(scene)), "L1.UV_FINITE")
    bpy.data.meshes["Hull"].uv_layers["UVMap"].uv[2].vector = (0.0, float("nan"))
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.UV_FINITE", Severity.ERROR)
    assert_exit(findings, 1)


# ── L1.ZERO_AREA_UV ──────────────────────────────────────────────────────────────────────────────

@case
def zero_area_uv_is_a_warning_that_does_not_fail():
    scene = sampled_hull()
    assert_silent(export_tank.lint(source_of(scene)), "L1.ZERO_AREA_UV")
    for corner in bpy.data.meshes["Hull"].uv_layers["UVMap"].uv:
        corner.vector = (0.25, 0.75)
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.ZERO_AREA_UV", Severity.WARNING)
    assert_exit(findings, 0)


@case
def zero_area_uv_ignores_a_material_that_samples_nothing():
    """The law's own scope: a collapsed UV under an untextured substance is irrelevant, because
    nothing reads it."""
    scene = clean_scene()
    layer = unwrap(bpy.data.meshes["Hull"], ((0.25, 0.75), (0.25, 0.75), (0.25, 0.75)))
    assert layer.active_render, "the fixture's UV layer is not the one a texture would fall back to"
    assert_silent(export_tank.lint(source_of(scene)), "L1.ZERO_AREA_UV")


# ── L1.SUBSTANCE_IDENTITY ────────────────────────────────────────────────────────────────────────

#: A second library, standing somewhere a repository would not keep the canonical one. The law's
#: identity is the path relationship, so this file is a stranger however its materials are named.
SURPLUS_LIBRARY = os.path.join(_WORK, "assets", "surplus", "materials.blend")


def plated(name, path=MATERIAL_LIBRARY, local=False):
    """The clean tank with its hull wearing a material called `name` — linked out of the library at
    `path`, or authored locally under that name."""
    if not local:
        write_material_library(name, path=path)
    scene = clean_scene()
    material = bpy.data.materials.new(name) if local else link_material(name, path=path)
    assert material.name == name, "the fixture's material is called {}".format(material.name)
    bpy.data.meshes["Hull"].materials[0] = material
    return scene


@case
def substance_identity_the_canonical_library_s_own_material_is_clean():
    findings = export_tank.lint(source_of(plated("RHA")))
    assert_silent(findings, "L1.SUBSTANCE_IDENTITY")
    assert_exit(findings, 0)


@case
def substance_identity_leaves_a_material_that_is_no_substance_alone():
    """The hull's local `Steel`: not a key, not a near-miss, not linked. Ordinary art."""
    assert_silent(export_tank.lint(source_of(clean_scene())), "L1.SUBSTANCE_IDENTITY")


@case
def substance_identity_a_local_counterfeit():
    """The defect the law exists for: the exported glTF carries a material NAME, so a local
    datablock typed `RHA` would bind armour the library never issued."""
    findings = export_tank.lint(source_of(plated("RHA", local=True)))
    assert_fires(findings, "L1.SUBSTANCE_IDENTITY", Severity.ERROR)
    assert "local to this blend" in of(findings, "L1.SUBSTANCE_IDENTITY")[0].evidence
    assert_exit(findings, 1)


@case
def substance_identity_a_counterfeit_standing_beside_the_real_datablock():
    """Both wear the name `RHA`, so the two are one material to anything that reads names — which
    is what the exported glTF does. The law separates them by datablock, and refuses only the one
    the library never issued."""
    scene = plated("RHA")
    bpy.data.meshes["Turret"].materials.append(bpy.data.materials.new("RHA"))
    hits = of(export_tank.lint(source_of(scene)), "L1.SUBSTANCE_IDENTITY")
    assert len(hits) == 1, "expected the counterfeit alone, got {}".format(
        [(finding.subject.element, finding.evidence) for finding in hits]
    )
    assert hits[0].subject.element == "on object `Turret`", hits[0].subject
    assert "local to this blend" in hits[0].evidence, hits[0].evidence


@case
def substance_identity_a_registry_name_linked_from_some_other_library():
    """Linked is not the law; linked FROM `assets/materials/materials.blend` is."""
    findings = export_tank.lint(source_of(plated("RHA", path=SURPLUS_LIBRARY)))
    assert_fires(findings, "L1.SUBSTANCE_IDENTITY", Severity.ERROR)
    assert "surplus" in of(findings, "L1.SUBSTANCE_IDENTITY")[0].evidence


@case
def substance_identity_a_case_folded_near_miss():
    findings = export_tank.lint(source_of(plated("rha", local=True)))
    assert_fires(findings, "L1.SUBSTANCE_IDENTITY", Severity.ERROR)
    assert "reads as the registry key `RHA`" in of(findings, "L1.SUBSTANCE_IDENTITY")[0].evidence


@case
def substance_identity_a_copy_suffixed_near_miss():
    """`RHA.001` out of the canonical library itself — Blender's collision suffix on a real link is
    still not the key, and it is refused as the near-miss it is rather than as an unknown one."""
    findings = export_tank.lint(source_of(plated("RHA.001")))
    assert_fires(findings, "L1.SUBSTANCE_IDENTITY", Severity.ERROR)
    hits = of(findings, "L1.SUBSTANCE_IDENTITY")
    assert len(hits) == 1, "one material, one finding: {}".format(hits)
    assert "reads as the registry key `RHA`" in hits[0].evidence, hits[0].evidence


@case
def substance_identity_a_linked_material_the_registry_does_not_declare():
    findings = export_tank.lint(source_of(plated("Unobtainium")))
    assert_fires(findings, "L1.SUBSTANCE_IDENTITY", Severity.ERROR)
    assert "declares no such key" in of(findings, "L1.SUBSTANCE_IDENTITY")[0].evidence


# ── the canon file ───────────────────────────────────────────────────────────────────────────────

def write_canon(name, document):
    path = os.path.join(_WORK, "canon-{}.json".format(name))
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(document if isinstance(document, str) else json.dumps(document))
    return path


@case
def a_missing_canon_refuses_both_laws_that_are_stated_in_it():
    """A law whose canonical input never arrived has not passed. Each refuses under its own id, so
    no report can be read as the law itself having been evaluated."""
    scene = clean_scene()
    findings = export_tank.lint(source_of(scene, canonical=None))
    for check_id in ("L1.SPEC_REFERENCES", "L1.SUBSTANCE_IDENTITY"):
        assert_fires(findings, check_id + ".canon-missing", Severity.ERROR)
        assert_silent(findings, check_id)
        hits = of(findings, check_id + ".canon-missing")
        assert len(hits) == 1, "{} refused {} times".format(check_id, len(hits))
        assert export_tank.CANON_COMMAND in hits[0].repair, hits[0].repair
    assert_exit(findings, 1)


@case
def canon_read_takes_the_generator_s_document_and_nothing_else():
    path = write_canon("good", {
        "node_references": [{"field": "volumes", "node": "Hull"}],
        "substance_keys": ["RHA", "Rubber"],
    })
    read, note = export_tank.Canon.read(path)
    assert note is None, note
    assert read.node_references == (("volumes", "Hull"),), read.node_references
    assert read.substance_keys == frozenset({"RHA", "Rubber"}), read.substance_keys

    for absent, expected in (
        (None, "no --canon file"),
        (os.path.join(_WORK, "canon-never-written.json"), "not readable"),
        (write_canon("truncated", '{"node_references": ['), "not readable"),
        (write_canon("half", {"node_references": []}), "shape"),
        (write_canon("wrong", {"node_references": [{"node": "Hull"}], "substance_keys": []}),
         "shape"),
    ):
        read, note = export_tank.Canon.read(absent)
        assert read is None, "{} was read as a canon file".format(absent)
        assert expected in note, note


# ── L1.TEXTURE_SOURCE ────────────────────────────────────────────────────────────────────────────

@case
def texture_source_file_that_is_not_there():
    scene = sampled_hull()
    assert_silent(export_tank.lint(source_of(scene)), "L1.TEXTURE_SOURCE")
    image = bpy.data.images["Painted"]
    assert image.packed_file is None, "the fixture packed the image, so the file is not the source"
    os.remove(bpy.path.abspath(image.filepath))
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.TEXTURE_SOURCE", Severity.ERROR)
    assert_exit(findings, 1)


@case
def texture_source_node_holding_no_image():
    scene = sampled_hull()
    assert_silent(export_tank.lint(source_of(scene)), "L1.TEXTURE_SOURCE")
    for node in bpy.data.materials["Painted"].node_tree.nodes:
        if node.type == "TEX_IMAGE":
            node.image = None
    assert_fires(export_tank.lint(source_of(scene)), "L1.TEXTURE_SOURCE", Severity.ERROR)


# ── L1.SOURCE_CENSUS ─────────────────────────────────────────────────────────────────────────────

@case
def source_census_counts_the_live_source():
    """A primitive is a material slot the polygons reference, not an object and not a mesh — so the
    fixture makes the three counts disagree. A SUBSTANCE primitive is one wearing a library-linked
    material, so the hull's local `Steel` is a primitive that is not a substance, and the object is
    ballistic through the linked plate alone."""
    write_material_library("RHA")
    scene = clean_scene()
    mesh = reshape(
        "Hull",
        positions=((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0),
                   (2.0, 0.0, 0.0), (3.0, 0.0, 0.0), (2.0, 1.0, 0.0)),
        faces=((0, 1, 2), (3, 4, 5)),
    )
    mesh.materials.append(link_material("RHA"))
    mesh.polygons[1].material_index = 1
    rows = census_rows(export_tank.lint(source_of(scene)))
    assert rows["objects"] == "3" and rows["meshes"] == "2", rows
    assert rows["primitives"] == "3", rows
    assert rows["ballistic objects"] == "1", rows
    assert rows["substance `RHA`"] == "1 primitive(s)", rows
    assert "substance `Steel`" not in rows, "a local material was counted as a substance: {}".format(rows)


@case
def source_census_without_a_baseline_is_neither_pass_nor_fail():
    scene = clean_scene()
    findings = export_tank.lint(source_of(scene))
    rows = census_rows(findings)
    assert rows["baseline"].startswith("no source baseline"), rows["baseline"]
    assert "baseline" not in rows["objects"], "an absent baseline still printed a comparison"
    assert_fires(findings, "L1.SOURCE_CENSUS", Severity.INFO)
    assert_exit(findings, 0)


@case
def source_census_against_the_previous_commit():
    write_material_library("RHA")
    scene = clean_scene()
    bpy.data.meshes["Hull"].materials[0] = link_material("RHA")
    path = commit_blend("census")
    rows = census_rows(export_tank.lint(source_of(scene, filepath=path)))
    assert rows["baseline"].startswith("compared against"), rows["baseline"]
    assert rows["objects"] == "3 (baseline 3, +0)", rows["objects"]

    scene.collection.objects.link(bpy.data.objects.new("Sponson", triangle_mesh("Sponson")))
    findings = export_tank.lint(source_of(scene, filepath=path))
    rows = census_rows(findings)
    assert rows["objects"] == "4 (baseline 3, +1)", rows["objects"]
    assert rows["meshes"] == "3 (baseline 2, +1)", rows["meshes"]
    assert rows["substance `RHA`"] == "1 (baseline 1, +0) primitive(s)", rows
    assert_exit(findings, 0)


@case
def source_census_without_the_lfs_object():
    """The baseline is resolved offline: a pointer whose object this clone never fetched is an
    absent baseline, not a download and not a verdict."""
    scene = clean_scene()
    pointer = (
        b"version https://git-lfs.github.com/spec/v1\n"
        b"oid sha256:" + b"0" * 64 + b"\nsize 63000000\n"
    )
    path = commit_blend("lfs", contents=pointer)
    findings = export_tank.lint(source_of(scene, filepath=path))
    rows = census_rows(findings)
    assert "LFS object" in rows["baseline"], rows["baseline"]
    assert rows["objects"] == "3", rows["objects"]
    assert_exit(findings, 0)


# ── the door's own modes ─────────────────────────────────────────────────────────────────────────

def glb_json(path):
    """The JSON chunk of a glb, as a dict."""
    with open(path, "rb") as handle:
        magic, _version, _total = struct.unpack("<4sII", handle.read(12))
        assert magic == b"glTF", "{} is not a glb".format(path)
        length, kind = struct.unpack("<II", handle.read(8))
        assert kind == 0x4E4F534A, "{} does not start with a JSON chunk".format(path)
        return json.loads(handle.read(length))


@case
def an_l1_error_stops_before_the_raw_export():
    """A candidate cut from a refused source is a file nobody may consume, so it is never written."""
    clean_scene()
    bpy.data.objects["Hull"].modifiers.new(name="Bevel", type="BEVEL")
    path = os.path.join(_WORK, "raw-never-written.glb")
    findings = export_tank.run("export", raw=path)
    assert_fires(findings, "L1.MODIFIER_STACK", Severity.ERROR)
    assert not os.path.exists(path), "the exporter ran on a source the pass refused"
    assert_exit(findings, 1)


@case
def the_raw_export_writes_the_active_scene_only():
    """`use_active_scene=True` is what makes EXPORT-BOUND mean what the source pass measured: a
    workbench scene in the same file is outside the door and outside the bytes."""
    clean_scene()
    workbench = bpy.data.scenes.new("Workbench")
    workbench.collection.objects.link(bpy.data.objects.new("Jig", triangle_mesh("Jig")))
    path = os.path.join(_WORK, "raw-active-scene.glb")
    assert not export_tank.export_raw(path), "the clean fixture failed to export"
    names = {node.get("name") for node in glb_json(path).get("nodes", [])}
    assert "Hull" in names, "the exported document does not hold the active scene: {}".format(names)
    assert "Jig" not in names, "a workbench scene reached the candidate: {}".format(names)


@case
def the_raw_export_writes_no_animation():
    """The exporter animates by default. The explicit argument is defence against that default,
    beside — never instead of — L1.ANIMATION."""
    clean_scene()
    turret = bpy.data.objects["Turret"]
    # Through the keying path, not `actions.new`: 5.1's slotted actions hold no fcurves of their own.
    turret.keyframe_insert("location", frame=1)
    turret.location[0] = 1.0
    turret.keyframe_insert("location", frame=8)
    assert turret.animation_data.action is not None, "the fixture animated nothing"
    path = os.path.join(_WORK, "raw-no-animation.glb")
    assert not export_tank.export_raw(path), "the fixture failed to export"
    assert "animations" not in glb_json(path), "the candidate carries an animation clip"


@case
def a_resolved_library_is_not_an_unresolved_one():
    """The precondition measures placeholders, not links: a library that loaded is silent. The
    firing case needs a saved blend reopened without its library, and lives in the door's own
    end-to-end test."""
    write_material_library("RHA")
    plated("RHA")
    assert not export_tank.check_unresolved_library(), "a resolved library read as unresolved"


@case
def lint_mode_reads_the_live_blend():
    """`run('lint')` builds its Source from the open blend rather than from a fixture, and this
    process never saved one nor was given a canon — so a clean scene reports L1.SAVED_SOURCE, both
    canon refusals, and a census that says it has nothing to compare against."""
    clean_scene()
    findings = export_tank.run("lint")
    assert {finding.check.id for finding in findings} == {
        "L1.SAVED_SOURCE",
        "L1.SOURCE_CENSUS",
        "L1.SPEC_REFERENCES.canon-missing",
        "L1.SUBSTANCE_IDENTITY.canon-missing",
    }, "run('lint') reported {}".format([finding.check.id for finding in findings])
    assert "no source baseline" in census_rows(findings)["baseline"]
    assert_exit(findings, 1)


@case
def lint_mode_reads_the_canon_file_it_is_given():
    """The `--canon` wiring, end to end: `run` reads the file, and the law is evaluated against it
    rather than refused for want of it."""
    path = write_canon("wired", {
        "node_references": [{"field": "volumes", "node": "Sponson"}],
        "substance_keys": list(CANON_KEYS),
    })
    clean_scene()
    findings = export_tank.run("lint", path)
    assert_silent(findings, "L1.SPEC_REFERENCES.canon-missing")
    assert_silent(findings, "L1.SUBSTANCE_IDENTITY.canon-missing")
    assert_fires(findings, "L1.SPEC_REFERENCES", Severity.ERROR)
    assert of(findings, "L1.SPEC_REFERENCES")[0].subject.name == "Sponson"


@case
def the_command_line_carries_the_canon_path_to_the_pass():
    """The whole plumbing the wrapper will use: `--canon <file>` on the command line, through the
    parser, into the law that is stated in it."""
    path = write_canon("cli", {
        "node_references": [{"field": "volumes", "node": "Sponson"}],
        "substance_keys": list(CANON_KEYS),
    })
    clean_scene()
    argv, sys.argv = sys.argv, ["blender", "--", "--mode", "lint", "--canon", path]
    printed = io.StringIO()
    try:
        with contextlib.redirect_stdout(printed):
            code = export_tank.main()
    finally:
        sys.argv = argv
    text = printed.getvalue()
    assert "canon-missing" not in text, "the canon file did not reach the pass:\n{}".format(text)
    assert "L1.SPEC_REFERENCES error" in text and "Sponson" in text, text
    assert code == 1, "an error in the report is a non-zero exit, got {}".format(code)


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
