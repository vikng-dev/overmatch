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

import os
import shutil
import subprocess
import sys
import tempfile

import bpy

GLB_RELPATH = "assets/tiger_1/tiger_1.glb"
BAKE_RELPATH = "scripts/encode-tank-ktx2.sh"
GATE_RELPATH = "scripts/tank/glb_ktx2.py"

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
    return glb


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
    try:
        result = bpy.ops.export_scene.gltf(filepath=raw, export_format="GLB")
        if "FINISHED" not in result:
            raise ExportError("gltf-export", f"export_tiger: export_scene.gltf returned {result}")
        print(f"export ▸ {raw} — {os.path.getsize(raw) / 1e6:.1f} MB (mipless, temporary)")
        bake(root=root, raw=raw, glb=glb)
    finally:
        shutil.rmtree(work, ignore_errors=True)

    print(f"EXPORTED {glb}")
    return glb
