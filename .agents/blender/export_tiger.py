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


class ExportError(SystemExit):
    """A named failure stage, so a caller can say WHICH step failed without parsing prose.

    Derives from SystemExit because that is what this module has always raised: a script door
    failure must stop the headless Blender run, not be swallowed by a bare `except Exception`.
    """

    def __init__(self, stage, message):
        super().__init__(message)
        self.stage = stage


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
    try:
        subprocess.run([script, raw, glb], cwd=root, check=True)
    except subprocess.CalledProcessError as exc:
        raise ExportError(
            "bake",
            f"export_tiger: mip bake failed (exit {exc.returncode}).\n"
            f"  {glb} is UNCHANGED — the previous good glb is still in place.",
        ) from exc

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
    """
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
