# Fair test of the rubble bed: the game's destruction model

Game state only: pieces, joints, loads and fill. Baseline `c81c068eede55e5d4ed3c88d7e00ff4008b91ecf`, the checked-out `main` tip when this lane began; branch `lane/rubble-bed-fair`. Only this report is retained.

- Cheaper for the server: **YES for the measured local activity cycle**, with a collapse-only cost increase.
- Beaching / traversal no worse: **NO** — every arm crosses, but some passes are slower than control.
- Pile survives: **YES**.
- Parked no worse: **NO**.
- Ride no worse: **UNCLEAR** — smaller shocks and no qualifying launches, but greater pitch.

**Verdict: worth building changed.** The continuous height-surface representation earns another build: it retains matter, reduces the exercised collapse-plus-three-pass workload's local mean cost, and rescues the already-beached fixture. The tested bearing implementation does not meet the owner's complete no-worse bar. Its point-raster patch width/area, conservative AABB underside and level-ground capacity on a crest remain approximations. The next build should make the loaded patch and underside geometric, account for slope/crest/eccentric loading, and establish a coherent give/commit law before taking on wire and view. This experiment does not establish that those corrections will pass; it does establish why selecting a nicer material pair would be the wrong substitute.

The numbers behind those decisions are **MEASURED local debug means** of **1.28–1.49 ms** for the bed versus **2.07–2.31 ms** for control across the complete collapse plus repeated-drive workload; **DERIVED 35.0–41.9% lower** in matched repetitions. First/fresh traversal takes **DERIVED 6.766–7.172 s** versus **6.156 s**, a **9.9–16.5%** delay. Nominal's third pass takes **13.422 s** versus **11.156 s**, **20.3%** longer. Primary parked endpoint travel is **DERIVED 3.787 / 6.216 / 1.506 cm** for loose/nominal/stiff versus **1.613 cm** for control. All end asleep and cease downward motion; the extra finite movement still counts against their actual control.

## Decision and comparison bar

The owner's bar is today's game on the same input, not perfect traversal or automatic rescue. A persistent beach in both arms is not itself a failure of the bed. Fresh collapse is the production-relevant comparison; conversion of an already welded pile is a separate migration diagnostic. No verdict below treats a carrier becoming conserved fill as lost matter.

## Method and limits

**MEASURED** means samples from the final independent process executions, or their phase aggregates. Ranges span all final repetitions; a timing mean/p99 range spans the separate executions' phase statistics. **DERIVED** marks durations from the shipped tick rate, acceleration from adjacent-tick vertical velocity, geometry/volume integration, percentages and engineering estimates. Equal endpoints describe repeatability of this fixture, not variation across placements. Development executions are excluded. The final set contains **MEASURED 90 executions across 30 configurations**, with **three independent processes** per configuration. Every configuration has an identical physical-telemetry hash across its repetitions after removing only the timing fields; timing ranges remain non-degenerate. A harness refinement extended driving windows and moved park onset to first rest. Its **36 unchanged rescue/mound executions** were retained and **54 driving/parking executions** rerun; the bed-model source hash is identical across both harness versions. Superseded streams are excluded.

The three preselected pairs are **MEASURED configurations**, not measured material properties: loose **34 degrees / 0.55**, nominal **38 degrees / 0.60**, stiff **44 degrees / 0.64**. None changed after an outcome. Fill uses the shipped Brick density, **MEASURED by asset/source read 1,845 kg/m3**; the whole-house mixture is represented by that single granular row. This does not establish mixed-material bulk properties. The specified Vesic expression is implemented as supplied, without an independent geotechnical validation. Its pressure is `q = rho * packing * g * B * (Nq + 1) * tan(phi)`, with `Nq = exp(pi * tan(phi)) * tan(pi/4 + phi/2)^2`; patch capacity is `q * area`. This is the supplied half-gamma-B-N-gamma form after cancellation, with radians inside the functions. Belts use the shipped gravel ground response as an explicit stand-in for loose masonry; no new grip or rolling number was tuned.

### A continuous physical surface

The lock pins Parry `0.31.1`; Avian is the vendored `0.8.0` fork. Verified sources: `vendor/avian3d-0.8.0-overmatch/src/collision/collider/parry/mod.rs:1193`, the locked Parry `src/shape/heightfield3.rs` and the existing `src/terrain_grid.rs:738` constructor. Parry exposes `set_cell_status`/`CELL_REMOVED`, internal-edge correction and read-only `heights()`, but no height-sample mutation API. Accordingly, each changed tile gets a newly constructed heightfield shape on its existing static entity.

**MEASURED apparatus configuration:** quarter-metre node spacing, millimetre depth units, sparse integer node storage, tiles with **32 by 32 cells / 33 by 33 samples**. **DERIVED:** tiles span **8 m**, and an interior node's triangular hat integrates to **0.0625 m2**, making one depth unit **0.0000625 m3** of bulk fill. Zero quads are removed; zero half-facets inside partially filled quads were not individually removed and can remain coplanar with the base at the fringe. Filled quads use the same anti-diagonal as the belt sampler and `FIX_INTERNAL_EDGES`; there are no vertical column faces. Tile boundaries share identical nodal heights. Their normals remain faceted, and internal-edge correction has no cross-tile adjacency.

The finite probe domain is **MEASURED configuration 256 m per side**, refusing out-of-domain deposits; it is not a volume sink. A separate, single-owner sample slab supplies fast belt queries and updates only changed samples. The immutable base grid is untouched. This avoids charging the bed for copying that entire enlarged slab on each update; production sparse tile lookup and border ownership are still unimplemented.

Tiles have `Layer::Terrain` and `FieldSurface`. Both `BlockField::contact_with_grid` and the force column's `Plane::sample` read the committed bed samples, so belts use the terrain path, not the convex "every other solid" path. The latter excludes these colliders, preventing double belt support. Hulls and pieces use the Avian heightfield. The same commit publishes both readers and shapes. Sleeping readers overlapping a changed tile wake; retained sleeping loads remain eligible for bearing. No body pose, velocity or force is written by the bed law.

### Volume, flow and loads

`merge_fragments` returns its unmerged input in the bed arm. `route` queues each sub-floor chunk once, at the event that frees it; the ordered post-split commit deposits its admitted occupied volume under its current transformed chunk AABB. This is an in-place deposit, without a flight/landing distribution. Projected AABB overlap weights approximate rotated convex footprints. A fractional depth-unit accumulator carries quantization rather than discarding matter.

Bearing groups solver point loads by body and sampled terrain belt loads by side. Connected raster contact nodes form a patch; occupied-node area and the narrow bounding width enter the given capacity expression. This remains a contact-point/raster patch approximation, not a geometric contact polygon. An overloaded node's bearing withdrawal is **DERIVED 2 mm/tick**, the integer floor of `g dt^2`. Repose is a separate conservative redistribution, not an independent rate-limited compaction law.

Displaced volume enters the nearest admissible outward ring. Bearing may heave fill uphill because it is driven by the loading body; a capped repose export must finish below its source after withdrawal. This makes each repose transfer decrease the integer squared-height potential, preventing uphill redistribution cycles without changing the material bracket. A slope-compatible capacity envelope around conservative projected body AABB undersides prevents exports cycling back into blocked cells. Injection checks every incident collision triangle's gradient. Repose then relaxes the actual triangular facets, conserving integer volume; it does not merely bound neighbouring sample differences. Its small angular excess is the declared millimetre quantization allowance. The conservative underside can forbid space that an exact rotated hull would leave free. Exact contact-area integration and obstacle geometry remain production work.

The primary arm commits changed tiles every tick. A separately declared nominal diagnostic commits every **MEASURED configuration four ticks**, **DERIVED 62.5 ms**, while the same laws continue each tick. It is not a physical-parameter retune.

### Scenarios and timing

All runs mount the shipped simulation plugin and use `OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1`. The unchanged control has the intervention inactive. **DERIVED timebase: 64 ticks/s.** One experiment process at a time runs through `scripts/serial-cargo test --lib --features dev_ui,bitprobe rubble_bed_fair_probe -- --ignored --nocapture --test-threads=1`; builds and other queued tests cannot overlap that process. Arm order reverses in the middle repetition. The operating system, read-only analysis and unrelated background work are not isolated. These timings therefore carry local scheduling noise.

- **A, fresh collapse:** the existing house support/bottom-band collapse input, with merging disabled before the freeing event in the bed arm. Both arms record the same **DERIVED 10 s** collapse window.
- **B, free drive:** full demand for **DERIVED 25 s** per pass, repeated across the evolving pile. Only the tank is repositioned between passes, with **DERIVED 2 s** of zero demand; pile and drivetrain state persist. The crossing marker is the same world Z in both arms, **MEASURED fixture configuration -27.3 m**, beyond the original pile. The fresh-lane comparison uses a separate identically collapsed world, not an adjacent strip falsely claimed untouched inside this narrow pile.
- **C, stuck start:** the existing standing helper, **MEASURED fixture offset 1.4 m** and approach **2 m**, preserves the complete **DERIVED 25 s** driven prefix, then records **DERIVED 15 s**. Bed activation removes retained grouping after the law stage, skipping pending splits, and routes newly sub-floor fragments to fill. This intentionally tests an old welded pile. Escape requires forward progress by the rig's data-derived belt span and a trailing **DERIVED 1 s** mean belly share below **MEASURED analysis configuration 50%**. The original house boundary is not an escape marker once the tank has pushed the pile beyond it.
- **D, parked:** drive over the fresh pile to its middle or the arrival watchdog, apply the shipped service hold until the first resting state (stopped belts and the shipped body rest test), then record **DERIVED 10 s** with the same hold, followed by **DERIVED 15 s** drive-off. No tank is teleported onto fill or clamped. A supplementary `park-stuck` case uses the exact already-beached C prefix and the same first-rest park, to compare a second live pile against its actual control rather than the historical creep number. A **DERIVED 30 s** brake watchdog records an explicit non-rested start if rest is never reached; it is not silently called a successful park.
- **Mound diagnostic:** a smooth cone seeded clear of the house, with **MEASURED authored apex 1.25 m before facet relaxation**. The initial relaxation preserves volume and enforces the collision-facet repose bound. Each angle has its own identical static-cone control; the yielding and static runs read the same gravel response. This is a diagnostic surface, not a claim that shipped collapse generates that exact cone.

**MEASURED local debug timings** bracket one complete headless `Bed::tick`/`App::update`, including simulation, bed work and collider publication; they exclude boot, artificial mound seeding, telemetry traversal, JSON serialization and file writes. Same machine: **MEASURED Apple M4, Mac16,1, 10 cores, 16 GiB, macOS 26.5.2**. Same `edit` profile: dev opt-level **1**, dependency opt-level **3**, LTO off, debug line tables. These compare this apparatus's arms only; they are not hosted or release timings.

Body/awake counts cover this house's live loose carriers, excluding sleeping/frozen carriers from awake counts. Active solver islands are distinct `BodyIslandNode` islands among awake pile bodies. Contact counts are active touching pairs involving a pile carrier or bed tile; dormant contact totals were not retained. A direct tank/bed contact can add one vehicle island beyond the recorded pile-body islands. The reported island bounds include that possible extra island rather than calling static fill zero vehicle solver work. Bed cell reads count actual depth lookups during the law and collider publication, including repeat reads; facet-queue visits are a narrower counter. They exclude telemetry reads and are not unique-cell counts. Read-counter overhead is inside the measured bed time.

Belly share is positive net upward hull-proxy reaction divided by the measured fixture weight; side plates are excluded. Support-loss diagnostics use awake samples below **MEASURED analysis configuration 5% of weight**; launch diagnostics additionally require upward speed above **MEASURED analysis configuration 0.05 m/s**. These are observations, not new game gates. Acceleration is **DERIVED adjacent-tick delta vertical speed / dt**, never across a reposition and never an Avian substep peak. Sleeping bodies' retained velocities are not treated as free flight.

## 1. Server cost

**MEASURED local debug timings, ms.** Each entry is the range of the separate repetitions' means or p99s. Singleton entries have equal rounded endpoints. These are complete headless tick brackets, not counts converted into time.

| Phase | Arm | Tick mean | Tick p99 | Bed work mean |
|---|---|---|---|---|
| collapse | control | 1.20–1.29 | 4.40–4.68 | 0.00 |
| collapse | loose | 1.69–1.87 | 3.50–4.27 | 0.05 |
| collapse | nominal | 1.70–1.93 | 3.87–6.01 | 0.04–0.05 |
| collapse | stiff | 1.73–1.76 | 3.83–4.02 | 0.04 |
| pass 1 | control | 2.31–2.46 | 6.85–7.47 | 0.00 |
| pass 1 | loose | 1.37–1.59 | 3.70–5.10 | 0.23–0.25 |
| pass 1 | nominal | 1.20–1.26 | 2.46–2.76 | 0.08 |
| pass 1 | stiff | 1.22–1.27 | 2.23–3.24 | 0.07–0.08 |
| pass 2 | control | 2.13–2.50 | 5.29–6.40 | 0.00 |
| pass 2 | loose | 1.31–1.43 | 3.79–4.02 | 0.19–0.20 |
| pass 2 | nominal | 1.26–1.52 | 1.98–4.18 | 0.07–0.08 |
| pass 2 | stiff | 1.28–1.36 | 1.85–2.18 | 0.05–0.06 |
| pass 3 | control | 2.11–2.38 | 5.40–6.87 | 0.00 |
| pass 3 | loose | 1.16–1.30 | 2.86–3.22 | 0.08–0.09 |
| pass 3 | nominal | 1.22–1.29 | 1.77–1.94 | 0.03 |
| pass 3 | stiff | 1.15–1.24 | 1.76–2.08 | 0.02 |
| fresh lane | control | 2.37–2.46 | 6.96–7.30 | 0.00 |
| fresh lane | loose | 1.36–1.42 | 3.79–3.95 | 0.23–0.24 |
| fresh lane | nominal | 1.20–1.36 | 2.60–4.55 | 0.08–0.11 |
| fresh lane | stiff | 1.20–1.40 | 2.23–3.93 | 0.07–0.08 |
| fresh park | control | 1.73–1.92 | 5.70–8.10 | 0.00 |
| fresh park | loose | 0.98–1.18 | 3.14–3.79 | 0.10–0.11 |
| fresh park | nominal | 0.91–1.10 | 1.55–2.44 | 0.04 |
| fresh park | stiff | 0.97–1.04 | 1.67–2.04 | 0.05 |
| drive off | control | 2.64–2.86 | 5.85–6.27 | 0.00 |
| drive off | loose | 1.23–1.26 | 2.59–3.32 | 0.11–0.12 |
| drive off | nominal | 1.13–1.21 | 1.78–2.57 | 0.05 |
| drive off | stiff | 1.13–1.19 | 1.79–1.98 | 0.04 |
| stuck rescue | control | 9.59–13.87 | 11.17–55.60 | 0.00 |
| stuck rescue | loose | 1.60–1.62 | 2.63–2.71 | 0.43 |
| stuck rescue | nominal | 1.49–1.53 | 2.69–2.82 | 0.32 |
| stuck rescue | stiff | 1.39–1.40 | 2.62–2.65 | 0.24 |
| stuck park | control | 2.34–2.36 | 10.33–11.29 | 0.00 |
| stuck park | loose | 0.93–0.96 | 1.70–1.79 | 0.04 |
| stuck park | nominal | 0.95–1.03 | 1.71–2.02 | 0.04 |
| stuck park | stiff | 0.93 | 1.72–1.77 | 0.04 |


**MEASURED work/body ranges over ticks and repetitions; DERIVED island bounds.** Cell reads include repeated depth lookups and shape publication, not unique cells. Island bounds count pile-body islands and allow at most one additional awake tank/bed island; graph identities sufficient to deduplicate that last island were not retained. The tank itself is additional to the loose-body census. Contact pairs are active touching pairs, not dormant broad-phase candidates.

| Phase | Arm | Loose bodies | Awake / tick | Island bounds | Pile pairs / tick | Bed reads / tick | Tile updates / tick |
|---|---|---|---|---|---|---|---|
| collapse | control | 1–15 | 0–15 | 0–10 | 0–376 | 0 | 0 |
| collapse | loose | 1–7 | 0–7 | 0–5 | 0–210 | 102–283,665 | 0–4 |
| collapse | nominal | 1–7 | 0–7 | 0–5 | 0–210 | 102–216,695 | 0–4 |
| collapse | stiff | 1–7 | 0–7 | 0–5 | 0–210 | 102–183,306 | 0–4 |
| pass 1 | control | 15–39 | 0–39 | 0–7 | 0–713 | 0 | 0 |
| pass 1 | loose | 0–7 | 0–7 | 0–3 | 0–210 | 89–1,675,056 | 0–6 |
| pass 1 | nominal | 0–7 | 0–7 | 0–4 | 0–209 | 34–1,404,498 | 0–6 |
| pass 1 | stiff | 0–7 | 0–7 | 0–4 | 0–210 | 45–1,070,364 | 0–6 |
| pass 2 | control | 39 | 0–21 | 0–4 | 0–531 | 0 | 0 |
| pass 2 | loose | 0 | 0 | 0–1 | 0–4 | 90–146,235 | 0–4 |
| pass 2 | nominal | 0 | 0 | 0–1 | 0–6 | 54–27,755 | 0–4 |
| pass 2 | stiff | 0 | 0 | 0–1 | 0–4 | 66–22,851 | 0–4 |
| pass 3 | control | 39 | 0–22 | 0–4 | 0–522 | 0 | 0 |
| pass 3 | loose | 0 | 0 | 0–1 | 0–4 | 83–88,827 | 0–4 |
| pass 3 | nominal | 0 | 0 | 0–1 | 0–6 | 58–20,526 | 0–4 |
| pass 3 | stiff | 0 | 0 | 0–1 | 0–6 | 61–24,259 | 0–4 |
| fresh lane | control | 15–39 | 0–39 | 0–7 | 0–713 | 0 | 0 |
| fresh lane | loose | 0–7 | 0–7 | 0–3 | 0–210 | 89–1,675,056 | 0–6 |
| fresh lane | nominal | 0–7 | 0–7 | 0–4 | 0–209 | 34–1,404,498 | 0–6 |
| fresh lane | stiff | 0–7 | 0–7 | 0–4 | 0–210 | 45–1,070,364 | 0–6 |
| fresh park | control | 40 | 0–31 | 0–3 | 0–567 | 0 | 0 |
| fresh park | loose | 0 | 0 | 0–1 | 0–2 | 95–75,533 | 0–2 |
| fresh park | nominal | 0 | 0 | 0–1 | 0–3 | 73–10,371 | 0–2 |
| fresh park | stiff | 0 | 0 | 0–1 | 0–2 | 94–9,782 | 0–2 |
| drive off | control | 40 | 1–29 | 1–4 | 8–558 | 0 | 0 |
| drive off | loose | 0 | 0 | 0–1 | 0–4 | 56–60,987 | 0–4 |
| drive off | nominal | 0 | 0 | 0–1 | 0–3 | 41–24,748 | 0–4 |
| drive off | stiff | 0 | 0 | 0–1 | 0–2 | 61–21,967 | 0–4 |
| stuck rescue | control | 61 | 49–53 | 1–4 | 1,016–1,289 | 0 | 0 |
| stuck rescue | loose | 2 | 0–2 | 0–2 | 0–19 | 64–14,534,039 | 0–7 |
| stuck rescue | nominal | 2 | 0–2 | 0–2 | 0–19 | 66–10,675,494 | 0–7 |
| stuck rescue | stiff | 2 | 0–2 | 0–2 | 0–19 | 62–7,663,069 | 0–6 |
| stuck park | control | 61 | 0–50 | 0–1 | 0–1,082 | 0 | 0 |
| stuck park | loose | 2 | 0–2 | 0–1 | 0–17 | 102–106 | 0 |
| stuck park | nominal | 2 | 0–2 | 0–1 | 0–17 | 102–106 | 0 |
| stuck park | stiff | 2 | 0–2 | 0–1 | 0–17 | 102–106 | 0 |


The complete **DERIVED 85 s** collapse-plus-three-pass workload excludes the approach/reposition intervals in its weighted mean. Its bed mean is **MEASURED 1.28–1.49 ms** against **2.07–2.31 ms** for control, with the paired savings stated above. Fresh collapse alone is more expensive: **MEASURED 1.69–1.93 ms** versus **1.20–1.29 ms**, despite fewer bodies. After collapse the bed holds only **DERIVED 1.5797 m3** of the house's **8.8212 m3** occupied material; **MEASURED seven bonded bodies** remain against control's **15**. It is not already an all-fill house. After the first drive the bed has **MEASURED zero loose bodies**, control **39**.

Bulk migration has a separate one-tick price: **MEASURED 400–403 / 296–297 / 217–218 ms** maxima in loose/nominal/stiff rescue, against control maxima **12–96 ms**. Those spikes are not hidden by the smaller rescue p99s. The real fresh collapse peaks at **MEASURED 28–29 ms** in both arms. No optimized or hosted speedup is established by these local debug readings.

## 2. Crossing and beaching

**DERIVED times and weight percentages from MEASURED tick samples.** Crossing uses the common marker; rescue uses the rig-span/recovered-belly criterion stated above. A hang below is a continuous sample run with speed below the analysis threshold and majority belly support, not a claim that every failure to cross has that load path.

| Window | Arm | Cross/escape | Time (s) | Mean belly % | Belly % range | Final second % | Worst qualifying hang (s) |
|---|---|---|---|---|---|---|---|
| pass 1 | control | 3/3 | 6.156 | 0.03 | 0.00–17.52 | 0.00 | 0.000 |
| pass 1 | loose | 3/3 | 6.766 | 0.30 | 0.00–119.12 | 0.00 | 0.000 |
| pass 1 | nominal | 3/3 | 6.891 | 0.18 | 0.00–177.41 | 0.00 | 0.000 |
| pass 1 | stiff | 3/3 | 7.172 | 0.16 | 0.00–76.79 | 0.00 | 0.000 |
| pass 2 | control | 3/3 | 12.031 | 0.00 | 0.00 | 0.00 | 0.000 |
| pass 2 | loose | 3/3 | 9.391 | 0.31 | 0.00–42.37 | 0.00 | 0.000 |
| pass 2 | nominal | 3/3 | 12.281 | 0.57 | 0.00–44.39 | 0.00 | 0.000 |
| pass 2 | stiff | 3/3 | 13.734 | 0.05 | 0.00–83.14 | 0.00 | 0.000 |
| pass 3 | control | 3/3 | 11.156 | 0.00 | 0.00 | 0.00 | 0.000 |
| pass 3 | loose | 3/3 | 7.203 | 0.37 | 0.00–94.71 | 0.00 | 0.000 |
| pass 3 | nominal | 3/3 | 13.422 | 0.82 | 0.00–209.74 | 0.00 | 0.000 |
| pass 3 | stiff | 3/3 | 9.062 | 0.27 | 0.00–68.58 | 0.00 | 0.000 |
| fresh lane | control | 3/3 | 6.156 | 0.03 | 0.00–17.52 | 0.00 | 0.000 |
| fresh lane | loose | 3/3 | 6.766 | 0.30 | 0.00–119.12 | 0.00 | 0.000 |
| fresh lane | nominal | 3/3 | 6.891 | 0.18 | 0.00–177.41 | 0.00 | 0.000 |
| fresh lane | stiff | 3/3 | 7.172 | 0.16 | 0.00–76.79 | 0.00 | 0.000 |
| stuck rescue | control | 0/3 | — | 64.03 | 9.61–122.66 | 69.37 | 0.484 |
| stuck rescue | loose | 3/3 | 2.891 | 0.07 | 0.00–65.44 | 0.00 | 0.016 |
| stuck rescue | nominal | 3/3 | 2.812 | 0.07 | 0.00–65.44 | 0.00 | 0.016 |
| stuck rescue | stiff | 3/3 | 2.875 | 0.07 | 0.00–65.44 | 0.00 | 0.016 |


Stuck-window forward progress and terminal speed (MEASURED positions/velocities; DERIVED progress):

| Arm | Forward progress (m) | Final speed (m/s) | Bodies remaining |
|---|---|---|---|
| control | 1.823 | 0.0295 | 61 |
| loose | 79.620 | 9.3837 | 2 |
| nominal | 80.774 | 9.4366 | 2 |
| stiff | 79.377 | 9.3671 | 2 |


All final B passes and fresh-lane runs cross, with **MEASURED zero qualifying B hang ticks** and **DERIVED zero final-second belly share**. The time penalties are real, even though they do not become new persistent beaches. Repositioning preserves the drivetrain, so later-pass crossing times include that memory as well as the changed pile; this is not a model of a natural turnaround. The fresh-world first pass independently repeats the first-pass penalty.

C supplies a bonus rather than a rescue gate. Control remains beached throughout its **DERIVED 15 s** continuation: final-second belly share **69.37%**, terminal speed **MEASURED 0.02954 m/s**, progress **DERIVED 1.823 m**. All bed repetitions escape in **DERIVED 2.812–2.891 s**, advancing **79.377–80.774 m**, with final-second belly share **0%**. The first three second-bin belly means are **DERIVED 65.35 / 61.98 / 58.67%** in control, versus **1.02 / 0 / 0%** for each bed pair. The literal longest speed/belly-threshold run is only **DERIVED 0.484 s** in control because its wiggles repeatedly cross the speed threshold; it does not mean the unresolved beach lasts only that long.

What carries the hung control is observed, not guessed: the dominant individual positive hull-support carrier has **DERIVED occupied-volume size 0.591 m**, with transformed height **0.487–0.534 m** over the qualifying frames. All positive-support carriers span **DERIVED sizes 0.502–0.608 m** and heights **0.340–0.733 m**. The bed rescue leaves **MEASURED two loose bodies**, but they carry no final belly reaction after escape. There is no persistent B/C bed hang requiring a surviving bonded piece to be named. The smooth artificial mound stalls below are carried by the bed's own crest.

## 3. Pile survival

**DERIVED** occupied-material retention, bulk integration and footprints; **MEASURED** admitted node/vertex heights. Body height is the highest transformed admitted chunk vertex. Body footprint is a conservative whole-carrier projected-AABB raster envelope. The combined footprint bounds are the larger component through their sum: bounds on the union of those envelopes, not exact convex occupied area. Once no bodies remain, the bed footprint is its exact positive triangle area. Accounting excludes the scripted removed base band and unrelated constructions.

| Stage | Arm | Material retained (%) | Pile height (m) | Footprint envelope bounds (m2) | Bed height (m) | Bed footprint (m2) | Loose bodies |
|---|---|---|---|---|---|---|---|
| after collapse | control | 100.00000 | 3.009 | 31.50 | 0.000 | 0.00 | 15 |
| after collapse | loose | 99.99961–100.00000 | 2.949 | 35.47–50.47 | 0.348 | 35.47 | 7 |
| after collapse | nominal | 99.99961–100.00000 | 2.949 | 32.94–48.19 | 0.386 | 32.94 | 7 |
| after collapse | stiff | 99.99973–100.00000 | 2.949 | 30.78–45.78 | 0.360 | 30.78 | 7 |
| after pass 1 | control | 100.00000 | 1.668 | 174.31 | 0.000 | 0.00 | 39 |
| after pass 1 | loose | 99.99961–99.99998 | 0.697 | 69.62 | 0.697 | 69.62 | 0 |
| after pass 1 | nominal | 99.99966–99.99999 | 0.781 | 60.56 | 0.781 | 60.56 | 0 |
| after pass 1 | stiff | 99.99965–99.99999 | 0.771 | 56.50 | 0.771 | 56.50 | 0 |
| after pass 2 | control | 100.00000 | 1.668 | 174.81 | 0.000 | 0.00 | 39 |
| after pass 2 | loose | 99.99997 | 0.683 | 70.84 | 0.683 | 70.84 | 0 |
| after pass 2 | nominal | 99.99982 | 0.781 | 60.53 | 0.781 | 60.53 | 0 |
| after pass 2 | stiff | 99.99980 | 0.769 | 56.97 | 0.769 | 56.97 | 0 |
| after pass 3 | control | 100.00000 | 1.670 | 177.12 | 0.000 | 0.00 | 39 |
| after pass 3 | loose | 99.99997 | 0.663 | 71.47 | 0.663 | 71.47 | 0 |
| after pass 3 | nominal | 99.99982 | 0.771 | 60.56 | 0.771 | 60.56 | 0 |
| after pass 3 | stiff | 99.99980 | 0.751 | 57.06 | 0.751 | 57.06 | 0 |


Physical bed-plus-body accounting remains **DERIVED at least 99.9996%** of admitted post-script material throughout the free passes; including the carried fractional unit gives **100% within floating-point summation precision**. The latter is accounting, not extra collider volume. There is no discarded boundary volume and no dynamic reconversion of fill.

Neither absurd end occurs. The pile remains after its first pass, with **MEASURED bed heights 0.697 / 0.781 / 0.771 m** and **DERIVED footprints 69.625 / 60.5625 / 56.5 m2**. After the third pass heights are **0.663 / 0.771 / 0.751 m**, footprints **71.46875 / 60.5625 / 57.0625 m2**. It is not unchanged from the collapsed state: tall bonded geometry becomes a lower, wider bed. An unchanged global maximum in a later pass is not evidence that the driven lane failed to deform; the high point can be beside it and heave preserves volume.

## 4. Ten-second parks

All park displacements start at the recorded pre-window pose and cover the entire **DERIVED 640 ticks / 10 s**. Endpoint travel, accumulated path and maximum downward excursion are different readings. The last-second sink is endpoint change over the final complete second, not the window's lowest point. Sleep is the tank's Avian island verdict.

| Pile | Arm | Brake-to-rest (s) | Rested starts | Travel (cm) | Path (cm) | Max sink (mm) | Sleeping ticks / 640 | Ends asleep | Last-second sink (mm) |
|---|---|---|---|---|---|---|---|---|---|
| fresh pile | control | 0.641 | 3/3 | 1.613 | 3.214 | 12.807 | 542 | 3/3 | 0.000 |
| fresh pile | loose | 0.422 | 3/3 | 3.787 | 5.142 | 39.283 | 579 | 3/3 | 0.000 |
| fresh pile | nominal | 0.422 | 3/3 | 6.216 | 7.116 | 62.134 | 584 | 3/3 | 0.000 |
| fresh pile | stiff | 0.453 | 3/3 | 1.506 | 1.852 | 14.419 | 545 | 3/3 | 0.000 |
| stuck pile | control | 0.047 | 3/3 | 0.459 | 1.671 | 3.522 | 571 | 3/3 | 0.000 |
| stuck pile | loose | 0.438 | 3/3 | 3.731 | 6.603 | 0.000 | 592 | 3/3 | 0.000 |
| stuck pile | nominal | 0.438 | 3/3 | 3.731 | 6.602 | 0.000 | 592 | 3/3 | 0.000 |
| stuck pile | stiff | 0.438 | 3/3 | 3.731 | 6.602 | 0.000 | 592 | 3/3 | 0.000 |


Every measured park starts with stopped belts and a body satisfying the shipped rest test, and every final park ends asleep. The fresh control sleeps **MEASURED 542/640 ticks**; loose/nominal/stiff sleep **579 / 584 / 545**. Their **DERIVED maximum datum drops** are **39.283 / 62.134 / 14.419 mm**, versus control **12.807 mm**. Datum drop includes rotation and support redistribution; it is not independently identified as material compaction.

The bed's per-second changed-node counts are **MEASURED** loose `2814, 0, 0, 0, 0, 0, 0, 0, 0, 0`, nominal `146, 0, 0, 0, 0, 0, 0, 0, 0, 0`, stiff `255, 18, 0, 0, 0, 0, 0, 0, 0, 0`. Final-second datum descent is **MEASURED zero** in every arm. This is finite settling/rocking, not an observed continuing ten-second downward ratchet. The samples do not separate force noise from physical settling, nor prove indefinite stability.

The historical control creep of **8.6–15.5 cm per 10 s** with no sleep is a reference, not this run's result. The actual matched controls here travel **DERIVED 1.613 cm** on the fresh pile and **0.459 cm** on the beached pile under the declared service hold, and sleep. Bed travel on the latter is **3.731 cm**. These arrivals and held-brake windows do not reproduce that historical unheld/no-demand diagnosis. Scoring against its larger number would conceal the present extra movement. Every arm also drives off the primary parked pile; see its measured cost and raw trajectory.

## 5. Ride

**MEASURED velocity samples; DERIVED adjacent-tick acceleration and orientation angles.** Ranges combine time and repetitions. No derivative crosses a fixture reposition. Sleeping resume velocities are treated as zero motion. These are tick-level observations, not substep peaks or a visual comfort rating.

| Window | Arm | Vertical speed (m/s) | Vertical acceleration (m/s2) | Pitch (deg) | Roll (deg) | Longest low-support run (ticks) | Qualifying launch ticks |
|---|---|---|---|---|---|---|---|
| free passes | control | -0.804–0.999 | -9.55–36.08 | -4.14–5.73 | -3.89–2.45 | 0–4 | 0–4 |
| free passes | loose | -0.521–0.503 | -4.75–14.86 | -7.04–8.36 | -0.51–1.39 | 0 | 0 |
| free passes | nominal | -0.802–0.538 | -7.37–26.29 | -8.37–8.77 | -0.61–2.80 | 0 | 0 |
| free passes | stiff | -0.645–0.721 | -6.52–30.15 | -8.99–10.42 | -0.45–2.76 | 0 | 0 |
| stuck rescue | control | -0.220–0.293 | -6.42–13.32 | -2.52–4.17 | -4.35–4.31 | 0 | 0 |
| stuck rescue | loose | -0.698–0.291 | -7.26–15.83 | -6.16–4.54 | -6.41–2.17 | 0 | 0 |
| stuck rescue | nominal | -0.697–0.193 | -7.26–7.96 | -5.79–4.14 | -5.54–1.75 | 0 | 0 |
| stuck rescue | stiff | -0.697–0.239 | -7.26–10.90 | -6.27–4.36 | -6.02–2.23 | 0 | 0 |
| fresh park | control | -0.076–0.071 | -1.62–2.43 | -2.02–-0.92 | 0.03–0.97 | 0 | 0 |
| fresh park | loose | -0.124–0.078 | -1.71–1.41 | -3.76–0.40 | -0.34–0.23 | 0 | 0 |
| fresh park | nominal | -0.211–0.046 | -2.94–4.09 | -4.79–-1.86 | -1.40–-0.21 | 0 | 0 |
| fresh park | stiff | -0.071–0.027 | -1.65–3.28 | -1.97–-1.79 | 1.60–2.25 | 0 | 0 |


The smooth surface removes the column probe's harsh first-pass shocks. Across the free passes, the upward tick-acceleration peak is **DERIVED 14.86 / 26.29 / 30.15 m/s2** versus control **36.08 m/s2**; vertical-speed extrema are smaller in magnitude. Bed free driving has **MEASURED zero qualifying unsupported/launch ticks**, while control has a longest **four-tick** low-support run, **DERIVED 62.5 ms**, with upward motion. This is a tick-level diagnostic, not a visual launch finding.

Pitch moves farther: bed extrema span **DERIVED -8.99 to +10.42 degrees** across the bracket versus control **-4.14 to +5.73 degrees**. Rolls are generally smaller. Lower shocks, larger pitch and preserved support are mixed evidence for ride feel. Headless motion does not settle which is preferable, so the ride no-worse verdict remains UNCLEAR rather than treating every larger angle as an automatic failure. The C migration drop is reported separately and is not substituted for fresh-collapse ride.

## 6. Smooth repose mounds

Each yielding mound is compared with the identical static smooth mound for its angle. **MEASURED** final pose/speed and withdrawal count; **DERIVED** retained shape, toe lowering/heave and weight share. The toe is the initial approach half, world Z at or above the mound centre. Same-cell differences do not mean matter disappeared.

| Pair | Arm | Crossed | Final hull Z (m) | Final speed (m/s) | Final belly (%) | Toe lowering (mm) | Toe heave (mm) | Bearing node withdrawals | Final mound height (m) |
|---|---|---|---|---|---|---|---|---|---|
| loose | control-loose | 0/3 | -18.245 | -0.000001 | 40.50 | 0 | 0 | 0 | 1.162 |
| loose | loose | 0/3 | -19.915 | 0.000180 | 55.54 | 415 | 261 | 2,450 | 0.812 |
| nominal | control | 0/3 | -17.169 | 0.000008 | 26.06 | 0 | 0 | 0 | 1.154 |
| nominal | nominal | 0/3 | -19.320 | 0.000012 | 45.76 | 378 | 194 | 1,283 | 0.848 |
| stiff | control-stiff | 0/3 | -18.281 | 0.000404 | 40.58 | 0 | 0 | 0 | 1.139 |
| stiff | stiff | 0/3 | -18.759 | 0.000017 | 43.78 | 292 | 187 | 530 | 0.867 |


A real repose-slope mound can stall this tank on a smooth surface. All static cones stall, and all yielding cones stall; this is not a new bed-only obstruction. The yielding arms advance farther into the mound and lower the toe by **DERIVED 292–415 mm**, but do not flatten a driveable route through it. With **MEASURED zero bonded bodies** present, the remaining support is the bed crest, **MEASURED 0.812–0.867 m** high at the end, with **DERIVED footprints 7.219–17.219 m2**. Loose finishes with a majority belly share; the other pairs also retain substantial hull support.

The true collision facets remain within **DERIVED 0.10 degrees** of their configured repose bound throughout the final runs; sampled collider/belt height disagreement is at most **DERIVED 0.000001342 m**. Thus this stall cannot be blamed on vertical column faces or the earlier neighbour-only slope error. It also cannot by itself reject the bed: the identical static controls stall. The supplied level-ground bearing expression and raster patch do not establish correct slope/crest/eccentric capacity.

## 7. Solver interaction and commit cadence

A commit comparison pairs the solver reading just before publication with the following physics tick. Force is the sum of absolute normal components of the transferred tank/bed solver forces, not Avian's accumulated `normal_impulse` field. These comparisons include real motion and changing support; they do not causally isolate cache loss. Pair retention is weighted by the number of old graph IDs; an absent denominator is not reported as zero continuity.

| Window | Arm | Commit ticks / phase | Tile updates / phase | Next point-count delta | Next normal-force delta (MN) | Retained graph IDs (%) | Next nonzero warm-start points |
|---|---|---|---|---|---|---|---|
| free passes | loose | 167–327 | 397–814 | -28–15 | -0.736–0.958 | 97.88–98.72 | 0–8 |
| free passes | nominal | 75–286 | 118–624 | -29–16 | -1.484–1.330 | 97.55–99.64 | 0–4 |
| free passes | stiff | 83–294 | 118–348 | -20–19 | -1.768–1.306 | 97.69–99.44 | 0–9 |
| free passes | nominal4 | 37–104 | 68–206 | -22–11 | -0.798–0.987 | 98.69–100.00 | 0–6 |
| park | loose | 32 | 45 | -11–4 | -0.125–0.125 | 97.67 | 0–5 |
| park | nominal | 25 | 26 | -9–5 | -0.178–0.125 | 98.44 | 0–4 |
| park | stiff | 49 | 50 | -11–13 | -0.208–0.091 | 100.00 | 0–4 |
| park | nominal4 | 9 | 13 | -15–1 | -0.197–-0.012 | 100.00 | 0 |
| rescue | loose | 58 | 84 | -17–9 | -1.342–0.167 | 98.95 | 0–6 |
| rescue | nominal | 61 | 82 | -15–14 | -0.497–0.256 | 99.00 | 0–7 |
| rescue | stiff | 64 | 86 | -16–10 | -0.780–0.623 | 100.00 | 0–4 |
| rescue | nominal4 | 31 | 52 | -14–14 | -0.488–0.000 | 100.00 | 0–3 |
| mound | loose | 233 | 233 | -71–52 | -3.466–0.456 | 100.00 | 0–8 |
| mound | nominal | 196 | 196 | -40–6 | -5.416–0.314 | 100.00 | 0–3 |
| mound | stiff | 150 | 150 | -27–6 | -5.821–2.891 | 100.00 | 0–3 |
| mound | nominal4 | 113 | 113 | -42–5 | -0.318–4.689 | 100.00 | 0 |

Graph pairs mostly persist: **DERIVED 97.55–100%** of old bed/tank graph identities are retained across the reported windows. Nonzero next-step warm-start points demonstrate that replacement does not uniformly erase every contact impulse. The pinned engine regenerates manifolds every tick and only runs its matcher for at most **four manifolds** (`vendor/avian3d-0.8.0-overmatch/src/collision/narrow_phase/system_param.rs:941`); pair identity is therefore not a warm-start guarantee.

The nominal four-tick cadence is cheaper in update counts: the primary park uses **MEASURED 13 tile updates** versus **26**, rescue **52** versus **82**, mound **113** versus **196**. Park travel improves to **DERIVED 3.958 cm** from **6.216 cm**, but remains above control's **1.613 cm**. The parked normal-force discontinuity still reaches **MEASURED 0.197 MN**, and the mound diagnostic reaches **4.689 MN**. Whole-workload mean time ranges overlap: **MEASURED 1.34–1.39 ms** at four ticks versus nominal's **1.29–1.42 ms** at one. This cadence is not an established removal of contact-force discontinuity or the parked penalty. Other cadences and a causal cache-only experiment remain unestablished.


## Production surface inventory and scope

These are **baseline source locations**, checked against `HEAD` rather than copied as current facts from the preceding report. The disposable hooks are absent from the committed tree. A production surface needs one admitted top-surface query, returning height/facet normal, supporting material and a revision/support identity at the appropriate authoritative or presented tick.

| Reader / lifecycle | Baseline file:line | Required change |
|---|---|---|
| Belt depth/contact oracle | `src/track/oracle.rs:515`, `src/track/oracle.rs:603` | Union bed facets with base terrain and blocks, including buried reads; select the supporting material. |
| Force-column terrain plane | `src/track/world_contact.rs:26`, `src/track/world_contact/terrain.rs:14` | Read the same committed facet. An oracle-only change leaves this separate integration path on base ground. |
| Grip and hard belt reactions | `src/track/world_contact.rs:345`, `src/track/stop_contacts.rs:229`, `src/ground_field.rs:269` | Carry the supporting ground response through sampled and solver belt paths; print softness must not become bearing yield. |
| Physical surface | `src/world.rs:305`, `src/terrain_grid.rs:738` | Own sparse heightfield tiles, shared boundary samples, empty geometry, dirty updates and `FieldSurface` admission. |
| The game's round march | `src/ballistics.rs:891`, `src/ballistics.rs:2123` | The march explicitly casts the base grid; new terrain colliders alone do not migrate this reader. Authority and presented march must read the bed at their respective stream cursor. |
| Spawn/respawn placement | `src/terrain_grid.rs:197`, `src/terrain_grid.rs:217`, `src/net/server.rs:1337` | Include the bed in maximum-over-footprint placement, retaining each rig's data-derived stance. |
| Decoration landing | `src/debris/decor.rs:94`, `src/debris/decor.rs:220` | Land/reseat presentation on the current presented top, with one deposit lifecycle rather than double-counting decoration as simulation matter. |
| Drawn running gear and caches | `src/track/view.rs:716`, `src/track/terrain.rs:39` | View sampling must include presented bed state; invalidate affected caches by bed revision rather than rebuilding the immutable map globally. |
| Client surface mesh | `src/terrain_grid.rs:768`, `src/terrain_lod.rs:212`, `src/world.rs:441` | Build/update separate bed geometry from its samples; retain the base terrain ladder. |
| Track prints | `src/track_prints.rs:514` | Project/refit to the supporting surface and its revision; zero print softness does not justify projecting through fill. |
| Ground dust | `src/vfx/muzzle.rs:693` | Read presented top height and material. |
| Runtime placement, aim and grade diagnostics | `src/net/debris_stress.rs:432`, `src/tank/battle.rs:601`, `src/tank/battle.rs:652` | Query the same current surface for runtime scenarios and operator tools. |
| Camera/aim casts | `src/world.rs:618` | Spatial casts meet the bed colliders automatically; retain their layers and correct presented-time geometry. |
| Sleeping/frozen support lifecycle | `src/debris.rs:245`, `src/track/forces.rs:510` | Shape changes must wake their supported owners; terrain belt contacts currently have no collider identity, so the bed also needs field-support identities/revisions and frozen-support handling. |
| Replication / late join | `src/net/protocol.rs:901`, `src/net/construction.rs:39`, `src/net/construction.rs:294` | Add admitted mutable tile snapshots/diffs, world identity and authoritative tick, with shared-delay presentation and late join. The append-only construction ledger is a mechanism to learn from, not a ready-made mutable tile protocol. `Shed` alone is insufficient. |
| Freeing/coarsening | `src/debris/crumble.rs:245`, `src/debris/crumble.rs:748`, `src/construction/breakage.rs:553` | Delete retained size-floor grouping and its plumbing; route sub-floor volume at the chosen freeing/deposit lifecycle. Do not ship the probe's migration of an existing welded pile. |

Map-authoring scatter/clearance and the base terrain shadow bake remain base-grid products. Runtime bed drawing/shadows need current geometry; this is not a request to rebake the map whenever fill moves.

**DERIVED engineering estimate**, including meaningful interface/lifecycle tests, Rust/data changes and the reader work above, not the disposable instrument's line count:

| Slice | Added/changed lines | What must land together |
|---|---:|---|
| Surface, deposits, repose, all readers, replication, mesh | 1,900–2,800 | Shared nodal boundaries and conservation, sparse tile publication, real facet slopes, obstacle-aware transport, surface-support wakeups, world/tick admission and late join. |
| Bearing | 650–1,050 | Actual contact patches/undersides, carried loads, conservative heave, rate/commit interaction and ownership/border ordering. The raster approximation is not promoted to a settled contact law. |
| Delete the weld | 100–200 | Grouping/storage deletion and source-state/ledger lifecycle checks; **DERIVED 80–140 deleted lines** separately. |
| Hosted remeasurement / freeze decision | 100–250 | Targeted instruments/scripts and reports. Freeze deletion remains conditional. |
| View-only seated chunks | 200–400 | Presentation seated on the delayed bed, independent of collision/conservation. |

**DERIVED approximately 3,000–4,700 added/changed lines**, updating the earlier estimate of **2,400–4,000**. The extra work is real triangle/cap transport, support lifecycle and contact-patch ownership. This is not a completed production diff. An engine contact-policy change, a fuller mixed-material law, or an indefinite-time constitutive model would enlarge it; none is silently priced as a small crumble edit. Conditional freeze deletion is excluded.

## What remains unestablished

- Hosted/two-core or release cost, long-lived crowded scenes, replication bandwidth and a production parallel implementation. The prototype's serial tile/law work is included in its local timings.
- Actual contact polygons and rotated underside integration, nonflat underlying terrain, parallel tile-border ownership, mixed-material packing/friction/bulk weight, and a validated slope/crest/eccentric or rate-dependent granular bearing law.
- A ten-second **unheld/no-demand** reproduction of the historical creep, indefinite parked stability, natural turnarounds, or a truly untouched adjacent strip inside the same narrow physical pile.
- Exact union/deduplication of the extra tank/bed solver island, dormant contact totals, a causal attribution of individual force jumps to warm-start loss, or a commit cadence that removes those jumps. The report states the measured active-pair counts and derived island bounds.
- Substep motion peaks, visual comfort/decoration quality, late join, end-to-end authority/presented-stream agreement, deposits' true flight/landing distribution and momentum, and changing-bed frozen-support correctness in production.

These limits qualify the next build, not production switches retained in this lane. No smaller-body floor, ground-yield alternative or vehicle-specific rule was tested or promoted by this result.

## Artifacts, restoration and checks

Final raw streams remain ignored in this bay as `target/fair-{arm}-{case}-r{1,2,3}.jsonl`, with matching logs. `target/fair-manifest.json` lists all final execution and timing-stripped state hashes and both harness manifests; its SHA-256 is `a4d909d0c7a6ea95291f3f98432bd05ce0cc6d664ef4b7891da9194db387cae9`. `target/fair-summary.json`, `target/fair-results.md` and `target/fair-executions.jsonl` hold the aggregates and execution ledger. The disposable bed-model source hash is `cbd21f2c0b71d0221e85d0e4c5c910f047326f1abc62150481fce704b4746042` and is identical across the two final harness generations.

Development is not mixed into those ranges. A capped-flow cycle, a finite-domain refusal and a neighbour-only slope implementation were corrected before the accepted model. An interrupted matrix is under `target/fair-interrupted/`; the completed shorter-pass/fixed-brake streams superseded by the protocol correction are under `target/fair-protocol-15/`. No material angle, packing value, vehicle data or body-size floor was tuned toward an outcome. Repose's downhill rule distinguishes unforced slump from loaded heave; it does not raise the convergence watchdog to hide a cycle.

All probe Rust files and hooks were deleted/restored before validation. `git diff --exit-code -- src` is clean. `cargo fmt --all --check` passed; CI clippy (`scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`) passed. Targeted tests passed: **MEASURED 11** crumble tests, **one** collider/oracle agreement test, **three** force-column terrain tests and **one** support-move wake test. All compilation used the required queue and feature set; no in-flight build was killed. Only this report is committed, with the requested co-author trailer. No push, GitHub comment, debugger, display/audio run, WiFi scan, forbidden-document read or damage-topic web search was used.
