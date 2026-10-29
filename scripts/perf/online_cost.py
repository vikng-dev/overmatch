# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""The online cost runner's server side and its tables (scripts/perf/run-online-cost.sh).

Three subcommands:

  server-window --label <cond> [--after-marker <file>] [--json out] <server.log>
      The server's stats lines inside one condition's window, aggregated per key.
  bots <server.log>
      How many bots the server says it spawned (the night wrapper's check that the N-bot lever took).
  tables <out-dir> ...
      The per-condition client network table (nettop) and server table, beside the client frame
      table `analyze.py` prints.

THE STATS LINE (src/net/server_stats.rs) is one bare `println!` per period on stdout, no timestamp:

    server_stats {"budget_us":15625,"clients":2,"cpu_pct":11.2,"entities":812,"overruns":0,
                  "period_s":1.0,"projectiles":3,"rss_mb":61.5,"tanks":3,"tick_hz":64.0,
                  "tick_us":{"max":900,"p50":210,"p95":480},"ticks":64}

`parse_stats_line` is the one place that knows that shape: the token `server_stats`, then one JSON
object. Its keys are not fixed here: nested objects flatten to dotted keys (`tick_us.p95`), every
numeric leaf is kept, nulls and strings are dropped. When the format moves, that function is the
change. A line without the token, or whose object does not parse, is not a stats line and is
skipped, so a server without the lever yields an empty table, not an error.

THE WINDOW. The lines carry no clock, so the runner marks the window by content: at the window's
start it saves the last stats line the server had printed (the marker), and at its end it saves
the server log (local: a copy; Edgegap: a container-log pull). The condition's stats are the lines
after the marker's last occurrence: periods that ended after the start edge, up to the end edge.
The runner takes the marker ~2 s before the client window opens (so the pull lands in the warmup)
and the end snapshot at the first poll past its close, so with a 1 s period the server window is
the client's plus up to ~3 s (DERIVED from the poll and period). No marker (no stats line yet, or
none saved) reads the whole log; a marker absent from the log (a truncated pull) reads the whole
log too, and says so.
"""

from __future__ import annotations

import argparse
import json
import re
import statistics
import sys
from pathlib import Path

ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
STATS_TOKEN = "server_stats"
# One spawn line per bot (`server: spawned circling bot 3 tank …`), or one line naming a count
# (`spawned 10 bots`). `\bspawned` never matches inside `respawned`.
BOT_SPAWN = re.compile(r"\bspawned\b.*\bbots?\b", re.IGNORECASE)
BOT_SPAWN_COUNT = re.compile(r"\bspawned\s+(\d+)\s+.*?\bbots\b", re.IGNORECASE)
BOT_KNOCKOUT = re.compile(r"\bbot\b.*\bknocked out\b", re.IGNORECASE)


def flatten(value: object, prefix: str = "") -> dict[str, float]:
    """Numeric leaves of a JSON value as dotted keys; booleans count as numbers, strings and nulls
    are dropped."""
    out: dict[str, float] = {}
    if isinstance(value, dict):
        for key, inner in value.items():
            out.update(flatten(inner, f"{prefix}.{key}" if prefix else str(key)))
    elif isinstance(value, bool):
        out[prefix] = float(value)
    elif isinstance(value, (int, float)):
        out[prefix] = float(value)
    return out


def parse_stats_line(line: str) -> dict[str, float] | None:
    """THE format function: the stats of one `server_stats {json}` line, flattened, or None.

    Expected: the token, then one JSON object, anywhere in the line (a container log may prefix
    it). Adapt the token or the decoding here, and only here.
    """
    clean = ANSI.sub("", line)
    at = clean.rfind(STATS_TOKEN)
    if at < 0:
        return None
    brace = clean.find("{", at + len(STATS_TOKEN))
    if brace < 0:
        return None
    try:
        value, _ = json.JSONDecoder().raw_decode(clean[brace:])
    except json.JSONDecodeError:
        return None
    stats = flatten(value)
    return stats or None


def after_marker(lines: list[str], marker: str) -> tuple[str, list[str]]:
    """The lines after the marker's last occurrence: (`marked`, …); without a marker, every line
    (`whole-log`); with a marker the log does not hold, every line (`marker-missing`)."""
    marker = marker.strip()
    if not marker:
        return "whole-log", lines
    for index in range(len(lines) - 1, -1, -1):
        if lines[index].strip() == marker:
            return "marked", lines[index + 1 :]
    return "marker-missing", lines


def aggregate(rows: list[dict[str, float]]) -> dict[str, dict[str, float]]:
    """Per key across the window's periods: n, median, max, and last-first (meaningful only for a
    cumulative counter, which this module cannot tell from a per-period value)."""
    keys = sorted({key for row in rows for key in row})
    out: dict[str, dict[str, float]] = {}
    for key in keys:
        values = [row[key] for row in rows if key in row]
        out[key] = {
            "n": len(values),
            "median": statistics.median(values),
            "max": max(values),
            "last_minus_first": values[-1] - values[0],
        }
    return out


def bots_spawned(log_lines: list[str]) -> int:
    """Bots the server reports spawning: the larger of the per-bot spawn lines and any count line."""
    single = 0
    counted = 0
    for line in log_lines:
        clean = ANSI.sub("", line)
        count = BOT_SPAWN_COUNT.search(clean)
        if count:
            counted = max(counted, int(count.group(1)))
        elif BOT_SPAWN.search(clean):
            single += 1
    return max(single, counted)


def read_lines(path: Path) -> list[str]:
    return path.read_text(errors="replace").splitlines() if path.exists() else []


def cmd_server_window(args: argparse.Namespace) -> int:
    log = Path(args.log)
    lines = read_lines(log)
    marker_path = Path(args.after_marker) if args.after_marker else None
    marker = marker_path.read_text(errors="replace") if marker_path and marker_path.exists() else ""
    mode, window = after_marker(lines, marker)
    rows = [stats for stats in map(parse_stats_line, window) if stats is not None]
    result = {
        "label": args.label,
        "log": str(log),
        "log_present": log.exists(),
        "window": mode,
        "stats_lines": len(rows),
        "bots_spawned": bots_spawned(lines),
        "bot_knockouts_in_window": sum(bool(BOT_KNOCKOUT.search(ANSI.sub("", line))) for line in window),
        "stats": aggregate(rows),
    }
    if args.json:
        Path(args.json).write_text(json.dumps(result, indent=1) + "\n")
    note = ""
    if not rows:
        note = " — no server_stats lines: the lever is off or absent, or the logs were not pulled"
    elif mode != "marked":
        note = f" — {mode}: the window is the whole log, not the condition's"
    print(
        f"   {args.label} server: {len(rows)} stats lines ({mode}), bots spawned {result['bots_spawned']}, "
        f"bot knockouts in the window {result['bot_knockouts_in_window']}{note}"
    )
    return 0


def cmd_bots(args: argparse.Namespace) -> int:
    print(bots_spawned(read_lines(Path(args.log))))
    return 0


def read_window(path: Path) -> dict[str, dict[str, str]]:
    """The window file's edges, keyed `start` / `end`."""
    rows = [line.split("\t") for line in path.read_text().splitlines() if line.strip()]
    header, body = rows[0], rows[1:]
    return {row[0]: dict(zip(header, row)) for row in body}


def net_row(path: Path) -> list[str] | None:
    """`window_s down_B up_B down_KB/s up_KB/s` from a window file, rates on the stream clock."""
    edges = read_window(path)
    try:
        start, end = edges["start"], edges["end"]
        span = float(end["span_s"]) - float(start["span_s"])
        down = int(end["bytes_in"]) - int(start["bytes_in"])
        up = int(end["bytes_out"]) - int(start["bytes_out"])
    except (KeyError, ValueError):
        return None
    if span <= 0:
        return None
    return [f"{span:.1f}", str(down), str(up), f"{down / span / 1000:.1f}", f"{up / span / 1000:.1f}"]


def print_table(header: list[str], rows: list[list[str]]) -> None:
    widths = [max([len(h)] + [len(r[i]) for r in rows]) for i, h in enumerate(header)]
    print("  ".join(h.ljust(w) for h, w in zip(header, widths)).rstrip())
    for row in rows:
        print("  ".join(c.ljust(w) for c, w in zip(row, widths)).rstrip())


def fmt(value: float) -> str:
    return f"{value:.0f}" if abs(value) >= 100 or value == int(value) else f"{value:.3g}"


def cmd_tables(args: argparse.Namespace) -> int:
    windows = sorted(p for d in args.dirs for p in Path(d).glob("*.window.tsv"))
    print("\n== client network: the measured client's process (nettop), over the measurement window")
    rows = []
    for path in windows:
        row = net_row(path)
        rows.append([path.name.removesuffix(".window.tsv"), *(row or ["-"] * 5)])
    print_table(["condition", "window_s", "down_B", "up_B", "down_KB/s", "up_KB/s"], rows)

    print("\n== server: its server_stats lines inside the window (median and max across periods)")
    summary, detail = [], []
    for path in windows:
        cond = path.name.removesuffix(".window.tsv")
        server = path.with_name(f"{cond}.server.json")
        if not server.exists():
            summary.append([cond, "-", "-", "-", "-"])
            continue
        result = json.loads(server.read_text())
        summary.append(
            [
                cond,
                str(result["stats_lines"]),
                result["window"],
                str(result["bots_spawned"]),
                str(result["bot_knockouts_in_window"]),
            ]
        )
        for key, agg in result["stats"].items():
            detail.append(
                [cond, key, str(agg["n"]), fmt(agg["median"]), fmt(agg["max"]), fmt(agg["last_minus_first"])]
            )
    print_table(["condition", "stats_lines", "window", "bots_spawned", "bot_knockouts"], summary)
    if detail:
        print()
        print_table(["condition", "key", "n", "median", "max", "last-first"], detail)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    window = sub.add_parser("server-window")
    window.add_argument("--label", required=True)
    window.add_argument("--after-marker")
    window.add_argument("--json")
    window.add_argument("log")
    bots = sub.add_parser("bots")
    bots.add_argument("log")
    tables = sub.add_parser("tables")
    tables.add_argument("dirs", nargs="+")
    args = parser.parse_args()
    return {"server-window": cmd_server_window, "bots": cmd_bots, "tables": cmd_tables}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
