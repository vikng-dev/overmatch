"""drive_bf.py — back-face-probe a list of glbs and report the red-pixel total for each.

    python3 drive_bf.py <geometry.glb> [<geometry.glb> ...]      # RES=2000 for the real run

Runs `backface.py` over 8 azimuths x 4 elevations and counts the pixels it flashed red. A
red pixel is a back face the camera can see, i.e. a pixel that back-face culling turns into
a hole, so the total IS the answer to "is `doubleSided: false` safe here". Resolution is the
whole game: at 900 px the whole-tank probe read 11 red pixels because the MG bore is
sub-pixel, and the run that decided the change was RES=2000.

Frames land in `out/bf_<stem>/` next to this script and are NOT deleted, so the worst frame
can be cropped afterwards to see WHAT was red. That step is not optional: 38 of the shipped
run's 115 red pixels were one pair of coincident faces, which reads very differently from a
hole once you look at it.

THE GATE FAILS CLOSED. This total is what authorises `doubleSided: false`, and a Blender run
that dies after 6 of 32 frames would otherwise total the 6 it managed and read as a smaller,
safer number — zero frames would read as a clean zero. So: a nonzero Blender exit, a missing
frame or a frame that is not RES x RES aborts the whole script with Blender's stderr and NO
number printed. There is no partial answer here, only an answer and an error.
"""

import glob
import os
import shutil
import subprocess
import sys

from PIL import Image

MAC_BLENDER = "/Applications/Blender.app/Contents/MacOS/Blender"
BLENDER = os.environ.get("BLENDER") or shutil.which("blender") or MAC_BLENDER
HERE = os.path.dirname(os.path.abspath(__file__))
AZ = "0,45,90,135,180,225,270,315"
EL = "35,10,-15,-40"
RES = int(os.environ.get("RES", "1000"))


def red_pixels(path):
    """Count emissive-red pixels. The probe's red is pure, so the threshold can be generous."""
    im = Image.open(path).convert("RGB")
    return sum(1 for r, g, b in im.getdata() if r > 150 and g < 80 and b < 80)


def expected_frames(prefix):
    """The exact filenames backface.py must produce, in its own naming scheme."""
    return [
        f"{prefix}_el{int(float(el)):+03d}_az{int(float(az)):03d}.png"
        for el in EL.split(",")
        for az in AZ.split(",")
    ]


def die(name, why, stderr=None):
    print(f"{name}: PROBE FAILED — {why}", file=sys.stderr)
    if stderr:
        print("--- blender stderr ---", file=sys.stderr)
        print(stderr.rstrip(), file=sys.stderr)
    raise SystemExit(2)


for name in sys.argv[1:]:
    outdir = os.path.join(HERE, "out", "bf_" + os.path.basename(name).replace(".glb", ""))
    os.makedirs(outdir, exist_ok=True)
    for f in glob.glob(outdir + "/*.png"):
        os.remove(f)
    prefix = outdir + "/p"
    r = subprocess.run(
        [BLENDER, "-b", "-P", f"{HERE}/backface.py", "--", name, prefix, AZ, EL],
        capture_output=True,
        text=True,
        env=dict(os.environ, RES=str(RES)),
    )
    if r.returncode != 0:
        die(os.path.basename(name), f"blender exited {r.returncode}", r.stderr)

    want = expected_frames(prefix)
    missing = [os.path.basename(p) for p in want if not os.path.exists(p)]
    if missing:
        die(
            os.path.basename(name),
            f"{len(missing)} of {len(want)} frames missing ({', '.join(missing[:4])}"
            f"{', ...' if len(missing) > 4 else ''})",
            r.stderr,
        )
    bad = []
    for p in want:
        with Image.open(p) as im:
            if im.size != (RES, RES):
                bad.append(f"{os.path.basename(p)} {im.size[0]}x{im.size[1]}")
    if bad:
        die(
            os.path.basename(name),
            f"{len(bad)} frames are not {RES}x{RES} ({', '.join(bad[:4])}"
            f"{', ...' if len(bad) > 4 else ''})",
            r.stderr,
        )
    stray = sorted(set(glob.glob(outdir + "/*.png")) - set(want))
    if stray:
        die(
            os.path.basename(name),
            f"{len(stray)} unexpected frames in {outdir} "
            f"({', '.join(os.path.basename(p) for p in stray[:4])})",
        )

    counts = [(red_pixels(p), os.path.basename(p)) for p in want]
    total = sum(n for n, _ in counts)
    worst = max(counts)
    print(
        f"{os.path.basename(name):<24} {len(want)} frames @ {RES}x{RES}  "
        f"red px total {total:>8}  worst {worst[1]} {worst[0]}"
    )
