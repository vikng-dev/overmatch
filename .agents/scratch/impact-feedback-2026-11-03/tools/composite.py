#!/usr/bin/env python3
"""Throwaway: compose the impact stills. One image per case and framing: top row today, bottom row
design, columns the times; each panel the whole scene over a zoom around the strike.
Usage: composite.py <out dir holding plans/> <raw> <composite dir> [case substring ...]"""
import json
import os
import re
import subprocess
import sys

OUT, RAW, DEST = sys.argv[1:4]
FILTERS = sys.argv[4:]
SPEC = json.load(open(f"{OUT}/plans/runs.json"))
FONT = "/System/Library/Fonts/Supplemental/Arial.ttf"
TMP = f"{RAW}/tmp"
os.makedirs(TMP, exist_ok=True)
os.makedirs(DEST, exist_ok=True)
PANEL_W, PANEL_H = 640, 360
VIEW = {"g500": ("gunner's sight", "500 m"), "g1000": ("gunner's sight", "1000 m"), "t500": ("third person", "500 m")}
REFERENCE = {"mud": "ap88_mud", "gravel": "ap88_gravel", "dirt": "he88_dirt"}
T0 = SPEC["t0"]


def frame(stem, t):
    return f"{RAW}/{stem}-{t:010.4f}s.jpg"


def run(*args):
    return subprocess.check_output(args, text=True)


def strike_centre(site, name):
    """Where the read is in this framing: the changed pixels of the reference case's read."""
    ref = f"{REFERENCE[site]}.design.{name}"
    before, after = frame(ref, SPEC["times"][0]), frame(ref, T0 + 1.0)
    box = run(
        "magick", before, after, "-compose", "difference", "-composite", "-colorspace", "Gray",
        "-blur", "0x1.5", "-threshold", "6%", "-format", "%@", "info:",
    )
    m = re.match(r"(\d+)x(\d+)\+(\d+)\+(\d+)", box)
    w, h, x, y = map(int, m.groups())
    print(site, name, "changed box", box, flush=True)
    # The read grows up from its contact: the crop centres on the box's lower part.
    return x + w // 2, y + int(h * 0.6)


centres = {}
for case, spec in SPEC["cases"].items():
    if FILTERS and not any(f in case for f in FILTERS):
        continue
    today_case = spec.get("today", case)
    times = [t for t in SPEC["times"][1:] if spec["flash"] or abs(t - (T0 + 0.02)) > 1e-3]
    for name in SPEC["framings"]:
        # Every framing looks at a point 3 m above the strike: the read grows from just under the centre.
        zoom = spec["zoom"]
        cx, cy = 960, 548 if zoom == 6 else 530
        cw, ch = PANEL_W // zoom, PANEL_H // zoom
        ox = max(min(cx - cw // 2, 1920 - cw), 0)
        oy = max(min(cy - ch // 2, 1080 - ch), 0)
        rows = []
        missing = False
        for arm, stem in (("TODAY", f"{today_case}.today.{name}"), ("DESIGN", f"{case}.design.{name}")):
            panels = []
            for t in times:
                src = frame(stem, t)
                if not os.path.exists(src):
                    print("missing", src, flush=True)
                    missing = True
                    break
                out = f"{TMP}/{stem}-{t:.2f}.png"
                scale = PANEL_W / 1920
                rect = (
                    f"rectangle {ox * scale:.0f},{oy * scale:.0f} "
                    f"{(ox + cw) * scale:.0f},{(oy + ch) * scale:.0f}"
                )
                subprocess.check_call([
                    "magick", src,
                    "(", "+clone", "-resize", f"{PANEL_W}x{PANEL_H}!", "-fill", "none", "-stroke", "#ffd400", "-strokewidth", "1", "-draw", rect, ")",
                    "(", "-clone", "0", "-crop", f"{cw}x{ch}+{ox}+{oy}", "+repage", "-filter", "Lanczos", "-resize", f"{PANEL_W}x{PANEL_H}!", ")",
                    "-delete", "0",
                    "-append",
                    "-font", FONT, "-pointsize", "21", "-gravity", "NorthWest",
                    "-fill", "#000000a0", "-stroke", "none", "-draw", "rectangle 0,0 280,30",
                    "-fill", "white", "-annotate", "+8+4", f"{arm}  +{t - T0:.2f} s",
                    "-gravity", "NorthWest", "-fill", "#000000a0", "-draw", f"rectangle 0,{PANEL_H} 170,{PANEL_H + 26}",
                    "-fill", "#ffd400", "-pointsize", "17", "-annotate", f"+8+{PANEL_H + 4}", f"zoom x{zoom}",
                    "-bordercolor", "#111111", "-border", "2",
                    out,
                ])
                panels.append(out)
            if missing:
                break
            row = f"{TMP}/{case}-{name}-{arm}.png"
            subprocess.check_call(["magick", *panels, "+append", row])
            rows.append(row)
        if missing:
            continue
        view, rng = VIEW[name]
        title = f"{spec['title']}  |  range {rng}  |  {view}  |  top today, bottom design  |  times after contact"
        width = (PANEL_W + 4) * len(times)
        dest = f"{DEST}/{case}.{name}.jpg"
        subprocess.check_call([
            "magick",
            "(", "-size", f"{width}x46", "xc:#111111", "-font", FONT, "-pointsize", "28", "-fill", "white", "-gravity", "West", "-annotate", "+12+0", title, ")",
            *rows,
            "-append", "-quality", "84", dest,
        ])
        print(dest, os.path.getsize(dest) // 1024, "KB", flush=True)
