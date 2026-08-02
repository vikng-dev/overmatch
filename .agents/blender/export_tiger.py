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
required. Measured on this model: 70.2 MB -> 62.3 MB on disk, and 32 bpp -> 8 bpp in VRAM.

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

EXPORT SETTINGS are `export_format='GLB'` and `export_tangents=True`, and the second one is a
deliberate, ratified change rather than a default anyone should add to.

TANGENTS ARE BAKED because the loader otherwise invents them. `bevy_gltf` generates tangents only
when the attribute is ABSENT (`loader/mod.rs:838`), so a glb without them ships a mesh whose
shading basis is computed at load by code no export gate ever saw. That is not hypothetical here:
the generated track-shoe levels each carried one vertex that mikktspace gives up on, while every
export-side check reported clean, because the checks were measuring the UVs and the loader was
measuring something else. The LOD levels bake theirs; this makes the tank glb — which carries L0,
the surface the whole error ladder is anchored to — do the same, so what is certified is what
renders. Yan ratified the cost (the file grows by roughly a third; it is LFS).

Everything else stays plain. Adding any OTHER argument changes the asset, so don't, without
re-running the structural comparison the bake's differ performs.

THIS DOOR NO LONGER REDUCES ANYTHING — L0 IS THE SOURCE
-------------------------------------------------------
It used to. The tank glb's `Link` was a 10° planar dissolve of the authored shoe and two further
tiers were cut beside it from a table in this file. All of that is gone (ADR 0033): the shipped
`Link` is now the artist's mesh, unmodified, because "the surface that ships is the surface that
anchors deviation" cannot be true of a mesh the exporter invented on the way out. Re-encoding an
over-tessellated source is AUTHORING — a human, in the .blend, once — and the result becomes the
source. That is what happened to this shoe: 5 550 triangles of authored tessellation became 1 661,
in the .blend, by hand.

The chain below L0 is cut by `scripts/lod/generate.py`, which reads the same .blend, searches
integer triangle targets against a global octave grid of deviation targets, and certifies every
level on the bytes it wrote. It is a separate, explicitly-run stage rather than a step inside this
export, because certifying a chain takes minutes of branch-and-bound and a GUI export must not
freeze for it. The seam between the two is `assets/lod_manifest.json`; this door only checks, and
says so loudly, whether the shoe it just exported still matches the one the chain was cut from.

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
now point at the hull's mesh datablocks (data-verified before the merge, same standard as the
retired glb-side dedupe) and every material carries `use_backface_culling`, which is the field the glTF
exporter derives `doubleSided` from. A plain export now produces 64 meshes, 11 materials and
nothing double-sided because that is what the .blend HOLDS.

There is deliberately no counter-check here. A tank that ships duplicated meshes or a
double-sided material is a MODEL-QUALITY problem, caught at review by a human — not by a
Tiger-shaped assertion wired into a general export door. What stays are the generic gates: the mip
gate on the baked bytes, and the `link ▸` / `lod   ▸` lines below.

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

import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile

import bpy

GLB_RELPATH = "assets/tiger_1/tiger_1.glb"
BAKE_RELPATH = "scripts/encode-tank-ktx2.sh"
GATE_RELPATH = "scripts/tank/glb_ktx2.py"

#: The track shoe, by OBJECT name. Its mesh carries the same name, which is why the game resolves
#: the two structurally rather than by string (`src/track/link_view.rs`). Read here only to report
#: what the exported bytes hold and to check the LOD chain has not gone stale under them.
LINK_OBJECT = "Link"

#: The seam to the LOD stage: the manifest `scripts/lod/generate.py` writes and
#: `scripts/lod/chain.py` derives the runtime chain from. Read-only from this door.
LOD_MANIFEST_RELPATH = "assets/lod_manifest.json"

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
    LAST_EXPORT["tangents"] = _check_tangents(glb)
    print(f"tan   \u25b8 {LAST_EXPORT['tangents']}")
    LAST_EXPORT["lod"] = _lod_chain_notice(root, glb)
    if LAST_EXPORT["lod"]:
        print(f"lod   ▸ {LAST_EXPORT['lod']}")
    return glb


# ── what the exported bytes hold ─────────────────────────────────────────────────────────────────

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


def _link_summary(glb):
    """`Link 1661 tris` read off the SHIPPED bytes, and whether that is the artist's mesh.

    INFORMATIONAL, and it reports on the ONE property this door is now responsible for: L0 is the
    source (ADR 0033 §1), so the shoe in these bytes must have exactly the triangle count the
    .blend holds. Read from the glb rather than remembered, because this also runs on the
    stock-exporter door, where nothing of ours touched the mesh at all.
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
    if authored is not None and written != authored:
        return (
            f"Link {written} tris — but the .blend holds {authored}. L0 IS THE SOURCE: something "
            f"between the artist's mesh and these bytes changed the shoe, and every deviation in "
            f"assets/lod_manifest.json is measured against a surface that does not ship."
        )
    return f"Link {written} tris (the source, unmodified)"


def _check_tangents(glb):
    """Every primitive the loader would generate tangents for must already carry them.

    THE RULE IS THE LOADER'S, not a blanket. `bevy_gltf` generates tangents when the attribute is
    absent AND the material needs them, which means a normal map (`needs_tangents`). So a primitive
    with a normal-mapped material and no TANGENT ships a shading basis computed at load by code no
    export gate ever saw — the class this bake exists to close. A primitive whose material has no
    normal map needs none and gets none: on this model that is the ballistic and collider volumes,
    which wear `Mat_Armor` and `Mat_Collider` and are not rendered surfaces at all.

    Checked on the BAKED BYTES, so it covers the stock-exporter door too.
    """
    gltf = _glb_json(glb)
    materials = gltf.get("materials", [])
    missing = []
    for mesh in gltf.get("meshes", []):
        for primitive in mesh["primitives"]:
            index = primitive.get("material")
            if index is None or "normalTexture" not in materials[index]:
                continue
            if "TANGENT" not in primitive["attributes"]:
                missing.append(f"{mesh.get('name')} ({materials[index].get('name')})")
    if missing:
        raise ExportError(
            "tangents",
            f"{len(missing)} normal-mapped primitive(s) ship without tangents, so bevy would "
            f"generate them at load and nothing here certified what renders: "
            f"{', '.join(missing[:6])}",
        )
    total = sum(len(mesh["primitives"]) for mesh in gltf.get("meshes", []))
    return f"{total} primitives, every normal-mapped one carries baked tangents"


def _lod_chain_notice(root, glb):
    """Say — loudly — when the shipped shoe no longer matches the chain that was cut from it.

    THE SEAM IS THE MANIFEST, AND THIS IS THE END OF IT THIS DOOR OWNS. `scripts/lod/generate.py`
    records the triangle count of the source it cut each chain from. If a re-export ships a
    different shoe, every level beside it is a reduction of a mesh that no longer exists and every
    switch distance in the manifest is measured against the wrong surface.

    A NOTICE, not an `ExportError`: the tank glb is legitimately re-exported for reasons that have
    nothing to do with the track (a texture, a hatch, a material), and wedging that behind a
    minutes-long chain regeneration would make people export around this door. The hard refusal
    lives where it costs nothing — `scripts/lod/chain.py --verify`, which `scripts/hooks/pre-push`
    runs on the committed bytes.
    """
    manifest_path = os.path.join(root, LOD_MANIFEST_RELPATH)
    if not os.path.isfile(manifest_path):
        return ""
    try:
        with open(manifest_path, encoding="utf-8") as handle:
            manifest = json.load(handle)
        gltf = _glb_json(glb)
    except (ExportError, OSError, ValueError):
        return ""
    relpath = os.path.relpath(glb, root)
    stale = []
    for asset in manifest.get("assets", []):
        level0 = (asset.get("levels") or [{}])[0]
        if level0.get("glb") != relpath:
            continue
        nodes = [n for n in gltf.get("nodes", []) if n.get("name") == level0.get("node")]
        if not nodes or nodes[0].get("mesh") is None:
            continue
        written = _mesh_triangles(gltf, gltf["meshes"][nodes[0]["mesh"]])
        if written != level0.get("tris"):
            stale.append(
                f"{asset['name']}: chain was cut from a {level0.get('tris')}-triangle "
                f"{level0.get('node')}, these bytes ship {written}"
            )
    if not stale:
        return ""
    return (
        "LOD CHAIN IS STALE — " + "; ".join(stale) + ". Re-run "
        "`blender -b --factory-startup --python scripts/lod/generate.py` and commit the new "
        "levels plus assets/lod_manifest.json."
    )


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

    work = tempfile.mkdtemp(prefix="tiger-export-")
    raw = os.path.join(work, "tiger_1.raw.glb")
    # The GUI door runs this on somebody's live scene, so what it touches it puts back.
    selected = [ob for ob in bpy.context.view_layer.objects if ob.select_get()]
    active = bpy.context.view_layer.objects.active
    try:
        # Straight out of the exporter, with the arguments the module doc froze. Nothing swaps a
        # mesh, nothing stacks a modifier: what the .blend holds is what ships.
        result = bpy.ops.export_scene.gltf(
            filepath=raw, export_format="GLB", export_tangents=True
        )
        if "FINISHED" not in result:
            raise ExportError("gltf-export", f"export_tiger: export_scene.gltf returned {result}")
        print(f"export ▸ {raw} — {os.path.getsize(raw) / 1e6:.1f} MB (mipless, temporary)")
        bake(root=root, raw=raw, glb=glb)
    finally:
        for ob in bpy.context.view_layer.objects:
            ob.select_set(ob in selected)
        if active is not None and active.name in bpy.context.view_layer.objects:
            bpy.context.view_layer.objects.active = active
        shutil.rmtree(work, ignore_errors=True)

    print(f"EXPORTED {glb}")
    return glb
