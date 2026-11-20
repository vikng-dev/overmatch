# Lab rebuild design

Subject: the measuring tool for the game's building-debris model (game state only): a vehicle
driving into or over a simulated brick building, and a round's event at a wall. Read-only design,
2026-11-20. Source read on `main` at `85fd25f5`; the lab at `lane/lab-replay`; the ram findings at
`lane/ram-a` (`.agents/scratch/ram-a-experiment.md`). Line counts are MEASURED with `wc -l` or
`git diff --stat` unless marked DERIVED. Nothing here was built or run.

**Decision in one paragraph.** The lab's own binary, World and staging file are not rebuilt.
`main`'s ram instrument already runs the real `overmatch-server` and a real `overmatch` client as
two processes, starts the pass from rest and gates on measured contact speed. The rebuild adds to
it: (1) a hull record on each of its per-tick rows, (2) a structure reading at the end of a
capture, (3) frames from the driving client and a small `scripts/debris-lab` that encodes, labels
and pairs them. The thaw order at `src/debris/freeze.rs:947` gets a declared key on `main`
regardless. About 750 lines replace 2,058 (DERIVED, section 5).

## 1. What exists on `main`

| Instrument | Drives | Measures | Server and client | Launched by | Users |
|---|---|---|---|---|---|
| `scripts/perf/collapse.py` (1,195 lines), `collapse.sh` (11) | Three modes: `--schedule` (match), `--ram`, and the default tankless round workload (`collapse.py:1075-1076`, `:982`) | Parses `server_stats` lines into `rows.json`, `counts.json`, `tick-attribution.json`, `result.json`; fairness and capacity verdicts | Separate processes: `overmatch-server`, then `overmatch` clients with `OVERMATCH_WINDOW=headless` (`collapse.py:430`, `:453-455`, `:824-856`); strips every inherited `OVERMATCH_`/`SPIKE_` variable (`:429`, `:823`) | `scripts/perf/collapse.sh …` under the build lock (`:1173`, `start_new_session`) | `scripts/perf/collapse.md`; `.agents/docs/design/construction-destruction.md`, `construction-carrier-prototype.md`, `server-frame-stall.md` |
| `src/net/ram.rs` (1,375) | One tank at a run-up DERIVED from its own drive data (`ram.rs:141-158`: d = v²/2a × `runup_margin`), spawned at rest facing a named map construction (`:160-201`, used by the shipped spawn at `server.rs:954-956`); the client's harness holds full throttle after a 256-tick lead (`harness.rs:427`, `:689`) | Speed at first contact (`ram.rs:264-269`, `:531`), by solver or hull sweep (`:497-527`); every tick's stage times, work counts and contact census; ends on rest, deadline, world change or approach timeout (`:84-90`) | Shipped `server::run` in its own process; refuses without the stats lever (`ram.rs:124-130`) | `OVERMATCH_RAM=<json>`; `collapse.py --ram scripts/perf/ram.json` (`:818-893`) | `collapse.py`, `scripts/perf/ram.json` |
| `src/net/loadtest.rs` (564) | Fells named map constructions at wall seconds after combat admission, through `felling_lines` and the ordinary deposit path (`loadtest.rs:238-351`) | Per-target `falls`, `dust`, `chunks`, awake/sleeping/frozen groups (`:147-178`); per-tick rows | Shipped server process plus N real clients and bots | `OVERMATCH_LOADTEST=<json>`; `collapse.py --schedule` | `collapse.py`, `scripts/perf/pile_park.py`, `rest-*.json`, `match-collapse.json` |
| `src/net/he_stress.rs` (509) | A shipped burst round fired straight down into the lowest-ordinal construction, every `period_s` (`he_stress.rs:96-138`); no tank | Counts rounds fired and events marched (`:140-153`) | Shipped server process, no client | `OVERMATCH_HE_STRESS=<n>:<period>`; `collapse.py` default mode (`:989-991`) | `collapse.py`, `scripts/edgegap/session.sh` |
| `src/net/server_stats.rs` (1,595) | Nothing | One `server_stats {json}` line a period: tick, physics, stage and phase times, CPU, per-link send bytes (`queued_bytes`, `:769`), and the `ram`, `loadtest` and `pieces` blocks (`:601-604`, `:849`) | Inside the shipped server | `OVERMATCH_SERVER_STATS_S` | `collapse.py`, `pile_park.py`, `scripts/aws/harness.py`, `scripts/edgegap/session.sh`, five more runners |
| `src/net/contact_census.rs` (1,658) | Nothing | Per-tick contact graph census and a per-piece rest record (`PieceRecord`: chunks, born, first ground, last impact, last moved; `:263-283`) | Inside the shipped server, armed by a recording trace | Through the ram or loadtest lever | `collapse.py` `piece_census` |
| `src/dev_capture/` (2,980 with `dev_capture.rs`) | A plan file: frames, held keys, cues | PNG frames from an offscreen image on a hidden window (`dev_capture.rs:542-604`); exits with failure if a frame is missing 3 s after the last (`:599`) | Mounted on every windowed root; on the network client only when it runs no harness script (`client.rs:520-525`). Online frame times count from startup on the wall clock: "network arrivals are not repeatable" (`plan.rs:174-177`, `clock.rs:1-4`) | `OVERMATCH_CAPTURE=<plan>.ron` | 27 plans under `scripts/capture/`, `scripts/shot/run-*.py`, `run-track-prints-cost.sh` |
| `dev_capture/wall.rs` (543), `assets/dev/capture-wall.ron` (8) | A round or bare charge at a house wall chosen by class, or at a point the plan names in a named construction (`plan.rs:445-448`); the dedicated server fires it from the same plan (`wall.rs:208-219`, `server.rs:94`); the offline root alone can stand extra constructions beside the house (`wall.rs:419-487`) | Nothing; it frames | Offline root, or shipped server plus capture clients | The plan's `wall` field | `scripts/shot/run-walk.py:22`, four capture plans |

Three facts the reviewers' summaries did not carry:
- The ram instrument already starts from rest under throttle and reports measured contact speed.
  The lab's "12 m/s spawn" fault never existed there.
- A scripted client can have a window (`client.rs:438-446`), but the capture plugin is mounted only
  on unscripted ones (`client.rs:520`). No tracked run today takes frames from a client that a
  harness script drives.
- The server keeps one map (`map.rs:59`, `MAP_ID`), and nothing places a construction on the
  dedicated server outside the map's manifest. The lab's turned `house_1` and `lane/ram-a`'s hall
  have no home on a real server yet.

## 2. The gap

**Only the lab measures** (kept):
- Hull state per tick: position, velocity, forward speed, and the forward speed change over the
  tick in m/s. The ram rows carry timings and counts, no hull state (`cost.rs:25-46`,
  `ram.rs:106-110`).
- What moved the hull per tick: solver impulse along its forward by the other body's kind, the
  sweep's exchange and verdicts. Read by the ram investigation (`lenses-1.md:73`).
- Crossing time and stall time.
- The struck wall's opening, the share of the building still in place, the largest loose piece's
  chunk count and speed.
- Two-panel labelled clips and `compare`.

**Duplicates** (not rebuilt):
- The server and client composition (`debris_lab/network.rs`, 227 lines; `mod.rs` admission):
  `server::run` and `client::run`.
- The run-up and contact speed (`lane/ram-a` staging): `ram.rs:141-201`, `:531`.
- Felling a house: `loadtest.rs`. A round at a wall: `dev_capture/wall.rs` on the server.
- Wire bytes: `server_stats.rs:744-779`. Tick time: `TickSample` (`cost.rs:25`).
- Offscreen frames: `dev_capture.rs:647-685`.
- Rest counts: `loadtest.rs:206-218`, `contact_census.rs` piece records.

**Unread, not to come back** (`lenses-1.md:57-71`, each checked against the lab source):
`bodies_dynamic`; the belt-reversal counter and its threshold (the rows hold `belt_mps`);
`removed_chunks`; the staging echo; `--seed`; the interactive mode and its eight `Run` branches;
`thrown_chunks` and its speed threshold (no reader named in either scratch report's verdicts);
the acceleration distributions in m/s² (a one-tick speed change read as a load);
the drawn-hole sampling in the wall opening (`debris_lab/wall.rs:118-155`: holes are drawing
state, `breakage.rs:43-47`).

`parsed_drive` comes back with a new job: with a real client the server no longer asserts the
input each tick, so the row's parsed throttle and steer are what `compare` checks first (section 3,
replay).

## 3. The design

### Where each recorder lives

| Recorder | Home | Owner of the data | Output |
|---|---|---|---|
| Hull row | `src/net/ram.rs`, a `hull` field on `RamWork` (`ram.rs:106`), filled in `Census::record` (`:466`) | The ram trace | each tick's row in `server_stats.ram.ticks` |
| Hull summary | `scripts/perf/collapse.py` `ram_summary` (`:768`) | The script | `result.json`: contact speed (exists), minimum forward speed, largest one-tick speed change, crossing s, stall s |
| Structure reading | new `src/net/ram/structure.rs`, called once when a capture ends (`ram.rs:537-541`) | The ram result | `server_stats.ram.structure` |
| Frames | `src/dev_capture.rs`, on the driving client | The capture plan | PNGs under the run folder |
| Encode, label, pair | `scripts/debris-lab` (new, Python, imports `collapse`) | The script | `clips/*.mp4`, `comparison.json` |

Definitions, each stated in `result.json` beside its value:
- **Forward speed change**: this row's forward speed minus the previous row's, m/s. No m/s² field.
- **Crossing**: from the first-contact tick to the first tick the hull's rear clears the
  construction's far bound along the recipe heading (`kind.bounds()` plus the stance's reach, as
  the lab did). Null when the capture ends first.
- **Stall**: seconds after first contact with forward speed ≤ 0. The sign needs no authored number.
- **In place**: a chunk above grade whose centre lies within its own bounding radius of where it
  was built. DERIVED from geometry, no tolerance to tune. It counts a standing chunk and a roof
  resting on its walls alike, and excludes a heap. Chunks whose built centre is below grade are
  reported separately and never enter the share. This closes two of `lane/ram-a`'s open faults.
- **Opening**: the departed (not in place) area of the struck panel's outer face, clipped at grade.
  The panel is the construction component the recipe's approach ray meets first: chosen by the
  recipe's heading and the construction's geometry, never by shape guessing
  (`debris_lab/wall.rs:24-30`). A panel with zero face area refuses the run at `prepare`
  (`ram.rs:205`), naming the recipe field.
- **Hull account**: read from the census's step snapshot, which is taken before sleeping and
  structural retirement (`contact_census.rs:2-4`), so a body retired in the tick still counts. The
  field doc names the excluded channel: implicit belt support and traction
  (`contact_transfer.rs:40-42`), so the channels do not sum to m·Δv.
- Every float is asserted finite where it is recorded; `null` means only a named absence.

### Staging

- The drive recipe stays `scripts/perf/ram.json`, parsed once at the server's edge
  (`ram.rs:47-73`). The server echoes it in every row and the runner refuses a mismatch
  (`collapse.py:632-633`). There is no embedded staging any more, so the "two owners" finding
  (`lenses-1.md:105`) has nothing to bite on.
- Clip staging is one capture plan, `scripts/capture/debris-lab.ron`: camera `Follow` offset
  (`Taste`: clip framing), frame size and clip rate (`Resolution`: bounded evidence, 8 fps),
  approach lead in view (`Taste`). The plan grammar is `dev_capture/plan.rs`'s; no second format.
- Windows are authored in seconds (`deadline_s`, `settle_s` already are). No tick counts in data.
- The recipe's `instance` and `heading_deg` are `Fixture` choices of the run, not facts. Its
  numbers are bare JSON today; the step that edits `Recipe` (step 5) moves it to a labelled RON
  file, per "existing bare constants migrate when a lane touches their module".

### A pass starts from a reachable state

Already true on `main`: spawn at rest (`server.rs:954`), 256 ticks of no demand, then full throttle
over the derived run-up; a contact below `minimum_speed_fraction` of governed speed invalidates the
run (`collapse.py:773-774`). Nothing sets a velocity. The house is met on any face by
`heading_deg`; the lab's "quarter turn" becomes a heading.

### Clips on a real client

- The driving client runs with `OVERMATCH_WINDOW=hidden` and the capture plan. One change makes
  that possible: mount `CapturePlugin` on any windowed network client (`client.rs:520`: `!headless`
  in place of `!simulate`). A plan with `keys` under a harness script is refused at plan admission:
  two input owners.
- Frames are anchored on an event the client observes, the pattern `fuze::ShotReady` already uses
  (`dev_capture.rs:557-565`): the first frame its own presented tank comes within the plan's
  approach lead of the target construction. Frame k is taken at anchor + k/8 s.
- Honest limit: frames are on the client's wall clock. Two clips are aligned on the same physical
  event to within one client frame, not tick-exact. The script refuses a run whose client did not
  keep up (the existing `capacity_frames` gate, `collapse.py:292`).
- One view per run: the follow camera. The lab's second fixed camera is a second plan and a second
  run, not a second camera in one World.
- `scripts/debris-lab run <recipe> <out>` calls the ram runner with the plan, then encodes.
  `compare <a> <b> <out>` stacks the two clips side by side, each panel captioned with its run name
  and revision, and writes `comparison.json`. `ffmpeg`, `ffprobe` and `magick` are found with
  `shutil.which` before anything is built or run.

### Same-seed replay as a property

Three things can differ between two runs of one binary; each gets a named owner.

1. **Entity ids.** The lab's ids varied because its view allocated in the server's World. A
   dedicated server process removes that source. It does not prove ids fixed: `Commands::spawn`
   takes its id when its system runs, and Startup systems run in parallel
   (`net/score.rs:204` is one such spawner). UNKNOWN whether any pair races on the shipped server;
   two probed runs settle it.
2. **Order that follows ids.** `freeze.rs:947` (`pieces.sort_unstable()`) switches thawed pieces to
   dynamic in entity order, and Avian builds a switched body's contacts in switch order
   (`freeze.rs:67-71`). **This should be ordered by a declared key on `main` regardless**: an id is
   an allocation accident, and the parallel-work lens asks that effect order not follow execution.
   **Key: the pose bits the piece froze at (`Frozen.pose`, `freeze.rs:163-166`), the entity only
   breaking a tie.** The construction-and-chunk key is not available: a debris piece need not be a
   construction's (`contact_census.rs:267`). `lane/ram-a` `8a7ab552` is this change with its test
   (+51 −9, MEASURED); it moved results there (eight runs over three id layouts matched). Land it
   as its own step. `breakage.rs:2009` sorts owner effects by root entity. A root has an id-free
   identity already, its ordinal in the id-sorted instance list (`scatter.rs:17`, `:48`); whether
   root entity order follows it on the server was not checked. If `compare` ever attributes a
   split there, that ordinal is the key.
3. **Input arrival.** A real client's input crosses UDP. Each row records the parsed throttle and
   steer. `compare` on two runs of one binary and recipe checks inputs first, then every hull and
   structure field, keyed by tick from spawn and by construction and chunk, never by entity. It
   reports one of: equal; inputs differ at tick N (outside input, the pair is not a replay); inputs
   equal and state differs at tick M, field F (an order dependence on the server: a finding against
   the game, with its tick).

Replay is then a checked property with an attributed failure, not a flag someone reads. `compare`
never gates a landing and holds no stored result.

### Provenance

`run.json`, written by the script:
- revision and tracked-diff digest when the script builds; for `--bin`, the revision is required,
  verified with `git rev-parse --verify`, and the run is refused if
  `git diff --quiet <revision> -- assets/` fails;
- sha256 of each binary it started;
- features and profile from the binaries, not the script: one startup line from server and client
  (`cfg!(feature = …)`, `cfg!(debug_assertions)`), parsed like the host line (`collapse.py:92`);
- the recipe as the server echoed it; sha256 of the plan text as the client logged it;
- the map's content digest, already in the handshake (`map.rs:616`);
- the host line, marked unpriced (section 4).
`compare` refuses on a differing recipe, plan digest, clip layout or frame count.

### Failure outcomes

| Failure | Outcome | Where |
|---|---|---|
| Recipe malformed, instance unknown, run-up out of bounds, panel without a face | refused before run, naming the field | `ram.rs:47-73`, `:160-201`, `prepare` |
| Plan malformed, `keys` under a script, output not writable | refused at plan admission | `dev_capture/plan.rs`, `dev_capture.rs:190` |
| A frame never arrives | client exits with failure 3 s after the last due frame | `dev_capture.rs:599` (exists) |
| The anchor never fires (no approach) | the script ends with frames unsaved: client exits with failure | new, beside the script watchdog |
| No contact, slow sim, a process dies | runner raises, kills its children | `collapse.py:859-891` (exists) |
| Contact below the speed gate | run invalid | `collapse.py:773` (exists) |
| Missing encode tools, low-power mode | refused before the build | `scripts/debris-lab` |
| Non-finite reading | panic naming tick and field | recorders |

### One path: deleted or merged

- `src/net/debris_lab/` (1,513 lines), `src/bin/debris_sandbox.rs`, `assets/dev/debris_lab.ron`,
  the `Cargo.toml` bin entry and the lab's two scratch reports never land.
- The lab's `ram`, `passes` and `burst` scenarios become one instrument, `ram.rs`, with the
  earlier event as data (step 5).
- `he_stress.rs` and the wall cue both fire a round at a house. They stay two: one is a periodic
  workload with a hosted user (`scripts/edgegap/session.sh`), the other one aimed cue. They state
  different knowledge. The rebuild uses the cue only.
- The testing lens edit from the lab lane lands in the reviewer's wording (`lenses-1.md:246`).

## 4. Tick cost honesty

The tool does comparison only, and says so in every output.
- `scripts/debris-lab` always passes `--rehearsal`; the host line then reads
  `UNPRICED LOCAL REHEARSAL` (`collapse.py:98-99`).
- A clip run renders on the same machine as the server. Its tick times compare only against
  another run with the same plan on the same machine. `comparison.json` carries
  `"timing": "comparison only, same machine, same plan"`, and prints mean, p50, p95, p99 and max.
- Hosted cost is the same recipe without a clip client: `collapse.sh --ram … --target IP:PORT
  --deployment ID`, which requires kernel evidence of two vCPU (`collapse.py:92-119`). The lab
  script has no hosted mode and adds none.

## 5. Build plan

Estimates are DERIVED from the code each step reuses or replaces.

| # | Step | Files | Lines | Tests (invariant; who relies) | What the lenses ask |
|---|---|---|---|---|---|
| 0 | Thaw order by frozen pose | `src/debris/freeze.rs` (cherry-pick `8a7ab552`) | +51 −9 MEASURED | `thaw_order_follows_no_entity_order`: the scene on fresh and on reused entity slots repeats every pose bit; a mutant thaw in entity order parts at the thaw. Relied on by `compare`. | parallel-work: order keyed, not executed. testing: results move; report them as observations. brevity: the module's ORDER note names the lab; reword to the invariant. |
| 1 | Hull rows and summary. **First useful step for the ram work.** | `src/net/ram.rs` (+~110: lab `metrics.rs` hull part ~60, `hull_contacts` 55), `scripts/perf/collapse.py` (+~45), `collapse.md` (+6), `test_collapse.py` (+~20) | ~210 | (a) In the existing shipped-server ram test (`ram.rs:882`): the hull row is the tank's state after the same step the census snapshots; a mutant sampling before the step (as `sample_speed` does, `ram.rs:264`) shifts every row by a tick. Relied on by crossing, stall and clip alignment. (b) Python: crossing and stall from a hand-made row table. | base-truths: no tank name, no threshold; the excluded belt channel named. error-handling: finite asserts; null only for a named absence. parallel-work: the recorder runs after `TickClosed`, inside `collector_us`, outside `tick_us`. brevity: no second speed sampler. |
| 2 | Structure reading | `src/net/ram/structure.rs` (+~150: lab `wall.rs` 182, minus hole sampling ~45 and the shape guess ~15, plus the in-place class ~25), `collapse.py` (+~15), tests (+~35) | ~200 | (a) Face area clips at grade, ignores other planes and vertex order (the lab's test, kept). (b) A below-grade chunk never enters the share; a group moved less than its own radius is in place, a displaced one is not. Relied on by "how much stands". | base-truths: panel chosen by recipe and geometry; holds for house #2. error-handling: zero-area panel refused at `prepare`. testing: fixture chunks labelled, class invariants only. |
| 3 | Clips and `compare` | `src/net/client.rs` (~6), `src/dev_capture.rs` (+~45 anchor), `dev_capture/plan.rs` (+~25), `scripts/capture/debris-lab.ron` (~15), `scripts/debris-lab` (~150: lab script 192 minus build and manifest code the runner owns), `collapse.py` (+~25 clip client), server and client startup line (~12) | ~290 | (a) Frames count from the anchor, not startup (mounts the shipped `CapturePlugin`, as its tests do). (b) Plan with `keys` under a script is refused. (c) Python: `compare` refuses a differing recipe; reports "inputs differ" apart from "state differs". | code-branching: `scripts/debris-lab` is the named user of the anchor and of capture on a scripted client. error-handling: tools checked first; unsaved frames fail the client. testing: no stored result, no gate. brevity: plan grammar has one home. |
| 4 | Fixture constructions off the map (the hall, flat ground) | generalise `dev_capture/wall.rs:419-487` to place from one data file on server and client before admission | ~90 | The server and the client stand the same construction at the same pose from one file; a mismatch refuses the join. | base-truths and ADR-0014: built at spawn from data, before any replica. code-branching: the offline-only placement and this one become one. |
| 5 | Earlier event as data: a felled house or a round at the wall before the pass; later passes as later joins at the same spawn | `ram.rs` `Recipe` gains `before`; first contact counts the target's loose pieces (`ram.rs:516` reads the root only); recipe moves to labelled RON | ~120 | First contact with a felled target's pieces arms the capture; today it would time out. | base-truths: one number, one job on the recipe. code-branching: `--schedule` plus `--ram` do not become a third mode. |

Steps 0 to 3 total about 750 lines, of which about 350 are Rust, against the lab's 2,058 with
1,521 Rust. Steps 4 and 5 are built on request (section 7).

## 6. Rejected alternatives

- **Land as built, plus the P2 fixes.** The four P2 roots are the composition itself; fixing them
  is this rebuild done inside the lab's files, keeping a third tank bundle alive meanwhile.
- **Keep two stacks** (the lab for feel, `collapse` for cost). Two run-ups, two wall choosers, two
  byte counters that must change together; the lab's own numbers already drifted from `main`'s.
- **A third binary** (`debris_sandbox` with the server in a second World in-process). Still a
  hand-built composition root, and render still shares the machine's pools with the tick.
- **A server-side scripted driver for exact replay.** It removes input variance but drives past
  the shipped input path; recording the parsed input and comparing it is smaller and keeps the
  path real.
- **Tick-stamped frames from the client's view cursor.** Exact pairing, but a new seam between the
  capture and the interpolation clock for a clip a person watches at 8 fps.

## 7. Open decisions for the owner

1. **The hall and flat ground (step 4).** The ram work's second fixture is not on the shipped map.
   Recommendation: run steps 1 to 3 on shipped map houses by heading first; build step 4 only if
   the ram verdict still needs the hall after that.
2. **Passes 2 and 3 and the round-then-drive scenario (step 5).** Recommendation: not now. The ram
   work needs one pass; build step 5 when rubble climbing is the question again.
3. **Belt reversals.** Recommendation: no counter. The rows carry both belt speeds; a count needs a
   jitter threshold, and nobody read the last one.
