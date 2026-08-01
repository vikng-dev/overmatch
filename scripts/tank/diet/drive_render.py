"""drive_render.py — render several glb variants through `render.py` in tex + wire mode.

    python3 drive_render.py <outdir> <texdir|-> <az,az,...> <label>=<glb> [<label>=<glb> ...]

Writes `<outdir>/<label>_<mode>_az<NNN>.png`. Environment passes straight through, so the
knobs `render.py` reads (RES, ELEV, DIST_F, MAT_COLOR, MAT_METAL, MAT_ROUGH) work here:

    DIST_F=2.9 python3 drive_render.py out link_tex 40,90,140,230 orig=a.glb lod0=b.glb

One Blender process per (variant, mode) rather than one for everything: a crash in one
variant then costs that variant, and the per-run factory reset is what keeps two variants
from sharing a half-configured scene.
"""

import os
import shutil
import subprocess
import sys

MAC_BLENDER = "/Applications/Blender.app/Contents/MacOS/Blender"
BLENDER = os.environ.get("BLENDER") or shutil.which("blender") or MAC_BLENDER
HERE = os.path.dirname(os.path.abspath(__file__))

args = sys.argv[1:]
OUT, TEX, ANGLES = args[0], args[1], args[2].split(",")

jobs = []
for spec in args[3:]:
    label, glb = spec.split("=", 1)
    for mode in ("tex", "wire"):
        jobs.append((label, glb, mode))

for label, glb, mode in jobs:
    prefix = f"{OUT}/{label}_{mode}"
    cmd = [
        BLENDER, "-b", "-P", f"{HERE}/render.py", "--",
        glb, prefix, mode, TEX if mode == "tex" else "-",
    ] + ANGLES
    r = subprocess.run(cmd, capture_output=True, text=True)
    ok = [ln for ln in r.stdout.splitlines() if "[render]" in ln]
    print(f"{label}/{mode}: {len(ok)} frames")
    if not ok:
        print(r.stdout[-2000:], r.stderr[-2000:])
