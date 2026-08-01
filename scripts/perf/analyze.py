# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Summarize frame-budget sweep streams from `SPIKE_FRAME_COST` JSONL (src/frame_cost.rs).

Consumes the raw one-row-per-frame `{t, frame_ms}` streams `scripts/perf/run-frame-sweep.sh`
captures (one file per ladder condition; the recorder interleaves `{t, occluded}` rows on every
window-occlusion transition and `{t, monitor, refresh_mhz, primary}` rows whenever the presenting
monitor resolves or changes) and prints:
  * per-condition p50/p95/p99/worst frame ms, with fps-equivalents and % of the 16.67 ms
    (60 fps) and 8.33 ms (120 fps) budgets;
  * a delta table against the ladder's baseline condition.

Percentiles live HERE, never in the recorder: the product signal is tail stutter, and any
in-process averaging destroys it. Validity is enforced like the scripts/shot analyzers — a stream
that fails any gate fails LOUD, because the whole failure class this harness fights is "invalid
capture masquerades as a real measurement":
  * a missing file, or fewer post-warmup rows than --min-rows (a short stream is a crashed or
    stalled run, not evidence);
  * a non-monotonic or unparseable row (a spliced or corrupted stream) — with exactly one
    exception, the half-written final row a killed capture leaves behind (see `parse_rows`);
  * with --expected-duration-s: rows must SPAN at least warmup + duration minus 2% — row COUNT
    alone cannot tell a 60 s run from a fast free-running process that died at 10 s;
  * unless --occluded-ok: no occluded=true interval may overlap the post-warmup measurement
    window — an occluded window's present is discarded and the frame loop free-runs, so those
    frame times are fiction. (--occluded-ok exists for the runner's SMOKE mode, whose window is
    hidden BY CONSTRUCTION and whose numbers are documented fiction.)
  * unless --display-ok: the measurement window must be covered by monitor rows, every one of them
    at least MIN_REFRESH_MHZ, and all the same monitor. A 60 Hz panel paces presentation to
    multiples of 16.67 ms whatever present mode the app negotiated, so every rung of the ladder
    quantizes to the same ~60 fps while the client's own "effective present mode Immediate" line
    still passes — the app's request is not the OS's pacing. (--display-ok is the same SMOKE-mode
    escape as --occluded-ok: a hidden window resolves no monitor and has no real numbers to
    poison.)

--validate-only runs exactly these gates and prints one OK line per stream, no tables — the
runner's per-condition gate, so runner and analyzer cannot drift apart on what "valid" means.

Usage:
    uv run scripts/perf/analyze.py [--warmup-s 10] [--min-rows 400] \
        [--expected-duration-s 60] [--occluded-ok] [--display-ok] [--validate-only] \
        [--baseline m350-2] out/m350-2.client.jsonl out/off-30.client.jsonl ...

Condition names are file basenames minus `.client.jsonl`/`.jsonl`.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import NamedTuple, NoReturn

BUDGETS_MS = (16.667, 8.333)
STATS = (("p50", 50), ("p95", 95), ("p99", 99), ("worst", 100))
# Span tolerance: timers, kill scheduling and the last partial frame eat a little of the nominal
# warmup+duration window; 2% forgives that and nothing else (a run dead at 1/3 duration is ~66% short).
SPAN_TOLERANCE = 0.98
# Slowest panel this harness will measure on, in millihertz. The threshold separates CLASSES of
# display rather than trimming a margin: the built-in ProMotion panel reports 120000 and the ARZOPA
# 2560x1600 external reports 60000, and there is no legitimate capture display between them. A
# 60 Hz surface caps every condition at 16.67 ms and destroys the whole ladder, so this is a
# validity threshold, not a preference.
MIN_REFRESH_MHZ = 100_000


def fail(msg: str) -> NoReturn:
    print(f"INVALID: {msg}", file=sys.stderr)
    sys.exit(1)


def pct(xs: list[float], q: float) -> float:
    xs = sorted(xs)
    if q >= 100:
        return xs[-1]
    # linear interpolation between closest ranks (same rule as scripts/render/analyze.py)
    pos = (len(xs) - 1) * q / 100.0
    lo = int(pos)
    hi = min(lo + 1, len(xs) - 1)
    frac = pos - lo
    return xs[lo] * (1 - frac) + xs[hi] * frac


def condition_name(path: Path) -> str:
    name = path.name
    for suffix in (".client.jsonl", ".jsonl"):
        if name.endswith(suffix):
            return name[: -len(suffix)]
    return name


class MonitorRow(NamedTuple):
    """One `{t, monitor, refresh_mhz, primary}` row: which panel presented, from when.

    `refresh_mhz` is None when winit could not answer (bevy's `Monitor::refresh_rate_millihertz`
    is an Option) — recorded rather than omitted, and failed rather than forgiven: not knowing the
    refresh and being on the wrong panel are the same evidential state.
    """

    t: float
    name: str | None
    refresh_mhz: int | None
    primary: bool

    def identity(self) -> tuple[str | None, int | None, bool]:
        """What "the same monitor" means — exactly the recorded fields, so a reader of the stream
        and this gate cannot disagree about whether the display changed."""
        return (self.name, self.refresh_mhz, self.primary)

    def describe(self) -> str:
        refresh = "unknown" if self.refresh_mhz is None else f"{self.refresh_mhz} mHz"
        return f"{self.name or '<unnamed>'} ({refresh}, primary={self.primary})"


def parse_rows(
    path: Path,
) -> tuple[list[tuple[float, float]], list[tuple[float, bool]], list[MonitorRow]]:
    """Split a stream into frame, occlusion-transition and monitor rows, enforcing monotonic time.

    Monotonicity is a validity gate, not pedantry: `t` is one process's `Time<Real>` elapsed
    clock, so time running backwards can only mean a spliced, concatenated or corrupted stream.

    ONE torn final row is tolerated, and only that one. The recorder writes through a
    `BufWriter` that flushes at most once a second (src/trace.rs, `JsonlSink::write`), so a
    capture killed at its cutoff — which is how every condition ends — can leave the last row
    half-written. That is the normal shape of a healthy stream, not corruption, and it is
    identified structurally rather than by guessing: a torn tail is the only line that can lack
    its terminating newline. Any unparseable row that IS newline-terminated sits in the middle of
    the stream, which no kill can produce, so it fails loud.
    """
    frames: list[tuple[float, float]] = []
    occlusions: list[tuple[float, bool]] = []
    monitors: list[MonitorRow] = []
    last_t: float | None = None
    text = path.read_text()
    lines = text.splitlines()
    if lines and not text.endswith("\n"):
        torn = lines.pop()
        print(
            f"note: {path} ends mid-row ({torn[:48]!r}) — the capture was killed mid-write; "
            "dropping that one torn tail row",
            file=sys.stderr,
        )
    for lineno, line in enumerate(lines, 1):
        line = line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
            t = float(row["t"])
            # Key PRESENCE, not truthiness, selects the row shape: an occlusion row carries
            # `occluded: false` as often as `true`, and a monitor row's `monitor`/`refresh_mhz`
            # are both legitimately null when the OS names no monitor or answers no refresh.
            kind = "occluded" if "occluded" in row else "monitor" if "monitor" in row else "frame"
            parsed: tuple[float, float] | tuple[float, bool] | MonitorRow
            if kind == "occluded":
                parsed = (t, bool(row["occluded"]))
            elif kind == "monitor":
                refresh = row["refresh_mhz"]
                name = row["monitor"]
                parsed = MonitorRow(
                    t,
                    None if name is None else str(name),
                    None if refresh is None else int(refresh),
                    bool(row["primary"]),
                )
            else:
                parsed = (t, float(row["frame_ms"]))
        except (json.JSONDecodeError, KeyError, TypeError, ValueError) as err:
            fail(f"{path}:{lineno} unparseable row ({err})")
        if last_t is not None and t < last_t:
            fail(
                f"{path}:{lineno} timestamps run backwards ({t:.3f} after {last_t:.3f}) "
                "— a spliced or corrupted stream, not one process's clock"
            )
        last_t = t
        if kind == "occluded":
            occlusions.append(parsed)
        elif kind == "monitor":
            monitors.append(parsed)
        else:
            frames.append(parsed)
    return frames, occlusions, monitors


def occluded_intervals(
    occlusions: list[tuple[float, bool]], t_first: float, t_last: float
) -> list[tuple[float, float]]:
    """Fold occlusion TRANSITIONS into closed [start, end] occluded intervals.

    The state before the first transition is unknown (winit only reports changes). Conservative
    rule: if the first transition says "not occluded", the window is treated as occluded from the
    stream's start until then — on a healthy run that first transition is the window becoming
    visible during warmup, so the conservative interval costs nothing; on a run that started
    covered it is exactly the truth. A stream that ENDS occluded closes its interval at the last
    row.
    """
    intervals: list[tuple[float, float]] = []
    occluded_since: float | None = None
    for index, (t, occluded) in enumerate(occlusions):
        if occluded:
            if occluded_since is None:
                occluded_since = t
        elif occluded_since is not None:
            intervals.append((occluded_since, t))
            occluded_since = None
        elif index == 0:
            intervals.append((t_first, t))
    if occluded_since is not None:
        intervals.append((occluded_since, t_last))
    return intervals


def check_display(path: Path, monitors: list[MonitorRow], window_start: float) -> None:
    """Fail unless the measurement window was presented, throughout, on one fast-enough monitor.

    Scoped to the measurement window exactly like the occlusion gate, and for the same reason: the
    warmup is where the client PARKS its window on the primary display (src/frame_cost.rs), so a
    window born on the external panel legitimately records that panel and then the primary one a
    moment later. Those pre-warmup rows are provenance, not failure — every row at or before the
    window opens collapses into the one that was in force when it opened.

    A measurement window no row covers is INVALID rather than unremarkable: "the window never
    resolved a monitor" and "the window was on the 60 Hz panel" are the same evidential state, and
    this gate exists because the state that LOOKS clean is the dangerous one.
    """
    if not monitors:
        fail(
            f"{path}: no monitor row in the stream — the client never resolved which display was "
            "presenting, so this capture cannot be proven off a 60 Hz panel (whose 16.67 ms pacing "
            "every other gate here passes). Old binary, or a window that never reached a screen?"
        )
    covering = [row for row in monitors if row.t <= window_start]
    if not covering:
        fail(
            f"{path}: the presenting display was not resolved until t={monitors[0].t:.1f}s, after "
            f"the measurement window opened at t={window_start:.1f}s — the measured frames have no "
            "display provenance at all"
        )
    # The state in force when the window opened, plus every change after it.
    states = [covering[-1], *(row for row in monitors if row.t > window_start)]
    for row in states:
        if row.refresh_mhz is None or row.refresh_mhz < MIN_REFRESH_MHZ:
            fail(
                f"{path}: presented on {row.describe()} — below the {MIN_REFRESH_MHZ} mHz floor. "
                "A 60 Hz panel paces presentation to multiples of 16.67 ms whatever present mode "
                "the app negotiated, so every condition quantizes to ~60 fps and the whole ladder "
                "reads the same; these frame times are fiction. Disconnect the external display, "
                "or re-run once the client has parked its window on the primary one."
            )
    changes = [row for row in states[1:] if row.identity() != states[0].identity()]
    if changes:
        first = changes[0]
        fail(
            f"{path}: the presenting display CHANGED at t={first.t:.1f}s, mid-measurement — from "
            f"{states[0].describe()} to {first.describe()}. Part of this window was paced by one "
            "panel and part by another; the percentiles average two machines"
        )


def load_stream(
    path: Path,
    warmup_s: float,
    min_rows: int,
    expected_duration_s: float | None,
    occluded_ok: bool,
    display_ok: bool,
) -> list[float]:
    if not path.is_file():
        fail(f"{path} does not exist")
    frames, occlusions, monitors = parse_rows(path)
    if not frames:
        fail(f"{path} has no frame rows")
    # Warmup is relative to the stream's own first row: `t` is app-elapsed time, and the first
    # seconds cover asset load, shader compilation and the settle after spawn. Every row shape
    # carries the same clock, so the bounds are taken across all of them.
    stamps = [t for t, _ in frames] + [t for t, _ in occlusions] + [row.t for row in monitors]
    t_first = min(stamps)
    t_last = max(stamps)
    if expected_duration_s is not None:
        required = SPAN_TOLERANCE * (warmup_s + expected_duration_s)
        span = t_last - t_first
        if span < required:
            fail(
                f"{path}: rows span {span:.1f}s < required {required:.1f}s "
                f"({warmup_s:g}s warmup + {expected_duration_s:g}s measure, 2% tolerance) "
                "— the process died or stalled early; row count alone cannot see this"
            )
    window_start = t_first + warmup_s
    if not display_ok:
        check_display(path, monitors, window_start)
    if not occluded_ok:
        for start, end in occluded_intervals(occlusions, t_first, t_last):
            if end > window_start:
                fail(
                    f"{path}: window occluded t={start - t_first:.1f}s..{end - t_first:.1f}s "
                    f"overlaps the measurement window (warmup ends at {warmup_s:g}s) — an "
                    "occluded surface presents nothing and the frame loop free-runs; these "
                    "frame times are fiction"
                )
    kept = [ms for t, ms in frames if t - t_first >= warmup_s]
    if len(kept) < min_rows:
        fail(
            f"{path}: {len(kept)} post-warmup rows < required {min_rows} "
            f"({len(frames)} total; crashed, stalled, or killed early?)"
        )
    return kept


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("streams", nargs="+", type=Path)
    parser.add_argument("--warmup-s", type=float, default=10.0)
    parser.add_argument("--min-rows", type=int, default=400)
    parser.add_argument(
        "--expected-duration-s",
        type=float,
        help="fail unless row timestamps span at least warmup + this many seconds (2%% tolerance)",
    )
    parser.add_argument(
        "--occluded-ok",
        action="store_true",
        help="skip the occlusion gate (SMOKE mode only: hidden window, numbers are fiction)",
    )
    parser.add_argument(
        "--display-ok",
        action="store_true",
        help="skip the presenting-display gate (SMOKE mode only: a hidden window resolves no "
        "monitor, and its numbers are fiction)",
    )
    parser.add_argument(
        "--validate-only",
        action="store_true",
        help="run the validity gates and print OK per stream; no tables",
    )
    parser.add_argument(
        "--baseline",
        help="condition name the delta table subtracts (default: the first stream)",
    )
    args = parser.parse_args()

    conditions: dict[str, dict[str, float]] = {}
    counts: dict[str, int] = {}
    for path in args.streams:
        name = condition_name(path)
        if name in conditions:
            fail(f"duplicate condition name {name!r} ({path})")
        kept = load_stream(
            path,
            args.warmup_s,
            args.min_rows,
            args.expected_duration_s,
            args.occluded_ok,
            args.display_ok,
        )
        conditions[name] = {label: pct(kept, q) for label, q in STATS}
        counts[name] = len(kept)

    if args.validate_only:
        for name, count in counts.items():
            print(f"{name} VALID: {count} post-warmup frame rows, gates passed")
        return 0

    baseline = args.baseline or condition_name(args.streams[0])
    if baseline not in conditions:
        fail(f"baseline {baseline!r} is not among the streams ({', '.join(conditions)})")

    width = max(len("condition"), *(len(name) for name in conditions))
    stat_labels = [label for label, _ in STATS]

    print(f"\nframe time, ms (post-warmup {args.warmup_s:g}s discarded)")
    print(f"{'condition':<{width}} {'n':>7} " + " ".join(f"{s:>8}" for s in stat_labels))
    for name, stats in conditions.items():
        cells = " ".join(f"{stats[s]:>8.2f}" for s in stat_labels)
        print(f"{name:<{width}} {counts[name]:>7} {cells}")

    print("\nfps-equivalent (1000 / frame ms)")
    print(f"{'condition':<{width}} " + " ".join(f"{s:>8}" for s in stat_labels))
    for name, stats in conditions.items():
        cells = " ".join(f"{1000.0 / stats[s]:>8.1f}" for s in stat_labels)
        print(f"{name:<{width}} {cells}")

    for budget in BUDGETS_MS:
        print(f"\n% of {budget:.2f} ms budget ({1000.0 / budget:.0f} fps); >100 = over budget")
        print(f"{'condition':<{width}} " + " ".join(f"{s:>8}" for s in stat_labels))
        for name, stats in conditions.items():
            cells = " ".join(f"{100.0 * stats[s] / budget:>8.1f}" for s in stat_labels)
            print(f"{name:<{width}} {cells}")

    print(f"\ndelta vs baseline {baseline!r}, ms (positive = slower than baseline)")
    print(f"{'condition':<{width}} " + " ".join(f"{'d' + s:>8}" for s in stat_labels))
    base = conditions[baseline]
    for name, stats in conditions.items():
        if name == baseline:
            continue
        cells = " ".join(f"{stats[s] - base[s]:>+8.2f}" for s in stat_labels)
        print(f"{name:<{width}} {cells}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
