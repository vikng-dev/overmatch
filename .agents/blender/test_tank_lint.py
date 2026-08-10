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
import hashlib
import io
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import traceback

import bpy
import mathutils

_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.dirname(os.path.dirname(_HERE))
sys.path.insert(0, _HERE)
sys.path.insert(0, os.path.join(_ROOT, "scripts", "tank"))
sys.path.insert(0, os.path.join(_ROOT, "scripts"))

import export_tank  # noqa: E402
import report  # noqa: E402
import toolchain  # noqa: E402
from report import Severity  # noqa: E402


# ── fixture scaffolding ──────────────────────────────────────────────────────────────────────────

_WORK = tempfile.mkdtemp(prefix="tank-lint-test-")


def stored_at(stem, holder=None, collection="assets", extension=".blend", spec=True):
    """SAVE THE SESSION at the path a case is about, and return it.

    L1.SAVED_SOURCE measures a file on disk and the file this Blender has open, so its fixtures are
    real saves and not paths handed to a `Source`. Every case that moves the session puts it back
    with `stored_at("testbed")`, which is where the rest of the suite expects to be stored.
    """
    directory = os.path.join(_WORK, collection, holder or stem)
    os.makedirs(directory, exist_ok=True)
    if spec:
        with open(os.path.join(directory, stem + ".tank.ron"), "w", encoding="utf-8") as handle:
            handle.write("()\n")
    path = os.path.join(directory, stem + extension)
    bpy.ops.wm.save_as_mainfile(filepath=path)
    assert bpy.data.filepath == path, "Blender stored the session at {}".format(bpy.data.filepath)
    return path


#: Where the suite's session is stored: the layout L1.SAVED_SOURCE requires, with a real sibling
#: spec beside it, so every case that is not about that law runs against a clean stored path.
BLEND_PATH = stored_at("testbed")


def purge():
    """Empty this .blend of everything a case can have left behind, libraries first — linked
    datablocks are freed by their library, not one at a time."""
    for library in list(bpy.data.libraries):
        bpy.data.libraries.remove(library)
    for scene in list(bpy.data.scenes):
        if scene is not bpy.context.window.scene:
            bpy.data.scenes.remove(scene)
    for collection in (
        bpy.data.objects, bpy.data.meshes, bpy.data.materials, bpy.data.node_groups,
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


def surface(name):
    """A material's own Material Output and Principled BSDF, cleared of Blender's defaults. Returns
    `(material, tree, shader)` — where every fixture below hangs a texture."""
    material = bpy.data.materials.new(name)
    tree = material.node_tree
    tree.nodes.clear()
    output = tree.nodes.new("ShaderNodeOutputMaterial")
    shader = tree.nodes.new("ShaderNodeBsdfPrincipled")
    tree.links.new(shader.outputs["BSDF"], output.inputs["Surface"])
    return (material, tree, shader)


def group_material(name, layer=None, spare=None, decoy_output=False):
    """A material whose texture lives inside a node GROUP, driven from outside through the group's
    own input socket — the shape a walk that stops at Group Input misreads.

    NOTHING here is in the first interface slot: the used input and the read output are both
    declared SECOND, so a walk matching sockets positionally instead of by identifier crosses the
    interface into the wrong one.

    `spare` adds a second group output carrying a second texture and connects it to nothing
    outside; its value is the image that texture holds, `False` meaning the broken one nothing
    reads. `decoy_output` adds an inactive Group Output node, which Blender does not evaluate.
    """
    material, tree, shader = surface(name)

    group = bpy.data.node_groups.new(name + "_Group", "ShaderNodeTree")
    group.interface.new_socket("Unused", in_out="INPUT", socket_type="NodeSocketVector")
    group.interface.new_socket("Vec", in_out="INPUT", socket_type="NodeSocketVector")
    if spare is not None:
        group.interface.new_socket("Spare", in_out="OUTPUT", socket_type="NodeSocketColor")
    group.interface.new_socket("Colour", in_out="OUTPUT", socket_type="NodeSocketColor")
    inner_in = group.nodes.new("NodeGroupInput")
    if decoy_output:
        stale = group.nodes.new("NodeGroupOutput")
        ignored = group.nodes.new("ShaderNodeTexImage")
        group.links.new(ignored.outputs["Color"], stale.inputs["Colour"])
    inner_out = group.nodes.new("NodeGroupOutput")
    if decoy_output:
        # Blender keeps the FIRST output node active (measured, 5.1.2), so the flag is moved by
        # hand — the point of the fixture is that the active one is not the first one in the tree.
        stale.is_active_output = False
        inner_out.is_active_output = True
        assert inner_out.is_active_output and not stale.is_active_output, (
            "the fixture's decoy Group Output is the active one"
        )
    reached = group.nodes.new("ShaderNodeTexImage")
    reached.image = stored_image(name + "_inner")
    group.links.new(inner_in.outputs["Vec"], reached.inputs["Vector"])
    group.links.new(reached.outputs["Color"], inner_out.inputs["Colour"])
    if spare is not None:
        stray = group.nodes.new("ShaderNodeTexImage")
        stray.image = stored_image(name + "_stray") if spare else None
        group.links.new(stray.outputs["Color"], inner_out.inputs["Spare"])

    node = tree.nodes.new("ShaderNodeGroup")
    node.node_tree = group
    tree.links.new(node.outputs["Colour"], shader.inputs["Base Color"])
    if layer is not None:
        uv = tree.nodes.new("ShaderNodeUVMap")
        uv.uv_map = layer
        tree.links.new(uv.outputs["UV"], node.inputs["Vec"])
    return material


def chained_material(name, hops, layer=None, coordinate=None):
    """A texture driven through `hops` Mapping nodes. Every hop is a coordinate transform and none
    of them is a coordinate SOURCE, so the answer is whatever stands at the far end."""
    material, tree, shader = surface(name)
    texture = tree.nodes.new("ShaderNodeTexImage")
    texture.image = stored_image(name)
    tree.links.new(texture.outputs["Color"], shader.inputs["Base Color"])
    socket = texture.inputs["Vector"]
    for _hop in range(hops):
        mapping = tree.nodes.new("ShaderNodeMapping")
        tree.links.new(mapping.outputs["Vector"], socket)
        socket = mapping.inputs["Vector"]
    if layer is not None:
        uv = tree.nodes.new("ShaderNodeUVMap")
        uv.uv_map = layer
        tree.links.new(uv.outputs["UV"], socket)
    elif coordinate is not None:
        tree.links.new(tree.nodes.new("ShaderNodeTexCoord").outputs[coordinate], socket)
    return material


def muted_material(name, layer="UVMap", muted=True):
    """A Vector Math node between the UV Map and the texture. It is not a coordinate the door can
    carry, so the same graph is a UV source or a refusal on one flag: Blender evaluates a MUTED
    node through its internal link and skips what it does."""
    material, tree, shader = surface(name)
    texture = tree.nodes.new("ShaderNodeTexImage")
    texture.image = stored_image(name)
    tree.links.new(texture.outputs["Color"], shader.inputs["Base Color"])
    maths = tree.nodes.new("ShaderNodeVectorMath")
    maths.mute = muted
    uv = tree.nodes.new("ShaderNodeUVMap")
    uv.uv_map = layer
    tree.links.new(uv.outputs["UV"], maths.inputs[0])
    tree.links.new(maths.outputs["Vector"], texture.inputs["Vector"])
    return material


def nested_material(name):
    """ONE material holding two Image Texture nodes in two different node trees — where Blender
    names them the SAME thing, because it numbers a name inside its own tree only. The outer one is
    sound; the inner one holds no image."""
    material, tree, shader = surface(name)
    outer = tree.nodes.new("ShaderNodeTexImage")
    outer.image = stored_image(name + "_outer")
    tree.links.new(outer.outputs["Color"], shader.inputs["Base Color"])

    group = bpy.data.node_groups.new(name + "_Group", "ShaderNodeTree")
    group.interface.new_socket("Fac", in_out="OUTPUT", socket_type="NodeSocketFloat")
    inner_out = group.nodes.new("NodeGroupOutput")
    inner = group.nodes.new("ShaderNodeTexImage")
    group.links.new(inner.outputs["Color"], inner_out.inputs["Fac"])
    node = tree.nodes.new("ShaderNodeGroup")
    node.node_tree = group
    tree.links.new(node.outputs["Fac"], shader.inputs["Roughness"])
    assert outer.name == inner.name, "the fixture's two texture nodes are called {} and {}".format(
        outer.name, inner.name
    )
    return material


def write_textured_library(name, path=None):
    """A library blend holding one material that SAMPLES a texture, so a linked material and a
    local one of the same name each carry a node tree with an identically named node in it."""
    path = path or os.path.join(_WORK, "library-textured-{}.blend".format(name))
    purge()
    material, tree, shader = surface(name)
    assert material.name == name, "the donor was renamed to {}".format(material.name)
    texture = tree.nodes.new("ShaderNodeTexImage")
    texture.image = stored_image(name + "_library")
    tree.links.new(texture.outputs["Color"], shader.inputs["Base Color"])
    bpy.data.libraries.write(path, {material}, fake_user=True)
    return path


def hull_wearing(build, unwrapped=True):
    """The clean tank with its hull unwrapped and wearing the material `build(name)` makes — what
    the sampled-UV laws need in front of them before they measure anything at all."""
    scene = clean_scene()
    mesh = bpy.data.meshes["Hull"]
    if unwrapped:
        unwrap(mesh)
    mesh.materials[0] = build("Painted")
    return scene


def sampled_hull(unwrapped=True, **material):
    """The clean tank wearing one flat texture-sampling material."""
    return hull_wearing(lambda name: textured_material(name, **material), unwrapped=unwrapped)


def git(directory, *arguments):
    subprocess.run(("git",) + arguments, cwd=directory, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def commit_blend(name, contents=None, become=False):
    """A git worktree holding one committed `assets/<name>/<name>.blend` — the baseline
    L1.SOURCE_CENSUS resolves from HEAD. `contents` replaces the blend with literal bytes, which is
    how the LFS-pointer case gets a baseline whose object this clone does not hold; `become` stores
    the SESSION there, which is what a case comparing two censuses of one asset needs — both are
    then anchored on the same repository, as they are in the door."""
    top = os.path.join(_WORK, "repo-" + name)
    directory = os.path.join(top, "assets", name)
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, name + ".blend")
    with open(os.path.join(directory, name + ".tank.ron"), "w", encoding="utf-8") as handle:
        handle.write("()\n")
    if contents is None:
        # `copy=True`: the fixture writes a blend without becoming it, so the session stays stored
        # where `stored_at` put it and L1.SAVED_SOURCE stays silent for the rest of the run.
        # `relative_remap=False`: a committed tank blend holds `//../materials/materials.blend`
        # (MEASURED on the live tiger), and remapping would rewrite it to point back at where this
        # fixture was standing rather than beside where it now stands.
        bpy.ops.wm.save_as_mainfile(filepath=path, copy=not become, relative_remap=False)
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


def source_of(scene=None, filepath=None, canonical=CANON):
    """A `Source` read off the live blend through the same path the door uses, with the one fact a
    headless fixture cannot set — the canon file the wrapper writes — overridden.

    The stored path is NOT overridden by default: the session is really saved at `BLEND_PATH`, so
    `Source.live` reads it the way the door does. `filepath` exists for the census cases, which
    need the blend to stand in a git worktree somewhere else.
    """
    live = export_tank.Source.live(scene or bpy.context.window.scene)
    if filepath is not None:
        live = dataclasses.replace(live, filepath=filepath)
    return dataclasses.replace(live, canon=canonical)


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


#: The canonical material library of the repository the session's blend stands in: `BLEND_PATH` is
#: `<_WORK>/assets/testbed/testbed.blend`, so `<_WORK>` is the root and this is its one library.
#: Nothing under the real `assets/` is read.
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
    # As Blender stores it in a saved blend: `//` relative to the file that links it, which is what
    # lets a checkout move. MEASURED on the live tiger: `//../materials/materials.blend`.
    linked.library.filepath = bpy.path.relpath(path)
    assert linked.library.filepath.startswith("//"), linked.library.filepath
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

def only_saved_source(scene, expected):
    """The findings of one relocated session, asserting the law refused for exactly one reason. A
    fixture that trips two clauses proves neither."""
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.SAVED_SOURCE", Severity.ERROR)
    hits = of(findings, "L1.SAVED_SOURCE")
    assert len(hits) == 1, "the fixture tripped {} clauses: {}".format(
        len(hits), [finding.evidence for finding in hits]
    )
    assert expected in hits[0].evidence, hits[0].evidence
    return hits[0]


@case
def saved_source_unsaved_blend():
    """The one clause a saved session cannot reach: a Blender that has never written this model."""
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.SAVED_SOURCE")
    findings = export_tank.lint(source_of(scene, filepath=""))
    assert_fires(findings, "L1.SAVED_SOURCE", Severity.ERROR)
    hits = of(findings, "L1.SAVED_SOURCE")
    assert len(hits) == 1 and "never been written to disk" in hits[0].evidence, (
        "an unsaved blend is one refusal that says so, not the path clauses reading an empty "
        "string: {}".format([finding.evidence for finding in hits])
    )
    assert_exit(findings, 1)


@case
def saved_source_wrong_layout():
    scene = clean_scene()
    assert_silent(export_tank.lint(source_of(scene)), "L1.SAVED_SOURCE")
    try:
        stored_at("testbed", collection="scratch")
        only_saved_source(scene, "not assets/testbed/testbed.blend")
    finally:
        stored_at("testbed")


@case
def saved_source_missing_spec():
    scene = clean_scene()
    try:
        stored_at("lonely", spec=False)
        only_saved_source(scene, "no sibling lonely.tank.ron")
    finally:
        stored_at("testbed")


@case
def saved_source_a_file_that_is_no_longer_there():
    """A blend renamed or deleted after it was opened. The session still holds the model and still
    names the path, and nothing at that path is what the next reader would open."""
    scene = clean_scene()
    try:
        path = stored_at("ghost")
        assert os.path.isfile(path), "the fixture did not store a blend"
        os.remove(path)
        only_saved_source(scene, "no file stands at this path")
    finally:
        stored_at("testbed")


@case
def saved_source_a_stored_file_that_is_not_a_blend():
    """Blender stores wherever it is told (measured, 5.1.2: `save_as_mainfile` keeps a `.blend2`
    name verbatim). Every derived path, the hooks' trio discovery and the release prune name the
    source by its extension, so a file without it is an asset none of them can find."""
    scene = clean_scene()
    try:
        stored_at("oddball", extension=".blend2")
        only_saved_source(scene, "whose extension is `.blend2`")
    finally:
        stored_at("testbed")


@case
def saved_source_a_path_this_session_does_not_hold():
    """A well-formed stored path, on disk, with its sibling sheet — belonging to another file. The
    report would carry that path over this session's model."""
    scene = clean_scene()
    elsewhere = os.path.join(_WORK, "assets", "otherbed")
    os.makedirs(elsewhere, exist_ok=True)
    with open(os.path.join(elsewhere, "otherbed.tank.ron"), "w", encoding="utf-8") as handle:
        handle.write("()\n")
    path = os.path.join(elsewhere, "otherbed.blend")
    shutil.copyfile(BLEND_PATH, path)
    findings = export_tank.lint(source_of(scene, filepath=path))
    hits = of(findings, "L1.SAVED_SOURCE")
    assert len(hits) == 1, [finding.evidence for finding in hits]
    assert BLEND_PATH in hits[0].evidence, hits[0].evidence


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


@case
def unapplied_scale_reads_the_parent_inverse_matrix():
    """The scale both channels miss. Blender writes the inverse of the parent's world matrix into
    `matrix_parent_inverse` when a child is parented, so a parent that was scaled at that moment
    leaves its scale composed into every descendant's local transform with `scale` and
    `delta_scale` both reading (1,1,1)."""
    scene = clean_scene()
    turret = bpy.data.objects["Turret"]
    assert_silent(export_tank.lint(source_of(scene)), "L1.UNAPPLIED_SCALE")
    turret.matrix_parent_inverse = mathutils.Matrix.Diagonal((2.0, 1.0, 1.0, 1.0))
    assert tuple(turret.scale) == (1.0, 1.0, 1.0) and tuple(turret.delta_scale) == (1.0, 1.0, 1.0), (
        "the fixture moved a scale channel, which is the clause above"
    )
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.UNAPPLIED_SCALE", Severity.WARNING)
    hits = of(findings, "L1.UNAPPLIED_SCALE")
    assert len(hits) == 1 and hits[0].subject.element == "parent inverse", [
        (finding.subject.element, finding.evidence) for finding in hits
    ]
    assert_exit(findings, 0)


@case
def unapplied_scale_leaves_a_parent_inverse_that_only_offsets_alone():
    """The shape the live tank is in: MEASURED, 37 of the tiger's 86 objects carry a non-identity
    parent inverse and every one of them is a pure translation. Translation is not scale, and a
    warning that fires on all of them is noise."""
    scene = clean_scene()
    bpy.data.objects["Turret"].matrix_parent_inverse = mathutils.Matrix.Translation(
        (0.5, -1.25, 3.0)
    )
    assert_silent(export_tank.lint(source_of(scene)), "L1.UNAPPLIED_SCALE")


@case
def unapplied_scale_leaves_an_exactly_rotated_parent_inverse_alone():
    """A rotation stored exactly — a quarter turn about Z, written as the signed permutation it is
    — is scale-free bit-exactly, and the law reads it as the rotation it is.

    Its counterpart is the price of no tolerance, and it is recorded here rather than softened:
    `Matrix.Rotation(pi/2, 4, "Z")` stores cos(pi/2) as 6.1e-17, so ITS columns are 1.0000000000000058
    long and the law says so. That row is a warning, it never fails a build, and it names the
    measured Gram — which is the honest thing to print about a stored basis that is not unit.
    """
    scene = clean_scene()
    bpy.data.objects["Turret"].matrix_parent_inverse = mathutils.Matrix((
        (0.0, -1.0, 0.0, 0.0),
        (1.0, 0.0, 0.0, 0.0),
        (0.0, 0.0, 1.0, 0.0),
        (0.0, 0.0, 0.0, 1.0),
    ))
    assert_silent(export_tank.lint(source_of(scene)), "L1.UNAPPLIED_SCALE")
    inexact = mathutils.Matrix.Rotation(math.pi / 2.0, 4, "Z")
    bpy.data.objects["Turret"].matrix_parent_inverse = inexact
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

#: A RON path of the shape the generator emits. It is deliberately one no code here could have
#: invented: the field vocabulary is Rust's, pinned in `src/bake.rs`, and this side only has to
#: carry whatever arrives in the document through to the report unchanged.
FIELD = 'weapons["Coax_MG"].barrel'


@case
def spec_references_resolve_to_one_object_each():
    """Several references, resolving. Python knows none of these paths — they arrive in the
    document."""
    scene = clean_scene()
    resolving = canon(
        ("volumes", "Hull"), ("roadwheels[3].node", "Turret"), (FIELD, "Muzzle")
    )
    assert_silent(export_tank.lint(source_of(scene, canonical=resolving)), "L1.SPEC_REFERENCES")


@case
def spec_references_a_node_the_scene_does_not_hold():
    scene = clean_scene()
    absent = canon(("volumes", "Hull"), (FIELD, "Sponson"))
    findings = export_tank.lint(source_of(scene, canonical=absent))
    assert_fires(findings, "L1.SPEC_REFERENCES", Severity.ERROR)
    hits = of(findings, "L1.SPEC_REFERENCES")
    assert len(hits) == 1, "only the unresolved reference is a finding: {}".format(hits)
    assert hits[0].subject.name == "Sponson", hits[0].subject
    assert hits[0].subject.element == "declared in `{}`".format(FIELD), hits[0].subject
    assert FIELD in hits[0].repair, "the repair does not send the artist to the line: {}".format(
        hits[0].repair
    )
    assert "0 export-bound object(s)" in hits[0].evidence, hits[0].evidence
    assert_exit(findings, 1)


@case
def spec_references_a_name_two_objects_carry():
    """A reference to a shared name addresses whichever node the exporter writes second, which is
    nobody's decision."""
    library = write_library("Hull")
    scene = clean_scene()
    link_object(library, "Hull")
    findings = export_tank.lint(source_of(scene, canonical=canon((FIELD, "Hull"))))
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


# ── the graph walk the four texture laws share ───────────────────────────────────────────────────

@case
def texture_uv_source_leaves_a_group_through_the_socket_it_was_entered_by():
    """The texture is inside a group and its UV Map node is outside it. A walk that stops at Group
    Input calls this a coordinate it cannot carry; Blender carries it, because the group node's
    input socket and the Group Input node's output socket are one socket seen from two sides."""
    scene = hull_wearing(lambda name: group_material(name, layer="UVMap"))
    assert_silent(export_tank.lint(source_of(scene)), "L1.TEXTURE_UV_SOURCE")


@case
def texture_uv_source_reads_a_group_texture_naming_a_layer_the_mesh_lacks():
    """The same graph, one layer name changed — so the group is entered, left, and the answer is
    the UV Map node's, not the interface's."""
    scene = hull_wearing(lambda name: group_material(name, layer="Decals"))
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.TEXTURE_UV_SOURCE", Severity.ERROR)
    hits = of(findings, "L1.TEXTURE_UV_SOURCE")
    assert len(hits) == 1, [finding.subject.element for finding in hits]
    assert "layer `Decals`" in hits[0].evidence, hits[0].evidence
    assert "group `Painted_Group`" in hits[0].subject.element, hits[0].subject


@case
def texture_source_ignores_a_group_output_the_surface_does_not_read():
    """A group's SECOND output, connected to nothing outside, carries its texture into no material.
    A walk that queues every Group Output reports a defect in geometry the exporter never writes."""
    scene = hull_wearing(lambda name: group_material(name, layer="UVMap", spare=False))
    assert_silent(export_tank.lint(source_of(scene)), "L1.TEXTURE_SOURCE")


@case
def texture_source_reads_the_group_output_the_surface_does_read():
    """The same group with the reached texture broken instead — so the fixture above proves scope
    and not blindness."""
    scene = hull_wearing(lambda name: group_material(name, layer="UVMap", spare=True))
    for node in bpy.data.node_groups["Painted_Group"].nodes:
        if node.type == "TEX_IMAGE" and node.image.name.endswith("_inner"):
            node.image = None
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.TEXTURE_SOURCE", Severity.ERROR)
    assert "group `Painted_Group`" in of(findings, "L1.TEXTURE_SOURCE")[0].subject.element


@case
def texture_source_ignores_a_group_output_node_blender_does_not_evaluate():
    """A tree may hold several Group Output nodes and Blender evaluates exactly one. The stale one
    carries a texture into nothing."""
    scene = hull_wearing(lambda name: group_material(name, layer="UVMap", decoy_output=True))
    assert_silent(export_tank.lint(source_of(scene)), "L1.TEXTURE_SOURCE")


@case
def texture_uv_source_follows_a_chain_no_cutoff_gives_up_on():
    """Twenty Mapping nodes. A depth cutoff answers `active-render` past its limit, which passes a
    texture whose UV Map names a layer the mesh does not carry."""
    scene = hull_wearing(lambda name: chained_material(name, 20, layer="Decals"))
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.TEXTURE_UV_SOURCE", Severity.ERROR)
    assert "layer `Decals`" in of(findings, "L1.TEXTURE_UV_SOURCE")[0].evidence


@case
def texture_uv_source_refuses_a_deep_non_uv_coordinate():
    """The same chain ending at Generated: a procedural coordinate is refused however far away it
    is declared."""
    scene = hull_wearing(lambda name: chained_material(name, 20, coordinate="Generated"))
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.TEXTURE_UV_SOURCE", Severity.ERROR)
    assert "Generated" in of(findings, "L1.TEXTURE_UV_SOURCE")[0].evidence


@case
def texture_uv_source_refuses_a_mapping_node_driven_by_nothing():
    """A Mapping node's own unlinked Vector input is the socket's CONSTANT, not the active-render
    layer. Only the texture's own unlinked Vector is Blender's UV fallback."""
    scene = hull_wearing(lambda name: chained_material(name, 1))
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.TEXTURE_UV_SOURCE", Severity.ERROR)
    assert "unlinked" in of(findings, "L1.TEXTURE_UV_SOURCE")[0].evidence


@case
def texture_uv_source_reads_a_muted_node_the_way_blender_does():
    """One flag, two graphs: unmuted, a Vector Math node is a coordinate the door cannot carry;
    muted, Blender routes its internal link and the UV Map node behind it is what the texture
    reads."""
    scene = hull_wearing(lambda name: muted_material(name, muted=False))
    assert_fires(export_tank.lint(source_of(scene)), "L1.TEXTURE_UV_SOURCE", Severity.ERROR)
    scene = hull_wearing(lambda name: muted_material(name, muted=True))
    assert_silent(export_tank.lint(source_of(scene)), "L1.TEXTURE_UV_SOURCE")


# ── L1.UV_FINITE ─────────────────────────────────────────────────────────────────────────────────

@case
def uv_finite_nan_in_a_sampled_layer():
    scene = sampled_hull()
    assert_silent(export_tank.lint(source_of(scene)), "L1.UV_FINITE")
    bpy.data.meshes["Hull"].uv_layers["UVMap"].uv[2].vector = (0.0, float("nan"))
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.UV_FINITE", Severity.ERROR)
    assert_exit(findings, 1)


@case
def uv_finite_reads_a_layer_sampled_from_inside_a_group():
    """SAMPLED is decided by the same walk. A texture inside a group, driven by a UV Map node
    outside it, samples that layer — so a NaN in it is this law's, and a walk that misreads the
    interface drops the layer and reports nothing."""
    scene = hull_wearing(lambda name: group_material(name, layer="UVMap"))
    assert_silent(export_tank.lint(source_of(scene)), "L1.UV_FINITE")
    bpy.data.meshes["Hull"].uv_layers["UVMap"].uv[2].vector = (0.0, float("nan"))
    assert_fires(export_tank.lint(source_of(scene)), "L1.UV_FINITE", Severity.ERROR)


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
def zero_area_uv_reads_a_layer_sampled_from_inside_a_group():
    scene = hull_wearing(lambda name: group_material(name, layer="UVMap"))
    assert_silent(export_tank.lint(source_of(scene)), "L1.ZERO_AREA_UV")
    for corner in bpy.data.meshes["Hull"].uv_layers["UVMap"].uv:
        corner.vector = (0.25, 0.75)
    assert_fires(export_tank.lint(source_of(scene)), "L1.ZERO_AREA_UV", Severity.WARNING)


@case
def zero_area_uv_ignores_a_material_that_samples_nothing():
    """The law's own scope: a collapsed UV under an untextured substance is irrelevant, because
    nothing reads it."""
    scene = clean_scene()
    layer = unwrap(bpy.data.meshes["Hull"], ((0.25, 0.75), (0.25, 0.75), (0.25, 0.75)))
    assert layer.active_render, "the fixture's UV layer is not the one a texture would fall back to"
    assert_silent(export_tank.lint(source_of(scene)), "L1.ZERO_AREA_UV")


# ── L1.SUBSTANCE_IDENTITY ────────────────────────────────────────────────────────────────────────

#: A second library, standing somewhere this repository does not keep the canonical one.
SURPLUS_LIBRARY = os.path.join(_WORK, "assets", "surplus", "materials.blend")

#: A library at the SAME three-component suffix under a DIFFERENT root — another checkout's copy,
#: another registry, and nothing about its path tail or its contents says so.
IMPOSTOR_LIBRARY = os.path.join(_WORK, "impostor", "assets", "materials", "materials.blend")


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
def substance_identity_a_registry_name_linked_from_another_repository():
    """CANONICAL is one file, not three directory names. A second checkout — or a downloaded
    asset pack — holds `assets/materials/materials.blend` too, and its `RHA` was issued by a
    registry this repository never read."""
    findings = export_tank.lint(source_of(plated("RHA", path=IMPOSTOR_LIBRARY)))
    assert_fires(findings, "L1.SUBSTANCE_IDENTITY", Severity.ERROR)
    hits = of(findings, "L1.SUBSTANCE_IDENTITY")
    assert len(hits) == 1, [finding.evidence for finding in hits]
    assert IMPOSTOR_LIBRARY in hits[0].evidence, hits[0].evidence
    assert MATERIAL_LIBRARY in hits[0].evidence, (
        "the row does not name the library this source's own repository holds: {}".format(
            hits[0].evidence
        )
    )


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
def a_missing_canon_is_one_refusal_of_the_door_s_own():
    """The door failed to feed the pass, which is the door's to say — one row, under the door's own
    id, naming the command that writes the file. The two laws stated in it do not run: a report
    naming them would read as if they had been evaluated."""
    scene = clean_scene()
    findings = export_tank.lint(source_of(scene, canonical=None))
    assert_fires(findings, "door.canon-missing", Severity.ERROR)
    hits = of(findings, "door.canon-missing")
    assert len(hits) == 1, "the door refused {} times".format(len(hits))
    assert export_tank.CANON_COMMAND in hits[0].repair, hits[0].repair
    assert hits[0].check.stage == report.Stage.DOOR, hits[0].check.stage
    assert_silent(findings, "L1.SPEC_REFERENCES")
    assert_silent(findings, "L1.SUBSTANCE_IDENTITY")
    assert_exit(findings, 1)


@case
def the_source_inventory_is_closed():
    """Every check id an L1 pass can emit is a SOURCE-stage id the design's inventory declares, and
    the door's own refusals are DOOR-stage. A law that fails closed on missing input does so under
    the door's id, never by inventing a source law nobody ratified."""
    declared = {
        "L1.SAVED_SOURCE", "L1.EXPORT_SCOPE", "L1.LOCAL_MODEL_DATA", "L1.MODIFIER_STACK",
        "L1.DEFORMATION", "L1.ANIMATION", "L1.TRANSFORM_FINITE", "L1.HANDEDNESS",
        "L1.UNAPPLIED_SCALE", "L1.UNIQUE_NAMES", "L1.DEFAULT_NAMES", "L1.SPEC_REFERENCES",
        "L1.NONEMPTY_MESH", "L1.FINITE_MESH_DATA", "L1.ZERO_AREA_TRIANGLE",
        "L1.DUPLICATE_TRIANGLE", "L1.LOOSE_GEOMETRY", "L1.TEXTURE_UV_SOURCE", "L1.UV_FINITE",
        "L1.ZERO_AREA_UV", "L1.SUBSTANCE_IDENTITY", "L1.TEXTURE_SOURCE", "L1.SOURCE_CENSUS",
    }
    emitted = set()
    for check in export_tank.L1_CHECKS:
        for finding in check(source_of(clean_scene(), canonical=None)):
            emitted.add(finding.check.id)
            assert finding.check.stage == report.Stage.SOURCE, (
                "{} is not a source row".format(finding.check.id)
            )
    assert emitted <= declared, "the source pass emitted {}".format(sorted(emitted - declared))
    assert len(export_tank.L1_CHECKS) == len(declared), (
        "{} checks for {} declared ids".format(len(export_tank.L1_CHECKS), len(declared))
    )


@case
def canon_read_takes_the_generator_s_document_and_nothing_else():
    path = write_canon("good", {
        "node_references": [{"field": FIELD, "node": "Hull"}],
        "substance_keys": ["RHA", "Rubber"],
    })
    read, note = export_tank.Canon.read(path)
    assert note is None, note
    assert read.node_references == ((FIELD, "Hull"),), read.node_references
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


@case
def texture_source_reads_two_same_named_nodes_in_one_material():
    """A name is unique inside its own node tree and nowhere else. Two textures called `Image
    Texture` in one material's two trees are two textures, and the second one is where the defect
    is."""
    scene = hull_wearing(nested_material)
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.TEXTURE_SOURCE", Severity.ERROR)
    hits = of(findings, "L1.TEXTURE_SOURCE")
    assert len(hits) == 1 and "holds no image" in hits[0].evidence, [
        (finding.subject.element, finding.evidence) for finding in hits
    ]
    assert "group `Painted_Group`" in hits[0].subject.element, hits[0].subject


@case
def texture_source_reads_two_same_named_nodes_in_two_same_named_materials():
    """A material name is unique inside its own library and nowhere else — the counterfeit case the
    substance law is built on — so two materials called `Painted` each hold their own `Image
    Texture`, and the sound one must not answer for the broken one."""
    library = write_textured_library("Painted")
    scene = clean_scene()
    linked = link_material("Painted", path=library)
    bpy.data.meshes["Hull"].materials[0] = linked
    local, tree, shader = surface("Painted")
    assert local.name == "Painted", "the fixture's local material is {}".format(local.name)
    broken = tree.nodes.new("ShaderNodeTexImage")
    tree.links.new(broken.outputs["Color"], shader.inputs["Base Color"])
    bpy.data.meshes["Turret"].materials.append(local)
    assert broken.name in {node.name for node in linked.node_tree.nodes}, (
        "the fixture's two texture nodes do not share a name"
    )
    findings = export_tank.lint(source_of(scene))
    assert_fires(findings, "L1.TEXTURE_SOURCE", Severity.ERROR)
    hits = of(findings, "L1.TEXTURE_SOURCE")
    assert len(hits) == 1 and "holds no image" in hits[0].evidence, [
        (finding.subject.element, finding.evidence) for finding in hits
    ]


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
def source_census_counts_a_substance_only_from_the_canonical_library():
    """The same mechanism L1.SUBSTANCE_IDENTITY holds: a material linked from ANY library was a
    substance, so a linked art material — or another repository's copy of `materials.blend` — moved
    the ballistic and per-substance counts the source diff is read for."""
    scene = plated("RHA", path=IMPOSTOR_LIBRARY)
    rows = census_rows(export_tank.lint(source_of(scene)))
    assert rows["ballistic objects"] == "0", rows
    assert rows["shells"] == "0", rows
    assert not [row for row in rows if row.startswith("substance")], rows


#: One tetrahedron's faces, outward-wound — the smallest closed shell, the same one the consumer
#: contract's own fixtures are built from.
TETRAHEDRON = ((0, 2, 1), (0, 1, 3), (0, 3, 2), (1, 2, 3))


def solids(*corners):
    """`(positions, faces)` for one unit tetrahedron per given corner."""
    positions = []
    faces = []
    for index, (x, y, z) in enumerate(corners):
        positions.extend(((x, y, z), (x + 1.0, y, z), (x, y + 1.0, z), (x, y, z + 1.0)))
        faces.extend(tuple(corner + index * 4 for corner in face) for face in TETRAHEDRON)
    return (tuple(positions), tuple(faces))


def armoured_hull(corners, substances=("RHA",), slot_of_face=None):
    """The clean tank whose hull is one tetrahedron per corner, wearing library substances. Returns
    the scene; `slot_of_face` gives each face's material slot, all of them the first by default."""
    write_material_library(*substances)
    scene = clean_scene()
    positions, faces = solids(*corners)
    mesh = reshape("Hull", positions=positions, faces=faces)
    for name in substances:
        mesh.materials.append(link_material(name))
    for index, polygon in enumerate(mesh.polygons):
        polygon.material_index = 1 + (slot_of_face(index) if slot_of_face else 0)
    return scene


@case
def source_census_counts_one_shell_per_edge_connected_component():
    """Two closed solids in one substance primitive that touch at nothing are two shells — the
    partition the consumer contract publishes as shell ids, computed here on the stored mesh."""
    scene = armoured_hull(((0.0, 0.0, 0.0), (8.0, 0.0, 0.0)))
    rows = census_rows(export_tank.lint(source_of(scene)))
    assert rows["ballistic objects"] == "1", rows
    assert rows["shells"] == "2", rows


@case
def source_census_welds_shells_by_exact_position():
    """One surface authored twice, on distinct vertices at the same coordinates, shares every edge
    once welded and is therefore one shell — as the contract sees it."""
    scene = armoured_hull(((0.0, 0.0, 0.0), (0.0, 0.0, 0.0)))
    assert census_rows(export_tank.lint(source_of(scene)))["shells"] == "1"


@case
def source_census_shells_are_edge_connected_and_not_vertex_connected():
    """Two solids meeting at a single welded corner are two shells: §13.7's legal touch, and the
    line the contract's own partition draws."""
    scene = armoured_hull(((0.0, 0.0, 0.0), (-1.0, 0.0, 0.0)))
    assert census_rows(export_tank.lint(source_of(scene)))["shells"] == "2"


@case
def source_census_partitions_shells_inside_one_primitive_at_a_time():
    """A primitive is one material slot, and the contract welds and partitions each on its own. Two
    solids in DIFFERENT substances that share a welded edge are two shells, not one."""
    scene = armoured_hull(
        ((0.0, 0.0, 0.0), (0.0, 0.0, 0.0)), substances=("RHA", "Rubber"),
        slot_of_face=lambda index: index // len(TETRAHEDRON),
    )
    rows = census_rows(export_tank.lint(source_of(scene)))
    assert rows["shells"] == "2", rows
    assert rows["substance `RHA`"] == "1 primitive(s)" and rows["substance `Rubber`"] == \
        "1 primitive(s)", rows


@case
def source_census_counts_mesh_datablocks_by_identity():
    """A linked mesh datablock and a local one may carry the same name — the census that counts
    names cannot see the second one arrive."""
    library = write_library("Hull")
    scene = clean_scene()
    assert census_rows(export_tank.lint(source_of(scene)))["meshes"] == "2"
    linked = link_object(library, "Hull")
    assert linked.data.name == "Hull", "the fixture did not produce a shared mesh name"
    rows = census_rows(export_tank.lint(source_of(scene)))
    assert rows["meshes"] == "3", rows
    assert rows["objects"] == "4", rows


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
    # The session STANDS in the fixture's worktree for this case: the baseline is HEAD of the
    # worktree the blend lives in, and both censuses are anchored on the repository the blend is in
    # — which is one repository in the door, and has to be one here.
    try:
        commit_blend("census", become=True)
        rows = census_rows(export_tank.lint(source_of(scene)))
        assert rows["baseline"].startswith("compared against"), rows["baseline"]
        assert rows["objects"] == "3 (baseline 3, +0)", rows["objects"]

        scene.collection.objects.link(bpy.data.objects.new("Sponson", triangle_mesh("Sponson")))
        findings = export_tank.lint(source_of(scene))
        rows = census_rows(findings)
        assert rows["objects"] == "4 (baseline 3, +1)", rows["objects"]
        assert rows["meshes"] == "3 (baseline 2, +1)", rows["meshes"]
        assert rows["substance `RHA`"] == "1 (baseline 1, +0) primitive(s)", rows
        assert rows["shells"] == "1 (baseline 1, +0)", rows
        assert_exit(findings, 0)
    finally:
        stored_at("testbed")


@case
def source_census_reads_a_baseline_out_of_the_lfs_object_store():
    """The shape the shipped asset is really in: HEAD holds a pointer and the bytes are in this
    clone's own object store. They have to be laid out as an ASSET before they are counted — a tank
    blend links `//../materials/materials.blend`, so a baseline read at a content-addressed store
    path resolves its library nowhere, classifies no substance, and prints a baseline of zero
    against a live source that has them."""
    write_material_library("RHA")
    scene = clean_scene()
    bpy.data.meshes["Hull"].materials[0] = link_material("RHA")
    try:
        top = os.path.join(_WORK, "repo-lfs-object")
        directory = os.path.join(top, "assets", "lfsobj")
        os.makedirs(directory, exist_ok=True)
        with open(os.path.join(directory, "lfsobj.tank.ron"), "w", encoding="utf-8") as handle:
            handle.write("()\n")
        path = os.path.join(directory, "lfsobj.blend")
        bpy.ops.wm.save_as_mainfile(filepath=path, relative_remap=False)
        with open(path, "rb") as handle:
            model = handle.read()
        oid = hashlib.sha256(model).hexdigest()
        with open(path, "wb") as handle:
            handle.write("version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize {}\n"
                         .format(oid, len(model)).encode())
        git(top, "init", "-q")
        git(top, "config", "user.email", "lint@overmatch.test")
        git(top, "config", "user.name", "tank lint")
        git(top, "add", "-A")
        git(top, "commit", "-q", "-m", "baseline")
        objects = os.path.join(top, ".git", "lfs", "objects", oid[:2], oid[2:4])
        os.makedirs(objects, exist_ok=True)
        with open(os.path.join(objects, oid), "wb") as handle:
            handle.write(model)

        rows = census_rows(export_tank.lint(source_of(scene)))
        assert rows["baseline"].startswith("compared against"), rows["baseline"]
        assert rows["substance `RHA`"] == "1 (baseline 1, +0) primitive(s)", rows
        assert rows["shells"] == "1 (baseline 1, +0)", rows
    finally:
        stored_at("testbed")


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
    findings = export_tank.check_source_census(source_of(scene, filepath=path))
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
    """`run('lint')` builds its Source from the open blend rather than from a fixture. The session
    is stored where L1.SAVED_SOURCE wants it and was given no canon, so a clean scene reports both
    canon refusals and a census that says it has nothing to compare against."""
    clean_scene()
    findings = export_tank.run("lint")
    assert {finding.check.id for finding in findings} == {
        "L1.SOURCE_CENSUS",
        "door.canon-missing",
    }, "run('lint') reported {}".format([finding.check.id for finding in findings])
    assert "no source baseline" in census_rows(findings)["baseline"]
    assert_exit(findings, 1)


@case
def lint_mode_reads_the_canon_file_it_is_given():
    """The `--canon` wiring, end to end: `run` reads the file, and the law is evaluated against it
    rather than refused for want of it."""
    path = write_canon("wired", {
        "node_references": [{"field": FIELD, "node": "Sponson"}],
        "substance_keys": list(CANON_KEYS),
    })
    clean_scene()
    findings = export_tank.run("lint", path)
    assert_silent(findings, "door.canon-missing")
    assert_fires(findings, "L1.SPEC_REFERENCES", Severity.ERROR)
    assert of(findings, "L1.SPEC_REFERENCES")[0].subject.name == "Sponson"


@case
def the_command_line_carries_the_canon_path_to_the_pass():
    """The whole plumbing the wrapper will use: `--canon <file>` on the command line, through the
    parser, into the law that is stated in it."""
    path = write_canon("cli", {
        "node_references": [{"field": FIELD, "node": "Sponson"}],
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


# ── door.toolchain ───────────────────────────────────────────────────────────────────────────────

@case
def the_exporter_pin_passes_on_the_pinned_blender():
    """This suite runs in the Blender the door runs in, so the pin is silent here or the machine is
    not the one the shipped bytes were cut on."""
    assert not export_tank.check_exporter(), "the exporter pin fired on the pinned Blender: {}"\
        .format(report.render_text(export_tank.check_exporter()))


@case
def an_unpinned_exporter_refuses_before_the_source_pass_runs():
    """The version comes from the add-on, so the mutation is the pin it is compared against. It
    refuses ahead of every check: a frozen argument list is a promise about one exporter, and
    measuring a model with another certifies nothing about the bytes it would write."""
    pinned = toolchain.GLTF_EXPORTER_VERSION
    toolchain.GLTF_EXPORTER_VERSION = "0.0.1"
    try:
        clean_scene()
        findings = export_tank.run("lint")
        assert_fires(findings, "door.toolchain", Severity.ERROR)
        assert pinned in findings[0].evidence and "0.0.1" in findings[0].evidence, (
            "the row names neither the running exporter nor the pin: {}".format(
                findings[0].evidence)
        )
        assert [finding.check.id for finding in findings] == ["door.toolchain"], (
            "the source pass ran on an unpinned exporter: {}".format(
                sorted({finding.check.id for finding in findings}))
        )
        assert_exit(findings, 1)
    finally:
        toolchain.GLTF_EXPORTER_VERSION = pinned


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
