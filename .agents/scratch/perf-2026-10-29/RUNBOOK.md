# Two visible frame-cost runs (2026-10-29)

1. **Track-print relief levels** (cosmetic terrain marks, issue 125): TRACK PRINTS at LIGHTING, PARALLAX
   and SUN SHADOW, on the mud pack and on the base pack, with prints held on screen.
2. **Issue 123:** the forest's rendered frame cost, with 635 segmented trees against the graybox cones
   they replaced, in a scene inside the forest and in the default duel scene.

Each measurement is one command. The window must be visible, so the owner runs them; an agent cannot.

## The binary

`bin/` sits beside this file in the main checkout. It is git-ignored and holds three things:

- `bin/overmatch`: the binary.
- `bin/root/assets/`: an APFS clone of that commit's `assets/`, used as `ASSET_ROOT`.
- `bin/BUILD.txt`: the binary's provenance. Each run's `manifest.txt` copies it.

It was built from `49f0982` (lane `lane/agent-a5028721241bd9f3f`) with this command:

```
scripts/serial-cargo build --locked --release --bin overmatch --features track_prints_demo
```

The build uses release codegen and the shipped feature set (`dev_tools`), plus `track_prints_demo`.
It has no `dev_ui` and no `bitprobe`, because no shipped build has them.

The track-print demo used to compile only under `debug_assertions`. The new `track_prints_demo` feature
puts it into an optimized build. A shipped build enables no such feature, so it is unchanged. In the
forest runs the demo stays inert, because its environment variable is not set.

To rebuild, run the command above in a bay. Then copy `target/release/overmatch` and a clone of
`assets/` (`cp -cR`) here, and rewrite `BUILD.txt`.

## Preconditions

Each of these makes the numbers valid. None of them is optional.

- **The lane is merged.** `scripts/perf/run-track-prints-cost.sh`, `run-forest-cost.sh` and
  `frame-run.zsh` must exist in the main checkout.
- **The machine is quiet.** `pgrep -fl 'rustc|cargo|/overmatch( |$)|codex exec'` prints nothing. No
  browser video is playing, and Focus / Do Not Disturb is on.
- **The machine is plugged in.** It should also be idle for about 5 minutes beforehand, so the
  chassis is not already hot.
- **The window is on the primary display.** That is the built-in 120 Hz panel, at 2560x1440 physical.
  The client parks itself there, and the gates fail a condition on any other monitor or surface size.
- **The display stays lit and awake.** Brightness must be at least 1 notch. Wrap every run in
  `caffeinate -dims`, as the commands below do. If the display sleeps, the monitor disappears and the
  client exits; a smoke run died this way at 03:25. Set the screensaver to Never, because
  `caffeinate` does not reset the screensaver's timer.
- **Hands off.** The window steals focus once per condition. Touch nothing until the runner prints
  `complete`.

## Commands, in order (from the main checkout)

```
cd /Users/Yan/github/vikng-dev/overmatch
P=.agents/scratch/perf-2026-10-29
export BIN=$PWD/$P/bin ASSET_ROOT=$PWD/$P/bin/root
```

**1. Warm the shader cache with visible throwaway runs. Discard their numbers. About 3 min.**

The first visible launch of a binary stalls on Metal pipeline compilation.

```
caffeinate -dims env DURATION_S=5 COOLDOWN_S=0 scripts/perf/run-track-prints-cost.sh $P/runs/warm-prints mud-sun
caffeinate -dims env DURATION_S=5 COOLDOWN_S=0 scripts/perf/run-forest-cost.sh $P/runs/warm-forest forest-trees forest-cones
```

**2. Track-print relief levels. About 13–15 min (DERIVED: 6 × (startup + 50 s warmup + 60 s measure + 15 s cooldown)).**

```
caffeinate -dims scripts/perf/run-track-prints-cost.sh $P/runs/track-prints
```

**3. Forest, issue 123. About 7–8 min (DERIVED: 4 × (startup + 15 s warmup + 60 s measure + 15 s cooldown)).**

```
caffeinate -dims scripts/perf/run-forest-cost.sh $P/runs/forest
```

**4. Optional drift check: the same conditions in reverse order. About 20 min.**

```
caffeinate -dims scripts/perf/run-track-prints-cost.sh $P/runs/track-prints-rev base-sun base-parallax base-lighting mud-sun mud-parallax mud-lighting
caffeinate -dims scripts/perf/run-forest-cost.sh $P/runs/forest-rev duel-trees duel-cones forest-trees forest-cones
```

## Reading and recording

Each runner prints its analyzer tables:

- **Track prints:** one table per ground, with deltas against that ground's `-lighting` condition.
- **Forest:** one table per scene, with deltas against that scene's `-cones` condition.

Each run's `manifest.txt` holds the provenance, every gate's receipt, and the following per condition:

- **Track prints:** the hold line. It records the view, its start on the stream's clock, the relief
  and range in force, and the number of stamps laid.
- **Forest:** the scatter line. It counts the graybox firs and the trees that were placed.

Compare conditions by p50. On this rig, repeat runs agree to about 1–2 % (MEASURED, profiling
notes).

Record the numbers as MEASURED, with the gates stated as passed:

- **Forest:** in `.agents/scratch/destruction-2026-10-26/`, which is issue 123's acceptance.
- **Track prints:** beside this runbook.

A runner stops at the first INVALID condition and names the gate that failed:

| Gate | Cause | What to do |
|---|---|---|
| Occluded | Something covered the window. | Rerun. |
| Monitor or surface | The window landed on the 60 Hz external panel. | Rerun. |
| Client exited early | The display slept. | Rerun. |
| The hold began after the window opened | Cold cache. | Rerun, or raise `WARMUP_S`. |

A runner clears its output directory when it starts. So rerun the remaining conditions into a new
directory, then analyze both directories together:

```
uv run scripts/perf/analyze.py --warmup-s 50 --expected-duration-s 60 --min-rows 1200 \
  --baseline mud-lighting $P/runs/track-prints/mud-*.client.jsonl $P/runs/track-prints-2/mud-*.client.jsonl
```

For the forest, use `--warmup-s 15`.

## Scenes

The stills are in `scenes/`. They are hidden captures at 1280x720 that show what each scene draws.

### Track prints

The scene comes from the demo (`src/track_prints/demo.rs`). It sets the player's tank down on the
spot at 10 s, then drives it straight, stops, pivots and drives out. From about 37 s,
`OVERMATCH_TRACK_PRINTS_DEMO_HOLD=pivot` holds the camera about 10 m off the pivot for good. During the
hold the demo forces no relief level, so the relief comes from the player's setting.

There are two spots, each fixture data where one pack dominates the painted mask:

- **mud:** `599.9,-599.9`, heading 195°.
- **base:** `70,-20`, heading 90°.

The condition writes `track_prints` into `video.ron`, together with `track_print_range` (Far, the
shipped default). The `RANGE=` and `VIEW=drive` knobs change those.

Stills: `scenes/mud-lighting-pivot.jpg` and `scenes/base-sun-pivot.jpg`.

### Forest

There are two scenes:

- **forest:** set with `OVERMATCH_START=540,570,0`. The player's tank stands in the map's north belt.
  The camera, as spawned, looks +Z and is pitched 27° down. About 48 trunks stand within 150 m of it
  (DERIVED from `level.json`, using a 72° horizontal field). Stills: `scenes/forest-trees.jpg` and
  `scenes/forest-cones.jpg`.
- **duel:** the default duel, with no fir in frame. This measures what the forest costs when nobody
  looks at it. Still: `scenes/duel-trees.jpg`.

The whole frame is measured. So a trees condition also carries the sim's share, a convex collider per
segment.

### The cones

The forest runner builds `runs/<run>/root-cones` on each run. Every entry in it is a symlink into
`ASSET_ROOT`, except `maps/kalinovo/prototypes.ron`. That file is rewritten so the three fir ids
resolve to the graybox `Fir` rows the file itself records. `forest_tick_cost_bed` makes the same
substitution.

The cone code is the one the map drew up to **`d16a458`**, the last commit whose map resolved its
firs to cones. `88c346f` switched the map to `Tree(asset: "fir_1")` and kept `Fir` for this
comparison. The Fir branch of `scatter::spawn` has not changed since.

**Not prepared: a binary built from `d16a458` itself.** It would carry every change made since
(terrain packs, track prints, the damage model) into the delta, alongside the forest.

## Smoke (2026-10-29, hidden window)

These runs prove that the plumbing works and nothing more. No frame time from them is a measurement.

- **`run-track-prints-cost.sh`** (mud-lighting, base-sun):
  - The demo armed with its hold and laid prints: 9,823 and 10,055 stamps.
  - The hold started at real t = 40.4 s and 37.4 s, both inside the 50 s warmup.
  - The injected relief and range were in force.
  - The streams were valid.
- **`run-forest-cost.sh`** (all four conditions):
  - The cones placed 635 graybox firs and 0 trees; the trees placed 0 and 635.
  - The start lever took in the forest scene.
  - No asset failed to load, and the streams were valid.
- **`run-frame-sweep.sh`**, after its loop moved into `frame-run.zsh`:
  - m350-5 and off-5 were valid, and the probe gate passed.
  - One earlier attempt died when the display slept. The liveness gate caught it, and the run passed
    under `caffeinate`.
