#!/usr/bin/env python3
"""The building-debris model's lab (game state only): a ram pass filmed from the driving client,
and two passes compared.

  scripts/debris-lab run RECIPE OUT        one ram run (collapse.py --ram) with a labelled clip
  scripts/debris-lab compare A B OUT       two runs: a replay verdict and a side-by-side clip

Tick cost is comparison only: every run is an unpriced local rehearsal whose client renders on the
server's machine, so its times compare only with another run of the same plan on the same machine.
Hosted cost is collapse.sh --ram without a clip (scripts/perf/collapse.md)."""

import argparse
import hashlib
import json
import math
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PLAN = ROOT / "scripts/capture/debris-lab.ron"
FONT = ROOT / "assets/fonts/BarlowCondensed-SemiBold.ttf"
TOOLS = ("ffmpeg", "ffprobe", "magick")
TIMING = "comparison only, same machine, same plan"


def refuse(why):
    raise SystemExit(f"debris-lab: {why}")


def check_tools():
    missing = [tool for tool in TOOLS if shutil.which(tool) is None]
    if missing:
        refuse(f"missing {', '.join(missing)}; install them (brew install ffmpeg imagemagick)")


def check_power():
    if sys.platform == "darwin" and re.search(r"lowpowermode\s+1", run_text(["pmset", "-g"])):
        refuse("low-power mode is on; its clocks make tick times incomparable")


def run_text(command, **kwargs):
    return subprocess.run(command, check=True, capture_output=True, text=True, **kwargs).stdout


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def one_line(text, marker, log):
    lines = [line for line in text.splitlines() if marker in line]
    if len(lines) != 1:
        refuse(f"{log}: expected one {marker!r} line, found {len(lines)}")
    return lines[0].split(marker, 1)[1].strip()


def provenance(out, result):
    """What made the run: the build, the binaries, the recipe as the server echoed it, the plan."""
    server, client = (out / "server.log").read_text(), (out / "client.log").read_text()
    builds = {role: one_line(text, f"{role}: build: ", f"{role}.log")
              for role, text in (("server", server), ("client", client))}
    if builds["server"] != builds["client"]:
        refuse(f"server and client builds differ: {builds}")
    return dict(
        revision=run_text(["git", "rev-parse", "HEAD"], cwd=ROOT).strip(),
        tracked_diff_sha256=hashlib.sha256(run_text(["git", "diff", "HEAD", "--binary"], cwd=ROOT).encode()).hexdigest(),
        binaries={name: sha256(ROOT / "target/debug" / name) for name in ("overmatch-server", "overmatch")},
        build=builds["server"],
        recipe=result["recipe"],
        plan_sha256=sha256(PLAN),
        host="UNPRICED LOCAL REHEARSAL: " + one_line(server, "server: host ", "server.log"),
        timing=TIMING,
    )


def frames_of(out):
    """The run's frames in time order, each with its plan time (the stamp dev_capture names it by)."""
    frames = sorted((out / "frames").glob("*.png"))
    if len(frames) < 2:
        refuse(f"{out}/frames holds {len(frames)} frames; the plan takes a sequence")
    return [(frame, float(re.search(r"-(\d+\.\d+)s\.png$", frame.name)[1])) for frame in frames]


def rate(frames):
    """Frames a second: the plan's spacing, which must be even for a clip to play at it."""
    gaps = {round(b - a, 4) for (_, a), (_, b) in zip(frames, frames[1:])}
    if len(gaps) != 1:
        refuse(f"frame times are unevenly spaced: {sorted(gaps)} s")
    return 1 / gaps.pop()


def label(out, name, revision):
    """Burn the run's name, revision, frame time and the timing caveat into each frame."""
    recipe = json.loads((out / "result.json").read_text())["recipe"]
    labelled = out / "labelled"
    labelled.mkdir()
    frames = frames_of(out)
    for frame, at in frames:
        text = (f"{name}  {revision[:10]}  {recipe['instance']} heading {recipe['heading_deg']} deg  "
                f"t {at - frames[0][1]:.3f} s  tick cost: comparison only")
        subprocess.run(["magick", str(frame), "-font", str(FONT), "-gravity", "north", "-fill", "white",
                        "-undercolor", "#000000a0", "-pointsize", "28", "-annotate", "+0+12", text,
                        str(labelled / frame.name)], check=True)
    return labelled


def encode(frames_dir, fps, clip):
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-framerate", str(fps), "-pattern_type", "glob",
                    "-i", str(frames_dir / "*.png"), "-c:v", "libx264", "-pix_fmt", "yuv420p", str(clip)],
                   check=True)


def layout(clip):
    """The clip's frame count, width and height, read back from the encoded file."""
    probe = json.loads(run_text(["ffprobe", "-v", "error", "-count_frames", "-select_streams", "v:0",
                                 "-show_entries", "stream=nb_read_frames,width,height", "-of", "json", str(clip)]))
    stream = probe["streams"][0]
    return dict(frames=int(stream["nb_read_frames"]), width=stream["width"], height=stream["height"])


def run(args):
    check_tools()
    check_power()
    out = args.out.resolve()
    if out.exists() and any(out.iterdir()):
        refuse(f"{out} is not empty; give each run its own folder")
    status = subprocess.run([str(ROOT / "scripts/perf/collapse.sh"), "--ram", str(args.recipe.resolve()),
                             "--rehearsal", "--clip", str(PLAN), "--out", str(out)], cwd=ROOT).returncode
    if status:
        refuse(f"the ram run failed (status {status}); logs: {out}")
    result = json.loads((out / "result.json").read_text())
    record = provenance(out, result)
    clip = out / "clip.mp4"
    frames = frames_of(out)
    encode(label(out, out.name, record["revision"]), rate(frames), clip)
    record["clip"] = layout(clip)
    if record["clip"]["frames"] != len(frames):
        refuse(f"{clip} holds {record['clip']['frames']} frames of {len(frames)}")
    (out / "run.json").write_text(json.dumps(record, indent=2) + "\n")
    structure = result["structure"]
    print(f"debris-lab: {out.name}: {result['end']} after {result['window_s']:.3f} s; in place "
          f"{structure['in_place_share']:.3f}, opening {structure['opening_m2']:.2f} m2, "
          f"{len(structure['displaced'])} chunks displaced; clip {clip}")


def first_difference(a, b, keys):
    """The first tick at which `keys` of the hull rows differ, and the field; None when equal."""
    rows_a, rows_b = ({t["tick"]: t["hull"] for t in r["ticks"]} for r in (a, b))
    for tick in sorted(rows_a.keys() | rows_b.keys()):
        if tick not in rows_a or tick not in rows_b:
            return tick, "tick"
        for key in keys(rows_a[tick]):
            if rows_a[tick][key] != rows_b[tick][key]:
                return tick, f"hull.{key}"
    return None


def verdict(a, b):
    """Inputs first, then state, keyed by capture tick and field and, at the capture's end, by
    construction and chunk, never by entity: equal; inputs differ at tick N (outside input, not a
    replay); or state differs at tick M, field F (an order dependence on the server)."""
    inputs = first_difference(a, b, lambda row: ["drive"])
    if inputs:
        return dict(verdict="inputs differ", tick=inputs[0])
    state = first_difference(a, b, lambda row: sorted(k for k in row if k != "drive"))
    if state:
        return dict(verdict="state differs", tick=state[0], field=state[1])
    end = min(a["last_tick"], b["last_tick"])
    displaced = [set(r["structure"]["displaced"]) for r in (a, b)]
    if displaced[0] != displaced[1]:
        construction, chunk = a["structure"]["construction"], min(displaced[0] ^ displaced[1])
        return dict(verdict="state differs", tick=end, field=f"construction {construction} chunk {chunk}")
    for key in sorted(a["structure"].keys() | b["structure"].keys()):
        if a["structure"].get(key) != b["structure"].get(key):
            return dict(verdict="state differs", tick=end, field=f"structure.{key}")
    return dict(verdict="equal")


def admit(records):
    """Two runs compare only under one recipe, plan and clip layout."""
    for field in ("recipe", "plan_sha256", "clip"):
        if records[0][field] != records[1][field]:
            refuse(f"runs differ in {field}: {records[0][field]} against {records[1][field]}")


def timing(result):
    window = sorted(t["tick_us"] for t in result["ticks"] if t["tick"] >= result["first_tick"])
    pick = lambda q: window[min(len(window) - 1, math.ceil(q*len(window)) - 1)]
    return dict(mean_us=sum(window)/len(window), p50_us=pick(0.5), p95_us=pick(0.95), p99_us=pick(0.99),
                max_us=window[-1])


def compare(args):
    check_tools()
    runs = [path.resolve() for path in (args.a, args.b)]
    records = [json.loads((run / "run.json").read_text()) for run in runs]
    admit(records)
    results = [json.loads((run / "result.json").read_text()) for run in runs]
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    found = verdict(*results)
    clip = out / "compare.mp4"
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(runs[0] / "clip.mp4"), "-i",
                    str(runs[1] / "clip.mp4"), "-filter_complex", "hstack=inputs=2", "-c:v", "libx264",
                    "-pix_fmt", "yuv420p", str(clip)], check=True)
    comparison = dict(runs=[str(run) for run in runs], revisions=[r["revision"] for r in records],
                      **found, timing=TIMING, tick_us={run.name: timing(r) for run, r in zip(runs, results)},
                      clip=str(clip))
    (out / "comparison.json").write_text(json.dumps(comparison, indent=2) + "\n")
    print(f"debris-lab: {found}; tick times {TIMING}; clip {clip}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    one = commands.add_parser("run", help="one ram run with a labelled clip")
    one.add_argument("recipe", type=Path, help="ram recipe JSON (scripts/perf/ram.json)")
    one.add_argument("out", type=Path, help="the run's own folder")
    one.set_defaults(handler=run)
    two = commands.add_parser("compare", help="two runs: replay verdict and side-by-side clip")
    two.add_argument("a", type=Path)
    two.add_argument("b", type=Path)
    two.add_argument("out", type=Path)
    two.set_defaults(handler=compare)
    args = parser.parse_args()
    args.handler(args)


if __name__ == "__main__":
    main()
