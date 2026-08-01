"""export_tiger.py — the SCRIPT door the Tiger glb leaves Blender through.

Every authoring script under `.agents/blender/` ends the same way: save the blend, then call
`export_tiger.export()` instead of `bpy.ops.export_scene.gltf(...)` directly.

    import sys, os, bpy
    sys.path.insert(0, os.path.join(REPO, ".agents/blender"))
    import export_tiger
    export_tiger.export()

THE GUI DOOR IS `.agents/blender/addons/overmatch_export.py`
-----------------------------------------------------------
Hands-on work happens in the Blender GUI, where nobody is going to run a script to export. That
add-on (install once: Preferences ▸ Add-ons ▸ Install…) adds **File ▸ Export ▸ Overmatch Tank
(.glb)** and also hooks the stock **File ▸ Export ▸ glTF 2.0** exporter, so a plain hand-export
that targets a tracked vehicle glb gets baked too. Same bake underneath: both doors end in the
`bake()` below, which is the only caller of `scripts/encode-tank-ktx2.sh` and the mip gate. Two
doors, one implementation — change the pipeline here and the GUI follows.

WHY A HELPER AND NOT A RAW `export_scene.gltf` CALL
---------------------------------------------------
Blender's glTF exporter embeds textures as PNG/JPEG, and bevy's PNG/JPEG loaders produce a
texture with exactly ONE mip level (`bevy_image-0.19.0/src/image.rs:1136`). On the Tiger that is
three 4k maps, three 2k maps and three 512s, every one of them minified hard at combat range —
shimmer on every rivet plus a texture-cache miss per fetch. The fix is
`scripts/encode-tank-ktx2.sh`, which re-encodes those images to mipped UASTC KTX2 *inside* the
glb. Bevy loads that through the ordinary `images[i].mimeType = "image/ktx2"` path
(`bevy_gltf-0.19.0/src/loader/mod.rs:1201` -> `bevy_image` `image.rs:439`), no extension support
required. Measured on this model: 71.1 MB -> 63.2 MB on disk, and 32 bpp -> 8 bpp in VRAM.

That bake is not optional and it is not a thing to remember. Folding it into the export step is
the whole point of this file: the bake becomes part of "export", so there is no separate action
to skip. The GUI add-on extends that to hand-exports out of the Blender UI, including the stock
glTF exporter. `scripts/hooks/pre-push` runs the same gate on the committed bytes as the backstop
for a glb that reached the tracked path some other way entirely.

WHY THE EXPORT GOES TO A TEMP FILE FIRST
----------------------------------------
`bpy.ops.export_scene.gltf` writes a MIPLESS glb. If it wrote straight to the tracked path, then
every failure mode of the bake — basisu not installed, an unhandled texture slot, a full disk —
would leave a mipless glb sitting exactly where the game and the release archive pick it up, and
the failure would be silent from then on. So the export lands in a temp file, the bake reads it,
and the tracked path is only ever written by the bake's own successful output. A failed bake
leaves the previous good glb untouched and raises.

EXPORT SETTINGS are plain defaults — `export_format='GLB'` and nothing else. Verified last
session against the shipped pipeline by dry-run export from an unmodified blend: zero structural
difference, identical size, matching generator string. Adding an argument here changes the asset,
so don't, without re-running that comparison.

THE TRACK-SHOE LOD STAGE — THE .BLEND IS THE SOURCE
---------------------------------------------------
194 shoes per Tiger make the track link the model's whole geometry bill, so it ships REDUCED: the
tank glb's own `Link` is the 10° planar dissolve of the authored mesh, and two further tiers ride
beside it as their own glbs for `src/track/link_view.rs` to swap in by distance. `LINK_LOD_TIERS`
below is the whole table.

The reduction runs HERE, on the authored mesh, because that is the only place the authored
topology exists. The retired route decimated the EXPORTED glb (`scripts/tank/diet/`), which meant
re-importing a mesh the exporter had split into 10 530 corner vertices and welding it back at
1e-5 — a guess at the connectivity the .blend had all along. Measured, that guess is very nearly
right (the two routes agree to 0.001 mm over 90 % of the surface and differ by more than 1 mm at
exactly three vertices), so it was not WRONG — it was unnecessary, unverifiable from the artist's
side, and it left the shipped shoe with no reproducible source but a shell script. Reducing from
the .blend deletes that whole round trip: the authored mesh goes in, the tiers come out, and the
recipe is a table in this file.

Every tier is `Decimate(DISSOLVE, angle) + Triangulate` on a DUPLICATE. The planar dissolve never
moves a vertex — it merges faces whose normals agree to within the angle and the exporter
re-triangulates the ngons — so every surviving position is an authored position. The distance
tiers add a quadric COLLAPSE pass on top of that, which does move vertices; that is legitimate at
250 m and not at 5 m, which is exactly the tier split.

`delimit={'UV'}` and not the GUI's empty default. Measured with `scripts/tank/diet/uvcheck.py`
against the authored mesh: with no delimiter the dissolve welds faces across a UV seam and the
longest UV edge on the result doubles the authored worst (2.79 against 1.36 uv), which is albedo
dragged over an island join; with `UV` it lands at 1.38 and every kept vertex still carries its
authored UV. The cost is 44 triangles (3 056 against 3 012).

LOD0 REPLACES `Link` BY OBJECT-DATA SWAP, and the mechanism matters. The main export's arguments
stay exactly as the paragraph above froze them, and `export_apply` is not among them — so a
modifier stack left on `Link` would be exported UNAPPLIED and silently ship the authored mesh.
Assigning `link.data = <reduced mesh>` for the duration of the export sidesteps that: the node
name, the parent, the children (`Link_Box`, `Pin_Start`, `Pin_End` — the datums the game measures
the track from), the transform and the `Mat_Track_Link` slot all belong to the OBJECT and are
untouched, and only the mesh the exporter reads changes. The original is restored in a `finally`
and the .blend is never saved, so the authored mesh survives every failure path.

The tier glbs are written into the temp work directory and moved onto their tracked paths only
after the tank glb's bake and mip gate have both passed — same rule as the tank glb itself, so a
failed export leaves the whole tracked set at its last good state rather than half-updated.

WHAT A RE-EXPORT USED TO THROW AWAY — AND WHY THIS FILE NO LONGER CARES
-----------------------------------------------------------------------
Two properties of the shipped glb used to be glb surgery: the MG dedupe (the coax and hull MG34s
are one model, so the coax nodes shared the hull's meshes and the orphans were collected) and
back-face culling (`doubleSided` off, measured safe at 2 000 px over 32 camera positions). The
.blend had neither, so a plain re-export reverted both — 67 meshes and 15 materials against 64
and 11, and every material double-sided again — and this file grew a `_surgery` stage that
replayed them onto every raw export.

That stage is GONE, because replaying a fix onto every export is a workaround for a bad source,
not a pipeline. `.agents/blender/repair_source.py` fixed the .blend once: the three coax objects
now point at the hull's mesh datablocks (data-verified before the merge, same standard as
`dedupe.py`) and every material carries `use_backface_culling`, which is the field the glTF
exporter derives `doubleSided` from. A plain export now produces 64 meshes, 11 materials and
nothing double-sided because that is what the .blend HOLDS.

There is deliberately no counter-check here. A tank that ships duplicated meshes or a
double-sided material is a MODEL-QUALITY problem, caught at review by a human or an agent reading
`scripts/tank/diet/README.md`'s model-quality rules — not by a Tiger-shaped assertion wired into
a general export door. What stays are the generic gates: the mip gate on the baked bytes, the
tier-glb shape checks the game's loader depends on, and the `link ▸` line below.

LOD0 IS AN ASSET DECISION, NOT A BUDGET. 10° is there because Yan validated it in the GUI. The
angle is not free to be tuned by whoever next wants triangles: it is a property of how the shoe
was tessellated (a dihedral histogram says the 0.5–5° band is fine cylinders, and 10° spends
them), and changing it needs his eyeball IN THE GAME. `scripts/tank/diet/README.md` carries the
measurements and that rule.

COST: the bake is ~60 s wall clock on the Tiger (measured, M-series, 9 images, UASTC level 2/3 +
zstd 9). It is bit-for-bit reproducible — the same input glb bakes to the same sha256 — so a
re-export that changed nothing produces no diff for git-lfs to store.

THAT MINUTE LOOKS EXACTLY LIKE A HANG, SO IT HAS TO ANNOUNCE ITSELF
-------------------------------------------------------------------
The bake runs synchronously on Blender's main thread. While `subprocess.run` blocks, Blender's
event loop does not turn: no redraw, no status bar, and `window.cursor_set('WAIT')` never even
reaches the screen (the cursor is set on a window that will not be serviced until the call
returns — macOS paints its own beachball over it). A user who has not been told stares at a frozen
app for a minute and force-quits it, which is exactly what happened. So:

  * `BAKE_NOTICE` is printed BEFORE the freeze starts, and the GUI operator also `self.report`s it.
  * the bake's stdout is streamed line by line instead of inherited, so the per-image `ktx2  ▸`
    lines land in the console as they happen — the freeze is visibly making progress.
  * each of those lines ticks `window_manager.progress_update`, which on a platform whose
    compositor still honours the app cursor shows the percentage on the mouse pointer.

No modal operator, no threads: streaming a pipe is the whole mechanism. The console remains the
honest channel on macOS, which is why the notice says where to look.
"""

import contextlib
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
from collections import namedtuple

import bpy

GLB_RELPATH = "assets/tiger_1/tiger_1.glb"
BAKE_RELPATH = "scripts/encode-tank-ktx2.sh"
GATE_RELPATH = "scripts/tank/glb_ktx2.py"

#: The authored track shoe, by OBJECT name. Its mesh carries the same name, which is why the game
#: resolves the two structurally rather than by string (`src/track/link_view.rs`).
LINK_OBJECT = "Link"

#: What the planar dissolve is allowed to merge ACROSS. `UV` keeps it inside a UV island; without
#: it the dissolve welds islands together and drags the albedo over the join — measured, see the
#: module doc. `SHARP`/`NORMAL`/`MATERIAL` measured as no-ops on this mesh (it carries no sharp
#: edges, no seams and one material), so they are left out rather than carried as decoration.
LINK_DELIMIT = frozenset({"UV"})

#: One row per level of the shoe chain, NEAREST FIRST.
#:
#: `angle_deg` is the planar-dissolve limit, in degrees per this repo's RON convention (converted
#: once, at the modifier). `collapse_tris` is a quadric-collapse budget applied ON TOP of the
#: dissolve — `None` means planar only, which is the only thing allowed on the mesh a player walks
#: up to. `relpath` is where the level ships: `None` for LOD0, which is not a file but the `Link`
#: mesh INSIDE the tank glb.
#:
#: THE BUDGETS ARE SET BY DEVIATION, NOT BY ROUND NUMBERS. `src/track/link_view.rs` swaps levels at
#: distances derived from each level's measured worst point-to-surface deviation — one pixel in the
#: gunner optic is 8.333e-5 rad, so a level is honest beyond `worst_dev / 8.333e-5` metres and
#: nowhere nearer. Measured with `scripts/tank/diet/deviation.py` against the authored mesh:
#:
#:     LOD0  3 056 tris   0.99 mm ->  11.9 m   (planar only: no vertex moves)
#:     LOD1    477 tris  18.64 mm -> 223.7 m   (shipped switch: 250 m)
#:     LOD2    237 tris  44.72 mm -> 536.7 m   (shipped switch: 650 m)
#:
#: LOD1 asks for 500 rather than the ~380 a triangle-first reading would pick because the collapse
#: falls off a cliff there — 429 triangles measures 22.9 mm, which is only honest beyond 275 m and
#: would put faceting inside the shipped 250 m band. Triangles are the free variable; the switch
#: distance is the constraint.
#:
#: Planar alone floors at 1 354 triangles even at 60°, so the distance tiers cannot be reached
#: without the collapse pass; the collapse itself floors at ~213 triangles on this shoe, which is
#: why LOD2 asks for 250 and not the 192 the retired glb-surgery route reached from its welded
#: copy. A budget below the floor is a loud failure, not a silent near-miss.
LinkLod = namedtuple("LinkLod", "label angle_deg collapse_tris relpath node")
LINK_LOD_TIERS = (
    LinkLod("LOD0", 10.0, None, None, LINK_OBJECT),
    LinkLod("LOD1", 10.0, 500, "assets/tiger_1/tiger_1_link.lod1.glb", "Link_LOD1"),
    LinkLod("LOD2", 10.0, 250, "assets/tiger_1/tiger_1_link.lod2.glb", "Link_LOD2"),
)

#: What a shipped tier glb must be for `link_view.rs` to load it as
#: `GltfAssetLabel::Primitive { mesh: 0, primitive: 0 }`: one node, one mesh, one indexed primitive
#: carrying position, normal and UV, and NO material (the reduced levels wear the base shoe's
#: `Mat_Track_Link`, and their tangents are generated at bind). Asserted on the bytes before they
#: are published, so a Blender exporter default that changes under us fails the export instead of
#: the game.
LOD_GLB_ATTRIBUTES = ("POSITION", "NORMAL", "TEXCOORD_0")

#: What the last successful export/bake measured — `raw_bytes`, `out_bytes`, `verify` (the gate's
#: one-line summary). The GUI door reports these back to the user; the scripted door prints them.
LAST_EXPORT = {}

#: Said before the UI goes dark. The console line is the load-bearing half on macOS: a GUI Blender
#: launched from Finder writes stdout nowhere the user can see, so the notice names the fix.
BAKE_NOTICE = (
    "KTX2 mip bake starting — this takes ~60-90 s and BLENDER'S WINDOW WILL FREEZE for all of it "
    "(no redraw, spinning cursor). That is normal. DO NOT force-quit: quitting mid-bake leaves the "
    "previous good glb in place but wastes the export. Per-image progress prints to the system "
    "console (Window ▸ Toggle System Console on Windows; on macOS relaunch Blender from a terminal "
    "to see it)."
)

#: Emitted once per encoded image by `scripts/encode-tank-ktx2.sh`, and once up front with the
#: total. The bake's progress meter is nothing more than counting these off the pipe.
_IMAGE_LINE = "ktx2  ▸"
_TOTAL_LINE = "images ▸"


class ExportError(SystemExit):
    """A named failure stage, so a caller can say WHICH step failed without parsing prose.

    Derives from SystemExit because a headless `blender --background --python export.py` reports a
    script failure through its EXIT CODE and nothing else: an unhandled `Exception` there prints a
    traceback and still exits 0 (measured), while an unhandled `SystemExit` exits non-zero. CI and
    every agent-driven export depend on that, so the internals keep raising this.

    The cost is that SystemExit kills Blender's embedded interpreter outright when a script is run
    from the Text Editor — the app vanishes with no dialog, indistinguishable from a crash. So the
    doors do not let it escape into a GUI: `export()` converts it to `ExportFailed` there. See
    `_surface`.
    """

    def __init__(self, stage, message):
        super().__init__(message)
        self.stage = stage


class ExportFailed(Exception):
    """The same failure, in the form a GUI can survive: an ordinary exception.

    Carries `.stage` so `overmatch_export.py`'s `_stage_of` keeps naming the step that failed.
    """

    def __init__(self, stage, message):
        super().__init__(message)
        self.stage = stage


def _surface(exc):
    """Print a door failure loudly and re-raise it in the form this environment can survive.

    Headless keeps the SystemExit (the exit code IS the report — see `ExportError`). Anything with
    a UI gets `ExportFailed`, because a SystemExit escaping into Blender's Text Editor terminates
    the embedded CPython and takes the whole app down with no message: the one failure mode that
    looks like a crash. Either way the message reaches stdout first, so it is never lost.
    """
    stage = getattr(exc, "stage", None) or type(exc).__name__
    print(f"\nEXPORT FAILED [{stage}]\n{exc}", file=sys.stderr)
    sys.stderr.flush()
    if bpy.app.background:
        raise exc
    raise ExportFailed(stage, str(exc)) from exc


class _Progress:
    """The window-manager progress cursor, or a no-op wherever there is no UI to drive it.

    `progress_begin`/`progress_update` are the only feedback Blender can give from inside a
    blocking call — they set the cursor directly rather than queueing a redraw. Every call is
    guarded because this module runs headless as often as it runs in the GUI.
    """

    def __init__(self):
        self.wm = None
        if bpy.app.background:
            return
        try:
            wm = bpy.context.window_manager
            wm.progress_begin(0.0, 100.0)
        except (AttributeError, RuntimeError):
            return
        self.wm = wm

    def update(self, percent):
        if self.wm is None:
            return
        try:
            self.wm.progress_update(percent)
        except (AttributeError, RuntimeError):
            self.wm = None

    def end(self):
        if self.wm is None:
            return
        try:
            self.wm.progress_end()
        except (AttributeError, RuntimeError):
            pass
        self.wm = None


def repo_root():
    """The git work-tree root, walked up from the open .blend (which lives under assets/)."""
    directory = os.path.dirname(bpy.data.filepath)
    if not directory:
        raise ExportError("repo-root", "export_tiger: no .blend is open (bpy.data.filepath is empty)")
    while directory != os.path.dirname(directory):
        if os.path.exists(os.path.join(directory, ".git")):
            return directory
        directory = os.path.dirname(directory)
    raise ExportError("repo-root", f"export_tiger: {bpy.data.filepath} is not inside a git work tree")


def preflight(root):
    """Everything the bake needs, checked BEFORE the minute-long export. Returns the bake script."""
    script = os.path.join(root, BAKE_RELPATH)
    if not os.path.isfile(script):
        raise ExportError("preflight", f"export_tiger: missing {script}")
    if not shutil.which("basisu"):
        raise ExportError(
            "preflight",
            "export_tiger: `basisu` is not on PATH — the mip bake cannot run.\n"
            "  Install it (brew install basis_universal) and re-run. Refusing to export a "
            "mipless glb over the tracked one.",
        )
    return script


def _run_bake(script, raw, glb, root):
    """Run the bake with its stdout on a pipe, echoing and counting it. Returns the exit code.

    The pipe is the entire progress mechanism. `subprocess.run` with an inherited stdout gives the
    user a minute of nothing (the bake's own lines sit in Blender's console buffer behind a main
    thread that never returns to the event loop); reading it line by line and printing with an
    explicit flush puts each `ktx2  ▸` line on screen the moment the encoder finishes an image, and
    ticks the cursor percentage with it.

    stderr is folded into stdout so a basisu failure keeps its position in the sequence instead of
    surfacing after everything else. PYTHONUNBUFFERED is set because the bake's own python phases
    (unpack/repack/diff) would otherwise block-buffer into the pipe and arrive in one lump.
    """
    env = dict(os.environ, PYTHONUNBUFFERED="1")
    progress = _Progress()
    total, done = 0, 0
    try:
        proc = subprocess.Popen(
            [script, raw, glb], cwd=root, env=env, text=True, bufsize=1,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        for line in proc.stdout:
            print(line, end="", flush=True)
            if line.startswith(_TOTAL_LINE):
                # "images ▸ 9 to encode"
                field = line[len(_TOTAL_LINE):].strip().split(" ", 1)[0]
                total = int(field) if field.isdigit() else 0
            elif line.startswith(_IMAGE_LINE):
                done += 1
                # Unknown total (an older bake script) still moves, it just cannot promise 100%.
                progress.update(100.0 * done / total if total else min(90.0, 10.0 * done))
        proc.stdout.close()
        return proc.wait()
    finally:
        progress.end()


def bake(root, raw, glb):
    """Mip-bake the mipless glb at `raw` onto `glb`, then gate the result. Returns `glb`.

    Split out of `export()` so the GUI add-on's stock-exporter callback — which is handed a glb
    that Blender has ALREADY written — can reuse the exact same bake and gate instead of carrying
    a second copy of the invocation. `raw` and `glb` must differ: the tracked path is only ever
    written by a bake that succeeded.
    """
    script = preflight(root)

    LAST_EXPORT.clear()
    LAST_EXPORT["raw_bytes"] = os.path.getsize(raw)

    # The bake unpacks, encodes one KTX2 per image with role-derived colour-space flags, repacks,
    # and finishes with the structural differ (accessor hashes on both sides). A non-zero exit
    # here means the tracked glb was NOT replaced.
    print(f"\nbake  ▸ {BAKE_NOTICE}\n", flush=True)
    returncode = _run_bake(script, raw, glb, root)
    if returncode != 0:
        raise ExportError(
            "bake",
            f"export_tiger: mip bake failed (exit {returncode}).\n"
            f"  {glb} is UNCHANGED — the previous good glb is still in place.",
        )

    # Same gate `scripts/hooks/pre-push` runs on the committed bytes. Milliseconds, and it makes
    # the export self-certifying instead of trusting the step above. `python3` may be missing from
    # a GUI Blender's stripped PATH, so fall back to the interpreter we are already running in
    # (the gate is stdlib-only).
    python = shutil.which("python3") or sys.executable
    gate = subprocess.run(
        [python, GATE_RELPATH, "verify", glb], cwd=root, capture_output=True, text=True,
    )
    print(gate.stdout, end="")
    if gate.returncode != 0:
        print(gate.stderr, end="")
        raise ExportError(
            "verify",
            f"export_tiger: {os.path.basename(glb)} failed the mip gate — "
            f"{(gate.stdout + gate.stderr).strip().splitlines()[0] if (gate.stdout or gate.stderr) else 'no output'}",
        )

    LAST_EXPORT["out_bytes"] = os.path.getsize(glb)
    LAST_EXPORT["verify"] = gate.stdout.replace("mip   ▸", "").strip().splitlines()[0] \
        if gate.stdout.strip() else ""
    LAST_EXPORT["link"] = _link_summary(glb)
    if LAST_EXPORT["link"]:
        print(f"link  ▸ {LAST_EXPORT['link']}")
    return glb


# ── the track-shoe LOD stage ─────────────────────────────────────────────────────────────────────

def _triangles(ob):
    """Triangles the exporter would write for `ob` WITH its modifier stack evaluated.

    Counted off the evaluated mesh rather than predicted from a ratio, because that is the number
    that ships: a `Decimate` ratio is a lever on an internal edge count, not on triangles.
    """
    depsgraph = bpy.context.evaluated_depsgraph_get()
    evaluated = ob.evaluated_get(depsgraph)
    mesh = evaluated.to_mesh()
    count = sum(len(polygon.vertices) - 2 for polygon in mesh.polygons)
    evaluated.to_mesh_clear()
    return count


def _fit_collapse(ob, budget):
    """Add a quadric-collapse modifier and bisect its ratio to `budget` triangles. Returns tris.

    Bisection and not arithmetic: the map from ratio to triangle count is neither linear nor
    continuous (collapses that would make the mesh non-manifold are refused), so the only honest
    way to hit a budget is to ask the modifier. 24 halvings resolve the ratio to 6e-8, and the
    search stops early once it is inside 6 % of the budget — the same shape as the retired
    `scripts/tank/diet/decimate_planar.py`, kept so the two produce comparable tiers.

    Refuses loudly below the mesh's collapse floor. A silent near-miss there is worse than a
    failure: it would ship whatever the modifier happened to floor at, under a budget that reads
    as if it were met.
    """
    collapse = ob.modifiers.new("Collapse", "DECIMATE")
    collapse.decimate_type = "COLLAPSE"
    collapse.use_collapse_triangulate = True

    low, high, best = 0.0, 1.0, None
    for _ in range(24):
        middle = (low + high) / 2
        collapse.ratio = middle
        bpy.context.view_layer.update()
        count = _triangles(ob)
        if count <= budget:
            best = (middle, count)
            low = middle
        else:
            high = middle
        if best and budget * 0.94 <= best[1] <= budget:
            break

    if best is None:
        collapse.ratio = 0.0
        bpy.context.view_layer.update()
        raise ExportError(
            "link-lod",
            f"export_tiger: cannot reach {budget} triangles — this shoe's collapse floor is "
            f"{_triangles(ob)}. Raise the budget in LINK_LOD_TIERS (and re-measure the tier's "
            f"deviation, because the switch distance in src/track/link_view.rs is derived from it).",
        )

    collapse.ratio = best[0]
    bpy.context.view_layer.update()
    return best[1]


def _reduced_link_mesh(link, tier):
    """The mesh for `tier`, reduced from the AUTHORED `link` object without touching it.

    Built on a duplicate that lives for the length of this call: modifiers are stacked on the
    copy, `new_from_object` bakes the evaluated result into a standalone mesh datablock, and the
    copy is deleted. The caller owns the returned mesh and must remove it.
    """
    scene = bpy.context.scene
    duplicate = link.copy()
    duplicate.data = link.data.copy()
    duplicate.parent = None
    duplicate.location = (0.0, 0.0, 0.0)
    scene.collection.objects.link(duplicate)
    try:
        planar = duplicate.modifiers.new("Planar", "DECIMATE")
        planar.decimate_type = "DISSOLVE"
        planar.angle_limit = math.radians(tier.angle_deg)
        planar.delimit = set(LINK_DELIMIT)
        # Before the collapse, not after: the quadric metric wants a triangle field rather than the
        # ngons the dissolve leaves. Doing it here also means the count reported below is the count
        # that ships, instead of trusting the exporter's own triangulation to agree.
        duplicate.modifiers.new("Triangulate", "TRIANGULATE")
        bpy.context.view_layer.update()
        dissolved = _triangles(duplicate)
        if tier.collapse_tris:
            _fit_collapse(duplicate, tier.collapse_tris)

        depsgraph = bpy.context.evaluated_depsgraph_get()
        mesh = bpy.data.meshes.new_from_object(
            duplicate.evaluated_get(depsgraph), depsgraph=depsgraph
        )
        mesh.name = tier.node
        triangles = sum(len(polygon.vertices) - 2 for polygon in mesh.polygons)
        print(
            f"link  ▸ {tier.label}: planar {tier.angle_deg:g}° delimit="
            f"{','.join(sorted(LINK_DELIMIT)) or '-'} -> {dissolved} tris"
            + (f", collapse -> {triangles} tris" if tier.collapse_tris else "")
            + f" ({len(mesh.vertices)} verts)"
        )
        return mesh
    finally:
        data = duplicate.data
        bpy.data.objects.remove(duplicate, do_unlink=True)
        bpy.data.meshes.remove(data)


def _write_lod_glb(mesh, tier, path):
    """Export `mesh` ALONE to `path` as the tier glb the game loads.

    A temporary object is the only way to hand the glTF exporter a mesh — it exports objects, not
    datablocks — so one is made, selected, exported and deleted. `use_selection` is what keeps the
    other 60-odd objects of the tank out of a 20 KB file; the material slots are cleared AND
    `export_materials='NONE'` is passed, so nothing of `Mat_Track_Link` reaches these bytes from
    either direction.
    """
    scene = bpy.context.scene
    mesh.materials.clear()
    ob = bpy.data.objects.new(tier.node, mesh)
    if ob.name != tier.node:
        bpy.data.objects.remove(ob, do_unlink=True)
        raise ExportError(
            "link-lod",
            f"export_tiger: the blend already holds an object called `{tier.node}` — Blender "
            f"renamed the export copy to `{ob.name}`, which would change the node name in "
            f"{os.path.basename(path)}. Rename the existing object.",
        )
    scene.collection.objects.link(ob)
    try:
        for other in bpy.context.view_layer.objects:
            other.select_set(False)
        ob.select_set(True)
        bpy.context.view_layer.objects.active = ob
        result = bpy.ops.export_scene.gltf(
            filepath=path,
            export_format="GLB",
            use_selection=True,
            export_materials="NONE",
            export_normals=True,
            export_texcoords=True,
            export_tangents=False,
        )
        if "FINISHED" not in result:
            raise ExportError("link-lod", f"export_tiger: {tier.label} export returned {result}")
    finally:
        bpy.data.objects.remove(ob, do_unlink=True)
    return path


def _glb_json(path):
    """The JSON chunk of a glb, as a dict. Stdlib only — the same reading `glb_ktx2.py` does."""
    with open(path, "rb") as handle:
        magic, _version, _length = struct.unpack("<4sII", handle.read(12))
        if magic != b"glTF":
            raise ExportError("link-lod", f"export_tiger: {path} is not a glb")
        chunk_length, chunk_type = struct.unpack("<II", handle.read(8))
        if chunk_type != 0x4E4F534A:  # 'JSON'
            raise ExportError("link-lod", f"export_tiger: {path} does not start with a JSON chunk")
        return json.loads(handle.read(chunk_length))


def _mesh_triangles(gltf, mesh):
    """Triangles of a glTF mesh — indexed primitives only, which is all this pipeline writes."""
    total = 0
    for primitive in mesh["primitives"]:
        if "indices" not in primitive:
            raise ExportError("link-lod", f"export_tiger: mesh `{mesh.get('name')}` is not indexed")
        total += gltf["accessors"][primitive["indices"]]["count"] // 3
    return total


def _check_lod_glb(path, tier, triangles):
    """Refuse to publish a tier glb the game's loader would not read. Returns a summary line."""
    gltf = _glb_json(path)
    meshes = gltf.get("meshes", [])
    if len(meshes) != 1 or len(gltf.get("nodes", [])) != 1:
        raise ExportError(
            "link-lod",
            f"export_tiger: {os.path.basename(path)} holds {len(gltf.get('nodes', []))} nodes and "
            f"{len(meshes)} meshes — the loader reads mesh 0, primitive 0 and expects exactly one "
            f"of each.",
        )
    primitives = meshes[0]["primitives"]
    if len(primitives) != 1:
        raise ExportError(
            "link-lod",
            f"export_tiger: {os.path.basename(path)} splits into {len(primitives)} primitives — "
            f"the loader reads primitive 0 only, so the rest would never be drawn.",
        )
    primitive = primitives[0]
    missing = [name for name in LOD_GLB_ATTRIBUTES if name not in primitive["attributes"]]
    if missing:
        raise ExportError(
            "link-lod",
            f"export_tiger: {os.path.basename(path)} is missing {', '.join(missing)} — the shoe "
            f"renders under a normal-mapped material and needs all of {LOD_GLB_ATTRIBUTES}.",
        )
    if primitive.get("material") is not None or gltf.get("materials"):
        raise ExportError(
            "link-lod",
            f"export_tiger: {os.path.basename(path)} carries a material. The reduced levels wear "
            f"the base shoe's own material at bind time; shipping one here is a second answer to "
            f"how the track looks.",
        )
    written = _mesh_triangles(gltf, meshes[0])
    if written != triangles:
        raise ExportError(
            "link-lod",
            f"export_tiger: {os.path.basename(path)} holds {written} triangles, but {tier.label} "
            f"was reduced to {triangles} — the exporter re-tessellated the mesh.",
        )
    return f"{tier.label} {written} tris"


@contextlib.contextmanager
def _link_reduced_to(link, mesh):
    """Hold `link`'s object data at `mesh` for the length of the block, then put it back.

    The rename is part of the swap and not decoration: the authored mesh datablock is called
    `Link`, so a second mesh asking for that name is handed `Link.001` by Blender and the tank glb
    would ship its shoe under a name nothing else in the repo uses (`scripts/tank/diet/extract.py`
    finds it by mesh name). Freeing the name for the duration keeps the exported bytes identical to
    what a hand-decimated `Link` would have produced.

    Nothing here survives the block: the reduced mesh is removed and the authored one gets its name
    and its object back, on every path, and the .blend is never saved either way.
    """
    authored = link.data
    name = authored.name
    authored.name = f"{name}.authored"
    mesh.name = name
    link.data = mesh
    try:
        yield mesh
    finally:
        link.data = authored
        bpy.data.meshes.remove(mesh)
        authored.name = name


def _link_summary(glb):
    """`Link 3056 tris` read off the SHIPPED bytes — the one line that says whether LOD0 is in.

    INFORMATIONAL. It reports, it never fails the export: a glb whose shoe is the authored mesh is
    a legitimate thing to have produced (the stock glTF exporter can do it), it is just not the
    thing this pipeline produces, and the reader deserves to be told which one they got.

    Read from the glb rather than remembered from the reduction, because this also runs on the
    stock-exporter door (`overmatch_export.py`'s callback bakes a glb Blender wrote on its own,
    with no LOD stage in front of it). Comparing against the authored polygon count is what turns
    that into a statement instead of a number.
    """
    try:
        gltf = _glb_json(glb)
    except (ExportError, OSError, ValueError):
        return ""
    nodes = [node for node in gltf.get("nodes", []) if node.get("name") == LINK_OBJECT]
    if not nodes or nodes[0].get("mesh") is None:
        return ""
    written = _mesh_triangles(gltf, gltf["meshes"][nodes[0]["mesh"]])
    link = bpy.data.objects.get(LINK_OBJECT)
    authored = (
        sum(len(polygon.vertices) - 2 for polygon in link.data.polygons)
        if link is not None and link.type == "MESH"
        else None
    )
    if authored is not None and written == authored:
        return (
            f"Link {written} tris — AUTHORED, so the LOD stage did NOT run. This glb came straight "
            f"from the stock glTF exporter: the shoe ships at full detail and the two tier glbs "
            f"beside it were not rebuilt. Re-export through File ▸ Export ▸ Overmatch Tank (or "
            f"export_tiger.export()) to get the reduced shoe and the LOD glbs."
        )
    return f"Link {written} tris"


def _link_lod_glbs(link, work):
    """Build every FILE tier into `work`. Returns `[(staged_path, tracked_relpath, summary)]`.

    Staged rather than published: these land on their tracked paths only after the tank glb's own
    bake and gate have passed, so one failed export cannot leave LOD1 newer than the shoe it is a
    reduction of.
    """
    staged = []
    for tier in LINK_LOD_TIERS:
        if tier.relpath is None:
            continue
        mesh = _reduced_link_mesh(link, tier)
        try:
            triangles = sum(len(polygon.vertices) - 2 for polygon in mesh.polygons)
            path = os.path.join(work, os.path.basename(tier.relpath))
            _write_lod_glb(mesh, tier, path)
        finally:
            bpy.data.meshes.remove(mesh)
        staged.append((path, tier.relpath, _check_lod_glb(path, tier, triangles)))
    return staged


def export(root=None, glb=None):
    """Export the open blend to `glb`, mip-baked. Returns the path written.

    Raises (loudly) rather than writing a mipless glb: see the module doc.

    THE BOUNDARY: every failure below is an `ExportError`, i.e. a `SystemExit`, and this is the
    point where that stops being safe. Run from Blender's Text Editor, an escaping SystemExit ends
    the embedded interpreter and the whole application closes with no traceback and no dialog — the
    single failure mode that is indistinguishable from a crash, and the reason this door catches
    `BaseException` rather than `Exception`. `_surface` prints the message and re-raises in
    whichever form the current environment survives.
    """
    try:
        return _export(root, glb)
    except KeyboardInterrupt:
        raise
    except BaseException as exc:
        _surface(exc)


def _export(root, glb):
    """`export()` minus the failure boundary — everything here may raise `ExportError` freely."""
    root = root or repo_root()
    glb = glb or os.path.join(root, GLB_RELPATH)
    preflight(root)  # fail before the minute of export, not after

    link = bpy.data.objects.get(LINK_OBJECT)
    if link is None or link.type != "MESH":
        raise ExportError(
            "link-lod",
            f"export_tiger: this blend has no mesh object called `{LINK_OBJECT}` — the shoe LOD "
            f"stage has nothing to reduce, and the game's track would ship at full detail.",
        )

    work = tempfile.mkdtemp(prefix="tiger-export-")
    raw = os.path.join(work, "tiger_1.raw.glb")
    # The GUI door runs this on somebody's live scene, so what it touches it puts back.
    selected = [ob for ob in bpy.context.view_layer.objects if ob.select_get()]
    active = bpy.context.view_layer.objects.active
    try:
        # Files first, then the mesh swap: every temporary object is gone before the tank export
        # starts, so nothing the LOD stage made can leak into the tank glb.
        staged = _link_lod_glbs(link, work)
        with _link_reduced_to(link, _reduced_link_mesh(link, LINK_LOD_TIERS[0])):
            result = bpy.ops.export_scene.gltf(filepath=raw, export_format="GLB")
            if "FINISHED" not in result:
                raise ExportError(
                    "gltf-export", f"export_tiger: export_scene.gltf returned {result}"
                )
        print(f"export ▸ {raw} — {os.path.getsize(raw) / 1e6:.1f} MB (mipless, temporary)")
        bake(root=root, raw=raw, glb=glb)

        # Only now: the tank glb passed its bake and its gate, so the tier glbs beside it are
        # publishable. `shutil.move` rather than `os.replace` because the work directory can sit on
        # another filesystem.
        for path, relpath, summary in staged:
            shutil.move(path, os.path.join(root, relpath))
            print(f"link  ▸ {relpath} — {summary}")
        LAST_EXPORT["lods"] = [summary for _path, _relpath, summary in staged]
    finally:
        for ob in bpy.context.view_layer.objects:
            ob.select_set(ob in selected)
        if active is not None and active.name in bpy.context.view_layer.objects:
            bpy.context.view_layer.objects.active = active
        shutil.rmtree(work, ignore_errors=True)

    print(f"EXPORTED {glb}")
    return glb
