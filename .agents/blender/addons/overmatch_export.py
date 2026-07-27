"""overmatch_export.py — the GUI door the tank glb leaves Blender through.

Install this once (Preferences ▸ Add-ons ▸ Install… ▸ pick this file, then tick it) and the mip
bake stops being a thing to remember:

  * **File ▸ Export ▸ Overmatch Tank (.glb)** — a first-class exporter entry. Same file dialog as
    any other exporter, defaulted to the tracked glb for the open blend, and it runs the whole
    export ▸ bake ▸ verify chain.
  * **File ▸ Export ▸ glTF 2.0** — the stock exporter. If it is pointed at a tracked vehicle glb
    (`VEHICLE_GLBS` below), this add-on hooks the exporter's own post-export callback and bakes
    that write too. Exporting anywhere else is left completely alone.

Both doors end in `.agents/blender/export_tiger.py` — the SCRIPT door, called by the authoring
scripts under `.agents/blender/`. There is exactly one bake implementation
(`scripts/encode-tank-ktx2.sh`) and exactly one gate (`scripts/tank/glb_ktx2.py verify`); this file
adds no encoding decisions of its own, it only makes those reachable from the GUI.

WHY THE BAKE MATTERS: Blender embeds textures as PNG/JPEG and bevy's PNG/JPEG loaders produce ONE
mip level, so a raw glTF export ships a tank that shimmers on every rivet at combat range and
burns 32 bpp of VRAM instead of 8. `scripts/encode-tank-ktx2.sh` re-encodes the embedded images to
mipped UASTC KTX2 in place. Long version in `export_tiger.py`'s module doc.

WHICH HOOK, AND WHY NOT THE ONE THE DOCS NAME
---------------------------------------------
The glTF add-on offers two module-level entry points (Blender 5.1,
`scripts/addons_core/io_scene_gltf2/__init__.py:1329-1347`): it walks
`bpy.context.preferences.addons.keys()`, looks each name up in `sys.modules`, and collects

    module.glTF2_pre_export_callback / module.glTF2_post_export_callback   -> export_settings
    module.glTF2ExportUserExtension  (a class, instantiated per export)    -> gltf_user_extensions

`glTF2_post_export_callback` is a trap for this job. It is invoked at
`blender/exp/export.py:39-41`, which is BEFORE `__write_file(...)` on line 42 — "post export" there
means post-gather, and the glb does not exist on disk yet. A bake hung off it would encode the
PREVIOUS file. The user-extension hook is the one that fires after the bytes land: `save()` returns
at `__init__.py:1371` and `export_user_extensions('post_export_hook', ...)` runs at line 1380. So
this add-on registers `glTF2ExportUserExtension` and implements `post_export_hook` (plus
`pre_export_hook`, `__init__.py:1353`, to stash the previous good glb).

TEMP-FIRST ORDERING, PRESERVED
------------------------------
The scripted door exports to a temp file and lets the bake write the tracked path, so a failed bake
leaves the last good glb untouched. The stock exporter cannot be told to do that — it writes where
the user pointed it. So the callback reproduces the guarantee from both ends: `pre_export_hook`
copies the existing tracked glb aside, `post_export_hook` moves the freshly written mipless file to
a temp dir and bakes it back onto the tracked path, and any failure restores the stash. Net effect
is identical to the scripted door: the tracked path only ever holds a mip-baked glb.

macOS GUI PATH
--------------
Blender launched from Finder/Dock inherits launchd's PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), not
your shell's — so homebrew's `basisu` is invisible to it and the bake would fail on a machine where
the terminal runs it fine. `_resolve_basisu()` looks on PATH, then at the homebrew prefixes, then
asks `brew --prefix`, and `_prepare_env()` puts the winner's directory on `os.environ['PATH']` for
the subprocesses. A missing `basisu` is a popup, never a silent skip.
"""

bl_info = {
    "name": "Overmatch Tank Export",
    "author": "overmatch",
    "version": (1, 0, 0),
    "blender": (4, 2, 0),
    "location": "File > Export > Overmatch Tank (.glb)",
    "description": "Export the tank glb with the KTX2 mip bake folded in (and bake stock glTF "
                   "exports that target a tracked vehicle glb).",
    "category": "Import-Export",
}

import importlib.util
import os
import shutil
import subprocess
import tempfile

import bpy
from bpy.types import Operator
from bpy_extras.io_utils import ExportHelper

# The scope guard, repo-relative. An export whose destination is one of these, inside a git work
# tree that carries the bake scripts, gets baked. Everything else — scratch paths, other projects,
# a .gltf next door — is left alone silently. Add a vehicle here when it starts being tracked.
VEHICLE_GLBS = (
    "assets/tiger_1/tiger_1.glb",
)

EXPORT_TIGER_RELPATH = ".agents/blender/export_tiger.py"

# Searched in order, appended to whatever PATH we inherit. /usr/bin..sbin are re-asserted because a
# stripped GUI environment is a real possibility and `git`/`python3` live there.
TOOL_PATHS = ("/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin")

# Set while our own operator drives `export_scene.gltf`, so the callback does not bake a file that
# is about to be baked (and, since the bake itself is a subprocess and never re-enters bpy, this is
# the only re-entrancy that can occur).
_SUPPRESS_CALLBACK = False


# ── environment ──────────────────────────────────────────────────────────────────────────────────

def _augmented_path():
    parts = [p for p in os.environ.get("PATH", "").split(os.pathsep) if p]
    for extra in TOOL_PATHS:
        if extra not in parts and os.path.isdir(extra):
            parts.append(extra)
    return os.pathsep.join(parts)


def _executable(path):
    return bool(path) and os.path.isfile(path) and os.access(path, os.X_OK)


def _resolve_basisu():
    """Absolute path to `basisu`, or None. PATH ▸ known homebrew prefixes ▸ `brew --prefix`."""
    path = _augmented_path()

    found = shutil.which("basisu", path=path)
    if _executable(found):
        return found

    for prefix in ("/opt/homebrew", "/usr/local", "/home/linuxbrew/.linuxbrew"):
        candidate = os.path.join(prefix, "bin", "basisu")
        if _executable(candidate):
            return candidate

    brew = shutil.which("brew", path=path)
    if _executable(brew):
        try:
            prefix = subprocess.run(
                [brew, "--prefix"], capture_output=True, text=True, timeout=20, check=True,
            ).stdout.strip()
        except (subprocess.SubprocessError, OSError):
            prefix = ""
        candidate = os.path.join(prefix, "bin", "basisu") if prefix else ""
        if _executable(candidate):
            return candidate

    return None


BASISU_MISSING = (
    "`basisu` was not found, so the KTX2 mip bake cannot run.\n"
    "Install it with:  brew install basis_universal\n"
    "then restart Blender (a GUI Blender does not see PATH changes made after it launched).\n"
    "Refusing to leave a mipless glb on the tracked path."
)


def _prepare_env():
    """Make `basisu` (and git/python3) reachable by the subprocesses `export_tiger` spawns.

    `export_tiger` uses `shutil.which` and plain `subprocess.run`, both of which read
    `os.environ`, so putting the resolved directory there is all the plumbing needed — no second
    copy of the bake invocation. Returns the basisu path, or None if it could not be found.
    """
    basisu = _resolve_basisu()
    path = _augmented_path()
    if basisu:
        directory = os.path.dirname(basisu)
        if directory not in path.split(os.pathsep):
            path = directory + os.pathsep + path
    os.environ["PATH"] = path
    return basisu


# ── repo / target resolution ─────────────────────────────────────────────────────────────────────

def _repo_root_for(path):
    """The git work-tree root containing `path`, or None. Walks up — no absolute paths baked in."""
    if not path:
        return None
    directory = os.path.dirname(os.path.realpath(path))
    while directory != os.path.dirname(directory):
        if os.path.exists(os.path.join(directory, ".git")):
            return directory
        directory = os.path.dirname(directory)
    return None


def _tracked_vehicle_root(filepath):
    """`(root, relpath)` if `filepath` is a tracked vehicle glb of a repo that carries the bake.

    None otherwise — that None is the whole scope guard for the stock-exporter callback.
    """
    if not filepath:
        return None
    root = _repo_root_for(filepath)
    if not root:
        return None
    rel = os.path.relpath(os.path.realpath(filepath), root)
    if rel not in VEHICLE_GLBS:
        return None
    if not os.path.isfile(os.path.join(root, EXPORT_TIGER_RELPATH)):
        return None
    return root, rel


def _canonical_glb(root):
    """The tracked glb belonging to the open blend (matched by folder), else the first one."""
    blend_dir = os.path.dirname(os.path.realpath(bpy.data.filepath)) if bpy.data.filepath else ""
    for rel in VEHICLE_GLBS:
        candidate = os.path.join(root, rel)
        if blend_dir and os.path.dirname(candidate) == blend_dir:
            return candidate
    return os.path.join(root, VEHICLE_GLBS[0])


def _load_export_tiger(root):
    """Import `<root>/.agents/blender/export_tiger.py` fresh, by path.

    By path rather than by `sys.path` insertion so that two work trees never collide in
    `sys.modules`, and fresh every time so an edit to the script door is picked up without
    restarting Blender.
    """
    path = os.path.join(root, EXPORT_TIGER_RELPATH)
    if not os.path.isfile(path):
        raise RuntimeError(f"missing {path} — is this really the overmatch work tree?")
    spec = importlib.util.spec_from_file_location("overmatch_export_tiger", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# ── reporting ────────────────────────────────────────────────────────────────────────────────────

def _popup(title, message, icon='ERROR'):
    """A modal-ish popup, because the callback path has no operator to `self.report` through."""
    print(f"[overmatch] {title}: {message}")
    if bpy.app.background:
        return
    lines = [line for line in message.splitlines() if line.strip()]

    def draw(self, _context):
        for line in lines:
            self.layout.label(text=line)

    try:
        bpy.context.window_manager.popup_menu(draw, title=title, icon=icon)
    except (AttributeError, RuntimeError):
        pass  # No window manager (background, or an odd context) — the print above stands in.


def _summary(glb, module):
    """"71.1 MB mipless → 63.2 MB baked, mips verified (…)" from what the bake recorded."""
    last = getattr(module, "LAST_EXPORT", {}) or {}
    out_mb = os.path.getsize(glb) / 1e6
    raw = last.get("raw_bytes")
    sizes = f"{raw / 1e6:.1f} MB mipless → {out_mb:.1f} MB baked" if raw else f"{out_mb:.1f} MB"
    verified = (last.get("verify") or "").strip()
    verified = f"mips verified ({verified})" if verified else "mips verified"
    return f"{os.path.basename(glb)} — {sizes}, {verified}"


def _stage_of(exc):
    return getattr(exc, "stage", None) or type(exc).__name__


# ── the menu operator ────────────────────────────────────────────────────────────────────────────

class OVERMATCH_OT_export_tank(Operator, ExportHelper):
    """Export this tank to glb with the KTX2 mip bake and the mip gate folded in"""

    bl_idname = "overmatch.export_tank"
    bl_label = "Overmatch Tank (.glb)"
    bl_options = {'REGISTER'}

    filename_ext = ".glb"
    filter_glob: bpy.props.StringProperty(default="*.glb", options={'HIDDEN'})

    def invoke(self, context, event):
        root = _repo_root_for(bpy.data.filepath)
        if not root:
            self.report({'ERROR'}, "Save the .blend inside the overmatch work tree first — the "
                                   "export path and the bake scripts are found by walking up "
                                   "from it.")
            return {'CANCELLED'}
        self.filepath = _canonical_glb(root)
        context.window_manager.fileselect_add(self)
        return {'RUNNING_MODAL'}

    def execute(self, context):
        global _SUPPRESS_CALLBACK

        root = _repo_root_for(bpy.data.filepath)
        if not root:
            self.report({'ERROR'}, "no work tree above the open .blend — cannot find the bake.")
            return {'CANCELLED'}

        if not _prepare_env():
            self.report({'ERROR'}, "basisu not found — see the popup. Nothing was written.")
            _popup("Overmatch export: basisu missing", BASISU_MISSING)
            return {'CANCELLED'}

        glb = bpy.path.abspath(self.filepath)
        window = context.window
        if window:
            window.cursor_set('WAIT')
        try:
            module = _load_export_tiger(root)
            # The chain: temp mipless export ▸ bake onto `glb` ▸ verify. The tracked path is only
            # written by a bake that succeeded, so a failure here leaves the previous glb alone.
            _SUPPRESS_CALLBACK = True   # our own export_scene.gltf must not re-trigger the hook
            try:
                module.export(root=root, glb=glb)
            finally:
                _SUPPRESS_CALLBACK = False
        except BaseException as exc:  # SystemExit-derived: see export_tiger.ExportError
            if isinstance(exc, KeyboardInterrupt):
                raise
            message = f"{_stage_of(exc)} failed — {exc}"
            self.report({'ERROR'}, message)
            _popup("Overmatch export failed", str(exc))
            return {'CANCELLED'}
        finally:
            if window:
                window.cursor_set('DEFAULT')

        self.report({'INFO'}, f"Exported {_summary(glb, module)}")
        return {'FINISHED'}


def menu_func_export(self, _context):
    self.layout.operator(OVERMATCH_OT_export_tank.bl_idname, text="Overmatch Tank (.glb)")


# ── the stock-exporter callback ──────────────────────────────────────────────────────────────────

class glTF2ExportUserExtension:
    """Discovered by io_scene_gltf2 (`__init__.py:1334-1336`) and instantiated once per export.

    Only the two hooks below exist on this class; the exporter's dispatcher (`io/exp/
    user_extensions.py`) `getattr`s each hook name and skips what is absent, so this costs nothing
    on the exports it does not care about.
    """

    def __init__(self):
        self.target = None      # (root, relpath) once the guard has said yes
        self.stash = None       # copy of the previous good glb, restored if the bake fails

    # `__init__.py:1353` — before anything is gathered; `gltf_filepath` is already set (line 1122).
    def pre_export_hook(self, export_settings):
        self.target = None
        self.stash = None
        if _SUPPRESS_CALLBACK:
            return
        filepath = export_settings.get("gltf_filepath")
        if export_settings.get("gltf_format") != "GLB":
            return
        target = _tracked_vehicle_root(filepath)
        if target is None:
            return                      # not a tracked vehicle glb: silence, by design
        self.target = target
        glb = os.path.realpath(filepath)
        if os.path.isfile(glb):
            # Held so a failed bake can put the last good (mip-baked) glb back, matching the
            # scripted door's guarantee. APFS clones this, so the 70 MB is not really copied.
            handle, stash = tempfile.mkstemp(prefix="overmatch-prev-", suffix=".glb")
            os.close(handle)
            try:
                shutil.copyfile(glb, stash)
                self.stash = stash
            except OSError:
                os.unlink(stash)

    # `__init__.py:1380` — after `save()` returned, i.e. after `__write_file`. The mipless glb is
    # on disk at this point, which is exactly why this hook and not `glTF2_post_export_callback`.
    def post_export_hook(self, export_settings):
        target, stash = self.target, self.stash
        self.target, self.stash = None, None
        if target is None:
            return
        root, rel = target
        glb = os.path.realpath(export_settings.get("gltf_filepath"))
        if not os.path.isfile(glb):
            return

        if not _prepare_env():
            self._restore(glb, stash)
            _popup("Overmatch mip bake: basisu missing", BASISU_MISSING)
            return

        work = tempfile.mkdtemp(prefix="overmatch-bake-")
        raw = os.path.join(work, os.path.basename(glb))
        try:
            # Reinstate temp-first ordering after the fact: the exporter wrote a mipless glb onto
            # the tracked path, so move it out of the way and let the bake write that path.
            shutil.move(glb, raw)
            module = _load_export_tiger(root)
            module.bake(root=root, raw=raw, glb=glb)
        except BaseException as exc:
            if isinstance(exc, KeyboardInterrupt):
                raise
            if not os.path.isfile(glb) and os.path.isfile(raw):
                shutil.move(raw, glb)   # last resort: at least do not leave a hole
            self._restore(glb, stash)
            _popup(
                "Overmatch mip bake failed",
                f"{rel}\n{_stage_of(exc)} failed — {exc}\n"
                + ("The previous mip-baked glb was restored."
                   if stash else "No previous glb to restore."),
            )
            return
        finally:
            shutil.rmtree(work, ignore_errors=True)
            if stash and os.path.isfile(stash):
                os.unlink(stash)

        _popup("Overmatch mip bake", _summary(glb, module), icon='CHECKMARK')

    @staticmethod
    def _restore(glb, stash):
        if stash and os.path.isfile(stash):
            try:
                shutil.move(stash, glb)
            except OSError:
                pass


# ── registration ─────────────────────────────────────────────────────────────────────────────────

def register():
    bpy.utils.register_class(OVERMATCH_OT_export_tank)
    bpy.types.TOPBAR_MT_file_export.append(menu_func_export)


def unregister():
    bpy.types.TOPBAR_MT_file_export.remove(menu_func_export)
    bpy.utils.unregister_class(OVERMATCH_OT_export_tank)


if __name__ == "__main__":
    register()
