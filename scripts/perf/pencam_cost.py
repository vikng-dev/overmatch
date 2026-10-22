#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""The pen cam's on-screen frame cost from one capture: frames while a surface is on screen
against frames while no surface is, on the same clock; and, for a paired `-off` run, its frames in
the `-on` run's windows against the `-on` run's (the workload with and without the surface).

Inputs are a `SPIKE_FRAME_COST` stream (src/frame_cost.rs: `{t, frame_ms}` rows on `Time<Real>`)
and the same client's log, which carries the marks the pen cam writes on that clock
(src/damage_cam/pen_cam/game.rs `log_presence`, game/killcam.rs `log_run`; `debug!` level, so the
capture runs the client with `RUST_LOG=...,overmatch::damage_cam::pen_cam=debug`):

    pen cam: the slot shows <Kind> on <Surface> at t=<s> s (Time<Real>)
    pen cam: the slot shows nothing at t=<s> s (Time<Real>)
    kill cam: the run starts at t=<s> s (Time<Real>)
    kill cam: the run ends at t=<s> s (Time<Real>)

A mark opens a segment that the next mark closes; a segment is classed `corner` (the shooter's
kinds on the Corner surface), `glass` (the victim's kind, Victim), `lesson` (any kind on the World
surface: the kill cam's lesson, the full-viewport base look) or `killcam` (the run, which holds
the lesson plus its hit and killer beats). A surface's `off` frames are the ones with NO class on
screen (two surfaces back to back would otherwise be each other's baseline).

A frame row stamped `t` carries the frame that ENDED at that Update (`Time<Real>::delta`), while
a mark at `t` is the state written during that Update, first drawn by the frame that ends at the
next row: a window is therefore `(start, end]` over row stamps, and the cut keeps the row at the
cut itself.
`--windows-from <log>` takes the segments from ANOTHER run's log — the paired `-off` condition,
whose scripted schedule puts the same workload at the same seconds without the surface; that
alignment is DERIVED from the schedule, not measured, and the output says so. `--paired-json`
then adds the `paired` row: the `-on` run's window against this run's, the surface's cost over
the same workload, carrying the two runs' baseline drift (the `off` rows show it).

Validity is the frame sweep's and is NOT re-checked here: run `analyze.py --validate-only` on the
stream first (the runner does). Percentiles are `analyze.py`'s `pct`, so the numbers compare with
the sweep's. A requested surface with no segment is a failed capture, not a zero.

Usage:
    uv run scripts/perf/pencam_cost.py --stream out/corner-on.client.jsonl --log out/corner-on.log \
        --surface corner [--surface glass] [--windows-from out/corner-on.log] [--warmup-s 5] \
        [--json out/corner-on.cost.json]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import NamedTuple

from analyze import pct

SHOWS = re.compile(
    r"pen cam: the slot shows (?P<kind>[A-Za-z]+) on (?P<surface>[A-Za-z]+) at t=(?P<t>[0-9.]+) s"
)
CLEARS = re.compile(r"pen cam: the slot shows nothing at t=(?P<t>[0-9.]+) s")
RUN = re.compile(r"kill cam: the run (?P<edge>starts|ends) at t=(?P<t>[0-9.]+) s")
GLASS = re.compile(r"pen cam: the x-ray is (?P<edge>on|off) at t=(?P<t>[0-9.]+) s")

# The slot's kinds by surface class. The shooter's corner frames (src/damage_cam/pen_cam/timeline.rs
# `CornerKind`): Glass is the penetration frame, Slice the non-penetration, Ricochet and Damaged the
# rest; Victim is the victim frame's drawing mode, which the slot no longer runs. The x-ray
# (game/xray.rs) draws beside the slot and writes its own marks (`xray::log_presence`).
CORNER_KINDS = frozenset({"Glass", "Slice", "Ricochet", "Damaged"})
GLASS_KINDS = frozenset({"Victim"})
SURFACES = ("corner", "glass", "lesson", "killcam")
STATS = (("p50", 50), ("p95", 95), ("p99", 99), ("worst", 100))


class Mark(NamedTuple):
    t: float
    # One of: corner, glass, lesson, none (the slot); run-start, run-end (the kill cam);
    # glass-on, glass-off (the x-ray).
    label: str


class Segment(NamedTuple):
    start: float
    end: float


def classify(kind: str, surface: str) -> str:
    if surface == "World":
        return "lesson"
    if surface != "Corner":
        raise ValueError(f"unknown surface {surface!r} — the slot draws on Corner or World")
    if kind in CORNER_KINDS:
        return "corner"
    if kind in GLASS_KINDS:
        return "glass"
    raise ValueError(f"unknown corner kind {kind!r} — add it to CORNER_KINDS or GLASS_KINDS")


def parse_marks(lines) -> list[Mark]:
    """Every mark in log order; a log without marks yields an empty list."""
    marks: list[Mark] = []
    for line in lines:
        if m := SHOWS.search(line):
            marks.append(Mark(float(m["t"]), classify(m["kind"], m["surface"])))
        elif m := CLEARS.search(line):
            marks.append(Mark(float(m["t"]), "none"))
        elif m := RUN.search(line):
            marks.append(Mark(float(m["t"]), "run-start" if m["edge"] == "starts" else "run-end"))
        elif m := GLASS.search(line):
            marks.append(Mark(float(m["t"]), f"glass-{m['edge']}"))
    return marks


# Marks on one stamp come from one frame, where the slot's and the kill cam's loggers have no
# mutual order: a replay opens the run straight on its lesson, so `lesson(t)` may be logged before
# `run-start(t)`. Equal stamps are reconciled by rank, not log order (the sort is stable otherwise).
RANK = {
    "run-start": 0,
    "glass-on": 0,
    "corner": 1,
    "glass": 1,
    "lesson": 1,
    "none": 2,
    "run-end": 3,
    "glass-off": 3,
}


def segments(marks: list[Mark], end_t: float) -> dict[str, list[Segment]]:
    """Each class's on-screen segments. A slot mark closes the previous slot segment and opens its
    own; `none` only closes. The run's start and end bracket a killcam segment; a lesson lives only
    inside a run (the run's end closes it, one outside a run is a contract break). The x-ray's
    on and off bracket a glass segment, beside the slot's. Anything still open at `end_t` (the
    stream's last frame) closes there."""
    out: dict[str, list[Segment]] = {s: [] for s in SURFACES}
    slot: Mark | None = None
    run_start: float | None = None
    glass_start: float | None = None
    marks = sorted(marks, key=lambda mark: (mark.t, RANK[mark.label]))

    def close_slot(at: float) -> None:
        nonlocal slot
        if slot is not None and at > slot.t:
            out[slot.label].append(Segment(slot.t, at))
        slot = None

    def close_glass(at: float) -> None:
        nonlocal glass_start
        if glass_start is not None and at > glass_start:
            out["glass"].append(Segment(glass_start, at))
        glass_start = None

    for mark in marks:
        if mark.label == "run-start":
            run_start = mark.t
        elif mark.label == "run-end":
            if run_start is not None and mark.t > run_start:
                out["killcam"].append(Segment(run_start, mark.t))
            run_start = None
            if slot is not None and slot.label == "lesson":
                close_slot(mark.t)
        elif mark.label == "glass-on":
            glass_start = mark.t
        elif mark.label == "glass-off":
            close_glass(mark.t)
        elif mark.label == "none":
            close_slot(mark.t)
        else:
            if mark.label == "lesson" and run_start is None:
                raise ValueError(f"a World showing at t={mark.t} s outside a kill cam run")
            close_slot(mark.t)
            slot = mark
    close_slot(end_t)
    close_glass(end_t)
    if run_start is not None and end_t > run_start:
        out["killcam"].append(Segment(run_start, end_t))
    return out


def frames(path: Path) -> list[tuple[float, float]]:
    """`(t, frame_ms)` per frame row, in stream order. Non-frame rows (occlusion, monitor, surface)
    are skipped. Exactly one unparseable row is forgiven and only as the LAST line — the half-written
    row a killed capture leaves — as `analyze.py` forgives it; anything else is a corrupt stream."""
    rows: list[tuple[float, float]] = []
    lines = path.read_text().splitlines()
    for i, line in enumerate(lines):
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            if i == len(lines) - 1:
                break
            sys.exit(f"{path}:{i + 1}: unparseable row in the middle of the stream")
        if "frame_ms" not in row:
            continue
        t = float(row["t"])
        if rows and t < rows[-1][0]:
            sys.exit(f"{path}:{i + 1}: time runs backwards ({t} after {rows[-1][0]})")
        rows.append((t, float(row["frame_ms"])))
    if not rows:
        sys.exit(f"{path}: no frame rows")
    return rows


def split(
    rows: list[tuple[float, float]],
    windows: list[Segment],
    from_t: float,
    to_t: float | None = None,
    *,
    busy: list[Segment] | None = None,
) -> tuple[list[float], list[float]]:
    """Frame rows stamped after `from_t` and up to `to_t`: inside any window (`(start, end]`, see
    the module doc), and outside every `busy` segment (every class's windows; default:
    `windows`). A row inside a busy segment that is not a window counts on neither side."""
    busy = windows if busy is None else busy
    inside: list[float] = []
    outside: list[float] = []
    for t, ms in rows:
        if t < from_t or (to_t is not None and t > to_t):
            continue
        if any(w.start < t <= w.end for w in windows):
            inside.append(ms)
        elif not any(w.start < t <= w.end for w in busy):
            outside.append(ms)
    return inside, outside


def clip(windows: list[Segment], to_t: float | None) -> list[Segment]:
    """The windows cut at `to_t`; one starting there or later is gone."""
    if to_t is None:
        return windows
    return [Segment(w.start, min(w.end, to_t)) for w in windows if w.start < to_t]


def stats(ms: list[float]) -> dict[str, float | int] | None:
    if not ms:
        return None
    out: dict[str, float | int] = {"n": len(ms), "mean": sum(ms) / len(ms)}
    for name, q in STATS:
        out[name] = pct(ms, q)
    return out


def fmt(s: dict[str, float | int] | None) -> str:
    if s is None:
        return f"{'-':>6} {'-':>7} {'-':>7} {'-':>7} {'-':>7} {'-':>7}"
    return (
        f"{s['n']:>6} {s['p50']:>7.2f} {s['mean']:>7.2f} {s['p95']:>7.2f} "
        f"{s['p99']:>7.2f} {s['worst']:>7.2f}"
    )


def fmt_delta(a: dict[str, float | int], b: dict[str, float | int]) -> str:
    return (
        f"{a['p50'] - b['p50']:>+7.2f} {a['mean'] - b['mean']:>+7.2f} {a['p95'] - b['p95']:>+7.2f} "
        f"{a['p99'] - b['p99']:>+7.2f} {a['worst'] - b['worst']:>+7.2f}"
    )


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--stream", type=Path, required=True, help="the client's SPIKE_FRAME_COST stream")
    ap.add_argument("--log", type=Path, required=True, help="the same client's log (the marks)")
    ap.add_argument(
        "--surface", action="append", choices=SURFACES, required=True,
        help="a surface to report; each one must have at least one segment",
    )
    ap.add_argument(
        "--windows-from", type=Path,
        help="take the segments from this other run's log (the paired -on run) instead of --log",
    )
    ap.add_argument(
        "--paired-json", type=Path,
        help="the paired -on run's --json output: adds its window against this run's (DERIVED)",
    )
    ap.add_argument("--warmup-s", type=float, default=5.0, help="frames before this are discarded")
    ap.add_argument(
        "--until-s", type=float, default=None,
        help="frame rows stamped after this second are discarded and windows clipped to it: where "
        "the workload changes class (the death screen after a knockout)",
    )
    ap.add_argument("--label", default=None, help="a name for the table (default: the stream's basename)")
    ap.add_argument("--json", type=Path, help="write the numbers here too")
    args = ap.parse_args()

    rows = frames(args.stream)
    t_first, t_last = rows[0][0], rows[-1][0]
    marks_log = args.windows_from or args.log
    marks = parse_marks(marks_log.read_text(errors="replace").splitlines())
    segs = {s: clip(w, args.until_s) for s, w in segments(marks, t_last).items()}
    own_marks = parse_marks(args.log.read_text(errors="replace").splitlines()) if args.windows_from else marks
    label = args.label or args.stream.name.removesuffix(".client.jsonl").removesuffix(".jsonl")
    derived = args.windows_from is not None
    busy = [w for ws in segs.values() for w in ws]
    paired = json.loads(args.paired_json.read_text())["surfaces"] if args.paired_json else None

    missing = [s for s in args.surface if not segs[s]]
    if missing:
        sys.exit(
            f"{label}: no {', '.join(missing)} segment in {marks_log} — the surface never came on "
            "screen (or the client ran without the pen cam debug marks); nothing to measure"
        )
    if derived and any(parse_marks_has(own_marks, s) for s in args.surface):
        sys.exit(
            f"{label}: --windows-from given, but {args.log} carries its own marks for a requested "
            "surface — an -off run that showed the surface is not an off run"
        )

    from_t = t_first + args.warmup_s
    source = (
        f"DERIVED from {marks_log} (the paired run; same schedule)"
        if derived
        else "MEASURED from this run's marks"
    )
    until = f", until {args.until_s:g} s" if args.until_s is not None else ""
    print(
        f"{label}: {len(rows)} frames over {t_last - t_first:.1f} s, warmup {args.warmup_s:g} s"
        f"{until}; windows {source}"
    )
    print(f"{'surface':<8} {'state':<5} {'n':>6} {'p50':>7} {'mean':>7} {'p95':>7} {'p99':>7} {'worst':>7}  on-screen")
    result: dict[str, object] = {
        "label": label,
        "stream": str(args.stream),
        "windows_from": str(marks_log),
        "windows_derived": derived,
        "warmup_s": args.warmup_s,
        "until_s": args.until_s,
        "span_s": t_last - t_first,
        "surfaces": {},
    }
    for surface in args.surface:
        windows = segs[surface]
        inside, outside = split(rows, windows, from_t, args.until_s, busy=busy)
        on, off = stats(inside), stats(outside)
        total = sum(w.end - w.start for w in windows)
        print(f"{surface:<8} {'on':<5} {fmt(on)}  {len(windows)} segments, {total:.1f} s")
        print(f"{surface:<8} {'off':<5} {fmt(off)}")
        if on and off:
            print(f"{surface:<8} {'delta':<5} {'':>6} {fmt_delta(on, off)}")
        entry: dict[str, object] = {
            "segments": [list(w) for w in windows],
            "on_screen_s": total,
            "on": on,
            "off": off,
        }
        if paired is not None and on:
            on_run = paired[surface]["on"]
            print(f"{surface:<8} {'paired':<5} {'':>6} {fmt_delta(on_run, on)}  on-run window vs this run's (DERIVED)")
            entry["paired"] = {"on_run": on_run, "delta": {k: on_run[k] - on[k] for k, _ in STATS}}
        result["surfaces"][surface] = entry  # type: ignore[index]
    seen = [s for s in SURFACES if s not in args.surface and segs[s]]
    if seen:
        print(f"also on screen, not requested: {', '.join(seen)}")
    if args.json:
        args.json.write_text(json.dumps(result, indent=2) + "\n")
    return 0


def parse_marks_has(marks: list[Mark], surface: str) -> bool:
    """Whether `marks` open the class at all (a run start for the kill cam, a slot mark else)."""
    if surface == "killcam":
        return any(m.label == "run-start" for m in marks)
    return any(m.label == surface for m in marks)


if __name__ == "__main__":
    sys.exit(main())
