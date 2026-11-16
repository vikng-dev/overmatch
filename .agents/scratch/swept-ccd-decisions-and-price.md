# Swept CCD decisions and repeated release price

Subject: the multiplayer tank game's rigid-body driving physics and environment destruction
model; game state only.

Continue `lane/swept-ccd` on top of `602c0a57`. This report supersedes the decisions in
[swept-ccd.md](swept-ccd.md) and [swept-ccd-sensors-and-price.md](swept-ccd-sensors-and-price.md);
their observations remain historical evidence.

## Per-collider decision

The per-collider gate lands. The previous AABB probe's 24.15 / 55.40 mm peaks at 8 / 10 m/s
were ordinary first-step contact: DERIVED at 64 Hz, travel past the fixture's 0.1 m gap is
25 / 56.25 mm. The earlier zero peaks came from thin track-side slices selecting the unrelated
hull for CCD. They were not an engine obligation.

The body early-out remains. Each supported collider then compares the conservative body motion
bound with its own thickness before mass properties, swept bounds or trees. Reusing the body's
bound includes offset rotation and preserves upstream single-collider eligibility. No default,
threshold or speculative-contact law changes.

## Verification record

MEASURED: all five current CCD tests pass. Each mutant below was applied alone, run through
`scripts/serial-cargo test --lib --features dev_ui,bitprobe <filter> -- --nocapture`, observed red
(exit 101), then restored. No mutant remains in shipping source.

| Mutant | Test filter | Failure |
|---|---|---|
| Remove the per-collider gate | `a_thin_child_does_not_force` | Thick child clamped at its wall |
| Skip all moving colliders after support selection | `a_fast_thin_child_and_a_single_slab` | Thin single collider crossed its wall |
| Replace the collider motion bound with COM translation | `an_offset_rotating_child` | Offset child rotated through its wall |

Logs: `/private/tmp/ccd4-mutant-{no-collider-gate,no-thin-sweeps,translation-only}.log`;
structured exit results: `/private/tmp/ccd4-mutants.json`. The unmutated run is
`/private/tmp/ccd4-green.log`.

MEASURED: both independent published-crate drafts compile offline, with unchanged published
manifests and small path-dependency harnesses (`default-features=false`, `3d,parry-f32`).
Both diffs dry-apply cleanly with `patch -p1` to a pristine extraction. Commands and archive
SHA-256 are in the drafts. Logs: `/private/tmp/ccd4-{per-collider,sensor}-draft.log`.
The sensor observer/thickness proposal is compile-checked only; it is not a shipped runtime claim.

At the per-collider batch boundary, MEASURED source inventory against published 0.8.0:
890 added + 236 removed = 1,126 lines, including 78 in the fork-only file; CCD 14+2=16,
metrics 98+27=125. Subsequent batches remove the thickness change and remeasure the merged tree.

Each commit runs formatting, CI clippy and the affected CCD/metrics tests. Probe/track tests
run when those sites change. Independent landing review remains the orchestrator's gate.

MEASURED per-collider commit gates: `cargo fmt --all --check`; CI clippy with
`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`; targeted
`avian_swept_ccd` (5 passed) and `avian_body_size_metrics` (4 passed), all through
`scripts/serial-cargo`, plus `git diff --check`. Log: `/private/tmp/ccd4-d1-gates.log`.

## Non-solid scope

The fork retains only moving-side query exclusions and their wall-contact test. The thickness
half and its transition test are removed. MEASURED before merging main: metrics is back to
60 added + 20 removed = 80 fork lines; the source tree is 853+229=1,082 lines, including
78 fork-only lines. The CCD skip now names non-solid colliders alongside missing poses.

The sensor draft retains the broader thickness proposal with per-backend persistent queues
fed by removal observers. It is compile-checked only. Sweep radius has CCD-speed and
contact-recycling readers; the previous sleeping claim in the draft and patch inventory is fixed.
The helper-restating module-doc line is deleted. Probe's `or_else` cleanup lands with its rewrite.

MEASURED after narrowing: removing only `Without<Sensor>` failed the retained non-solid
wall-contact test (exit 101); the sensor clamped the body before its solid collider reached the
wall. Source restored. Log: `/private/tmp/ccd4-mutant-moving-sensor.log`.

MEASURED narrowed-sensor commit gates: formatting and CI clippy passed; `avian_swept_ccd`
4 passed, `avian_body_size_metrics` 4 passed. The final diff check caught whitespace-only
context lines in fenced draft patches; those document lines were trimmed and the diff rechecked.
Log: `/private/tmp/ccd4-d2-gates.log`.

## Signed hull/wall probe

The probe uses Avian's signed convex shape contact distance at the current body/local-collider
pose, including hull tilt, rather than a bounding-box minimum across its width. Shape queries
place the wall at 0.1 m signed separation. It asserts no tunnelling and peak intrusion no greater
than one tick's travel at the test speed (plus floating-point query roundoff); settled distance
is only reported. The first-touch update now uses `or_else`.

MEASURED before lazy construction, one probe run, metres (negative settled intrusion means
separation). All three approach speeds passed:

```text
CCD_PROBE speed=6 peak_intrusion_m=0 settled_intrusion_m=-0.006247805 tick_travel_m=0.09375 velocity=Vec3(-0.005075951, 0.00056405476, 0.009142528) first_touch=Some("hull proxy Hull_Collider")
CCD_PROBE speed=8 peak_intrusion_m=0.024152366 settled_intrusion_m=-0.0015402351 tick_travel_m=0.125 velocity=Vec3(-0.0109014185, 0.010689352, 0.0019009099) first_touch=Some("hull proxy Hull_Collider")
CCD_PROBE speed=10 peak_intrusion_m=0.05540262 settled_intrusion_m=-0.0027852983 tick_travel_m=0.15625 velocity=Vec3(-0.013570169, 0.014060323, 0.0056100953) first_touch=Some("hull proxy Hull_Collider")
```

MEASURED probe commit gates passed: formatting, CI clippy, CCD 4/4, metrics 4/4, ordinary
track-scorecard tests, and the ignored signed-distance probe. Log: `/private/tmp/ccd4-d3-gates.log`.

## Lazy sweep shape

A local `LazyCell` constructs `CcdFastShape` only when a candidate passes all filters and reaches
TOI dispatch, once per moving collider. Eligibility still needs thickness before the gate and
reuses that value in the shape. This is a three-line lazy-initialization change, so its patch
and fresh published-crate compile proof are folded into the per-collider draft. No switch or
extra counter API ships.

MEASURED: the three signed-probe output lines, including peak, settled and velocity values,
are identical before/after this construction-only change. This is one report comparison, not a
persistent output-pinning test. The published-crate check, formatting, CI clippy, CCD 4/4, metrics
4/4 and signed probe passed. Log: `/private/tmp/ccd4-d4-gates.log`. Before merging main the
source tree is MEASURED 856 added + 231 removed = 1,087 lines; CCD 18+4=22, metrics 60+20=80.

## Release pricing: before and after

All values below are **MEASURED**, three runs per case, local release rehearsal on the same
Apple M4. Each timing cell is mean / p50 / p99 / worst in microseconds, nearest-rank quantiles.
Before is `602c0a57`'s retained release record; after is the four decisions before merging main.
Every after run held `scripts/build-lock`, with admitted one-minute load at or below 4. Both
binaries were built with `scripts/serial-cargo build --release --locked --bin overmatch-server
--bin overmatch --features dev_ui,bitprobe`. Every game process used the required unattended,
hidden, muted environment. A separate unmeasured GPU warmup preceded measured runs.

The driver repeats the earlier recipes and windows: 30 tanks/one real client through 60 s of
the collapse rehearsal; one tank/one real client on the same open-ground spawn, ticks 257–512.
Population, link, combat, continuity and schedule checks are retained. Capacity verdicts remain
observations and are preserved in the raw record; these timings do not attest hosted capacity.

| Run | CCD before | CCD after | Physics total before | Physics total after |
|---|---|---|---|---|
| 30-tank-1 | 772.479/899.834/2431.583/6009.792 | 132.064/96.583/895.500/4511.625 | 2201.431/1413.125/12724.083/17568.709 | 2350.740/775.000/12709.542/16891.084 |
| 30-tank-2 | 462.527/425.500/1984.375/4776.958 | 142.980/103.500/673.584/4133.125 | 2390.315/1077.000/14247.750/17771.709 | 2782.821/766.250/13303.375/17161.542 |
| 30-tank-3 | 744.608/859.042/2339.708/5444.458 | 119.177/91.667/664.625/4367.125 | 2149.578/1383.125/12088.542/16729.583 | 2303.698/701.625/12374.166/17139.667 |
| 1-tank-1 | 32.595/50.500/113.166/141.292 | 4.721/6.375/13.667/16.708 | 195.219/198.625/410.292/453.375 | 168.187/160.042/303.458/323.000 |
| 1-tank-2 | 30.671/49.417/65.916/102.291 | 4.761/6.167/14.166/27.208 | 193.907/197.875/328.250/380.500 | 167.549/162.417/282.833/326.375 |
| 1-tank-3 | 33.435/50.250/85.208/104.792 | 4.773/6.250/14.500/22.958 | 196.131/197.125/340.459/346.500 | 166.568/160.584/304.042/373.625 |

Same statistic order for per-tick counts. Swept colliders are counted after the per-collider
gate, before swept bounds; fast bodies are results after the body gate; TOI queries are outer
collider-pair dispatches, excluding internal Parry retries or subshape recursion. Temporary
counters are reduced through existing per-body results, as before, and are removed after pricing.

| Run | Phase | Ticks | Admitted load | Fast bodies | Swept colliders | TOI queries |
|---|---|---:|---:|---|---|---|
| 30-tank-1 | before | 3807 | 3.935 | 19.610/23.000/66.000/77.000 | 257.309/243.000/2139.000/6036.000 | 153.000/110.000/1951.000/5707.000 |
| 30-tank-1 | after | 3777 | 3.701 | 22.431/25.000/74.000/83.000 | 120.438/100.000/849.000/5872.000 | 44.335/0.000/711.000/5556.000 |
| 30-tank-2 | before | 3772 | 3.329 | 12.102/11.000/67.000/84.000 | 189.907/111.000/1999.000/5850.000 | 134.820/51.000/1890.000/5626.000 |
| 30-tank-2 | after | 3786 | 3.604 | 24.218/28.000/60.000/76.000 | 130.160/109.000/580.000/5884.000 | 48.708/0.000/455.000/5556.000 |
| 30-tank-3 | before | 3824 | 3.563 | 18.783/22.000/54.000/65.000 | 240.730/232.000/1851.000/6025.000 | 139.586/105.000/1677.000/5692.000 |
| 30-tank-3 | after | 3782 | 3.912 | 20.364/23.000/53.000/62.000 | 106.109/89.000/586.000/5856.000 | 37.710/0.000/484.000/5556.000 |
| 1-tank-1 | before | 256 | 3.350 | 0.574/1.000/1.000/1.000 | 6.316/11.000/11.000/11.000 | 2.871/5.000/5.000/5.000 |
| 1-tank-1 | after | 256 | 3.476 | 0.602/1.000/1.000/1.000 | 2.406/4.000/4.000/4.000 | 0.000/0.000/0.000/0.000 |
| 1-tank-2 | before | 256 | 3.746 | 0.574/1.000/1.000/1.000 | 6.316/11.000/11.000/11.000 | 2.871/5.000/5.000/5.000 |
| 1-tank-2 | after | 256 | 3.329 | 0.613/1.000/1.000/1.000 | 2.453/4.000/4.000/4.000 | 0.000/0.000/0.000/0.000 |
| 1-tank-3 | before | 256 | 3.999 | 0.613/1.000/1.000/1.000 | 6.746/11.000/11.000/11.000 | 3.066/5.000/5.000/5.000 |
| 1-tank-3 | after | 256 | 3.796 | 0.609/1.000/1.000/1.000 | 2.438/4.000/4.000/4.000 | 0.000/0.000/0.000/0.000 |

Open-ground fast ticks alone (MEASURED; acceleration ticks below the body gate excluded):

| Run | Fast ticks | CCD | Physics total | Distinct (fast bodies, swept colliders, TOI queries) |
|---|---:|---|---|---|
| 1-tank-1 | 154 | 7.176/6.917/14.708/16.708 | 178.650/170.250/303.458/323.000 | [(1, 4, 0)] |
| 1-tank-2 | 157 | 7.109/6.708/16.375/27.208 | 178.978/176.583/282.833/292.125 | [(1, 4, 0)] |
| 1-tank-3 | 156 | 7.195/6.875/16.750/22.958 | 176.737/168.958/343.167/373.625 | [(1, 4, 0)] |

Raw after samples, summaries, admission waits, capacity verdicts, scripts, recipes, binary digests
and the exact temporary instrumentation diff are in [swept-ccd-pricing-after.json.gz](swept-ccd-pricing-after.json.gz).
The [before record](swept-ccd-pricing.json.gz) is unchanged. Full after runner artifacts remain
at `/private/tmp/ccd4-pricing`; capture log: `/private/tmp/ccd4-pricing-run.log`. No conclusion
is drawn from a single run, and no observed timing or count becomes a test obligation.

## Merged-tree source census

Merged main `f279a1a57c58a7e56c9900bdff09bceb1db0f7f5` into this bay after pricing. The two conflicts were the Avian patch
inventory and upstream index: both retain main's current seams, drafts and retirement records,
plus this lane's two CCD proposals. Source merged without conflict. Temporary pricing counters
were restored byte-for-byte to the shipping source before the merge; none ship.

MEASURED again on the merged tree, using `diff --minimal -u` per source file against freshly
extracted cached published archives, including fork-only source. Full per-file counts and archive
digests: [swept-ccd-fork-lines.json](swept-ccd-fork-lines.json).

| Crate | Added | Removed | Total |
|---|---:|---:|---:|
| avian3d | 704 | 167 | 871 |
| bevy_pbr | 27 | 20 | 47 |
| lightyear_netcode | 140 | 19 | 159 |
| lightyear_avian3d | 0 | 0 | 0 |

MEASURED combined fork total: **1077** lines. Avian includes **78** fork-only
lines; CCD is **18+4=22**, metrics **43+18=61**, matching merged main for metrics.
The index has **19** current draft rows, **13** targeting Avian. CCD proposals account for
**7** non-solid query/skip lines and **15** eligibility/lazy-shape lines; the thickness proposal
has no shipping lines. All other per-file counts were remeasured and are in the linked census.

Across all three repetitions per case, MEASURED CCD means fell from 462.527–772.479 to
119.177–142.980 us in the rehearsal, and from 30.671–33.435 to 4.721–4.773 us in open driving.
Rehearsal physics means were 2,149.578–2,390.315 before and 2,303.698–2,782.821 us after;
there is no consistent reduction in the total. Populations match but resulting contacts and
fast-body counts differ; the table is a repeated workload observation, not a paired deterministic
simulation or hosted two-core forecast. Every open-ground fast tick after the change had
1 fast body, 4 swept colliders and 0 TOI pair queries, versus 1/11/5 before.

## Track scorecard

MEASURED: one full scorecard run on the merged tree. All law lines passed. The comparison
is with the pre-existing local card saved before this run; it embeds no source revision, so
these are observations across the available snapshots, not an isolated A/B attribution to CCD.
The complete before/after cards are in [swept-ccd-scorecard.json.gz](swept-ccd-scorecard.json.gz).
All rows of the selected observation families are shown below; moved outputs are not test gates.

| Family | Case | Quantity | Before | After |
|---|---|---|---:|---:|
| turning | wide, short run-up | radius_m | 101.004 | 101.004 |
| turning | wide, short run-up | speed_m_s | 4.92772 | 4.92772 |
| turning | tight, short run-up | radius_m | 35.6274 | 35.6274 |
| turning | tight, short run-up | speed_m_s | 4.90631 | 4.90631 |
| turning | wide, long run-up | radius_m | 212.478 | 212.478 |
| turning | wide, long run-up | speed_m_s | 9.95398 | 9.95398 |
| turning | tight, long run-up | radius_m | 75.827 | 75.827 |
| turning | tight, long run-up | speed_m_s | 9.25634 | 9.25634 |
| turning | neutral pivot | radius_m | 0.00012752 | 0.00012752 |
| turning | neutral pivot | speed_m_s | 1.63507e-05 | 1.63507e-05 |
| acceleration | concrete | top_speed_m_s | 10.6489 | 10.6489 |
| acceleration | dirt | top_speed_m_s | 10.5474 | 10.5474 |
| acceleration | gravel | top_speed_m_s | 10.5992 | 10.5992 |
| acceleration | mud | top_speed_m_s | 4.35821 | 4.35821 |
| acceleration | sand | top_speed_m_s | 4.84958 | 4.84958 |
| acceleration | sandy loam | top_speed_m_s | 10.6328 | 10.6328 |
| climbing | step 0.2 m | max_rise_m | 0.210349 | 0.210349 |
| climbing | step 0.2 m | time_s | 4.5625 | 4.5625 |
| climbing | step 0.4 m | max_rise_m | 0.448245 | 0.447625 |
| climbing | step 0.4 m | time_s | 5.09375 | 5.07812 |
| climbing | step 0.6 m | max_rise_m | 0.616829 | 0.616829 |
| climbing | step 0.6 m | time_s | 6.51562 | 6.51562 |
| climbing | step 0.8 m | max_rise_m | 0.00444801 | 0.00394517 |
| climbing | step 0.8 m | time_s | — | — |
| climbing | step 1 m | max_rise_m | 0.00394517 | 0.00394517 |
| climbing | step 1 m | time_s | — | — |
| climbing | step 1.2 m | max_rise_m | 0.00394517 | 0.00394517 |
| climbing | step 1.2 m | time_s | — | — |
| walls | full width, 10 m | max_rise_m | 0.00394517 | 0.00394517 |
| walls | Left belt lane only, x -1.957 .. -1.132 m | max_rise_m | 0.117589 | 0.0904063 |
| walls | Right belt lane only, x 1.132 .. 1.957 m | max_rise_m | 0.0990936 | 0.0929793 |
| trenches | 0.9012500047683716 | max_sink_m | 0.0575465 | 0.0575465 |
| trenches | 0.9012500047683716 | time_s | 5.32812 | 5.32812 |
| trenches | 1.8025000095367432 | max_sink_m | 0.112054 | 0.112054 |
| trenches | 1.8025000095367432 | time_s | 5.5 | 5.5 |
| trenches | 2.7037501335144043 | max_sink_m | 0.180908 | 0.180908 |
| trenches | 2.7037501335144043 | time_s | 5.67188 | 5.67188 |
| trenches | 4.506249904632568 | max_sink_m | 0.45413 | 0.447837 |
| trenches | 4.506249904632568 | time_s | — | — |

Card law verdicts (MEASURED): slope=True, ride=True, rubble=True.

## Final verification and remaining work

MEASURED on the merged tree: formatting passed; CI clippy passed with the required feature set
and warnings denied; all 109 `tests::avian` tests passed, all 16 ordinary track-scorecard tests
passed (8 demand-run observations ignored), the signed probe passed, and the full track scorecard
ran once and passed (107.98 s). Its three law lines all passed. The merged signed-probe outputs
are identical to the pre-merge outputs above. Log: `/private/tmp/ccd4-merged-gates.log`.

Both published-crate patches dry-apply from their final Markdown text, and both offline library
checks passed; per-collider was rechecked with lazy construction included. Sensor observer
thickness remains compile-only and unshipped.

All six lenses were applied locally to the changed items; no unresolved lane finding or backlog
row names one. Independent landing review belongs to the orchestrator. No agent was spawned, no
web access, external filing, push, reset, stash or target deletion was used. The subsequent
collapse-tail investigation and review fixes are recorded in
[swept-ccd-collapse-diagnosis.md](swept-ccd-collapse-diagnosis.md) and the
[controlled experiment](swept-ccd-controlled-collapse.md); the live-run causal landing blocker remains open.
The release prices precede the main merge; the final scorecard and source census use the merged
tree. Local capacity was not established by the 30-tank runs.
