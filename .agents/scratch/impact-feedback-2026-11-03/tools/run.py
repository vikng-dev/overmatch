#!/usr/bin/env python3
"""Throwaway: run the impact stills' capture plans, hidden, muted and unattended, one binary at a
time under the machine lock, turning each frame into JPEG. Skips a run whose frames exist.
Usage: run.py <bay> <out> <raw> [stem substring ...]"""
import glob
import json
import os
import subprocess
import sys
import time

BAY, OUT, RAW = sys.argv[1:4]
FILTERS = sys.argv[4:]
SPEC = json.load(open(f"{OUT}/plans/runs.json"))
BIN = os.environ.get("STILLS_BIN", f"{BAY}/target/debug/overmatch")
os.makedirs(f"{RAW}/logs", exist_ok=True)
frames = len(SPEC["times"])
# A chunk holds the machine lock for about one compile's time, then lets the other bays in.
LIMIT = int(os.environ.get("STILLS_LIMIT", "8"))
ORDER = {"g500": 0, "t500": 1, "g1000": 2}
done = 0
for run in sorted(SPEC["runs"], key=lambda r: ORDER[r["stem"].split(".")[-1]]):
    stem = run["stem"]
    if FILTERS and not any(f in stem for f in FILTERS):
        continue
    if len(glob.glob(f"{RAW}/{stem}-*.jpg")) == frames:
        continue
    if done == LIMIT:
        print("CHUNK-DONE", flush=True)
        sys.exit(3)
    done += 1
    env = dict(os.environ)
    env.update(run["env"])
    env.update(
        OVERMATCH_UNATTENDED="1",
        OVERMATCH_WINDOW="hidden",
        OVERMATCH_MUTE="1",
        OVERMATCH_CAPTURE=run["plan"],
    )
    started = time.time()
    with open(f"{RAW}/logs/{stem}.log", "w") as log:
        code = subprocess.call(
            [BIN, "--offline"],
            cwd=BAY,
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
    pngs = sorted(glob.glob(f"{RAW}/{stem}-*.png"))
    for png in pngs:
        subprocess.check_call(["magick", png, "-quality", "92", png[:-4] + ".jpg"])
        os.remove(png)
    print(f"{time.strftime('%H:%M:%S')} {stem}: exit {code}, {len(pngs)} frames, {time.time() - started:.0f} s", flush=True)
print("RUNS-DONE", flush=True)
