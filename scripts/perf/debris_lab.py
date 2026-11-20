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
import re
import shutil
import subprocess
import sys
from pathlib import Path

from collapse import BIN, ROOT, percentile

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
        binaries={name: sha256(BIN / name) for name in ("overmatch-server", "overmatch")},
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


def label(out, frames, recipe, name, revision):
    """Burn the run's name, revision, frame time and the timing caveat into each frame."""
    labelled = out / "labelled"
    labelled.mkdir()
    for frame, at in frames:
        text = (f"{name}  {revision[:10]}  {recipe['instance']} heading {recipe['heading_deg']} deg  "
                f"t {at - frames[0][1]:.3f} s  tick cost: {TIMING}")
        subprocess.run(["magick", str(frame), "-font", str(FONT), "-gravity", "north", "-fill", "white",
                        "-undercolor", "#000000a0", "-pointsize", "24", "-annotate", "+0+12", text,
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
    encode(label(out, frames, result["recipe"], out.name, record["revision"]), rate(frames), clip)
    record["clip"] = layout(clip)
    if record["clip"]["frames"] != len(frames):
        refuse(f"{clip} holds {record['clip']['frames']} frames of {len(frames)}")
    (out / "run.json").write_text(json.dumps(record, indent=2) + "\n")
    structure = result["structure"]
    print(f"debris-lab: {out.name}: {result['end']} after {result['window_s']:.3f} s; in place "
          f"{structure['in_place_share']:.3f}, mean displacement {structure['displacement_m']} m, opening "
          f"{structure['opening_m2']:.2f} m2, {len(structure['displaced'])} chunks displaced; clip {clip}")


def onset(result, run):
    """The capture tick from which the server parsed a nonzero drive: where the run's input begins."""
    tick = next((t["tick"] for t in result["ticks"] if any(t["hull"]["drive"])), None)
    if tick is None:
        refuse(f"{run} never drove; a ram run holds its throttle")
    if tick == result["ticks"][0]["tick"]:
        refuse(f"{run} drove from its first captured tick; no rest precedes its input")
    return tick


def first_difference(pairs, keys):
    """The first `(key, field)` at which the paired hull rows differ in `keys`; None when equal."""
    for key, (a, b) in pairs:
        for field in keys(a):
            if a[field] != b[field]:
                return key, f"hull.{field}"
    return None


def verdict(a, b):
    """Two runs keyed from their input onsets (the first tick each server parsed a nonzero drive),
    and at the capture's end by construction and chunk, never by entity. Over UDP each run's input
    arrives on its own tick; the keying holds while the hull rests before it: the rows agree tick
    for tick from spawn, and the later run held the earlier one's last row until its own onset.
    Otherwise the state differs before input. After the onset, over the ticks both runs hold,
    inputs first, then state: equal; inputs differ (outside input, not a replay); or state differs,
    where the same inputs gave different state, at a field or at an unequal end (`last_tick`)."""
    onsets = [onset(a, "a"), onset(b, "b")]
    rows = [{t["tick"]: t["hull"] for t in r["ticks"]} for r in (a, b)]
    found = dict(onsets=onsets)
    resting = ((tick, [r[min(tick, o - 1)] for r, o in zip(rows, onsets)])
               for tick in range(min(rows[0]), max(onsets)))
    before = first_difference(resting, lambda row: sorted(row))
    if before:
        return dict(verdict="state differs before input", tick=before[0], field=before[1], **found)
    after = [{t - o: hull for t, hull in r.items() if t >= o} for r, o in zip(rows, onsets)]
    shared = [(k, (after[0][k], after[1][k])) for k in sorted(after[0].keys() & after[1].keys())]
    inputs = first_difference(shared, lambda row: ["drive"])
    if inputs:
        return dict(verdict="inputs differ", after_onset=inputs[0], **found)
    state = first_difference(shared, lambda row: sorted(k for k in row if k != "drive"))
    if state:
        return dict(verdict="state differs", after_onset=state[0], field=state[1], **found)
    ends = [r["last_tick"] - o for r, o in zip((a, b), onsets)]
    if ends[0] != ends[1]:
        return dict(verdict="state differs", after_onset=min(ends), field="last_tick", **found)
    displaced = [set(r["structure"]["displaced"]) for r in (a, b)]
    if displaced[0] != displaced[1]:
        construction, chunk = a["structure"]["construction"], min(displaced[0] ^ displaced[1])
        return dict(verdict="state differs", after_onset=ends[0],
                    field=f"construction {construction} chunk {chunk}", **found)
    for key in sorted(a["structure"].keys() | b["structure"].keys()):
        if a["structure"].get(key) != b["structure"].get(key):
            return dict(verdict="state differs", after_onset=ends[0], field=f"structure.{key}", **found)
    return dict(verdict="equal", **found)


def admit(records):
    """Two runs compare only under one recipe, plan and clip layout."""
    for field in ("recipe", "plan_sha256", "clip"):
        if records[0][field] != records[1][field]:
            refuse(f"runs differ in {field}: {records[0][field]} against {records[1][field]}")


def timing(result):
    """The contact window's tick times; p99 and the worst are result.json's own."""
    window = sorted(t["tick_us"] for t in result["ticks"] if t["tick"] >= result["first_tick"])
    return dict(mean_us=result["tick_mean_us"], p50_us=percentile(window, 0.5),
                p95_us=percentile(window, 0.95), p99_us=result["p99_us"], max_us=result["worst_tick"]["tick_us"])


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
