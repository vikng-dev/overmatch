# Sensor CCD fix and release sweep price

Subject: the multiplayer tank game's rigid-body driving physics and environment destruction
model; game state only.

## Decision and shipped change

Continue `lane/swept-ccd` from `f64838c8`. Only the sensor/disabled-collider bug fix changes CCD
behaviour. Per-collider motion gating remains rejected in this lane; the ignored hull-first wall
probe and [its original evidence](swept-ccd.md) remain.

The moving-side query extends the existing #1101 exclusion to sensors at its existing site.
Minimum body CCD thickness excludes sensors and disabled colliders and refreshes on either
marker's addition/removal, independently of shape, transform and COM changes. Sweep radius is
unchanged because sleeping and contact recycling also consume it. No setting, threshold,
contact tolerance, speculative law, counter API or instrumentation remains from the experiment.

## Regression and mutant evidence

`src/tests/avian_swept_ccd.rs` states the server's reliance on non-solid attachments leaving solid
collision unchanged. It reads the existing `IS_FAST` flag rather than adding an engine counter.
The thick-body guard makes a thin solid collider fast, adds a sensor/disabled marker, checks that
it is no longer fast, then restores the solid and its fast status. COM is held fixed; the fixture
owns its threshold and reads its step from the physics harness. The wall guard checks geometric
planes: the fast body passes its non-solid attachment's impact plane while its solid collider
stays before the wall. No observed clamped position is a test obligation.

MEASURED: the initial unfixed code failed both guards; both passed with the fix. Each mutation was
then applied alone, run, observed red (test exit 101), and reverted before continuing:

| Mutant | Command filter after `scripts/serial-cargo test --lib --features dev_ui,bitprobe` | Result |
|---|---|---|
| Restore all colliders to the thickness minimum (`if true`) | `a_thin_non_solid_attachment -- --nocapture` | Failed: the thin sensor made the thick body fast |
| Remove `Without<Sensor>` from the sweep query; retain #1101's disabled filter | `a_fast_bodys_non_solid_attachment -- --nocapture` | Failed: the moving sensor stopped the body before its solid wall contact |

MEASURED final gates all passed:

- `cargo fmt --all --check`.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe avian_swept_ccd -- --nocapture`: 2 passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe avian_body_size_metrics -- --nocapture`: 4 passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe swept_ccd_probe -- --ignored --nocapture`:
  1 passed, all three approach speeds, with `OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=hidden OVERMATCH_MUTE=1`.
- `git diff --check`.

Logs: `/private/tmp/ccd-sensor-red.log`, `ccd-sensor-green.log`, `ccd-mutant-thickness.log`,
`ccd-mutant-sweep.log` and `ccd-gates.log` in that same directory. Final rechecks are in
`ccd-final-gates.log` and `ccd-draft-final-check.log`. The final fixtures also hold
COM fixed and derive speed from the harness timestep; these strengthen isolation without
changing the thickness or moving-sensor predicates reversed by the mutants.

The six lenses were read and applied locally to every changed item: no unresolved finding;
no backlog row names an edited file. Independent six-lens landing review remains the
orchestrator's gate. No agents were spawned.

## Independent published-crate proof and source inventory

MEASURED archive SHA-256:
`ca7479ccec5f1440675946bada8c805f2b6d514af78e1d2b0937ca53b1e2c543`.
The [upstream draft](../../upstream/avian-ccd-sensors-swept-and-counted.md) has no fork markers.
Its patch was applied with `patch -p1 --batch` after restoring pristine extracted source, in a
new, initially empty directory:

`/private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/draft-check-ccd/sensors-e_xlweg3/avian3d-0.8.0`.

The sibling `check-harness` depends on that crate, with `default-features = false` and
`features = ["3d", "parry-f32"]`, and contains `pub use avian3d;`. Its dependency composition
leaves unused, uncached published dev-dependencies out of resolution. No published manifest
or source beyond the draft patch was edited. MEASURED: this command passed for Avian and the
harness, without network access:

```sh
scripts/serial-cargo check --offline --lib --manifest-path \
  /private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/draft-check-ccd/sensors-e_xlweg3/check-harness/Cargo.toml \
  --target-dir target/draft-check-ccd
```

MEASURED by `diff -ruN` of the archive's extracted `src/` and the final shipping fork's `src/`:
881 added + 235 removed = **1,116 fork lines**, **1,038** in upstream files plus the unchanged
**78** in the fork-only `stationary_miss.rs`. CCD is **5 added + 1 removed = 6** lines;
body-size metrics, including its prior batching patch, is **98 + 27 = 125**. The preceding
inventory was **1,071 / 993 / 78**, CCD **6**, metrics **80** (MEASURED in the original lane
report). [OVERMATCH_PATCH.md](../../vendor/avian3d-0.8.0-overmatch/OVERMATCH_PATCH.md) records the
combined totals. MEASURED after the new README row: **29** open-candidate rows, **12** for Avian.

## Dedicated release measurement

Basis: `f64838c8` plus this sensor fix, with no other CCD behaviour change. The normal release
server/client were built through the queue:

```sh
scripts/serial-cargo build --release --locked --bin overmatch-server --bin overmatch --features dev_ui,bitprobe
```

MEASURED host: Apple M4, macOS 26.5.2; the server reported available parallelism 10 and
compute/async/IO pools 10/1/1. This is a local release rehearsal, not a hosted two-core price.
Every game process had `OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=hidden OVERMATCH_MUTE=1`.
Every run held `scripts/build-lock`; its 1-minute load was checked before launch, waiting while
above 4. The table preserves the admitted load for each run; the raw record includes all waits.

The existing release harness already serializes `SolverDiagnostics::swept_ccd` as
`TickSample.avian.swept_ccd_us`; no timer exposure change was needed. Temporary, uncommitted
instrumentation carried two integer counts through the existing per-body `CcdResult`, then
reduced them beside its existing result collection. It counted fast bodies after the body gate,
supported moving colliders entering the sweep, and outer collider-pair `compute_ccd_toi`
dispatches. Internal Parry retries and composite-subshape recursion are excluded. The existing
timer includes this counting work. All instrumentation was removed from shipping source.
The exact instrumentation diff and measured binary digests are in the compressed run record.

The larger case uses the named rehearsal's recipe and populations:
`scripts/perf/collapse.sh --target local --schedule scripts/perf/match-collapse.json --rehearsal --tanks 30 --clients 1`,
with `--bin target/release`. A local wrapper retained its admission, population, combat, sample
continuity and schedule checks, and set the required hidden window instead of the runner's
GPU-less default. The observation keeps ticks whose instrument `elapsed_s <= 60`. Its cohort
is 29 circling bots plus one scripted client; the counters cover every fast body, including
construction bodies after departures, not just tanks.

The single-tank case uses the same release server and real hidden client, no bots, on the
shipped map at XZ (120, 120). Spawn Y is DERIVED from the shipped heightmap's nearby maximum
plus the existing spawn clearance; its exact inputs/result are in the raw record. DERIVED from the level's instance translations and that XZ point: the nearest authored
construction origin is 243.469 m. An out-of-window event at 3600 s admits the
existing trace without causing a collapse. The client runs the existing Arc script, with its
primary click beyond the run. We keep instrument ticks 257–512, the nominal drive/acceleration
window; admission and client release have separate epochs, so this is not a claim of an exact
command boundary. Every selected tick has one tank and zero scheduled events. Fast-tick
statistics below separate actual sweep work from acceleration ticks below CCD's threshold.

The first cold hidden-client attempt lost its connection, leaving 29 tanks; it is excluded.
A separate GPU warm-up reached script release and five consecutive complete population/link
periods before the retries. All three measured rehearsals kept 30 tanks, one link and combat,
and completed their scheduled events. All missed the runner's capacity verdict: server tick
rates were MEASURED 63.236 / 62.649 / 63.524 Hz and none of the hidden clients met its frame
coverage gate. Their physics timing samples are retained for this dedicated price; no capacity
claim follows. The single-tank runs passed the capacity gate.

All following numbers are **MEASURED**, from **three runs per case**. Each slash-separated cell
is **mean / p50 / p99 / worst** per tick. Timings are microseconds; quantiles use nearest rank.

| Run | Ticks | Pre-run load | CCD, us | Physics total, us |
|---|---:|---:|---|---|
| 30-tank-1 | 3807 | 3.935 | 772.479/899.834/2431.583/6009.792 | 2201.431/1413.125/12724.083/17568.709 |
| 30-tank-2 | 3772 | 3.329 | 462.527/425.500/1984.375/4776.958 | 2390.315/1077.000/14247.750/17771.709 |
| 30-tank-3 | 3824 | 3.563 | 744.608/859.042/2339.708/5444.458 | 2149.578/1383.125/12088.542/16729.583 |
| 1-tank-1 | 256 | 3.350 | 32.595/50.500/113.166/141.292 | 195.219/198.625/410.292/453.375 |
| 1-tank-2 | 256 | 3.746 | 30.671/49.417/65.916/102.291 | 193.907/197.875/328.250/380.500 |
| 1-tank-3 | 256 | 3.999 | 33.435/50.250/85.208/104.792 | 196.131/197.125/340.459/346.500 |

The same runs' per-tick work counts (same statistic order):

| Run | Fast bodies | Colliders swept | TOI pair queries |
|---|---|---|---|
| 30-tank-1 | 19.610/23.000/66.000/77.000 | 257.309/243.000/2139.000/6036.000 | 153.000/110.000/1951.000/5707.000 |
| 30-tank-2 | 12.102/11.000/67.000/84.000 | 189.907/111.000/1999.000/5850.000 | 134.820/51.000/1890.000/5626.000 |
| 30-tank-3 | 18.783/22.000/54.000/65.000 | 240.730/232.000/1851.000/6025.000 | 139.586/105.000/1677.000/5692.000 |
| 1-tank-1 | 0.574/1.000/1.000/1.000 | 6.316/11.000/11.000/11.000 | 2.871/5.000/5.000/5.000 |
| 1-tank-2 | 0.574/1.000/1.000/1.000 | 6.316/11.000/11.000/11.000 | 2.871/5.000/5.000/5.000 |
| 1-tank-3 | 0.613/1.000/1.000/1.000 | 6.746/11.000/11.000/11.000 | 3.066/5.000/5.000/5.000 |

The single-tank drive windows include acceleration. On their fast ticks alone:

| Open run | Fast ticks | CCD, us | Physics total, us |
|---|---:|---|---|
| 1 | 147 | 56.095/52.875/132.625/141.292 | 230.052/220.083/426.042/453.375 |
| 2 | 147 | 52.761/52.084/77.417/102.291 | 226.257/220.458/359.750/380.500 |
| 3 | 157 | 53.857/52.042/86.917/104.792 | 226.098/222.542/343.416/346.500 |

MEASURED on every fast tick in all three open-ground runs: **1 fast body, 11 colliders swept,
5 TOI pair queries**. The drive-window means differ from these counts because the remaining
acceleration ticks have no fast body. The earlier flat-bed probe's zero TOI queries remains
an observation of that different fixture, not a universal ground-query count.

MEASURED rehearsal CCD means span **462.527–772.479 us**, with per-run p99 **1,984.375–2,431.583 us**
and worst **4,776.958–6,009.792 us**. Open-drive means span **30.671–33.435 us**; on fast ticks
alone the means span **52.761–56.095 us**. These ranges describe these repeated runs and their
workloads; they are not a forecast of savings from per-collider gating or a hosted allocation.

## Reviewable run record and trade

[swept-ccd-pricing.json.gz](swept-ccd-pricing.json.gz) holds every selected tick's raw timer and
count values, schemas, per-run summaries, pre-run load checks, capacity verdicts, scripts, recipes,
compiled binary digests and the temporary instrumentation diff. It is evidence, never a test
oracle. Read with Python's `gzip.open` and `json.load`; recompute quantiles from the named columns.
Full original runner artifacts remain under the requested scratch root's
`scratchpad/swept-ccd/pricing/`; the driver and capture logs are `/private/tmp/ccd-pricing.py`,
`ccd-pricing-final.log` and `ccd-pricing-retry.log`. The first invalid startup attempt and warm-up
are separate directories and contribute no pricing samples.

MEASURED in the retained hull-first wall probe: at approach speeds 6 / 8 / 10 m/s, existing hull
sweeps gave peak intrusion 0 / 0 / 0 mm; the rejected per-collider gate gave 0 / 24.153 / 55.403 mm.
Neither tunneled in those observations. The final sensor-fixed probe again measured zero peak
intrusion at all three speeds. Thus the evidence shows the hull sweep preserving initial wall
contact at the release cost above. It does not measure how much a different per-collider policy
would save, and supplies no recommendation beyond that trade.

Nothing remains to implement in this lane. Upstream publication and the orchestrator's independent
landing review/merge are intentionally outside it. No push, external filing, reset or stash was used.
