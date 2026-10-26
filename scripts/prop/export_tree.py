"""export_tree.py — the Blender half of the prop build: one blend in, one view glb out.

    blender --background --factory-startup assets/trees/<id>/<id>.blend \\
        --python scripts/prop/export_tree.py -- --out <candidate.glb>

Run by `scripts/prop/build.py`, never by hand. It asserts the pinned Blender and glTF exporter
(`scripts/toolchain.py`) and exports the active scene with `EXPORT_SETTINGS`; every check of what
came out is the wrapper's, under the system interpreter.

`export_extras` is the one setting the slice contract hangs on: the custom properties the
generator put on every node (`tree_segment`, `tree_part`) reach the glb as node extras only
through it. Exit is non-zero exactly when the pin or the export refused — Blender propagates a
`sys.exit` code from a `--python` script verbatim.
"""

from __future__ import annotations

import argparse
import os
import sys

import bpy

_SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _SCRIPTS)

import toolchain  # noqa: E402  — the paths above are what make this importable

#: The exporter arguments, frozen: a promise about the pinned exporter that reads them.
EXPORT_SETTINGS = {
    "export_format": "GLB",
    "export_extras": True,
    "export_yup": True,
    "export_animations": False,
    "export_tangents": False,
    "use_active_scene": True,
}


def assert_toolchain() -> None:
    running = {
        "Blender version": (bpy.app.version_string.split()[0], toolchain.BLENDER_VERSION),
        "Blender build": (bpy.app.build_hash.decode(), toolchain.BLENDER_BUILD),
    }
    wrong = ["{} is {!r}, pinned to {!r}".format(what, actual, expected)
             for what, (actual, expected) in running.items() if actual != expected]
    exporter = toolchain.finding(toolchain.gltf_exporter())
    if exporter is not None:
        wrong.append(exporter.evidence)
    if wrong:
        raise SystemExit("export_tree: toolchain refused — " + "; ".join(wrong))


def main(argv) -> int:
    parser = argparse.ArgumentParser(prog="export_tree.py", allow_abbrev=False)
    parser.add_argument("--out", required=True, help="where the candidate glb lands")
    arguments = parser.parse_args(argv)
    assert_toolchain()
    if not bpy.data.filepath:
        raise SystemExit("export_tree: no blend is open; pass the blend before --python")
    out = os.path.abspath(arguments.out)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    result = bpy.ops.export_scene.gltf(filepath=out, **EXPORT_SETTINGS)
    if result != {"FINISHED"} or not os.path.isfile(out):
        raise SystemExit("export_tree: export_scene.gltf returned {} and {} {}".format(
            result, "wrote" if os.path.isfile(out) else "did not write", out))
    print("export_tree ▸ {} — {:.2f} MB".format(out, os.path.getsize(out) / 1e6), flush=True)
    return 0


if __name__ == "__main__":
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    try:
        code = main(argv)
    except SystemExit as error:
        print(str(error), file=sys.stderr, flush=True)
        os._exit(1)
    sys.stdout.flush()
    os._exit(code)
