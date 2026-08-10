"""export_tank.py — the Blender half of the one asset door, for any tank.

Run by the outer wrapper, never by hand:

    blender --background --factory-startup assets/<id>/<id>.blend \\
      --python .agents/blender/export_tank.py -- \\
      --mode <lint|export|verify> \\
      --spec assets/<id>/<id>.tank.ron \\
      --glb assets/<id>/<id>.glb \\
      --canon <canon.json>

`lint` runs the L1 source pass over the open blend and prints one report. `export` and `verify`
run the same pass and, when it holds no error, write the raw candidate the rest of the chain reads
— the consumer contract, the texture bake and the comparison are the wrapper's phases, because
Blender's embedded interpreter is not where a Rust CLI or a minute-long encoder belongs.

The canon file is the wrapper's job to produce, with `asset_verify --canon`; two of the laws are
stated in canonical Rust lists and refuse mechanically without it.

Two refusals precede every mode and every check. `door.toolchain` asserts the glTF exporter this
Blender loaded against `scripts/toolchain.py` — the frozen `EXPORT_SETTINGS` are promises about
THAT exporter, and it is readable nowhere but in here. `door.unresolved-library` follows it:
Blender replaces a datablock whose library it cannot read with a placeholder carrying that
datablock's name, so a blend with an unresolved link is not the stored model and there is nothing
here worth measuring.

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
import dataclasses
import json
import math
import os
import re
import subprocess
import sys
import tempfile
from collections import Counter
from dataclasses import dataclass, field
from typing import List, Optional

import bpy

_SCRIPTS = os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))), "scripts",
)
sys.path.insert(0, os.path.join(_SCRIPTS, "tank"))
sys.path.insert(0, _SCRIPTS)

import report  # noqa: E402  — the paths above are what make these importable
import toolchain  # noqa: E402
from report import Check, Finding, Severity, Stage, Subject, SubjectKind  # noqa: E402


# ── the door itself ──────────────────────────────────────────────────────────────────────────────

MODE_UNIMPLEMENTED = Check(
    id="door.mode-unimplemented",
    stage=Stage.DOOR,
    severity=Severity.ERROR,
    law="the requested door mode runs its whole chain",
)

UNRESOLVED_LIBRARY = Check(
    id="door.unresolved-library",
    stage=Stage.DOOR,
    severity=Severity.ERROR,
    law="every library this blend links resolves, and every linked datablock is the library's own",
)


def check_exporter() -> List[Finding]:
    """The pin only this half can assert, ahead of every mode.

    `EXPORT_SETTINGS` below is a frozen argument list, which is a promise about the exporter that
    reads it: an add-on on its own release schedule, bundled with Blender but replaceable, and the
    thing that decides the bytes of every model. The wrapper's preflight pins the programs it can
    run; the exporter is importable only from inside Blender, so it is pinned here.
    """
    row = toolchain.finding(toolchain.gltf_exporter())
    return [row] if row else []


def check_unresolved_library() -> List[Finding]:
    """The door's precondition, ahead of every mode and every check.

    Blender substitutes a PLACEHOLDER for a datablock whose library it cannot read, and a
    placeholder satisfies the identity laws: it carries the name and the library filepath of the
    datablock that is not there. So a blend with an unresolved link is not the stored model, and
    measuring it certifies a substitute — the substance census, `L1.SUBSTANCE_IDENTITY` and the
    exported material assignments would all read as if the library were present.

    `ID.is_missing` is the placeholder flag; `Library.users_id` is every datablock that came (or
    should have come) out of one library.
    """
    findings = []
    for library in bpy.data.libraries:
        path = os.path.abspath(bpy.path.abspath(library.filepath))
        placeholders = sorted(
            "{} `{}`".format(type(datablock).__name__, datablock.name)
            for datablock in library.users_id
            if getattr(datablock, "is_missing", False)
        )
        readable = os.path.isfile(path)
        if readable and not placeholders:
            continue
        held = "{} placeholder datablock(s): {}".format(
            len(placeholders), ", ".join(placeholders)
        ) if placeholders else "no placeholder datablock, and the file is not readable"
        findings.append(Finding(
            UNRESOLVED_LIBRARY,
            Subject(SubjectKind.FILE, library.filepath, "linked library"),
            "{} — {}".format(
                "resolves to {}, which is not a readable file".format(path) if not readable
                else "resolves to {}".format(path),
                held,
            ),
            "restore the library at {} (or relink the datablocks to where it stands) and reopen the "
            "blend — Blender replaces a datablock it cannot read with a placeholder that carries "
            "its name, so what is open is not the stored model".format(path),
        ))
    return findings


# ── the canon file ───────────────────────────────────────────────────────────────────────────────

#: What writes the canon file, named in the refusal when there is none.
CANON_COMMAND = "cargo run --quiet --bin asset_verify -- --canon assets/<id>/<id>.tank.ron"

#: Why there is no canon, when nobody asked for one.
CANON_ABSENT = "no --canon file was given"


@dataclass(frozen=True)
class Canon:
    """The two canonical lists this pass may not maintain a second copy of, as the Rust CLI emits
    them: every typed node reference the tank's spec sheet makes, and the substance registry's keys.

    So `L1.SPEC_REFERENCES` holds no vocabulary of RON field names and `L1.SUBSTANCE_IDENTITY` no
    vocabulary of substances; both laws are stated here in words Rust owns.
    """

    #: `(RON field, node name)` pairs, in the order the generator emitted them.
    node_references: tuple
    #: Every material datablock name the registry declares.
    substance_keys: frozenset

    @classmethod
    def read(cls, path):
        """`(canon, why not)`, exactly one of which is None."""
        if not path:
            return (None, CANON_ABSENT)
        try:
            with open(path, encoding="utf-8") as handle:
                document = json.load(handle)
            references = tuple(
                (str(row["field"]), str(row["node"])) for row in document["node_references"]
            )
            keys = frozenset(str(key) for key in document["substance_keys"])
        except (OSError, ValueError) as error:
            return (None, "{} is not readable as one JSON document: {}".format(path, error))
        except (KeyError, TypeError) as error:
            return (None, "{} does not hold the canon file's shape: {}".format(path, error))
        return (cls(references, keys), None)


def _canon_gate(check: Check) -> Check:
    """The mechanical refusal beside a law whose canonical input is absent. Its own id, so a report
    can never be read as the law itself having been evaluated and passed."""
    return Check(
        id=check.id + ".canon-missing",
        stage=check.stage,
        severity=Severity.ERROR,
        law="the canonical list {} is stated in was read".format(check.id),
    )


def _canon_missing(gate: Check, source: "Source") -> List[Finding]:
    return [Finding(
        gate,
        Subject(SubjectKind.FILE, source.filepath or "<unsaved>"),
        source.canon_note,
        "write the canon file with `{}` and pass it as --canon <file> — a law whose input is "
        "missing has not passed".format(CANON_COMMAND),
    )]


# ── L1: scene hygiene and structure ──────────────────────────────────────────────────────────────

SAVED_SOURCE = Check(
    id="L1.SAVED_SOURCE",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="the live file IS the stored file: a readable assets/<id>/<id>.blend that this Blender has "
        "open, with a sibling <id>.tank.ron",
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

SPEC_REFERENCES = Check(
    id="L1.SPEC_REFERENCES",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="every typed node reference the canon file carries resolves to exactly one export-bound "
        "object",
)

SPEC_REFERENCES_CANON = _canon_gate(SPEC_REFERENCES)

#: Blender's collision suffix: exactly three digits, appended to an otherwise stock name.
_COPY_SUFFIX = re.compile(r"\.\d{3}$")

#: The object types the exporter may be handed. Everything else is a workbench leftover.
EXPORT_BOUND_TYPES = frozenset({"MESH", "EMPTY"})


@dataclass
class Source:
    """The lint's whole view of one tank: where the blend is stored, the export-bound objects, and
    the canonical lists two of the laws are stated in."""

    #: `bpy.data.filepath` — empty when the blend has never been saved.
    filepath: str
    scene_name: str
    objects: List[object] = field(default_factory=list)
    canon: Optional[Canon] = None
    #: Why there is no canon, when `canon` is None.
    canon_note: str = CANON_ABSENT

    @classmethod
    def live(cls, scene=None, canon_path=None) -> "Source":
        """The open blend. `view_layer.update()` first: `matrix_local` is otherwise stale."""
        bpy.context.view_layer.update()
        scene = scene or bpy.context.scene
        canon, note = Canon.read(canon_path)
        return cls(
            filepath=bpy.data.filepath,
            scene_name=scene.name,
            objects=list(scene.objects),
            canon=canon,
            canon_note=note or CANON_ABSENT,
        )


def check_saved_source(source: Source) -> List[Finding]:
    """The stored file is the model; the door refuses to certify anything else.

    STORED AND LIVE ARE THE SAME BYTES BY CONSTRUCTION, NOT BY A FLAG. The headless door hands
    Blender the file to open, so what this process holds came off that disk; the GUI adapter saves
    before it invokes, so what it certifies is what it just wrote. `bpy.data.is_dirty` is not read
    and is not the seam: Blender sets it while VERSIONING an older file at load, so a freshly
    opened, untouched blend reports dirty (MEASURED, 5.1.2) and no save clears it.

    What is left for this law to prove is that the path is real and is THIS session's: a file that
    exists, carries the `.blend` extension, sits at the layout every derived path is cut from, is
    the file `bpy.data.filepath` names, and has its spec sheet beside it. A path measured against a
    file that was renamed, deleted or never opened certifies a model nobody can reopen.
    """
    findings = []
    if not source.filepath:
        return [Finding(
            SAVED_SOURCE,
            Subject(SubjectKind.FILE, "<unsaved>"),
            "bpy.data.filepath is empty — this blend has never been written to disk",
            "save the blend as assets/<id>/<id>.blend beside its <id>.tank.ron, then re-run",
        )]

    directory, filename = os.path.split(source.filepath)
    stem, extension = os.path.splitext(filename)
    subject = Subject(SubjectKind.FILE, source.filepath)

    if extension != ".blend":
        findings.append(Finding(
            SAVED_SOURCE,
            subject,
            "the stored file is named `{}`, whose extension is `{}`".format(
                filename, extension or "<none>"
            ),
            "save it as {}.blend — every derived path, the trio discovery in the hooks and the "
            "release prune all name the source by that extension".format(stem),
        ))

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

    if not os.path.isfile(source.filepath):
        findings.append(Finding(
            SAVED_SOURCE,
            subject,
            "no file stands at this path — the blend was renamed or deleted after it was opened",
            "save the session back to {} (File ▸ Save As) — the door certifies a file the next "
            "reader can open, and there is none here".format(source.filepath),
        ))

    open_path = bpy.data.filepath
    if os.path.realpath(open_path or "") != os.path.realpath(source.filepath):
        findings.append(Finding(
            SAVED_SOURCE,
            subject,
            "this Blender has {} open".format(open_path or "no file at all"),
            "run the door against the blend this session holds — a report on one file's path and "
            "another file's model certifies neither",
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


def check_spec_references(source: Source) -> List[Finding]:
    """The spec sheet addresses geometry by node name and nothing infers a node from a name pattern,
    so a reference naming no object binds the sim to a part that is not there — and one naming two
    binds it to whichever the exporter wrote second."""
    if source.canon is None:
        return _canon_missing(SPEC_REFERENCES_CANON, source)
    carried = Counter(obj.name for obj in source.objects)
    findings = []
    for ron_field, node in source.canon.node_references:
        matches = carried[node]
        if matches == 1:
            continue
        findings.append(Finding(
            SPEC_REFERENCES,
            Subject(SubjectKind.OBJECT, node, "declared in `{}`".format(ron_field)),
            "{} export-bound object(s) carry this name".format(matches),
            "rename the object the spec means to `{}`, export the one it expects, or edit `{}` in "
            "the spec sheet to the name the model actually carries".format(node, ron_field),
        ))
    return findings


# ── L1: geometry and attributes ──────────────────────────────────────────────────────────────────

NONEMPTY_MESH = Check(
    id="L1.NONEMPTY_MESH",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="every export-bound mesh has at least one polygon and produces at least one loop triangle",
)

FINITE_MESH_DATA = Check(
    id="L1.FINITE_MESH_DATA",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="vertex positions, referenced UV coordinates, colour attributes and derived corner normals "
        "consumed by export are finite",
)

ZERO_AREA_TRIANGLE = Check(
    id="L1.ZERO_AREA_TRIANGLE",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="after Blender's own loop triangulation, every triangle has an exactly non-zero cross "
        "product in stored mesh coordinates — no tolerance",
)

DUPLICATE_TRIANGLE = Check(
    id="L1.DUPLICATE_TRIANGLE",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="after exact-position welding within a mesh, no two loop triangles have the same unordered "
        "three positions; opposite winding does not make a duplicate legal",
)

LOOSE_GEOMETRY = Check(
    id="L1.LOOSE_GEOMETRY",
    stage=Stage.SOURCE,
    severity=Severity.WARNING,
    law="vertices and edges used by no polygon are reported",
)

TEXTURE_UV_SOURCE = Check(
    id="L1.TEXTURE_UV_SOURCE",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="every image texture reachable from an active Material Output resolves to an existing UV "
        "layer: a named UV Map node when present, otherwise the mesh's active-render UV layer",
)

UV_FINITE = Check(
    id="L1.UV_FINITE",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="every UV coordinate used by a sampled texture is finite",
)

ZERO_AREA_UV = Check(
    id="L1.ZERO_AREA_UV",
    stage=Stage.SOURCE,
    severity=Severity.WARNING,
    law="for each triangle assigned to a texture-sampling material, the referenced UV triangle has "
        "an exactly non-zero signed area",
)


def _export_meshes(source: Source):
    """Every export-bound mesh datablock once, named through the first object that carries it. A
    datablock two objects share is one stored surface, so it is one finding and not two."""
    seen = set()
    meshes = []
    for obj in source.objects:
        if obj.type != "MESH" or obj.data is None or obj.data in seen:
            continue
        seen.add(obj.data)
        meshes.append((obj.data, Subject(
            SubjectKind.MESH, obj.data.name, "on object `{}`".format(obj.name)
        )))
    return meshes


def _export_mesh_objects(source: Source):
    """Every export-bound (object, mesh) pair. Material-facing checks read these rather than the
    deduplicated datablocks: a material slot can be linked to the object, so two objects sharing a
    mesh can sample different textures through it."""
    return [(obj, obj.data) for obj in source.objects
            if obj.type == "MESH" and obj.data is not None]


def _triangles(mesh):
    """Blender's own loop triangulation — the same one the exporter writes. It is a cache, so
    `calc_loop_triangles` is what fills it after any edit a fixture made."""
    mesh.calc_loop_triangles()
    return mesh.loop_triangles


def _flat(collection, attribute, width):
    """One `foreach_get` buffer: element `i`'s components are `[i * width : (i + 1) * width]`."""
    buffer = [0.0] * (len(collection) * width)
    collection.foreach_get(attribute, buffer)
    return buffer


def _cross(a, b, c):
    """`(b - a) x (c - a)`, in Python floats.

    Not `mathutils`: its vectors are single precision, and the law decides "exactly non-zero" at
    the width the positions are stored at, not at a coarser one that rounds a thin triangle to
    nothing.
    """
    ux, uy, uz = b[0] - a[0], b[1] - a[1], b[2] - a[2]
    vx, vy, vz = c[0] - a[0], c[1] - a[1], c[2] - a[2]
    return (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx)


def _vertex_positions(mesh):
    """Stored vertex positions as triples, in vertex-index order."""
    flat = _flat(mesh.vertices, "co", 3)
    return [tuple(flat[index:index + 3]) for index in range(0, len(flat), 3)]


def _triangle_vertices(mesh, triangles):
    """Each loop triangle's three vertex indices."""
    flat = _flat(triangles, "vertices", 3)
    return [tuple(flat[index:index + 3]) for index in range(0, len(flat), 3)]


def check_nonempty_mesh(source: Source) -> List[Finding]:
    """A mesh that triangulates to nothing exports a primitive with no surface, which the ballistic
    contract then has to refuse far from the file that holds it."""
    findings = []
    for mesh, subject in _export_meshes(source):
        triangles = _triangles(mesh)
        if len(mesh.polygons) and len(triangles):
            continue
        findings.append(Finding(
            NONEMPTY_MESH,
            subject,
            "{} polygon(s), {} loop triangle(s), {} vertices".format(
                len(mesh.polygons), len(triangles), len(mesh.vertices)
            ),
            "give it surface or delete the object — a mesh with no triangle exports a primitive "
            "nothing can consume",
        ))
    return findings


def _non_finite(subject, values, width, label, kind):
    """One finding per attribute per mesh: the count, and the first element that holds it."""
    for index in range(0, len(values), width):
        element = values[index:index + width]
        if all(math.isfinite(value) for value in element):
            continue
        count = sum(
            1 for start in range(0, len(values), width)
            if not all(math.isfinite(value) for value in values[start:start + width])
        )
        return [Finding(
            FINITE_MESH_DATA,
            dataclasses.replace(subject, element="{}, {}".format(subject.element, label)),
            "{} of {} {} are non-finite; first at {} {} = {}".format(
                count, len(values) // width, kind, kind[:-1], index // width, tuple(element)
            ),
            "retype or rebuild the attribute — a non-finite number reaches every consumer of this "
            "mesh and poisons whatever it is composed with",
        )]
    return []


def check_finite_mesh_data(source: Source) -> List[Finding]:
    """Everything export reads off a mesh datablock: positions, every UV layer it writes, colour
    attributes, and the corner normals it derives."""
    findings = []
    for mesh, subject in _export_meshes(source):
        findings.extend(_non_finite(
            subject, _flat(mesh.vertices, "co", 3), 3, "vertex positions", "positions"
        ))
        for layer in mesh.uv_layers:
            findings.extend(_non_finite(
                subject, _flat(layer.uv, "vector", 2), 2,
                "UV layer `{}`".format(layer.name), "coordinates",
            ))
        for attribute in mesh.color_attributes:
            findings.extend(_non_finite(
                subject, _flat(attribute.data, "color", 4), 4,
                "colour attribute `{}`".format(attribute.name), "colours",
            ))
        findings.extend(_non_finite(
            subject, _flat(mesh.corner_normals, "vector", 3), 3, "corner normals", "normals"
        ))
    return findings


def check_zero_area_triangle(source: Source) -> List[Finding]:
    """A triangle with no cross product has no normal and no plane; it is a hole the surface says
    is a face."""
    findings = []
    for mesh, subject in _export_meshes(source):
        triangles = _triangles(mesh)
        positions = _vertex_positions(mesh)
        collapsed = []
        for index, (a, b, c) in enumerate(_triangle_vertices(mesh, triangles)):
            if any(value != 0.0 for value in _cross(positions[a], positions[b], positions[c])):
                continue
            collapsed.append((index, (a, b, c)))
        if not collapsed:
            continue
        index, corners = collapsed[0]
        findings.append(Finding(
            ZERO_AREA_TRIANGLE,
            subject,
            "{} of {} loop triangles have an exactly zero cross product; first at triangle {}, "
            "vertices {} at {}".format(
                len(collapsed), len(triangles), index, corners,
                tuple(positions[corner] for corner in corners),
            ),
            "merge the coincident or collinear vertices (M ▸ By Distance, then Mesh ▸ Clean Up ▸ "
            "Degenerate Dissolve) — a zero-area face carries no direction for anything to read",
        ))
    return findings


def check_duplicate_triangle(source: Source) -> List[Finding]:
    """Welding by exact position first, because two triangles built on distinct vertices that sit
    at the same coordinates are one surface twice. Winding is not part of the key: a reversed copy
    is still a second face in the same place."""
    findings = []
    for mesh, subject in _export_meshes(source):
        triangles = _triangles(mesh)
        positions = _vertex_positions(mesh)
        weld = {}
        welded = [weld.setdefault(position, len(weld)) for position in positions]
        first = {}
        repeats = []
        for index, corners in enumerate(_triangle_vertices(mesh, triangles)):
            key = tuple(sorted(welded[corner] for corner in corners))
            if key in first:
                repeats.append((first[key], index, key))
            else:
                first[key] = index
        if not repeats:
            continue
        original, repeat, key = repeats[0]
        findings.append(Finding(
            DUPLICATE_TRIANGLE,
            subject,
            "{} of {} loop triangles repeat a triangle the mesh already carries; first at "
            "triangles {} and {}, welded vertices {}".format(
                len(repeats), len(triangles), original, repeat, key
            ),
            "delete the duplicated faces (Mesh ▸ Clean Up ▸ Merge by Distance, or select the doubled "
            "shell and delete it) — a face drawn twice is drawn once with an inside-out twin",
        ))
    return findings


def check_loose_geometry(source: Source) -> List[Finding]:
    """Geometry no polygon uses is invisible to the exporter and to the sim, and usually the residue
    of an edit nobody finished."""
    findings = []
    for mesh, subject in _export_meshes(source):
        used_vertices = set()
        used_edges = set()
        for polygon in mesh.polygons:
            used_vertices.update(polygon.vertices)
            used_edges.update(polygon.edge_keys)
        loose_vertices = [
            index for index in range(len(mesh.vertices)) if index not in used_vertices
        ]
        loose_edges = [
            index for index, edge in enumerate(mesh.edges)
            if tuple(sorted(edge.vertices)) not in used_edges
        ]
        if not loose_vertices and not loose_edges:
            continue
        findings.append(Finding(
            LOOSE_GEOMETRY,
            subject,
            "{} of {} vertices and {} of {} edges belong to no polygon; first loose vertex {}, "
            "first loose edge {}".format(
                len(loose_vertices), len(mesh.vertices), len(loose_edges), len(mesh.edges),
                loose_vertices[0] if loose_vertices else "none",
                loose_edges[0] if loose_edges else "none",
            ),
            "select it (Select ▸ All by Trait ▸ Loose Geometry) and either build the face it was "
            "meant to carry or delete it",
        ))
    return findings


def _output_node(material):
    """The active Material Output, or None. `use_nodes` is not read: Blender 5.1 deprecates it, and
    a material with no node tree has no texture either way."""
    tree = getattr(material, "node_tree", None)
    if tree is None:
        return None
    for target in ("ALL", "EEVEE", "CYCLES"):
        node = tree.get_output_node(target)
        if node is not None:
            return node
    return None


def _socket_of(sockets, identifier):
    """The socket on the other side of a group interface. Blender gives a group node's socket and
    the matching one on the group's own Group Input/Output node the SAME identifier, which is what
    makes the correspondence exact rather than positional — an interface socket added, removed or
    reordered moves both ends together."""
    for socket in sockets:
        if socket.identifier == identifier:
            return socket
    return None


def _group_output(node):
    """The Group Output node a group node's outputs come from: the active one, because a tree may
    hold several and only that one is evaluated."""
    tree = node.node_tree
    if tree is None:
        return None
    outputs = [inner for inner in tree.nodes if inner.type == "GROUP_OUTPUT"]
    for inner in outputs:
        if getattr(inner, "is_active_output", False):
            return inner
    return outputs[0] if outputs else None


def _at(context, socket):
    """One traversal position, as a hashable key. A node inside a group is reached THROUGH a group
    node, and the same inner socket reached through two group nodes is two positions."""
    return (tuple(group.as_pointer() for group in context), socket.as_pointer())


def _sources(socket, context):
    """Every `(node, output socket, context)` that actually drives this input socket.

    Three things between two shading operations are not shading operations, and Blender evaluates
    straight through all of them — so the walk does too, or it reads a graph the renderer and the
    exporter do not:

    - a MUTED node passes its `internal_links`, which map one of its inputs to one of its outputs;
      a muted node with no internal link for the output asked about (a group node, measured) drives
      nothing at all;
    - a GROUP node is entered at the Group Output socket corresponding to the OUTER output socket
      the link left from — never at every Group Output the tree holds;
    - a GROUP INPUT node is left through the corresponding input socket of the group node the walk
      entered by, which is why the context is a stack and not a flag.

    `context` is the group nodes entered, outermost first. The visited set is over
    (context, socket), so a diamond is walked once and a cycle terminates.
    """
    found = []
    pending = [(socket, context)]
    seen = set()
    while pending:
        target, where = pending.pop()
        key = _at(where, target)
        if key in seen:
            continue
        seen.add(key)
        for link in target.links:
            node, driving = link.from_node, link.from_socket
            if node.mute:
                pending.extend(
                    (internal.from_socket, where) for internal in node.internal_links
                    if internal.to_socket.as_pointer() == driving.as_pointer()
                )
            elif node.type == "GROUP":
                inner = _group_output(node)
                matched = _socket_of(inner.inputs, driving.identifier) if inner else None
                if matched is not None:
                    pending.append((matched, where + (node,)))
            elif node.type == "GROUP_INPUT":
                matched = _socket_of(where[-1].inputs, driving.identifier) if where else None
                if matched is not None:
                    pending.append((matched, where[:-1]))
            else:
                found.append((node, driving, where))
    return found


def _group_path(context):
    """The groups a node was reached through, as a report reads them. The node TREE is named, not
    the node holding it: a group node is auto-named `Group`, and the tree is what an artist opens."""
    return "".join(
        "group `{}` ▸ ".format(group.node_tree.name if group.node_tree else group.name)
        for group in context
    )


def _image_textures(material):
    """Every Image Texture reachable backwards from the active Material Output, node groups
    included, as `(node, context)` pairs. A texture the output does not reach is not exported and
    not sampled, so no law here is about it — which is exactly what the socket correspondence in
    `_sources` decides: a group's OTHER output, connected to nothing, carries none of the textures
    behind it into this material.
    """
    output = _output_node(material)
    if output is None:
        return []
    found = []
    seen = set()
    pending = [(socket, ()) for socket in output.inputs]
    while pending:
        socket, context = pending.pop()
        for node, _driving, where in _sources(socket, context):
            key = (tuple(group.as_pointer() for group in where), node.as_pointer())
            if key in seen:
                continue
            seen.add(key)
            if node.type == "TEX_IMAGE":
                found.append((node, where))
            pending.extend((inner, where) for inner in node.inputs)
    return sorted(found, key=lambda reached: (_group_path(reached[1]), reached[0].name))


#: What an Image Texture samples: a named UV layer, whichever layer is active for render, or a
#: coordinate the door does not carry into glTF.
UV_NAMED, UV_ACTIVE_RENDER, UV_UNSUPPORTED = "named", "active-render", "unsupported"

#: Vector nodes that only transform coordinates, so the UV source is whatever feeds them.
_UV_PASSTHROUGH = frozenset({"MAPPING", "REROUTE"})


def _vector_input(node):
    """The coordinate input of a node that consumes one."""
    return node.inputs.get("Vector") or (node.inputs[0] if len(node.inputs) else None)


def _uv_source(node, context=()):
    """Which UV layer an Image Texture reads, traced back through its Vector input and out through
    every group interface between here and whatever drives it.

    There is NO depth limit and no fallback: the walk either arrives at a coordinate the door can
    name — a UV Map node, Texture Coordinate ▸ UV, or the texture's own unlinked Vector, which is
    Blender's active-render default — or it refuses. A chain long enough to give up on is not a
    chain the exporter carries into glTF either. The visited set is what terminates a cycle; the
    node tree Blender itself builds holds none, so it guards a tree another writer produced.

    Only the TEXTURE's own Vector input falls back to the active-render layer. A Mapping node with
    nothing in its own Vector reads the socket's constant, which is not a UV at all.
    """
    socket = _vector_input(node)
    if socket is None:
        return (UV_ACTIVE_RENDER, None)
    fallback = (UV_ACTIVE_RENDER, None)
    seen = set()
    while True:
        key = _at(context, socket)
        if key in seen:
            return (UV_UNSUPPORTED, "a cycle through `{}`".format(socket.node.name))
        seen.add(key)
        driving = _sources(socket, context)
        if not driving:
            return fallback
        if len(driving) > 1:
            return (UV_UNSUPPORTED, "{} links driving one coordinate input".format(len(driving)))
        upstream, from_socket, context = driving[0]
        if upstream.type == "UVMAP":
            return (UV_NAMED, upstream.uv_map) if upstream.uv_map else (UV_ACTIVE_RENDER, None)
        if upstream.type == "TEX_COORD":
            if from_socket.name == "UV":
                return (UV_ACTIVE_RENDER, None)
            return (UV_UNSUPPORTED, "Texture Coordinate ▸ {}".format(from_socket.name))
        if upstream.type not in _UV_PASSTHROUGH:
            return (UV_UNSUPPORTED, "{}{} node `{}`".format(
                _group_path(context), upstream.type, upstream.name
            ))
        socket = _vector_input(upstream)
        if socket is None:
            return (UV_UNSUPPORTED, "{} node `{}` with no coordinate input".format(
                upstream.type, upstream.name
            ))
        fallback = (UV_UNSUPPORTED, "{}{} node `{}`, whose own coordinate input is unlinked".format(
            _group_path(context), upstream.type, upstream.name
        ))


def _active_render_uv(mesh):
    """The layer Blender exports as the first UV set, or None when the mesh carries none."""
    for layer in mesh.uv_layers:
        if layer.active_render:
            return layer.name
    return None


def _used_materials(obj):
    """`{slot index: material or None}` for the slots this object's polygons actually reference,
    resolved through `material_slots` because a slot can be linked to the object rather than to the
    mesh."""
    slots = obj.material_slots
    used = {}
    for polygon in obj.data.polygons:
        index = polygon.material_index
        if index in used:
            continue
        used[index] = slots[index].material if 0 <= index < len(slots) else None
    return used


def _texture_element(obj, material, node, context=()):
    """Where a texture finding sits: the object that carries the mesh, the material on it, the node
    groups the walk entered, and the node itself."""
    return "on object `{}`, material `{}` ▸ {}texture `{}`".format(
        obj.name, material.name, _group_path(context), node.name
    )


def check_texture_uv_source(source: Source) -> List[Finding]:
    """A texture whose UV layer the mesh does not carry samples nothing; a non-UV coordinate is a
    procedural the exporter cannot write at all."""
    findings = []
    for obj, mesh in _export_mesh_objects(source):
        layers = [layer.name for layer in mesh.uv_layers]
        render = _active_render_uv(mesh)
        for index, material in sorted(_used_materials(obj).items()):
            if material is None:
                continue
            for node, context in _image_textures(material):
                kind, detail = _uv_source(node, context)
                subject = Subject(
                    SubjectKind.MESH, mesh.name, _texture_element(obj, material, node, context)
                )
                if kind == UV_UNSUPPORTED:
                    findings.append(Finding(
                        TEXTURE_UV_SOURCE,
                        subject,
                        "sampled through {}, which is not a UV coordinate".format(detail),
                        "drive the texture from a UV Map node or Texture Coordinate ▸ UV and unwrap "
                        "the mesh — glTF carries UV sets, not procedural coordinates",
                    ))
                elif kind == UV_NAMED and detail not in layers:
                    findings.append(Finding(
                        TEXTURE_UV_SOURCE,
                        subject,
                        "UV Map node names layer `{}`; this mesh carries {}".format(
                            detail, layers or "no UV layer"
                        ),
                        "rename the layer to `{}` or point the UV Map node at a layer this mesh "
                        "carries".format(detail),
                    ))
                elif kind == UV_ACTIVE_RENDER and render is None:
                    findings.append(Finding(
                        TEXTURE_UV_SOURCE,
                        subject,
                        "falls back to the active-render UV layer; this mesh carries {}".format(
                            "no UV layer" if not layers else
                            "{} layer(s), none marked active for render".format(len(layers))
                        ),
                        "unwrap the mesh and mark the layer active for render (Object Data ▸ UV "
                        "Maps ▸ camera icon)",
                    ))
    return findings


def _sampled_uv_layers(obj):
    """`{slot index: [uv layer name, ...]}` — the layers a texture actually reads on this object.

    A texture whose UV source does not resolve is `L1.TEXTURE_UV_SOURCE`'s finding and is dropped
    here, so the UV laws below never report a second time on it.
    """
    mesh = obj.data
    render = _active_render_uv(mesh)
    sampled = {}
    for index, material in _used_materials(obj).items():
        if material is None:
            continue
        names = set()
        for node, context in _image_textures(material):
            kind, detail = _uv_source(node, context)
            name = detail if kind == UV_NAMED else (render if kind == UV_ACTIVE_RENDER else None)
            if name is not None and mesh.uv_layers.get(name) is not None:
                names.add(name)
        if names:
            sampled[index] = sorted(names)
    return sampled


def _sampled_triangles(mesh, triangles, sampled, layer_name):
    """The loop triangles whose material samples `layer_name`, with their three loop indices."""
    materials = _flat(triangles, "material_index", 1)
    loops = _flat(triangles, "loops", 3)
    return [
        (index, tuple(loops[index * 3:index * 3 + 3]))
        for index in range(len(triangles))
        if layer_name in sampled.get(int(materials[index]), ())
    ]


def _sampled_layer_names(sampled):
    return sorted({name for names in sampled.values() for name in names})


def check_uv_finite(source: Source) -> List[Finding]:
    """Only the UVs a texture reads: a collapsed or non-finite coordinate under an untextured
    substance is not a defect in the sampled surface."""
    findings = []
    for obj, mesh in _export_mesh_objects(source):
        sampled = _sampled_uv_layers(obj)
        if not sampled:
            continue
        triangles = _triangles(mesh)
        for layer_name in _sampled_layer_names(sampled):
            coordinates = _flat(mesh.uv_layers[layer_name].uv, "vector", 2)
            bad = sorted({
                loop
                for _, corners in _sampled_triangles(mesh, triangles, sampled, layer_name)
                for loop in corners
                if not (math.isfinite(coordinates[loop * 2])
                        and math.isfinite(coordinates[loop * 2 + 1]))
            })
            if not bad:
                continue
            findings.append(Finding(
                UV_FINITE,
                Subject(SubjectKind.MESH, mesh.name, "on object `{}`, sampled UV layer `{}`".format(
                    obj.name, layer_name
                )),
                "{} sampled corner(s) hold a non-finite UV; first at loop {} = {}".format(
                    len(bad), bad[0], (coordinates[bad[0] * 2], coordinates[bad[0] * 2 + 1])
                ),
                "re-unwrap the affected faces — a non-finite UV samples nothing and writes a NaN "
                "into the exported accessor",
            ))
    return findings


def check_zero_area_uv(source: Source) -> List[Finding]:
    """A UV triangle with no signed area samples a single point of the texture, so the whole face
    takes one texel's colour."""
    findings = []
    for obj, mesh in _export_mesh_objects(source):
        sampled = _sampled_uv_layers(obj)
        if not sampled:
            continue
        triangles = _triangles(mesh)
        for layer_name in _sampled_layer_names(sampled):
            coordinates = _flat(mesh.uv_layers[layer_name].uv, "vector", 2)
            checked = _sampled_triangles(mesh, triangles, sampled, layer_name)
            collapsed = []
            for index, corners in checked:
                (u0, v0), (u1, v1), (u2, v2) = (
                    (coordinates[loop * 2], coordinates[loop * 2 + 1]) for loop in corners
                )
                if (u1 - u0) * (v2 - v0) - (u2 - u0) * (v1 - v0) != 0.0:
                    continue
                collapsed.append((index, corners))
            if not collapsed:
                continue
            index, corners = collapsed[0]
            findings.append(Finding(
                ZERO_AREA_UV,
                Subject(SubjectKind.MESH, mesh.name, "on object `{}`, sampled UV layer `{}`".format(
                    obj.name, layer_name
                )),
                "{} of {} sampled loop triangles have an exactly zero signed UV area; first at "
                "triangle {}, corners {}".format(
                    len(collapsed), len(checked), index,
                    tuple((coordinates[loop * 2], coordinates[loop * 2 + 1]) for loop in corners),
                ),
                "re-unwrap the affected faces — a collapsed UV triangle paints the whole face with "
                "one texel",
            ))
    return findings


# ── L1: materials ────────────────────────────────────────────────────────────────────────────────

SUBSTANCE_IDENTITY = Check(
    id="L1.SUBSTANCE_IDENTITY",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="a material whose name is a registry key is the one linked from canonical "
        "assets/materials/materials.blend; a material linked from that library resolves to an "
        "exact registry key; case-folded and .### near-misses of registry keys are refused",
)

SUBSTANCE_IDENTITY_CANON = _canon_gate(SUBSTANCE_IDENTITY)

#: Where the canonical material library stands, innermost last. Identity is that path relationship
#: — the library datablock's own name is `materials.blend` for any file so called.
CANONICAL_LIBRARY = ("assets", "materials", "materials.blend")


def _canonically_linked(material) -> bool:
    """Whether this material IS the datablock the canonical library owns."""
    library = getattr(material, "library", None)
    if library is None:
        return False
    directory, filename = os.path.split(os.path.abspath(bpy.path.abspath(library.filepath)))
    holder = os.path.basename(directory)
    return (os.path.basename(os.path.dirname(directory)), holder, filename) == CANONICAL_LIBRARY


def _near_miss(name: str, keys) -> Optional[str]:
    """The registry key a name is not, but reads as: the same name up to letter case, Blender's
    `.###` collision suffix, or both."""
    spellings = {name.casefold(), _COPY_SUFFIX.sub("", name).casefold()}
    for key in sorted(keys):
        if key.casefold() in spellings:
            return key
    return None


def _export_materials(source: Source):
    """Every material an export-bound object's polygons reference, once, with an object that wears
    it. Keyed by name AND library: a counterfeit and the datablock it imitates share a name, which
    is the defect itself."""
    seen = set()
    materials = []
    for obj, _mesh in _export_mesh_objects(source):
        for _index, material in sorted(_used_materials(obj).items()):
            if material is None:
                continue
            library = material.library.filepath if material.library is not None else None
            if (material.name, library) in seen:
                continue
            seen.add((material.name, library))
            materials.append((material, obj))
    return materials


def check_substance_identity(source: Source) -> List[Finding]:
    """Membership in the substance registry IS wearing the registry's material, so the name and the
    datablock have to be one thing. A local material called `RHA` is a counterfeit: it would bind
    armour numbers the library never issued, and the exported glTF says nothing about where a
    material came from."""
    if source.canon is None:
        return _canon_missing(SUBSTANCE_IDENTITY_CANON, source)
    keys = source.canon.substance_keys
    findings = []
    for material, obj in _export_materials(source):
        subject = Subject(SubjectKind.MATERIAL, material.name, "on object `{}`".format(obj.name))
        canonical = _canonically_linked(material)
        if material.name in keys:
            if canonical:
                continue
            findings.append(Finding(
                SUBSTANCE_IDENTITY,
                subject,
                "bears the registry key `{}` and is {}".format(
                    material.name,
                    "local to this blend" if material.library is None else
                    "linked from {}".format(material.library.filepath),
                ),
                "delete it and link `{}` from assets/materials/{} (File ▸ Link) — a substance is "
                "the library's datablock, never a name typed over it".format(
                    material.name, CANONICAL_LIBRARY[-1]
                ),
            ))
            continue
        near = _near_miss(material.name, keys)
        if near is not None:
            findings.append(Finding(
                SUBSTANCE_IDENTITY,
                subject,
                "reads as the registry key `{}` without being it".format(near),
                "if this is armour, delete it and link `{}` from assets/materials/{}; if it is art, "
                "give it a name that is not one of the registry's".format(
                    near, CANONICAL_LIBRARY[-1]
                ),
            ))
            continue
        if canonical:
            findings.append(Finding(
                SUBSTANCE_IDENTITY,
                subject,
                "linked from the canonical library, and the registry declares no such key",
                "relink the material to a key the registry declares, or author `{}` in "
                "assets/materials/materials.ron — the datablock name is the join key between the "
                "library and the numbers".format(material.name),
            ))
    return findings


TEXTURE_SOURCE = Check(
    id="L1.TEXTURE_SOURCE",
    stage=Stage.SOURCE,
    severity=Severity.ERROR,
    law="every used image texture is packed or resolves to an existing readable file, and has "
        "non-zero dimensions",
)

SOURCE_CENSUS = Check(
    id="L1.SOURCE_CENSUS",
    stage=Stage.SOURCE,
    severity=Severity.INFO,
    law="the source census is printed and compared only against the previous committed source; no "
        "count in it is a verdict",
)


def check_texture_source(source: Source) -> List[Finding]:
    """The blend is the sole source, so a texture it only points at is not in it. Packed bytes or a
    readable file, and a decodable image either way."""
    findings = []
    seen = set()
    for obj, _mesh in _export_mesh_objects(source):
        for index, material in sorted(_used_materials(obj).items()):
            if material is None:
                continue
            for node, context in _image_textures(material):
                # By DATABLOCK, never by name: Blender numbers a node's name inside its own tree
                # only, so two materials — a counterfeit beside the datablock it imitates, say —
                # ordinarily both hold an `Image Texture` and one name key would check one of them.
                # The same node reached through two objects is still one node, which is what this
                # skips.
                if node.as_pointer() in seen:
                    continue
                seen.add(node.as_pointer())
                subject = Subject(
                    SubjectKind.MATERIAL, material.name,
                    "{}texture `{}`".format(_group_path(context), node.name),
                )
                image = node.image
                if image is None:
                    findings.append(Finding(
                        TEXTURE_SOURCE,
                        subject,
                        "the Image Texture node holds no image",
                        "assign an image or delete the node — the material samples a texture that "
                        "does not exist",
                    ))
                    continue
                if image.packed_file is None:
                    path = bpy.path.abspath(image.filepath, library=image.library)
                    if not path or not os.path.isfile(path) or not os.access(path, os.R_OK):
                        findings.append(Finding(
                            TEXTURE_SOURCE,
                            subject,
                            "image `{}` is not packed and its file is not readable: {}".format(
                                image.name, image.filepath or "<no filepath>"
                            ),
                            "pack it (File ▸ External Data ▸ Pack Resources) or repoint it at a "
                            "file in the repository — the blend is the sole source",
                        ))
                        continue
                width, height = tuple(image.size)
                if width and height:
                    continue
                findings.append(Finding(
                    TEXTURE_SOURCE,
                    subject,
                    "image `{}` decodes to {}x{}".format(image.name, width, height),
                    "replace it with an image that decodes — a zero-dimension texture bakes to "
                    "nothing and fails the encoder later and less locally",
                ))
    return findings


#: The census keys, and the row label each prints under. Sorted output orders the rows; this map
#: only names them.
_CENSUS_LABELS = (
    ("objects", "objects"),
    ("meshes", "meshes"),
    ("primitives", "primitives"),
    ("ballistic_objects", "ballistic objects"),
)


def census(source: Source) -> dict:
    """The source census: what this blend holds, counted.

    A primitive is one material slot a mesh's polygons reference — what the exporter splits a mesh
    into and what the consumer binds. CONSTRAINT: a primitive is a SUBSTANCE primitive when its
    material is library-linked, and an object is ballistic when it carries one — the same mechanism
    `L1.SUBSTANCE_IDENTITY` holds exactly, membership through the link to the canonical library.
    The census does not read the registry key list: it is INFO, and no count in it is a verdict.
    """
    meshes = set()
    primitives = 0
    ballistic = 0
    substances = Counter()
    for obj in source.objects:
        if obj.type != "MESH" or obj.data is None:
            continue
        meshes.add(obj.data.name)
        used = _used_materials(obj)
        primitives += len(used)
        linked = [material for material in used.values()
                  if material is not None and material.library is not None]
        if linked:
            ballistic += 1
        for material in linked:
            substances[material.name] += 1
    return {
        "objects": len(source.objects),
        "meshes": len(meshes),
        "primitives": primitives,
        "ballistic_objects": ballistic,
        "substances": dict(substances),
    }


#: How the baseline adapter's Blender subprocess hands its census back through Blender's own noise.
CENSUS_SENTINEL = "SOURCE-CENSUS-JSON"


def _git(directory, *arguments):
    """One git call in the worktree that holds the blend. Returns `(returncode, stdout bytes)`."""
    try:
        result = subprocess.run(
            ("git",) + arguments, cwd=directory, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        )
    except OSError as error:
        return (1, str(error).encode())
    return (result.returncode, result.stdout)


#: What Git LFS writes into the tree in place of the file. The blend is LFS-tracked, so HEAD holds
#: this and not the model.
_LFS_POINTER = b"version https://git-lfs.github.com/spec/v1"
_LFS_OID = re.compile(rb"^oid sha256:([0-9a-f]{64})$", re.MULTILINE)


def _baseline_blend(filepath, workdir):
    """The previous committed blend as a readable file, or `(None, why not)`.

    The baseline is HEAD of the worktree the blend lives in, resolved offline: an LFS object this
    clone has not fetched is an absent baseline, never a download and never a verdict.
    """
    directory = os.path.dirname(filepath) or "."
    code, top = _git(directory, "rev-parse", "--show-toplevel")
    if code:
        return (None, "{} is not inside a git worktree".format(directory))
    # Through `realpath` on both sides: git reports the worktree with its symlinks resolved, and a
    # path relative to the unresolved one climbs out of the repository instead of into it.
    relative = os.path.relpath(os.path.realpath(filepath), os.path.realpath(top.decode().strip()))
    revision = "HEAD:{}".format(relative)
    if _git(directory, "cat-file", "-e", revision)[0]:
        return (None, "HEAD holds no {}".format(relative))
    code, blob = _git(directory, "cat-file", "blob", revision)
    if code:
        return (None, "git could not read {}".format(revision))
    if blob.startswith(_LFS_POINTER):
        match = _LFS_OID.search(blob)
        if match is None:
            return (None, "HEAD:{} is an LFS pointer with no oid".format(relative))
        oid = match.group(1).decode()
        code, common = _git(directory, "rev-parse", "--git-common-dir")
        if code:
            return (None, "git could not locate this clone's object store")
        store = os.path.join(directory, common.decode().strip())
        stored = os.path.join(store, "lfs", "objects", oid[:2], oid[2:4], oid)
        if not os.path.isfile(stored):
            return (None, "the LFS object for HEAD:{} is not in this clone".format(relative))
        return (os.path.abspath(stored), None)
    path = os.path.join(workdir, "baseline.blend")
    with open(path, "wb") as handle:
        handle.write(blob)
    return (path, None)


def _census_of(path):
    """The census of a blend this session does not have open, computed by the same code — Blender
    opens it in a subprocess and this script prints its own census there."""
    result = subprocess.run(
        [
            bpy.app.binary_path, "--background", "--factory-startup", path,
            "--python", os.path.abspath(__file__), "--",
            "--mode", "lint", "--census-json",
        ],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    )
    for line in result.stdout.decode(errors="replace").splitlines():
        if line.startswith(CENSUS_SENTINEL):
            return json.loads(line[len(CENSUS_SENTINEL):])
    return None


def baseline_census(filepath):
    """The previous committed source's census, or `(None, why not)`."""
    if not filepath:
        return (None, "this blend has never been written to disk")
    with tempfile.TemporaryDirectory(prefix="tank-census-") as workdir:
        path, note = _baseline_blend(filepath, workdir)
        if path is None:
            return (None, note)
        counted = _census_of(path)
    if counted is None:
        return (None, "Blender could not read the previous committed {}".format(
            os.path.basename(filepath)
        ))
    return (counted, None)


def _census_finding(subject, element, evidence) -> Finding:
    return Finding(
        SOURCE_CENSUS,
        dataclasses.replace(subject, element=element),
        evidence,
        "nothing to repair — read the diff as evidence of what this edit did to the source",
    )


def _counted(current, baseline):
    """One census row: the absolute count, and the movement when there is a baseline to move from."""
    if baseline is None:
        return "{}".format(current)
    return "{} (baseline {}, {:+d})".format(current, baseline, current - baseline)


def check_source_census(source: Source) -> List[Finding]:
    """Evidence, never a gate. The design forbids reading any of these counts as a pass condition:
    a second vehicle is a different model, not a broken one."""
    current = census(source)
    baseline, note = baseline_census(source.filepath)
    subject = Subject(SubjectKind.FILE, source.filepath or "<unsaved>")
    findings = [_census_finding(
        subject, "baseline",
        "no source baseline — {}".format(note) if baseline is None else
        "compared against the previous committed source at HEAD",
    )]
    for key, label in _CENSUS_LABELS:
        findings.append(_census_finding(
            subject, label, _counted(current[key], None if baseline is None else baseline[key])
        ))
    substances = current["substances"]
    was = {} if baseline is None else baseline["substances"]
    for name in sorted(set(substances) | set(was)):
        findings.append(_census_finding(
            subject, "substance `{}`".format(name),
            "{} primitive(s)".format(_counted(
                substances.get(name, 0), None if baseline is None else was.get(name, 0)
            )),
        ))
    return findings


#: The source pass, in table order. A check appears here exactly once; the report's own sort, not
#: this order, decides what the console prints first.
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
    check_spec_references,
    check_nonempty_mesh,
    check_finite_mesh_data,
    check_zero_area_triangle,
    check_duplicate_triangle,
    check_loose_geometry,
    check_texture_uv_source,
    check_uv_finite,
    check_zero_area_uv,
    check_substance_identity,
    check_texture_source,
    check_source_census,
)


def lint(source: Source) -> List[Finding]:
    """Run the L1 source pass. Returns the whole report, sorted."""
    findings = []
    for check in L1_CHECKS:
        findings.extend(check(source))
    return report.sorted_findings(findings)


# ── the modes ────────────────────────────────────────────────────────────────────────────────────

RAW_EXPORT = Check(
    id="door.raw-export",
    stage=Stage.DOOR,
    severity=Severity.ERROR,
    law="the exporter writes the raw candidate the rest of the chain reads",
)

#: The exporter's whole argument list, frozen: any other argument changes the asset.
#:
#: `export_animations=False` is defence against the exporter's own default and never a substitute
#: for `L1.ANIMATION`. `use_active_scene=True` is what makes EXPORT-BOUND mean what the source pass
#: measured — without it the exporter writes every scene in the file, workbench scenes included.
EXPORT_SETTINGS = {
    "export_format": "GLB",
    "export_tangents": True,
    "export_animations": False,
    "use_active_scene": True,
}


def export_raw(path: str) -> List[Finding]:
    """Write the raw, mipless candidate to `path`. Nothing is repaired, reduced or replayed on the
    way out: the exported bytes are what the blend holds."""
    directory = os.path.dirname(path)
    if directory:
        os.makedirs(directory, exist_ok=True)
    try:
        result = bpy.ops.export_scene.gltf(filepath=path, **EXPORT_SETTINGS)
    except RuntimeError as error:
        result = str(error)
    if result == {"FINISHED"} and os.path.isfile(path):
        print("raw   ▸ {} — {:.1f} MB (mipless, temporary)".format(
            path, os.path.getsize(path) / 1e6
        ), flush=True)
        return []
    return [Finding(
        RAW_EXPORT,
        Subject(SubjectKind.FILE, path),
        "export_scene.gltf returned {} and {} the file".format(
            result, "wrote" if os.path.isfile(path) else "did not write"
        ),
        "read the exporter's own error above — the tracked glb is untouched, and the chain stops "
        "at the candidate it could not write",
    )]


def unimplemented(mode: str) -> List[Finding]:
    """A mode with no chain behind it refuses mechanically rather than passing silently."""
    return [Finding(
        MODE_UNIMPLEMENTED,
        Subject(SubjectKind.DOOR, mode),
        "mode `{}` has no chain behind it in this door".format(mode),
        "run `--mode lint`, or invoke the door through scripts/tank/asset_door.py, which passes "
        "the candidate path every other mode writes to",
    )]


def run(mode: str, canon: Optional[str] = None, raw: Optional[str] = None) -> List[Finding]:
    """The Blender half of one mode: the precondition, the source pass, and — for the two modes
    with a chain behind them — the raw candidate.

    An L1 ERROR stops before the export: a candidate cut from a refused source is a file nobody may
    consume, and writing one invites it being picked up. Warnings do not stop anything.
    """
    refused = check_exporter() or check_unresolved_library()
    if refused:
        return report.sorted_findings(refused)
    findings = lint(Source.live(canon_path=canon))
    if mode == "lint":
        return findings
    if report.has_error(findings):
        return findings
    if not raw:
        return report.sorted_findings(findings + unimplemented(mode))
    return report.sorted_findings(findings + export_raw(raw))


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
    parser.add_argument("--canon", help="the canon file `{}` writes: the canonical node-reference "
                                        "list and substance keys L1.SPEC_REFERENCES and "
                                        "L1.SUBSTANCE_IDENTITY are stated in".format(CANON_COMMAND))
    parser.add_argument("--raw", help="where export and verify write the raw candidate — the "
                                      "wrapper's temporary file, never the tracked glb, which only "
                                      "a chain that passed every stage may replace")
    parser.add_argument("--census-json", action="store_true",
                        help="print this blend's source census as one tagged JSON line and exit; "
                             "how L1.SOURCE_CENSUS reads the previous committed blend through this "
                             "same code, in a Blender that has that blend open")
    return parser.parse_args(argv)


def main() -> int:
    arguments = _parse()
    if arguments.census_json:
        print("{}{}".format(CENSUS_SENTINEL, json.dumps(census(Source.live()), sort_keys=True)),
              flush=True)
        return 0
    findings = run(arguments.mode, arguments.canon, arguments.raw)
    print(report.render_text(findings), end="", flush=True)
    print("{} ▸ {}".format(arguments.mode.ljust(5), report.summary(findings)), flush=True)
    return report.exit_code(findings)


if __name__ == "__main__":
    sys.exit(main())
