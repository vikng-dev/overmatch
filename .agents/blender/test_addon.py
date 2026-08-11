"""test_addon.py — the GUI adapter's plumbing, headless.

    /Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \\
      --python .agents/blender/test_addon.py

WHAT THIS CAN AND CANNOT PROVE. The add-on is an adapter: it prepares an environment, saves, and
runs the one door as a subprocess. Everything except the drawing is reachable without a window, and
that is what is measured here — registration, the operator's poll and the reason it gives, the scope
guard, the PATH preparation, the progress counter, the stock-exporter hook's restore, and the two
load-bearing ones: that a GUI export of a clean trio certifies through the REAL door, and that a
refused one surfaces the door's own report and leaves the tracked model alone.

The door is not mocked anywhere here — the two cases above launch it, which launches its own pinned
Blender, exactly as an artist's click does. The chain's own laws are NOT re-proved: that is
`scripts/tank/test_asset_door.py`, which drives every stage's refusal.

GUI-ONLY, and left to a human: the popup and the cursor percentage actually appearing on screen,
and the File ▸ Export menu entry being clickable. Both are drawing.
"""

import os
import sys
import tempfile
import time
import traceback

import bpy

_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.dirname(os.path.dirname(_HERE))
sys.path.insert(0, os.path.join(_HERE, "addons"))

import overmatch_export as addon  # noqa: E402

_WORK = tempfile.mkdtemp(prefix="overmatch-addon-test-")

CASES = []


def case(function):
    CASES.append(function)
    return function


def fake_tree(name, stem="testbed", holder=None, files=("blend", "tank.ron", "glb"), door=True):
    """A work tree shaped like this repository, with one asset trio in it. Only the shapes the
    scope guard reads are real — the files are empty."""
    root = os.path.join(_WORK, name)
    os.makedirs(os.path.join(root, ".git"), exist_ok=True)
    if door:
        os.makedirs(os.path.join(root, "scripts", "tank"), exist_ok=True)
        open(os.path.join(root, addon.DOOR_RELPATH), "w").close()
    directory = os.path.join(root, "assets", holder or stem)
    os.makedirs(directory, exist_ok=True)
    for extension in files:
        open(os.path.join(directory, "{}.{}".format(stem, extension)), "w").close()
    return (root, os.path.join(directory, stem + ".glb"))


# ── registration ─────────────────────────────────────────────────────────────────────────────────

@case
def register_and_unregister_are_symmetric():
    addon.register()
    assert hasattr(bpy.ops.overmatch, "export_tank"), "the operator did not register"
    assert addon.menu_func_export in bpy.types.TOPBAR_MT_file_export._dyn_ui_initialize(), \
        "the File ▸ Export entry was not appended"
    addon.unregister()
    assert addon.menu_func_export not in bpy.types.TOPBAR_MT_file_export._dyn_ui_initialize(), \
        "the File ▸ Export entry survived unregister"
    # A second cycle is what an add-on reload does, and a class left registered breaks it.
    addon.register()
    addon.unregister()


@case
def the_operator_refuses_an_unsaved_blend_and_says_why():
    addon.register()
    try:
        assert not bpy.data.filepath, "this case needs a Blender with no blend open"
        assert not addon.OVERMATCH_OT_export_tank.poll(bpy.context), \
            "the door certifies the stored file, so an unsaved blend has nothing to export"
    finally:
        addon.unregister()


# ── the scope guard ──────────────────────────────────────────────────────────────────────────────

@case
def a_sibling_trio_in_a_work_tree_with_the_door_is_a_tracked_asset():
    root, glb = fake_tree("tracked")
    resolved = addon.tracked_asset(glb)
    assert resolved is not None, "the trio was not recognised"
    assert os.path.realpath(resolved[0]) == os.path.realpath(root), resolved
    assert resolved[1].endswith(os.path.join("assets", "testbed", "testbed.blend")), resolved


@case
def the_scope_guard_names_no_vehicle():
    """Two different asset ids resolve through the same rule, with nothing added between them."""
    _root, first = fake_tree("two-tanks-a", stem="testbed")
    _root, second = fake_tree("two-tanks-b", stem="another_tank")
    assert addon.tracked_asset(first) is not None
    assert addon.tracked_asset(second) is not None


@case
def anything_that_is_not_the_trio_is_left_alone():
    cases = {
        "no spec sheet": fake_tree("no-ron", files=("blend", "glb"))[1],
        "no source blend": fake_tree("no-blend", files=("tank.ron", "glb"))[1],
        "folder does not name the asset": fake_tree("misfiled", holder="vehicles")[1],
        "no door in the work tree": fake_tree("no-door", door=False)[1],
    }
    for why, glb in cases.items():
        assert addon.tracked_asset(glb) is None, "{}: was taken for a tracked asset".format(why)
    outside = os.path.join(_WORK, "loose.glb")
    open(outside, "w").close()
    assert addon.tracked_asset(outside) is None, "a glb outside any work tree"
    assert addon.tracked_asset("") is None
    assert addon.tracked_asset(fake_tree("gltf")[1][:-4] + ".gltf") is None, "a .gltf is not a glb"


# ── environment preparation ──────────────────────────────────────────────────────────────────────

@case
def the_tool_directories_are_appended_once_and_only_if_they_exist():
    before = os.environ.get("PATH", "")
    extra = os.path.join(_WORK, "not-a-directory")
    try:
        os.environ["PATH"] = extra
        path = addon._prepare_env().split(os.pathsep)
        assert extra in path, "the inherited PATH was dropped"
        assert all(os.path.isdir(part) or part == extra for part in path), \
            "a directory that does not exist was appended: {}".format(path)
        again = addon._prepare_env().split(os.pathsep)
        assert len(again) == len(set(again)), "a second call duplicated entries: {}".format(again)
    finally:
        os.environ["PATH"] = before


# ── progress ─────────────────────────────────────────────────────────────────────────────────────

@case
def the_progress_counter_reads_the_encoder_lines_and_keeps_what_it_printed():
    script = (
        "import sys\n"
        "print('images ▸ 2 to encode')\n"
        "print('ktx2  ▸ 0.ktx2')\n"
        "print('ktx2  ▸ 1.ktx2')\n"
        "sys.exit(3)\n"
    )
    ticks = []
    original = addon.Progress.update
    addon.Progress.update = lambda self, percent: ticks.append(percent)
    try:
        code, printed = addon.run_streamed([sys.executable, "-c", script], _WORK)
    finally:
        addon.Progress.update = original
    assert code == 3, "the door's exit code was not returned: {}".format(code)
    assert ticks == [50.0, 100.0], ticks
    # Kept, not merely echoed: on macOS the popup is the only rendering an artist ever sees.
    assert printed == ["images ▸ 2 to encode", "ktx2  ▸ 0.ktx2", "ktx2  ▸ 1.ktx2"], printed


def _gone(pid, within=15.0):
    """Whether `pid` is no longer a live process. Polled: a killed grandchild is reaped by init
    rather than by us, so its disappearance is not instantaneous."""
    limit = time.monotonic() + within
    while time.monotonic() < limit:
        try:
            os.kill(pid, 0)
        except OSError:
            return True
        time.sleep(0.1)
    return False


@case
def a_door_that_never_returns_is_killed_and_refused():
    """Blender's main thread is INSIDE `run_streamed`, so a door that hangs is a Blender that hangs
    and a force-quit is the artist's only way out. The fake door launches a child of its own and
    then sleeps past an injected deadline: both processes must be gone, and what reaches the artist
    must be the ordinary refusal, naming the constant that decided it."""
    pids = os.path.join(_WORK, "hung.pids")
    script = (
        "import os, subprocess, sys, time\n"
        "child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(120)'])\n"
        "open({!r}, 'w').write('{{}} {{}}'.format(os.getpid(), child.pid))\n"
        "time.sleep(120)\n"
    ).format(pids)
    original = addon.DOOR_DEADLINE_SECONDS
    addon.DOOR_DEADLINE_SECONDS = 3.0
    started = time.monotonic()
    try:
        addon.run_streamed([sys.executable, "-c", script], _WORK)
    except addon.Refused as refusal:
        assert refusal.stage == "deadline", refusal.stage
        assert "DOOR_DEADLINE_SECONDS" in str(refusal), str(refusal)
        assert "tracked model is unchanged" in str(refusal), str(refusal)
    else:
        raise AssertionError("a door that outlived the deadline was waited on anyway")
    finally:
        addon.DOOR_DEADLINE_SECONDS = original
    assert time.monotonic() - started < 60, "the deadline did not end the wait"
    for pid in (int(field) for field in open(pids).read().split()):
        assert _gone(pid), "{} survived the deadline — the process group was not killed".format(pid)


# ── what reaches the artist when the door says no ────────────────────────────────────────────────

@case
def a_refusal_carries_the_stage_the_door_named_and_what_that_stage_said():
    """`refusal_of` reads the door's own `door  ▸` protocol and nothing else: the stage off the
    verdict line, and the lines after the last stage the door announced — which is what the stage
    that refused printed, errors first."""
    stage, message = addon.refusal_of([
        "door  ▸ toolchain: blender 5.1.2 (/usr/bin/blender)",
        "door  ▸ source: blender --background testbed.blend",
        "L1.MODIFIER_STACK error: object `Hull`",
        "  measured: 1 modifier(s)",
        "export ▸ 1 error, 0 warnings, 7 info",
        "door  ▸ export refused at source — testbed.glb is unchanged",
    ])
    assert stage == "source", stage
    assert "L1.MODIFIER_STACK" in message, message
    assert "blender --background" not in message, "the door's own announcements leaked in"
    assert "system console" in message, message


@case
def a_refusal_with_no_verdict_line_surfaces_whole():
    """A door that died some other way — a traceback, a killed subprocess — has named no stage and
    printed no verdict. Everything it did print is the evidence, so none of it is dropped."""
    stage, message = addon.refusal_of(["Traceback (most recent call last):", "  boom"])
    assert stage == "door", stage
    assert "Traceback" in message and "boom" in message, message
    assert addon.refusal_of([])[1].startswith("the door printed nothing at all")


# ── the stock-exporter hook ──────────────────────────────────────────────────────────────────────

def _hook_over(glb, contents=b"tracked bytes"):
    """One stock export at `glb`, with `contents` already there. Returns what is on disk after."""
    if contents is not None:
        with open(glb, "wb") as handle:
            handle.write(contents)
    extension = addon.glTF2ExportUserExtension()
    settings = {"gltf_format": "GLB", "gltf_filepath": glb}
    extension.pre_export_hook(settings)
    with open(glb, "wb") as handle:      # the stock exporter writing its own bytes
        handle.write(b"whatever the dialog held")
    extension.post_export_hook(settings)
    return open(glb, "rb").read() if os.path.isfile(glb) else None


@case
def a_stock_export_onto_a_tracked_model_puts_the_tracked_model_back():
    _root, glb = fake_tree("stock-tracked")
    assert _hook_over(glb) == b"tracked bytes", \
        "a hand-export reached the tracked path — its settings are not the door's and no source " \
        "pass ran"


@case
def a_stock_export_where_there_was_no_tracked_model_leaves_none():
    _root, glb = fake_tree("stock-absent")
    os.remove(glb)
    assert _hook_over(glb, contents=None) is None, \
        "an uncertified glb was left at the tracked path"


@case
def a_stock_export_anywhere_else_is_untouched():
    loose = os.path.join(_WORK, "elsewhere.glb")
    assert _hook_over(loose) == b"whatever the dialog held", \
        "the hook took an export it has no business in"


# ── the door, run the way the artist runs it ─────────────────────────────────────────────────────

def open_a_trio(name, defect):
    """Build the synthetic trio IN THIS BLENDER and leave it open, which is the artist's situation:
    a session holding a model, about to click Export. Returns the tracked glb's path."""
    sys.path.insert(0, _HERE)
    import fixture_tank  # noqa: PLC0415 — the trio builder, only needed by these cases

    directory = os.path.join(_WORK, name)
    library = fixture_tank.write_library(
        os.path.join(directory, "assets", "materials", "materials.blend")
    )
    asset = os.path.join(directory, "assets", "testbed")
    os.makedirs(asset, exist_ok=True)
    fixture_tank.build(asset, library, defect)
    assert bpy.data.filepath.endswith("testbed.blend"), bpy.data.filepath
    addon._prepare_env()
    return os.path.join(asset, "testbed.glb")


@case
def the_adapter_exports_the_open_blend_through_the_door():
    """The whole adapter, on a clean trio, in this Blender — which is what a GUI export is minus the
    drawing. `export_open_blend` saves the session and runs the REAL door on the file it just wrote;
    the door launches its own pinned Blender and every stage runs there.

    The bytes are the door's own claim, proved in `scripts/tank/test_asset_door.py`. What is proved
    here is that the adapter's two calls — save, then the door — still compose into a certified
    export, and that it is the tracked path they land on.
    """
    glb = open_a_trio("adapter-export", "none")
    landed = addon.export_open_blend(_ROOT)
    assert os.path.realpath(landed) == os.path.realpath(glb), (landed, glb)
    assert os.path.isfile(landed), "the door certified but wrote no model at {}".format(landed)
    assert os.path.getsize(landed) > 0


@case
def a_refused_export_surfaces_the_doors_own_report_and_writes_nothing():
    """The other half of an adapter: what a refusal looks like from the GUI.

    The trio carries a modifier, so the door's L1 pass refuses it — a real defect refused by the
    real law, not an injected exit code. What must reach the artist is the door's own report text
    and the stage it stopped at, and what must NOT happen is a model appearing at the tracked path.
    """
    glb = open_a_trio("adapter-refusal", "modifier")
    assert not os.path.isfile(glb), "the fixture wrote a model before the door ran"
    try:
        addon.export_open_blend(_ROOT)
    except addon.Refused as refusal:
        assert refusal.stage == "source", "the door's own stage did not reach the popup: {}".format(
            refusal.stage
        )
        message = str(refusal)
        assert "L1.MODIFIER_STACK" in message, \
            "the door's report did not reach the popup:\n{}".format(message)
        assert "system console" in message, "the popup did not point at the complete report"
    else:
        raise AssertionError("a source the L1 pass refuses was exported")
    assert not os.path.isfile(glb), "a refused export wrote the tracked model"


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
    print("\ntest_addon ▸ {} cases, {} passed, {} failed".format(
        len(CASES), len(CASES) - len(failed), len(failed)
    ))
    if failed:
        print("failed: {}".format(", ".join(failed)))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(run_cases())
