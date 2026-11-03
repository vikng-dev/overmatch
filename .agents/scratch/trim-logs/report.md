Game state only, including the game's damage model.

Removed unread SIM-EVIDENCE, interpolation/input-margin dumps, FRONTIER gaps/summaries,
command/impact/building/hit/track/bake/picture dumps, and their sole-purpose helpers.

Kept readers:

- Unchanged `server_stats`: `scripts/perf/online_cost.py`, `run-online-cost.sh`, `run-debris-stress.sh`.
- Listening/connection/input/completion: `scripts/ci/archive-smoke.sh`, `scripts/connect/batch.sh`.
- Harness release/input/first-fire: `scripts/shot/run-combat-start.py`, performance runners.
- Camera marks: `scripts/perf/pencam_cost.py`; window/render instrumentation:
  `scripts/perf/frame-run.zsh`, `run-ablation.sh`.
- Other lifecycle transitions, refusals and real faults (including FRONTIER resyncs,
  missing announcements, reload repairs, corrupt-state tripwires): operator.

MEASURED rerun: server 6,041,520 bytes/1,231.110145 s; client 6,371,426/1,231.310841 s.
The 30-bot sample contains 30 then 31 tanks after the player joins.
DERIVED bytes/hour, bytes × 3600/span, subtracting deleted lines without another game run:
server 17,666,552 → 255,124 (98.56% reduction); client 18,628,224 → 743,828 (96.01%).

Faults retained: two already-simulated input corrections; 222 B0004 warnings
(building parents lack InheritedVisibility); ClientEntityInUse absent here, but source
rejects an already-connected entity. Startup compression support now explicitly matches
the loaders' NONE fallback; only the server's expected missing-RenderApp extraction target
is filtered. Other fault targets remain visible.

Checks: formatting, CI clippy, targeted tests and whole-tree guards passed: 804 passed,
six existing manual/live tests ignored. No game run. Unrelated law/backlog work stays
outside this logging-only brief.
