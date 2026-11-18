# Crush probe, slice 0: the game's destruction model

Game state only: pieces, joints and loads. Baseline `67e33cd7`, fast-forwarded from `main`; experiment branch `lane/crush-probe`. No production change is retained.

**Binary answer: no, the sustained belly-supporting carriers do not suffer new joint failures under their gathered presses. Build slice 1 changed.** The immediate obstruction is the size-floor carrier grouping: its joints have already opened, but its colliders still move as one rigid body. Bring the design's “welds are rests” work ahead of, or into, the press slice; removing the blow gate alone leaves the measured support intact. This experiment does not demonstrate that ground yield or bulk bearing is the next missing truth.

## Method and repeatability

All quantities in the observation tables are **MEASURED**, unless a column explicitly says **DERIVED**. A repeated endpoint is intentional: each configured start was executed independently **MEASURED three times**, and its complete per-tick JSON stream was byte-identical across those repetitions. These are execution-repeatability ranges, not confidence intervals or randomized placement coverage.

The disposable instrument lived in `src/headless_test/track_scorecard/rubble_climb.rs`. It mounted on the existing shipped-plugin `Bed`, after Avian `PhysicsSystems::Writeback` and before `LandingSet`, before the actual landing split could retire its contact colliders. Each tick it:

- Enumerated every awake, unfrozen `DebrisPiece`.
- Oriented the canonical `ContactTransfers::solver_groups()` points toward each loose owner. Dynamic loose pieces receive solver contacts; implicit structural belt contacts are only staged for fixed surfaces (`src/track/rubble.rs:476`, `:494`).
- Used the shipped landing calculation exactly: `struck = min(effective_mass × closing_speed, max(impulse · normal, 0))` when closing speed exceeds rest speed; the press is `−(impulse − normal × struck) / dt` (`src/debris/crumble.rs:421`). Only presses entered the hypothetical solve; no peak blow force entered it.
- Called `crumble(structure, state, kernels, owned_chunks, presses, f32::MAX)` for every awake construction owner, including owners with no contacts. `crumble` clones `State` internally (`src/debris/crumble.rs:169`). It recorded new cracks/partings, raw fragments and the result of `merge_fragments` on a separate clone of the retained-edge list. It wrote no model state, ledger entry, split, force or velocity. The existing **DERIVED 16-pass** resolution bound remains (`src/debris/crumble.rs:116`); only the energy cap was lifted.
- Classified a hypothetical failure as a returned crack or parting. A raw fragment boundary already present before the solve was **not** counted as a new failure. Yard pieces have no construction `Piece`/`Breakage` and no joints (`src/track_sandbox/rubble.rs:66`); they were counted as single chunks with no assessment required.

Touches retained the original individual points, so their forces and moments reach the linear load assembly unchanged. For noise reporting only, points were summed per chunk collider/other collider; the patch lever is the force-magnitude-weighted contact centre relative to the carrier origin, expressed in its rotating frame. This centroid is not a complete replacement for a general contact wrench.

Belly support means positive net upward force delivered to the tank through its hull-proxy contacts with that loose piece. Tracks mean soft-run and bump-stop contacts, excluding side plates. The deciding share is the summed belly support on failing owners divided by summed belly support on all loose owners, integrated over the stated window. It is not divided by the tank's whole weight. Classification follows `Bed::role` (`src/headless_test/track_scorecard.rs:676`).

Fixture weight **MEASURED 559,170 N**; the vehicle's authored mass remains fixture data, not a law parameter. **DERIVED rest belly clearance 0.476604 m**: the shipped `tiger_1.sim.glb` `Hull_Collider` node's lowest local point is `1.042906 − 0.553042 = 0.489864 m`, plus the rig's **MEASURED −0.013260 m** rest datum. This independently agrees with the issue's rounded clearance. The server volume-size floor is **DERIVED from shipped data 0.500 m**, the cube root of occupied volume (`assets/ballistics/physical_laws.ron:9`, `src/debris/crumble.rs:234`). Bounding height is a different quantity.

| Configured start | Fixture request and duration (DERIVED from probe setup) | Forward travel, repetition range (m) | End speed, repetition range (m/s) | Low-speed ticks, repetition range |
|---|---|---:|---:|---:|
| `stuck` | Existing `standing-t1-x1.4`, full pedal, 25 s | 15.910–15.910 | 0.02931–0.02931 | 497–497 |
| `heap-s2` | Collapsed house, seed 2, pedal 0.3, 40 s | 45.240–45.240 | 2.433–2.433 | 51–51 |
| `heap-s6` | Collapsed house, seed 6, pedal 0.3, 40 s | 24.777–24.777 | 1.436–1.436 | 51–51 |
| `free` | Collapsed house, full pedal, 25 s | 176.157–176.157 | 10.633–10.633 | 36–36 |
| `yard` | Existing loose single-chunk yard, full pedal, 25 s | 191.808–191.808 | 10.633–10.633 | 36–36 |

The seed recipe was recovered from `2665f22b^:src/headless_test/track_scorecard/creep_stall.rs`: fractional golden-ratio placement nudges, **DERIVED within half a millimetre**, using the original coefficients and seed identities. The two historical light-pedal stalls do not reproduce on this baseline. The standing start also travels farther than the issue's trace, but ends belly-supported and slow with demand still applied. Thus this is the same scenario, not reproduction of the historical trajectory. The **MEASURED low-speed threshold was 0.05 m/s**, including launch ticks; those counts alone are not beaching counts.

## 1. Does belly support lie on pieces whose joints would fail?

Every cell below is a **MEASURED range across the repetitions**. The late window starts at **MEASURED tick 400**, the onset named by the issue's historical trace, and is reported alongside the entire drive to avoid hiding approach failures. It is not a stall detector. The simulation uses **DERIVED 64 ticks/s**.

| Scenario | Failing share of belly support, entire drive (%) | Failing share, late window (%) | Mean late belly support / fixture weight (%) |
|---|---:|---:|---:|
| `stuck` | 0.16646–0.16646 | **0–0** | 68.072–68.072 |
| `heap-s2` | 0.01238–0.01238 | 0.01635–0.01635 | 0.00724–0.00724 |
| `heap-s6` | 4.30024–4.30024 | 4.31257–4.31257 | 0.61281–0.61281 |
| `free` | 70.416–70.416 | 0–0 | 0.000133–0.000133 |
| `yard` | 0–0 | N/A: no belly contacts | 0–0 |

The free drive's high conditional belly-failure share is a brief encounter, not persistent beaching: its entire belly impulse is **MEASURED 4,042–4,042 N·s**, against **MEASURED 7,408,710–7,408,710 N·s** for `stuck`. A tiny denominator makes the conditional percentage large. In the standing case's late window, **MEASURED 100–100%** of belly support lies on carriers whose internal joints are already all separated.

## 2. Hypothetical fragments and retained height

There are **MEASURED zero new failing belly carriers** in the standing case's late window. For its approach failures and the other drives, the raw strength result and the actual size-floor result differ:

| Scenario | Raw fragments per failing belly assessment (MEASURED) | Raw volume size (m, MEASURED) | Raw current-pose vertical span (m, MEASURED) | Bodies after current coarsening (MEASURED) |
|---|---:|---:|---:|---:|
| `stuck`, approach | 7–69 | 0.1651–0.5148 | 0.2250–0.7198 | 1–5 |
| `heap-s2` | 8–8 | 0.2137–0.4041 | 0.1618–0.4401 | 1–1 |
| `heap-s6` | 10–10 | 0.1781–0.4164 | 0.1376–0.2605 | 1–1 |
| `free` | 9–9 | 0.1781–0.4164 | 0.3587–2.5521 | 1–1 |

Only one raw fragment among the standing approach's failing-belly samples reaches the floor: **MEASURED size 0.5148–0.5148 m**, vertical span **MEASURED 0.6770–0.6770 m**, above the rest belly clearance. All raw fragments in the other failing-belly samples are below the floor and rejoin their carrier. A tall thin fragment can have a small volume size; the free drive's tall span is not an error in the size test.

The sustained standing carriers already partition into **MEASURED 10–18 raw single chunks**, each **MEASURED 0.1717–0.2965 m** in volume size, but current coarsening returns **MEASURED one body per carrier**. At the final tick, the three belly carriers have heights **MEASURED 0.5285, 0.7086 and 0.7266 m**, all above the **DERIVED 0.4766 m** clearance. A press-only slice changes none of those carrier heights.

Those are frozen-pose geometric spans. The experiment commits no split, so it cannot measure the settled rubble height after a split, rotation, load redistribution or shedding. Treating independently grounded fragment heights as the height of a real stack would invent an outcome. The measured current-coarsening result, however, establishes that most hypothetical fractures do not lower the carrier at all.

## 3. Why the belly carriers hold

For `stuck` after the late-window boundary, **MEASURED seven distinct carriers** contribute belly support. Their volume sizes span **MEASURED 0.5021–0.6645 m** and current vertical spans **MEASURED 0.2988–1.6348 m**. Every one has all internal joints already open: this is retained carrier grouping, not a compact sound block proving masonry strength adequate.

At the final tick, across every repetition:

| Carrier | Size (m, MEASURED) | Internal joints / already open (MEASURED) | Retained carrier edges (MEASURED) | Belly force / fixture weight (%, MEASURED) |
|---|---:|---:|---:|---:|
| A | 0.5910–0.5910 | 29 / 29 | 15–15 | 30.392–30.392 |
| B | 0.5575–0.5575 | 18 / 18 | 13–13 | 26.051–26.051 |
| C | 0.6082–0.6082 | 33 / 33 | 15–15 | 5.492–5.492 |

For the entire standing drive, non-failing already-open carriers take **MEASURED 99.511–99.511%** of belly impulse; holding groups with some closed joints take **MEASURED 0.3223–0.3223%**, with the rest on hypothetical failures. Single chunks take **MEASURED 0–0%** there.

The yard supplies the single-chunk case: **MEASURED 100–100%** of its belly support is on pieces with no joint. Their world bounding spans are **MEASURED x 0.2930–0.4073 m, y 0.2742–0.4769 m, z 0.2870–0.4579 m**. They do not produce a lasting beach in this control.

The light-pedal heaps also have holding groups with closed joints. Over the entire drive their shares of belly impulse are **MEASURED 70.964–70.964%** for seed 2 and **MEASURED 88.210–88.210%** for seed 6; their late non-failing size ranges are **MEASURED 0.5092–0.6996 m** and **MEASURED 0.5092–0.7822 m** respectively. “Assessment held” does not prove “compact block squarely loaded”: holding-joint utilization was not recorded, and these drives cross. There is no evidence here for prioritizing ground yield or bulk bearing over the carrier constraint.

## 4. Free-running track control

Every percentage is **MEASURED across the entire drive**, with identical ranges across repetitions.

| Scenario | Failing loaded piece-ticks (%) | Distinct loaded pieces ever failing (%) | Track support impulse on failing owners (%) |
|---|---:|---:|---:|
| `free`, bonded-house control | **0.9566–0.9566** | **16.667–16.667** | **3.9228–3.9228** |
| `yard`, single chunks | 0–0 | 0–0 | 0–0 |
| `heap-s2` | 1.2357–1.2357 | 18.182–18.182 | 3.2885–3.2885 |
| `heap-s6` | 0.1634–0.1634 | 33.333–33.333 | 0.4334–0.4334 |

In the bonded control this is **MEASURED 15 failing piece-ticks out of 1,568**, and **MEASURED four distinct owners out of 24**. “Every piece under a free-running tank fails” is refuted for the current topology and assessment. It does not validate a revised law that starts judging the presently excluded carrier edges.

## 5. Cost, in counts

Ranges span all ticks and all repetitions. Calls and joint counts are **MEASURED**. Scalar operations are **DERIVED algorithmic counts** returned by `Crumble::work`, including factor/solve work over all passes, not hardware instructions or timings.

| Scenario | Assessments / tick (MEASURED) | Scalar operations / tick (DERIVED) | Scalar operations / assessed owner touching the tank (DERIVED) | Internal joints in tank-touching owners (MEASURED) |
|---|---:|---:|---:|---:|
| `stuck` | 0–58 | 0–20,493,681 | 0–5,542,252 | 9–1,023 |
| `heap-s2` | 0–32 | 0–9,975,584 | 0–1,552,844 | 7–434 |
| `heap-s6` | 0–33 | 0–10,453,846 | 0–1,479,771 | 7–434 |
| `free` | 0–39 | 0–10,721,203 | 0–1,029,633 | 7–370 |
| `yard` | 0–0 | 0–0 | N/A: no construction law | 0–0 |

The standing late window still averages **MEASURED 52.269–52.269 assessments/tick**, for **DERIVED 113,800–113,800 scalar operations/tick**, predominantly proving nothing can fail in already-open carriers. The much larger maximum belongs to the encounter with larger groups. Counts include awake owners away from the tank and empty-contact assessments, as requested. Per-owner scalar ranges only cover tank-touching owners because other owners' individual records were not retained. Yard's zero solve count is the no-joint case, not zero contact-solver cost.

## 6. Patch variation and verdict flips

No genuinely constant-load bonded carrier was established. The strongest available steady-support observation is explicitly **quasi-steady**: a standing-case belly carrier with linear speed and angular speed each below **MEASURED 0.05 m/s and 0.05 rad/s**, continuously over **MEASURED ticks 1,392–1,465**, or **MEASURED 74 ticks**. Each repetition gives the same results for the strongest continuously present chunk/other-collider press patch:

- Patch magnitude: **MEASURED 61.31–107.54 kN**, median **MEASURED 79.53 kN**.
- Adjacent-tick force-vector change: **MEASURED 2.106–48.828 kN**.
- Local lever coordinate spans: **MEASURED x 0.00359 m, y 0.03726 m, z 0.02964 m**; adjacent-tick lever displacement **MEASURED 0.000156–0.004319 m**.
- Owner verdict flips: **MEASURED 0–0** over the window; every verdict holds.

A stricter low-motion selection, each speed below **MEASURED 0.02** in its respective units, gives **MEASURED 13 consecutive ticks**, patch magnitude **MEASURED 112.02–126.11 kN**, maximum adjacent lever displacement **MEASURED 0.004864 m**, and **MEASURED 0–0 verdict flips**. These selections choose different patches/windows.

The forces fluctuate substantially, but this carrier's joints are all open. A non-flipping verdict here cannot show that a live joint near its threshold needs no filtering. Nor does this isolate solver noise from contact redistribution and slow motion. Filtering requirements remain unestablished; repeat this measurement with load-bearing closed rests after the topology is corrected, before promising a steady press can safely wait in a solve budget.

## Code confirmations and corrections to the design

All references are to the unchanged baseline source.

- **Blow-only gate: confirmed.** Presses accumulate at `src/debris/crumble.rs:427`; groups without blow patches are discarded at `:459`. Cracking additionally stops at the energy cap at `:206`. Press-only touches have zero blow and zero crushed volume at `:94`.
- **A freed group keeps its bonds/state: confirmed.** `src/construction/breakage.rs:587` clones state and retained edges, then narrows ownership at `:606`; `src/construction/fracture.rs:764` narrows member fates without resetting cracks or separation. **“Nothing judges them again” is false without qualification.** Landing blows still run the assessment (`src/debris/crumble.rs:684`), and changed spin/unfinished owner laws can run again (`src/construction/breakage.rs:1921`, `:1975`). What is absent is loose-owner slow-contact intake: `read_contacts` explicitly rejects moving groups at `src/construction/breakage.rs:904`.
- **Size-floor welds are not judged as contacts: confirmed.** Coarsening retains parted edges at `src/debris/crumble.rs:273` and `:304`, without closing their separation flags; the landing commit keeps them separated at `:708`. The graph excludes separated joints at `src/construction/fracture/parts.rs:130`. On subsequent coarsening, an old retained edge joins fragments regardless of their sizes (`src/debris/crumble.rs:292`), so these are carrier constraints with no current strength test. `src/construction/breakage.rs:553` is merely the field declaration, not proof by itself. The return at `src/debris/crumble.rs:711` is conditional on the merged result; it is not a blanket “floor-size body cannot fail” rule.
- **Shipped blow/press dividing speed: confirmed.** `src/debris/crumble.rs:625` reads the owner's `SleepThreshold.linear × PhysicsLengthUnit`. The required component comes through `Piece`/`ReducibleContacts` (`src/construction/breakage.rs:671`, `src/avian_ext/contact_reduction.rs:42`). Vendored defaults are **DERIVED 0.15 m/s** (`vendor/avian3d-0.8.0-overmatch/src/dynamics/rigid_body/sleeping.rs:103`) and **DERIVED length scale 1.0** (`vendor/avian3d-0.8.0-overmatch/src/dynamics/solver/plugin.rs:502`). Loaded construction carriers report **MEASURED 0.150000006–0.150000006 m/s**. Equality is a press: the blow test is strictly greater (`src/debris/crumble.rs:422`).
- **A further press-seam concern:** the reused blow algorithm attenuates each touch by its current fragment's mass share (`src/debris/crumble.rs:175`, `:188`), including when the input state is already disconnected by carrier edges. That plastic-stop rule is part of this experiment, exactly as requested. A sustained press should be judged at the load it actually carries; the revised implementation must justify or remove this attenuation for presses rather than quietly inherit the blow rule.

## Decision and limits

**Build slice 1 changed:** make retained carrier edges closed, cracked, push/friction-only rests before asking slow presses to part them. Include the monotone opening/shedding path in that same implementation boundary; a press-only gate change is insufficient. Then rerun the loaded-belly and bonded free-running controls on that topology. Do not move straight to ground yield based on the present non-failure result: the measured carriers are artificially rigid after their joints have already opened. Whether corrected rests actually open, how low the pile settles and whether shedding is excessive are still experiments, not established outcomes.

Not established: exact historical stall reproduction; a settled post-fracture height; hosted performance or a solve budget; closed-joint utilization for the crossing heaps; isolated constant-load solver noise; filtering needs near a live threshold; the free-running stop after carrier rests are admitted; ground yield or bulk bearing as a cause. No alternate law or production implementation was tested or retained.

The probe source was removed before validation and commit. Runs used `OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1`; no screen, audio, debugger, web damage research, GitHub write or push was used. Initial compilation used `scripts/serial-cargo test --lib --features dev_ui,bitprobe crush_slice_zero_probe -- --ignored --nocapture`; remaining repetitions invoked that exact compiled test executable in separate processes with `CRUSH_CASE`, `CRUSH_SEED`, `CRUSH_REP`, and `--test-threads=1`. The detached matrix completed successfully.

Raw ignored artifacts remain in this bay's `target/crush-{case}-s{seed}-r{rep}.jsonl`; runner/results logs are beside them, with the first run's log at `/tmp/crush-build-run.log`. The complete streams' SHA-256 values below apply to every repetition of each start:

| Start | SHA-256 |
|---|---|
| `stuck-s0` | `e888cfccf53951203760eeb43390927055a80daef896dff582f808a3014cd2ea` |
| `heap-s2` | `3137ec2547006dfef0499c96cd24b81ebea4c88c25c88acccbdcad6acecfd477` |
| `heap-s6` | `ab9c9823cf3ff21c46c1b6f25b71eff3abe4a908b9e7658df7bce1bb11f35479` |
| `free-s0` | `7f9e1b2a5230344c8a1842a185daf73cd32b57a968899a53a1814fdb4720bff3` |
| `yard-s0` | `a57685f1f4f799a4076b2eb2825728f2510d60b21cccbbfd45c2a4d882fb1502` |

Validation on the restored tree: `cargo fmt --all --check` passed; `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings` passed; `scripts/serial-cargo test --lib --features dev_ui,bitprobe debris::crumble` passed (**MEASURED 11 tests**). The probe matrix passed (**MEASURED 15 executions**). `git diff --exit-code -- src` confirms no source change remains.
