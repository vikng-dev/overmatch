"""test_addon.py — the GUI adapter's plumbing, headless.

    /Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \\
      --python .agents/blender/test_addon.py

WHAT THIS CAN AND CANNOT PROVE. The add-on is an adapter: it prepares an environment, shows
progress, and calls the one implementation. Everything except the drawing is reachable without a
window, and that is what is measured here — registration, the operator's poll and the reason it
gives, the scope guard, the PATH preparation, the progress counter, the stock-exporter hook's
restore, and — the load-bearing one — that every door entry point the adapter calls still exists
with the shape it calls it in.

The chain itself is NOT re-proved here. `scripts/tank/test_asset_door.py` runs it end to end,
including `--from-raw`, which is the exact path this adapter takes.

GUI-ONLY, and left to a human: the popup and the cursor percentage actually appearing on screen,
and the File ▸ Export menu entry being clickable. Both are drawing.
"""

import os
import sys
import tempfile
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
def the_progress_counter_reads_the_encoder_lines_and_returns_the_exit_code():
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
        code = addon.run_streamed([sys.executable, "-c", script], _WORK)
    finally:
        addon.Progress.update = original
    assert code == 3, "the door's exit code was not returned: {}".format(code)
    assert ticks == [50.0, 100.0], ticks


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


@case
def the_hook_stands_aside_while_the_door_itself_exports():
    _root, glb = fake_tree("stock-suppressed")
    addon._SUPPRESS_HOOK = True
    try:
        assert _hook_over(glb) == b"whatever the dialog held", \
            "the hook judged the door's own export"
    finally:
        addon._SUPPRESS_HOOK = False


# ── the coupling to the one implementation ───────────────────────────────────────────────────────

@case
def every_door_entry_point_the_adapter_calls_still_exists():
    """The whole risk of an adapter: the thing it adapts moves and nothing notices until a human
    exports. Each name below is called by `export_open_blend`."""
    door = addon.load(_ROOT, addon.DOOR_RELPATH, "test_asset_door_module")
    findings, blender = door.preflight("lint", launches_blender=False)
    assert blender is None, "a continuation asked for a Blender it does not launch"
    assert findings == [], findings
    assert callable(door.canon_file) and callable(door.derive)
    assert issubclass(door.Refused, Exception)
    parsed = door.parse(["export", "x.blend", "--from-raw", "r.glb"])
    assert parsed.from_raw == "r.glb", parsed

    source = addon.load(_ROOT, addon.SOURCE_PASS_RELPATH, "test_export_tank_module")
    assert callable(source.run)
    # The context `L1.SAVED_SOURCE` splits on. The adapter names IN_SESSION at the call, so the
    # constant disappearing must be a failure here rather than a NameError in front of an artist.
    assert source.IN_SESSION != source.FRESH, "the two L1.SAVED_SOURCE contexts are one"
    for name in ("render_text", "sorted_findings", "summary", "has_error", "Severity"):
        assert hasattr(source.report, name), "report.{} is gone".format(name)
        assert hasattr(door.report, name), "report.{} is gone".format(name)


@case
def the_adapter_exports_the_open_blend_through_the_door():
    """The whole adapter, on the synthetic trio, in this Blender — which is what a GUI export is
    minus the drawing. The fixture is built in-process, so what `export_open_blend` saves and hands
    over is the file this Blender has open, exactly as it would be for an artist.

    The bytes are the door's own claim and are proved in `scripts/tank/test_asset_door.py`; what is
    proved here is that the adapter's four calls into it still compose into a certified export.
    """
    sys.path.insert(0, _HERE)
    import fixture_tank  # noqa: PLC0415 — the trio builder, only needed by this case

    directory = os.path.join(_WORK, "adapter-export")
    library = fixture_tank.write_library(
        os.path.join(directory, "assets", "materials", "materials.blend")
    )
    asset = os.path.join(directory, "assets", "testbed")
    os.makedirs(asset, exist_ok=True)
    fixture_tank.build(asset, library, "none")
    assert bpy.data.filepath.endswith("testbed.blend"), bpy.data.filepath

    addon._prepare_env()
    glb = addon.export_open_blend(_ROOT)
    assert os.path.isfile(glb), "the door certified but wrote no model at {}".format(glb)
    assert os.path.getsize(glb) > 0


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
