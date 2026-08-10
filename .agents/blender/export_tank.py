"""export_tank.py — the Blender half of the one asset door, for any tank.

Run by the outer wrapper, never by hand:

    blender --background --factory-startup assets/<id>/<id>.blend \\
      --python .agents/blender/export_tank.py -- \\
      --mode <lint|export|verify> \\
      --spec assets/<id>/<id>.tank.ron \\
      --glb assets/<id>/<id>.glb

`lint` runs the L1 source pass over the open blend and prints one report. `export` and `verify`
run the derivation chain; until that chain lands they refuse with `door.mode-unimplemented`.

EXPORT-BOUND means every object in the active scene, because the exporter is invoked with
active-scene scope. Other workbench scenes in the same blend are outside the door.

Every refusal is a `scripts/tank/report.Finding`: check id, the severity compiled in beside it,
the subject, what was measured, the law, and the repair. Exit is non-zero exactly when the report
holds an error, which is the whole contract with the wrapper — Blender propagates a `sys.exit`
code from a `--python` script verbatim (measured, 5.1.2).

The transform laws read `matrix_local`, which the depsgraph recomputes lazily, so `Source.live`
calls `view_layer.update()` first. Reading it without that returns the matrix from before the last
channel write.
"""

from __future__ import annotations

import argparse
import math
import os
import re
import sys
from collections import Counter
from dataclasses import dataclass, field
from typing import List, Optional

import bpy

sys.path.insert(0, os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
    "scripts", "tank",
))

import report  # noqa: E402  — the path above is what makes it importable
from report import Check, Finding, Severity, Stage, Subject, SubjectKind  # noqa: E402


# ── the door itself ──────────────────────────────────────────────────────────────────────────────

MODE_UNIMPLEMENTED = Check(
    id="door.mode-unimplemented",
    stage=Stage.DOOR,
    severity=Severity.ERROR,
    law="the requested door mode runs its whole chain",
)


# ── L1: scene hygiene and structure ──────────────────────────────────────────────────────────────

SAVED_SOURCE = Check(
    id="L1.SAVED_SOURCE",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="the live file is saved at assets/<id>/<id>.blend, has no unsaved changes, and has a "
        "sibling <id>.tank.ron",
)

EXPORT_SCOPE = Check(
    id="L1.EXPORT_SCOPE",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="the active export scene is non-empty, every scoped object's parent is also scoped, and "
        "an export-bound object is a MESH or an EMPTY",
)

LOCAL_MODEL_DATA = Check(
    id="L1.LOCAL_MODEL_DATA",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="export-bound objects and mesh datablocks are local to the tank blend",
)

MODIFIER_STACK = Check(
    id="L1.MODIFIER_STACK",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="an export-bound object has zero modifiers, enabled or not",
)

DEFORMATION = Check(
    id="L1.DEFORMATION",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="an export-bound mesh has no shape keys and no armature deformation",
)

ANIMATION = Check(
    id="L1.ANIMATION",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="no export-bound object, mesh or shape-key datablock has an action, an NLA strip or a "
        "driver; empty AnimData is clean",
)

TRANSFORM_FINITE = Check(
    id="L1.TRANSFORM_FINITE",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="every translation, quaternion/matrix entry and scale component is finite",
)

HANDEDNESS = Check(
    id="L1.HANDEDNESS",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="every export-bound local transform has a strictly positive 3x3 determinant",
)

UNAPPLIED_SCALE = Check(
    id="L1.UNAPPLIED_SCALE",
    stage=Stage.SOURCE,
    severity=Severity.WARNING,
    law="every export-bound local scale is bit-exact (1,1,1)",
)

UNIQUE_NAMES = Check(
    id="L1.UNIQUE_NAMES",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="export-bound object names are non-empty and unique",
)

DEFAULT_NAMES = Check(
    id="L1.DEFAULT_NAMES",
    stage=Stage.SOURCE,
    severity=Severity.WARNING,
    law="object, mesh and used-material names do not match Blender's stock default-name vocabulary",
)

#: What Blender itself names a datablock nobody named. MEASURED off 5.1.2 by adding one of every
#: mesh primitive and an empty under `--factory-startup`, plus the `bpy.data.*.new` fallbacks.
STOCK_DEFAULT_NAMES = frozenset({
    "Circle", "Cone", "Cube", "Cylinder", "Empty", "Grid", "Icosphere", "Material", "Mesh",
    "Object", "Plane", "Sphere", "Suzanne", "Torus",
})

#: Blender's collision suffix: exactly three digits, appended to an otherwise stock name.
_COPY_SUFFIX = re.compile(r"\.\d{3}$")

#: The object types the exporter may be handed. Everything else is a workbench leftover.
EXPORT_BOUND_TYPES = frozenset({"MESH", "EMPTY"})


@dataclass
class Source:
    """The lint's whole view of one tank: where the blend is stored, whether that store is current,
    and the export-bound objects."""

    #: `bpy.data.filepath` — empty when the blend has never been saved.
    filepath: str
    #: `bpy.data.is_dirty` — whether memory holds edits the stored file does not.
    is_dirty: bool
    scene_name: str
    objects: List[object] = field(default_factory=list)

    @classmethod
    def live(cls, scene=None) -> "Source":
        """The open blend. `view_layer.update()` first: `matrix_local` is otherwise stale."""
        bpy.context.view_layer.update()
        scene = scene or bpy.context.scene
        return cls(
            filepath=bpy.data.filepath,
            is_dirty=bpy.data.is_dirty,
            scene_name=scene.name,
            objects=list(scene.objects),
        )


def check_saved_source(source: Source) -> List[Finding]:
    """The stored file is the model; the door refuses to certify anything else."""
    findings = []
    if not source.filepath:
        return [Finding(
            SAVED_SOURCE,
            Subject(SubjectKind.FILE, "<unsaved>"),
            "bpy.data.filepath is empty — this blend has never been written to disk",
            "save the blend as assets/<id>/<id>.blend beside its <id>.tank.ron, then re-run",
        )]

    directory, filename = os.path.split(source.filepath)
    stem = os.path.splitext(filename)[0]
    subject = Subject(SubjectKind.FILE, source.filepath)

    holder = os.path.basename(directory)
    collection = os.path.basename(os.path.dirname(directory))
    if holder != stem or collection != "assets":
        findings.append(Finding(
            SAVED_SOURCE,
            subject,
            "stored at {}/{}/{}, not assets/{}/{}".format(collection, holder, filename, stem, filename),
            "move the blend to assets/{stem}/{stem}.blend — the wrapper derives the spec and glb "
            "paths from that layout".format(stem=stem),
        ))

    if source.is_dirty:
        findings.append(Finding(
            SAVED_SOURCE,
            subject,
            "bpy.data.is_dirty — memory holds edits the stored file does not",
            "save the blend (Ctrl+S) and re-run; the door certifies the stored bytes, not the "
            "session",
        ))

    spec = os.path.join(directory, stem + ".tank.ron")
    if not os.path.isfile(spec):
        findings.append(Finding(
            SAVED_SOURCE,
            subject,
            "no sibling {}.tank.ron".format(stem),
            "write the spec sheet at {} — a model with no spec is not a tank".format(spec),
        ))
    return findings


def check_export_scope(source: Source) -> List[Finding]:
    """What the exporter is handed: a non-empty scene, closed under parenting, of exportable types."""
    if not source.objects:
        return [Finding(
            EXPORT_SCOPE,
            Subject(SubjectKind.SCENE, source.scene_name),
            "the active scene holds no objects",
            "link the tank's objects into the scene the door opens, or open the blend whose active "
            "scene holds them",
        )]

    scoped = set(source.objects)
    findings = []
    for obj in source.objects:
        if obj.parent is not None and obj.parent not in scoped:
            findings.append(Finding(
                EXPORT_SCOPE,
                Subject(SubjectKind.OBJECT, obj.name),
                "parented to `{}`, which the active scene does not hold".format(obj.parent.name),
                "link the parent into the export scene or clear the parent (Alt+P, Keep Transform) "
                "— the exporter writes this node's transform relative to a node it will not write",
            ))
        if obj.type not in EXPORT_BOUND_TYPES:
            findings.append(Finding(
                EXPORT_SCOPE,
                Subject(SubjectKind.OBJECT, obj.name),
                "object type {}".format(obj.type),
                "delete it or move it to a workbench scene — the door exports MESH and EMPTY only",
            ))
    return findings


def check_local_model_data(source: Source) -> List[Finding]:
    """Linked geometry is somebody else's file: the tank blend is the sole model truth."""
    findings = []
    for obj in source.objects:
        if obj.library is not None:
            findings.append(Finding(
                LOCAL_MODEL_DATA,
                Subject(SubjectKind.OBJECT, obj.name),
                "linked from {}".format(obj.library.filepath),
                "make it local (Object → Relations → Make Local) — the tank blend is the sole "
                "model truth",
            ))
        elif obj.override_library is not None:
            findings.append(Finding(
                LOCAL_MODEL_DATA,
                Subject(SubjectKind.OBJECT, obj.name),
                "library override of {}".format(obj.override_library.reference.name),
                "make it local (Object → Relations → Make Local) — an override wraps data another "
                "file owns",
            ))
        if obj.type == "MESH" and obj.data is not None and obj.data.library is not None:
            findings.append(Finding(
                LOCAL_MODEL_DATA,
                Subject(SubjectKind.MESH, obj.data.name, "on object `{}`".format(obj.name)),
                "mesh datablock linked from {}".format(obj.data.library.filepath),
                "make the mesh local (Object → Relations → Make Local → Object and Data)",
            ))
    return findings


def check_modifier_stack(source: Source) -> List[Finding]:
    """The stored mesh is L0. A modifier makes the exported surface something no one can inspect
    in the file."""
    findings = []
    for obj in source.objects:
        if not obj.modifiers:
            continue
        stack = ", ".join(
            "`{}` ({}, viewport {}, render {})".format(
                mod.name, mod.type, "on" if mod.show_viewport else "off",
                "on" if mod.show_render else "off",
            )
            for mod in obj.modifiers
        )
        findings.append(Finding(
            MODIFIER_STACK,
            Subject(SubjectKind.OBJECT, obj.name),
            "{} modifier(s): {}".format(len(obj.modifiers), stack),
            "apply the stack (Ctrl+A → Visual Geometry to Mesh, or Apply per modifier) and store "
            "the result — the stored mesh is the surface the sim and the LOD ladder are anchored to",
        ))
    return findings


def check_deformation(source: Source) -> List[Finding]:
    """A deformed mesh has no single stored surface. `find_armature` covers both bindings: the
    Armature modifier and old-style armature parenting."""
    findings = []
    for obj in source.objects:
        if obj.type != "MESH" or obj.data is None:
            continue
        keys = obj.data.shape_keys
        if keys is not None:
            findings.append(Finding(
                DEFORMATION,
                Subject(SubjectKind.MESH, obj.data.name, "on object `{}`".format(obj.name)),
                "shape-key datablock `{}` with {} key(s)".format(keys.name, len(keys.key_blocks)),
                "pick the shape that ships, apply it as the mesh, and delete the key blocks "
                "(Object Data ▸ Shape Keys ▸ ▾ ▸ Delete All Shape Keys)",
            ))
        armature = obj.find_armature()
        if armature is not None:
            findings.append(Finding(
                DEFORMATION,
                Subject(SubjectKind.OBJECT, obj.name),
                "deformed by armature `{}`".format(armature.name),
                "unbind the armature and store the rest pose as the mesh — a tank is posed by the "
                "sim through its node hierarchy, never by a skin",
            ))
    return findings


def _animation_findings(check: Check, subject: Subject, datablock) -> List[Finding]:
    """One datablock's AnimData. Empty AnimData — no action, no strips, no drivers — is clean."""
    anim = getattr(datablock, "animation_data", None)
    if anim is None:
        return []
    held = []
    if anim.action is not None:
        held.append("action `{}`".format(anim.action.name))
    # Strips, not tracks: the law reads "no NLA strip", and an empty track holds no animation.
    strips = sum(len(track.strips) for track in anim.nla_tracks)
    if strips:
        held.append("{} NLA strip(s)".format(strips))
    if len(anim.drivers):
        held.append("{} driver(s)".format(len(anim.drivers)))
    if not held:
        return []
    return [Finding(
        check,
        subject,
        "animation data holds " + ", ".join(held),
        "delete the action, strips and drivers (Blender File view ▸ Animation Data ▸ Delete) — the "
        "sim poses every node, so anything animated here fights it or is silently dropped",
    )]


def check_animation(source: Source) -> List[Finding]:
    """Objects, their meshes, and their shape-key datablocks — the three places a tank blend can
    carry animation the exporter or the sim would have to reconcile."""
    findings = []
    for obj in source.objects:
        findings.extend(_animation_findings(ANIMATION, Subject(SubjectKind.OBJECT, obj.name), obj))
        if obj.type != "MESH" or obj.data is None:
            continue
        mesh_subject = Subject(SubjectKind.MESH, obj.data.name, "on object `{}`".format(obj.name))
        findings.extend(_animation_findings(ANIMATION, mesh_subject, obj.data))
        keys = obj.data.shape_keys
        if keys is not None:
            key_subject = Subject(
                SubjectKind.MESH, obj.data.name, "shape keys `{}`".format(keys.name)
            )
            findings.extend(_animation_findings(ANIMATION, key_subject, keys))
    return findings


def _transform_channels(obj):
    """Every authored number the local transform is composed from, plus the composed matrix.

    The raw channels are read as well as the matrix because a non-finite delta or a rotation mode
    the matrix collapses still has to be named in the repair.
    """
    return [
        ("location", tuple(obj.location)),
        ("delta_location", tuple(obj.delta_location)),
        ("scale", tuple(obj.scale)),
        ("delta_scale", tuple(obj.delta_scale)),
        ("rotation_quaternion", tuple(obj.rotation_quaternion)),
        ("delta_rotation_quaternion", tuple(obj.delta_rotation_quaternion)),
        ("rotation_euler", tuple(obj.rotation_euler)),
        ("delta_rotation_euler", tuple(obj.delta_rotation_euler)),
        ("rotation_axis_angle", tuple(obj.rotation_axis_angle)),
        ("matrix_local", tuple(value for row in obj.matrix_local for value in row)),
    ]


def check_transform_finite(source: Source) -> List[Finding]:
    """A non-finite transform poisons every pose composed through this node."""
    findings = []
    for obj in source.objects:
        for name, values in _transform_channels(obj):
            if all(math.isfinite(value) for value in values):
                continue
            findings.append(Finding(
                TRANSFORM_FINITE,
                Subject(SubjectKind.OBJECT, obj.name, "channel `{}`".format(name)),
                "{} = {}".format(name, values),
                "retype the channel in the N-panel — every pose composed through this node "
                "inherits the non-finite value",
            ))
    return findings


def check_handedness(source: Source) -> List[Finding]:
    """A mirrored or flattened node inverts or destroys winding, which the ballistic contract reads
    as the surface's inside."""
    findings = []
    for obj in source.objects:
        determinant = obj.matrix_local.to_3x3().determinant()
        if determinant > 0.0:
            continue
        findings.append(Finding(
            HANDEDNESS,
            Subject(SubjectKind.OBJECT, obj.name),
            "local 3x3 determinant {!r}".format(determinant),
            "apply the mirror to the mesh and flip the face normals (Ctrl+A → Scale, then Mesh → "
            "Normals → Recalculate Outside) — a negative determinant inverts winding, a zero one "
            "collapses the node's frame",
        ))
    return findings


def check_unapplied_scale(source: Source) -> List[Finding]:
    """Reported generically here; the strict refusal for sim-consumed nodes is L2.UNIT_SCALE."""
    findings = []
    unit = (1.0, 1.0, 1.0)
    for obj in source.objects:
        scale = tuple(obj.scale)
        delta = tuple(obj.delta_scale)
        if scale == unit and delta == unit:
            continue
        findings.append(Finding(
            UNAPPLIED_SCALE,
            Subject(SubjectKind.OBJECT, obj.name),
            "scale {}, delta scale {}".format(scale, delta),
            "apply the scale (Ctrl+A → Scale) — bit-exact, not near: there is no scale that is "
            "almost 1",
        ))
    return findings


def check_unique_names(source: Source) -> List[Finding]:
    """Node names are how the spec sheet and the runtime address geometry, so a name that is empty
    or shared addresses nothing."""
    findings = []
    for obj in source.objects:
        if obj.name:
            continue
        findings.append(Finding(
            UNIQUE_NAMES,
            Subject(SubjectKind.OBJECT, "<empty>"),
            "an export-bound object has an empty name",
            "name it after what it is — the spec sheet and the runtime address nodes by name",
        ))
    for name, count in sorted(Counter(obj.name for obj in source.objects).items()):
        if count < 2 or not name:
            continue
        findings.append(Finding(
            UNIQUE_NAMES,
            Subject(SubjectKind.OBJECT, name),
            "{} export-bound objects share this name".format(count),
            "rename all but one — a linked or appended copy keeps its name, and the exporter "
            "writes both nodes under it",
        ))
    return findings


def _is_default_name(name: str) -> bool:
    """Stock name, with or without Blender's collision suffix. Exact case: a lowercased or
    near-miss name is a human's choice, and the registry's near-miss rule is L1.SUBSTANCE_IDENTITY's."""
    return _COPY_SUFFIX.sub("", name) in STOCK_DEFAULT_NAMES


def check_default_names(source: Source) -> List[Finding]:
    """A stock name says nobody decided what this is."""
    findings = []
    seen = set()

    def note(kind, name, element=None):
        if (kind, name) in seen or not _is_default_name(name):
            return
        seen.add((kind, name))
        findings.append(Finding(
            DEFAULT_NAMES,
            Subject(kind, name, element),
            "`{}` is a Blender stock default name".format(name),
            "rename it after the part it is — a stock name says nobody decided what this is",
        ))

    for obj in source.objects:
        note(SubjectKind.OBJECT, obj.name)
        if obj.type != "MESH" or obj.data is None:
            continue
        note(SubjectKind.MESH, obj.data.name, "on object `{}`".format(obj.name))
        for slot in obj.material_slots:
            if slot.material is not None:
                note(SubjectKind.MATERIAL, slot.material.name, "on object `{}`".format(obj.name))
    return findings


#: The scene-hygiene and structure pass, in table order. A check appears here exactly once; the
#: report's own sort, not this order, decides what the console prints first.
L1_CHECKS = (
    check_saved_source,
    check_export_scope,
    check_local_model_data,
    check_modifier_stack,
    check_deformation,
    check_animation,
    check_transform_finite,
    check_handedness,
    check_unapplied_scale,
    check_unique_names,
    check_default_names,
)


def lint(source: Source) -> List[Finding]:
    """Run the L1 source pass. Returns the whole report, sorted."""
    findings = []
    for check in L1_CHECKS:
        findings.extend(check(source))
    return report.sorted_findings(findings)


# ── the modes ────────────────────────────────────────────────────────────────────────────────────

def unimplemented(mode: str) -> List[Finding]:
    """`export` and `verify` refuse mechanically until the derivation chain lands behind them."""
    return [Finding(
        MODE_UNIMPLEMENTED,
        Subject(SubjectKind.DOOR, mode),
        "mode `{}` has no chain behind it in this door".format(mode),
        "run `--mode lint`, and export through the existing exporter until the generic door lands",
    )]


def run(mode: str) -> List[Finding]:
    if mode == "lint":
        return lint(Source.live())
    return unimplemented(mode)


def _parse(argv: Optional[List[str]] = None):
    """Arguments after Blender's own `--`."""
    if argv is None:
        argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    parser = argparse.ArgumentParser(prog="export_tank.py", allow_abbrev=False)
    parser.add_argument("--mode", required=True, choices=("lint", "export", "verify"))
    parser.add_argument("--spec", help="the tank's spec sheet, read by the derivation chain; the "
                                       "sibling L1.SAVED_SOURCE requires is derived from the "
                                       "blend stem, never from this path")
    parser.add_argument("--glb", help="the tracked glb the chain writes in export mode and "
                                      "compares against in verify mode")
    return parser.parse_args(argv)


def main() -> int:
    arguments = _parse()
    findings = run(arguments.mode)
    print(report.render_text(findings), end="", flush=True)
    print("{} ▸ {}".format(arguments.mode.ljust(5), report.summary(findings)), flush=True)
    return report.exit_code(findings)


if __name__ == "__main__":
    sys.exit(main())
