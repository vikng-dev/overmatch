"""overmatch_export.py — the GUI adapter over the one tank build.

Install this once (Preferences ▸ Add-ons ▸ Install… ▸ pick this file, then tick it) and
**File ▸ Export ▸ Overmatch Tank (.glb)** saves the open blend and runs exactly

    python3 scripts/tank/build.py build assets/<id>/<id>.blend

as a subprocess. This file holds no check, no census, no notice and no export setting of its own,
and it does not enter the chain anywhere except at the top: it prepares an environment a GUI Blender
does not have, saves, invokes the door, and shows what came back. Everything it could ask about a
model is a law with a home: `.agents/blender/export_tank.py` (the L1 source pass), `src/bake.rs`
(the consumer contract), `scripts/tank/glb_ktx2.py` (the derivation checks).

THE SEAM IS STORED TRUTH
------------------------
The door certifies the file on disk, never the session — `L1.SAVED_SOURCE` says so. So the operator
SAVES the blend first, and every stage after that reads the saved bytes. That single line is why
the GUI and the headless door cannot certify different models.

ONE ENTRANCE, AND THE LAUNCH IT COSTS
-------------------------------------
The door launches its own pinned Blender on the saved file, so a GUI export pays a MEASURED 30-60 s
more than an in-process source pass would. That is the whole price of there being one entrance: no
candidate crosses a process boundary, so nothing has to authenticate one, and the bytes a GUI export
lands are the bytes `build.py build` lands because it IS `build.py build`.

THE STOCK glTF EXPORTER
-----------------------
**File ▸ Export ▸ glTF 2.0** pointed at a tracked asset's glb is refused, and the previous model is
put back. The exporter's argument list is frozen (`export_tank.EXPORT_SETTINGS`) because those
arguments decide the bytes of every model, and a hand-export carries whatever the dialog happens to
hold; worse, it reaches the tracked path without a source pass ever having run. The hook exists to
keep that promise, not to bake around it. Exporting anywhere else is left completely alone.

Which hook, and why not the one the docs name (Blender 5.1,
`scripts/addons_core/io_scene_gltf2/__init__.py:1329-1347`): the exporter collects
`module.glTF2_pre_export_callback` / `glTF2_post_export_callback` and, separately, a
`glTF2ExportUserExtension` class instantiated per export. `glTF2_post_export_callback` fires BEFORE
the file is written — "post export" there means post-gather — so a hook hung off it would judge the
PREVIOUS file. The user-extension `post_export_hook` is the one that runs after the bytes land.

macOS GUI PATH
--------------
Blender launched from Finder/Dock inherits launchd's PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), not
your shell's, so homebrew's `basisu` and `~/.cargo/bin/cargo` are invisible to it and the chain
would fail on a machine where the terminal runs it fine. `_prepare_env()` appends the known tool
directories (and asks `brew --prefix`) to `os.environ['PATH']`, which is all the plumbing needed:
the door resolves every program through `scripts/toolchain.py`, which reads that PATH. A program
that is missing or unpinned is the door's own `door.toolchain` row, shown in a popup — never a
silent skip.
"""

bl_info = {
    "name": "Overmatch Tank Export",
    "author": "overmatch",
    "version": (2, 0, 0),
    "blender": (4, 2, 0),
    "location": "File > Export > Overmatch Tank (.glb)",
    "description": "Run the one asset door on the open tank blend: source pass, consumer "
                   "contract, KTX2 derivation, tracked model.",
    "category": "Import-Export",
}

import os
import select
import shutil
import signal
import subprocess
import tempfile
import time

import bpy
from bpy.types import Operator

#: The one door. Repo-relative: this add-on is installed into Blender's own directory and finds the
#: work tree by walking up from the open blend.
BUILD_RELPATH = os.path.join("scripts", "tank", "build.py")

#: Searched in order, appended to whatever PATH we inherit. `~/.cargo/bin` carries the consumer
#: contract's `cargo`; /usr/bin..sbin are re-asserted because a stripped GUI environment is a real
#: possibility and `git` and `python3` live there.
TOOL_PATHS = (
    os.path.expanduser("~/.cargo/bin"),
    "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin",
)

#: Homebrew prefixes to ask about when `brew` itself is not on the inherited PATH.
BREW_PREFIXES = ("/opt/homebrew", "/usr/local", "/home/linuxbrew/.linuxbrew")

#: Said before the UI goes dark. The console line is the load-bearing half on macOS: a GUI Blender
#: launched from Finder writes stdout nowhere the user can see, so the notice names the fix.
FREEZE_NOTICE = (
    "The asset door is running — this takes a few minutes (it opens a Blender of its own on the "
    "file you just saved) and BLENDER'S WINDOW WILL FREEZE "
    "for all of it (no redraw, spinning cursor). That is normal. DO NOT force-quit: quitting "
    "mid-run leaves the previous tracked model in place but wastes the export. The full report "
    "prints to the system console (Window ▸ Toggle System Console on Windows; on macOS relaunch "
    "Blender from a terminal to see it)."
)

#: The whole run's ceiling, from the moment the build is launched. A full chain is TENS OF MINUTES —
#: the build opens Blenders of its own, every image is KTX2-encoded and every source primitive gets
#: a certified LOD chain — and Blender's window is frozen for all of it, so this is generous by
#: construction: what it bounds is the freeze, not the work.
DOOR_DEADLINE_SECONDS = 120 * 60

#: Said when it expires, in the shape of every other refusal the artist reads: what happened, what
#: it means for the tracked model, what to do next.
DOOR_DEADLINE_EXPIRED = (
    "The asset door did not finish within {:.0f} minutes (DOOR_DEADLINE_SECONDS in this add-on) "
    "and was killed, along with every process it had launched.\n"
    "The tracked model is unchanged.\n"
    "Run the door in a terminal to see where it stopped:\n"
    "    python3 scripts/tank/build.py build <blend>"
)

#: Emitted once per encoded image by `scripts/encode-tank-ktx2.sh`, and once up front with the
#: total. The progress meter is nothing more than counting these off the door's stdout.
_IMAGE_LINE = "ktx2  ▸"
_TOTAL_LINE = "images ▸"


# ── environment ──────────────────────────────────────────────────────────────────────────────────

def _executable(path):
    return bool(path) and os.path.isfile(path) and os.access(path, os.X_OK)


def _brew_bin():
    """Homebrew's `bin`, asked for rather than assumed. Empty when there is no brew."""
    for prefix in BREW_PREFIXES:
        if os.path.isdir(os.path.join(prefix, "bin")):
            return os.path.join(prefix, "bin")
    brew = shutil.which("brew")
    if not _executable(brew):
        return ""
    try:
        prefix = subprocess.run(
            [brew, "--prefix"], capture_output=True, text=True, timeout=20, check=True,
        ).stdout.strip()
    except (subprocess.SubprocessError, OSError):
        return ""
    return os.path.join(prefix, "bin") if prefix else ""


def _prepare_env():
    """Put the tool directories a GUI Blender did not inherit onto `os.environ['PATH']`.

    Nothing here decides WHICH program runs: `scripts/toolchain.py` resolves and pins every one of
    them off this PATH, so this is the environment half and only that.
    """
    parts = [part for part in os.environ.get("PATH", "").split(os.pathsep) if part]
    for extra in TOOL_PATHS + (_brew_bin(),):
        if extra and extra not in parts and os.path.isdir(extra):
            parts.append(extra)
    os.environ["PATH"] = os.pathsep.join(parts)
    return os.environ["PATH"]


def _python():
    """The interpreter the wrapper runs under. Never `sys.executable`: inside Blender that is
    Blender, and handing it a script path opens the app instead of running the door."""
    return shutil.which("python3")


PYTHON_MISSING = (
    "`python3` was not found, so the asset door cannot be run.\n"
    "The door is a stdlib-only script; any python3 on PATH will do.\n"
    "Install one and restart Blender (a GUI Blender does not see PATH changes made after it "
    "launched)."
)


# ── repo and asset resolution ────────────────────────────────────────────────────────────────────

def repo_root_for(path):
    """The git work-tree root containing `path`, or None. Walks up — no absolute paths baked in."""
    if not path:
        return None
    directory = os.path.dirname(os.path.realpath(path))
    while directory != os.path.dirname(directory):
        if os.path.exists(os.path.join(directory, ".git")):
            return directory
        directory = os.path.dirname(directory)
    return None


def tracked_asset(glb):
    """`(root, blend)` when `glb` is a tracked asset's model, else None.

    THE SCOPE GUARD, and it names no vehicle: an asset is the sibling trio `<id>.blend`,
    `<id>.tank.ron`, `<id>.glb` in `assets/<id>/` of a work tree that carries the door. A second
    tank is a directory, never a line here.
    """
    if not glb:
        return None
    path = os.path.realpath(glb)
    directory, filename = os.path.split(path)
    stem, extension = os.path.splitext(filename)
    if extension.lower() != ".glb" or os.path.basename(directory) != stem:
        return None
    if os.path.basename(os.path.dirname(directory)) != "assets":
        return None
    root = repo_root_for(path)
    if not root or not os.path.isfile(os.path.join(root, BUILD_RELPATH)):
        return None
    blend = os.path.join(directory, stem + ".blend")
    if not os.path.isfile(blend) or not os.path.isfile(os.path.join(directory, stem + ".tank.ron")):
        return None
    return (root, blend)


# ── reporting and progress ───────────────────────────────────────────────────────────────────────

def popup(title, message, icon='ERROR'):
    """A popup, because the stock-exporter hook has no operator to `self.report` through."""
    print("[overmatch] {}: {}".format(title, message))
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


class Progress:
    """The window-manager progress cursor, or a no-op wherever there is no UI to drive it.

    `progress_begin`/`progress_update` are the only feedback Blender can give from inside a blocking
    call — they set the cursor directly rather than queueing a redraw. Every call is guarded because
    this module runs headless as often as it runs in the GUI.
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


def _readable(stream, timeout):
    """Whether `stream` has something to read within `timeout` seconds. A pipe that cannot be waited
    on says yes and is then read blocking, which checks the deadline between lines only."""
    try:
        return bool(select.select([stream], [], [], timeout)[0])
    except (OSError, ValueError):
        return True


def _kill_group(process):
    """The door AND the Blender it launched. `start_new_session=True` puts them in a process group
    of their own, so one signal reaches every one of them and none reaches this Blender."""
    try:
        os.killpg(os.getpgid(process.pid), signal.SIGKILL)
    except (AttributeError, OSError):
        process.kill()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass


def run_streamed(command, root):
    """Run the door with its stdout on a pipe, echoing, counting and KEEPING it. Returns
    `(exit code, every line it printed)`.

    A run that outlives `DOOR_DEADLINE_SECONDS` is killed with its process group and raised as a
    `Refused` at the stage `deadline` — the same path a red door takes to the popup. Blender's main
    thread is inside this call, so a door that never returns is a Blender that never returns.

    The pipe is the entire progress mechanism. `subprocess.run` with an inherited stdout gives the
    user minutes of nothing (the door's lines sit in Blender's console buffer behind a main thread
    that never returns to the event loop); reading it line by line and printing with an explicit
    flush puts each `ktx2  ▸` line on screen the moment the encoder finishes an image, and ticks the
    cursor percentage with it.

    The lines are also RETAINED, because on macOS a Blender launched from Finder writes stdout
    nowhere a user can read: the popup is then the only rendering of the door's report they get, and
    it can only show what was kept.

    stderr is folded into stdout so a failure keeps its position in the sequence instead of
    surfacing after everything else. PYTHONUNBUFFERED is set because the door's own python stages
    would otherwise block-buffer into the pipe and arrive in one lump.
    """
    env = dict(os.environ, PYTHONUNBUFFERED="1")
    progress = Progress()
    total, done = 0, 0
    printed = []
    expired = Refused("deadline", DOOR_DEADLINE_EXPIRED.format(DOOR_DEADLINE_SECONDS / 60.0))
    deadline = time.monotonic() + DOOR_DEADLINE_SECONDS
    process = None
    try:
        process = subprocess.Popen(
            command, cwd=root, env=env, text=True, bufsize=1,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True,
        )
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not _readable(process.stdout, remaining):
                raise expired
            line = process.stdout.readline()
            if not line:
                break
            print(line, end="", flush=True)
            printed.append(line.rstrip("\n"))
            if line.startswith(_TOTAL_LINE):
                # "images ▸ 9 to encode"
                field = line[len(_TOTAL_LINE):].strip().split(" ", 1)[0]
                total = int(field) if field.isdigit() else 0
            elif line.startswith(_IMAGE_LINE):
                done += 1
                # Unknown total (an older encoder) still moves, it just cannot promise 100%.
                progress.update(100.0 * done / total if total else min(90.0, 10.0 * done))
        try:
            return (process.wait(timeout=max(1.0, deadline - time.monotonic())), printed)
        except subprocess.TimeoutExpired:
            raise expired
    finally:
        progress.end()
        if process is not None:
            if process.poll() is None:
                _kill_group(process)
            process.stdout.close()


# ── the one call ─────────────────────────────────────────────────────────────────────────────────

class Refused(Exception):
    """The chain said no, or never came back. `stage` names where, so the operator can say it
    without parsing prose."""

    def __init__(self, stage, message):
        super().__init__(message)
        self.stage = stage


#: The chain's own console protocol: every stage it is about to run is announced on a `build ▸` or
#: `door  ▸` line, and the one that refused is named on a last line carrying `refused at <stage>`.
#: Read here for the same reason `_IMAGE_LINE` is — this file reads that stdout and holds no
#: vocabulary of its own.
_STAGE_LINES = ("build ▸ ", "door  ▸ ")
_REFUSED_MARK = "refused at "

#: How many of the refusing stage's lines the popup carries. A popup may summarize; the console and
#: the door's own stdout hold every row. Findings sort ERRORS FIRST, so a head of the stage's output
#: is the refusal itself — a tail would be whatever informational census followed it.
POPUP_LINES = 24


def refusal_of(printed):
    """`(stage, message)` for a door run that exited non-zero: the stage the door named, and what
    the stage that refused printed.

    NOTHING IS PARSED OUT OF THE REPORT. Its rows are the door's rendering (`scripts/tank/
    report.py`) and re-deriving them here would be this file holding a second vocabulary. What is
    read is only the chain's own stage lines: everything after the LAST stage it announced is
    what that stage said, and the report is at the head of it because errors sort first. A run that
    ends some other way — no announcement, no verdict — surfaces whole, under the stage `door`.
    """
    end = len(printed)
    stage = "door"
    for index in range(len(printed) - 1, -1, -1):
        if _REFUSED_MARK in printed[index]:
            stage = printed[index].split(_REFUSED_MARK, 1)[1].split(" ", 1)[0].strip() or stage
            end = index
            break
    announced = [index for index in range(end)
                 if printed[index].startswith(_STAGE_LINES)]
    said = printed[announced[-1] + 1:end] if announced else printed[:end]
    return (stage, "\n".join((said[:POPUP_LINES] or ["the build printed nothing at all"]) + [
        "", "The tracked model is unchanged. The complete report is in the system console.",
    ]))


def export_open_blend(root):
    """Save the open blend and run the door on it. Returns the tracked glb path.

    Two calls, and neither of them is a stage: `wm.save_mainfile`, because the door certifies stored
    truth, and then the door itself as a subprocess — which launches its own pinned Blender on that
    saved file and runs every stage there. The adapter never sees a candidate, a canon file or a
    finding, so there is nothing here that could disagree with a headless export.
    """
    python = _python()
    if not python:
        raise Refused("python3", PYTHON_MISSING)

    bpy.ops.wm.save_mainfile()
    blend = bpy.data.filepath
    stem = os.path.splitext(os.path.basename(blend))[0]

    code, printed = run_streamed(
        [python, os.path.join(root, BUILD_RELPATH), "build", blend], root,
    )
    if code:
        raise Refused(*refusal_of(printed))
    return os.path.join(os.path.dirname(blend), stem + ".glb")


# ── the menu operator ────────────────────────────────────────────────────────────────────────────

class OVERMATCH_OT_export_tank(Operator):
    """Run the asset door on this tank: source pass, consumer contract, KTX2 derivation, then the
    tracked model. The blend is saved first — the door certifies the stored file"""

    bl_idname = "overmatch.export_tank"
    bl_label = "Overmatch Tank (.glb)"
    bl_options = {'REGISTER'}

    @classmethod
    def poll(cls, _context):
        """A saved blend inside a work tree that carries the door. There is no file dialog and no
        path to choose: the door derives the spec sheet and the model from the blend's stem, and a
        second destination would be a second door."""
        if not bpy.data.filepath:
            cls.poll_message_set("Save the .blend as assets/<id>/<id>.blend first — the door "
                                 "certifies the stored file.")
            return False
        if repo_root_for(bpy.data.filepath) is None:
            cls.poll_message_set("The open .blend is not inside the overmatch work tree.")
            return False
        return True

    def invoke(self, context, event):
        return context.window_manager.invoke_confirm(self, event)

    def execute(self, context):
        root = repo_root_for(bpy.data.filepath)
        if root is None:
            self.report({'ERROR'}, "no work tree above the open .blend — cannot find the door.")
            return {'CANCELLED'}

        _prepare_env()
        window = context.window
        if window:
            window.cursor_set('WAIT')
        # Say it before the window stops redrawing. The status bar will not repaint until this
        # operator returns, so this lands in the Info editor after the fact — the console is what
        # the user reads DURING the freeze, which is what the notice tells them.
        self.report({'INFO'}, FREEZE_NOTICE)
        print("\n[overmatch] {}\n".format(FREEZE_NOTICE), flush=True)
        try:
            glb = export_open_blend(root)
        except BaseException as exc:  # noqa: BLE001 — a GUI must survive every failure below
            if isinstance(exc, KeyboardInterrupt):
                raise
            stage = getattr(exc, "stage", None) or type(exc).__name__
            self.report({'ERROR'}, "{} refused — see the console.".format(stage))
            popup("Overmatch export refused at {}".format(stage), str(exc))
            return {'CANCELLED'}
        finally:
            if window:
                window.cursor_set('DEFAULT')

        self.report({'INFO'}, "Exported {} — {:.1f} MB".format(
            os.path.basename(glb), os.path.getsize(glb) / 1e6
        ))
        return {'FINISHED'}


def menu_func_export(self, _context):
    self.layout.operator(OVERMATCH_OT_export_tank.bl_idname, text="Overmatch Tank (.glb)")


# ── the stock-exporter hook ──────────────────────────────────────────────────────────────────────

STOCK_EXPORT_REFUSED = (
    "A tracked tank model is written by the asset door and nothing else — the door's export "
    "settings are frozen, and this export ran no source pass at all.\n"
    "The previous model has been put back.\n"
    "Use File ▸ Export ▸ Overmatch Tank (.glb), or run:\n"
    "    python3 scripts/tank/build.py build {}"
)


class glTF2ExportUserExtension:
    """Discovered by io_scene_gltf2 and instantiated once per export.

    Only the two hooks below exist on this class; the exporter's dispatcher `getattr`s each hook
    name and skips what is absent, so this costs nothing on the exports it does not care about.

    It needs no exemption for the door's own export: that runs in a Blender of the door's own,
    launched `--factory-startup`, where this add-on is not loaded at all.
    """

    def __init__(self):
        self.target = None      # (root, blend) once the scope guard has said yes
        self.stash = None       # copy of the tracked model, put back after the export

    def pre_export_hook(self, export_settings):
        self.target = None
        self.stash = None
        if export_settings.get("gltf_format") != "GLB":
            return
        filepath = export_settings.get("gltf_filepath")
        target = tracked_asset(filepath)
        if target is None:
            return                      # not a tracked asset's model: silence, by design
        self.target = target
        glb = os.path.realpath(filepath)
        if not os.path.isfile(glb):
            return
        # Held so the tracked model can be put back over whatever this export writes. APFS clones
        # this, so the tens of megabytes are not really copied.
        handle, stash = tempfile.mkstemp(prefix="overmatch-tracked-", suffix=".glb")
        os.close(handle)
        try:
            shutil.copyfile(glb, stash)
            self.stash = stash
        except OSError:
            os.unlink(stash)

    def post_export_hook(self, export_settings):
        target, stash = self.target, self.stash
        self.target, self.stash = None, None
        if target is None:
            return
        _root, blend = target
        glb = os.path.realpath(export_settings.get("gltf_filepath"))
        if stash is not None:
            try:
                shutil.move(stash, glb)
            except OSError:
                pass
        elif os.path.isfile(glb):
            os.remove(glb)
        popup("Overmatch: the tracked model is the door's to write",
              STOCK_EXPORT_REFUSED.format(blend))


# ── registration ─────────────────────────────────────────────────────────────────────────────────

def register():
    bpy.utils.register_class(OVERMATCH_OT_export_tank)
    bpy.types.TOPBAR_MT_file_export.append(menu_func_export)


def unregister():
    bpy.types.TOPBAR_MT_file_export.remove(menu_func_export)
    bpy.utils.unregister_class(OVERMATCH_OT_export_tank)


if __name__ == "__main__":
    register()
