# Sweep session 3: remaining contacts

Subject: the video game's environment destruction model; game state only.
Branch: `lane/sweep`, continuing the session-2 checkpoint. The original-start reset does not apply.

## Decision and scope

No missed/excluded cast, held cut or plastic-exchange ordering defect was demonstrated for the
reported wall impulses. Preserve the law and data. The retained Avian contact policy admits
speculative planes while the sweep requires exact occupied-hull intersection. These planes
brake and deflect the source before these chunks enter the exact swept patch. Released bodies
supply another substantial obstruction. This is an outcome of retained contact/debris policy,
not evidence that brick cut capacity held the ram.

The owner decision is whether Avian's speculative envelope should participate in cut adjudication,
or contact with a standing construction should instead wait for exact occupancy. Either changes
the contact contract; neither is a repair to a missed convex cast. No substitute envelope,
engine contact rule, strength, asset, drive setting or debris route was introduced. All temporary
source probes were removed. The design doc's “Bodies in contact” owns the contract and its limit;
this report records the observations. Shared runtime lines changed in the final diff: none.

## Every nonzero placed-owner contact at +1 and +2

MEASURED: all six points belong to element 17, `north/pier_0`, part `pier`. All four chunks are
unburied and have no ground anchor. Every listed source collider was awake, mass-prepared and
cast against the listed chunk inside the bounds tree. Each cast returned no intersection;
none was held, excluded or never cast. This is the cast-miss case, distinct from the brief's
examples. The complete rows, including world points and source colliders, are in
[session-3-contacts.tsv](session-3-contacts.tsv). Normals point from construction into the hull.

| Tick | Point | Chunk | Normal (x,y,z) | Impulse (x,y,z), N s | Initial separation, m |
|---|---:|---:|---|---|---:|
| +1 | 4 | 569 | (-.033025485,.9423525,.3329879) | (-487.37177,6623.3994,7662.7163) | .08053176 |
| +1 | 5 | 569 | (-.033025485,.9423525,.3329879) | (-481.05743,6793.2935,7859.7324) | .07886412 |
| +1 | 6 | 569 | (-.033025485,.9423525,.3329879) | (-1145.739,17066.648,19747.027) | .07482344 |
| +1 | 9 | 568 | (-.022849323,.94239956,.3337077) | (-389.561,9906.632,11474.804) | .07360266 |
| +2 | 20 | 561 | (-.05772417,.53474176,.8430418) | (22004.838,8076.849,111310.21) | .10314192 |
| +2 | 29 | 559 | (-.057725485,.53474176,.84304154) | (28965.607,16623.3,93888.83) | .09750794 |

DERIVED sums of the printed rows: +1 `(-2503.7292,40389.9729,46744.2797)`;
+2 `(50970.445,24700.149,205199.04)` N s. Differences from resource sums are printed-f32
roundoff. MEASURED all 31 placed-owner points at +1 and 37 at +2 were inspected; only the six
above deliver the reported impulses. Their upward normals explain the riding response.

MEASURED verdicts on other, intersected chunks: +1 demand 39,538,650 N versus capacity
141,877.38 N; +2 demand 36,266,716 N versus capacity 394,710.2 N. Both patches break.
The braking chunks receive no demand/capacity verdict on their braking tick. Their measured
bounds are above grade: 568/569 span y=.675 to .9 m; 559/561 span y=.45 to .675 m.
MEASURED: 568/569 are swept on +2, and 559/561 on +3. Their later cast times are approximately
.00206/.00269 s and .00857/.00930 s, respectively.

## Why the casts miss

MEASURED independent static closest-contact queries at 65 equally spaced poses over the same
tick-start rigid-motion path:

| Tick | Source collider | Chunk | Start gap, m | Smallest sampled gap, m |
|---|---|---:|---:|---:|
| +1 | 6096v0 | 568 | .07360266 | .003350569 |
| +1 | 6096v0 | 569 | .07482332 | .004564275 |
| +2 | 6099v0 | 561 | .10314111 | .009368919 |
| +2 | 6100v0 | 559 | .09750732 | .0083348155 |

DERIVED continuous-path check: a hull point moves at most `(|v|+|omega|*R)*dt/128` between a
time and its nearest sampled pose, where R encloses the source AABB about its COM. This bound
is 1.348 mm for the first pair and 1.524/1.552 mm for the second pair. Subtracting it leaves
at least 2.003, 3.216, 7.845 and 6.782 mm clearance, respectively, before f32 roundoff. The
misses therefore agree with the complete constant-motion path, not just its sampled poses.

MEASURED: repeating geometry immediately after the plastic exchange still reports all four
relevant casts as misses. The recorded solver points begin with negative penetration, i.e.
the positive separations in the contact table. Pinned Avian 0.7 source:
`NarrowPhase::update_contacts` admits separated points using the velocity speculative margin;
`ContactNormalPart::solve_impulse` constrains positive separation against a contact plane.
The vendored CCD module describes the infinite-plane approximation and its ghost-collision
caveat. Exact membership is owned by `Geometry::swept` and `Motion::hit`; a cut is resolved only
after that predicate admits a chunk.

Changing strength cannot affect a chunk that never reaches the cut test. A changed contact
contract, or a changed relative path that actually closes these gaps, would change this outcome.
Neither was changed. This is a geometric/contact fact, not a missing foundation-strength fact.
The separation check describes the sweep's constant-motion path, not the subsequently coupled
trajectory under all solver forces and other contacts.

## Released bodies and subsequent motion

MEASURED non-placed construction-body impulses into the source at +2:

| Recorded owner entity | Impulse (x,y,z), N s |
|---|---|
| 6976v0 | (-4540.1343,-12862.352,230844.52) |
| 6967v0 | (-116.77321,192.4357,1066.5818) |
| 6962v0 | (684.6768,244.83641,789.35693) |
| 6959v0 | (-37.767048,-119.384995,359.96506) |

The first two remained live released owners of construction 635 at readback. A recorded owner
can already have departed by readback; the final probe records identity without requiring it
still be live. DERIVED sum: approximately `(-4009.998,-12544.465,233060.424)` N s. At +3 the
placed wall contributes zero while owner 6976v0 contributes approximately
`(-36037.688,51944.94,207124.98)` N s. The reversal cannot be attributed to placed-wall impulses
alone. Debris routing and separate-body contact remain with decor. This is a contact-account
observation, not a complete terrain/drive force account or an audit of decor's launch law.

MEASURED unchanged recipe before and after diagnostic-only instrumentation, with the same results:

| Tick from first contact | Speed, m/s | Velocity z, m/s | Swept that tick | Cumulative |
|---|---:|---:|---:|---:|
| 0 | 10.834826 | -10.77953 | 12 | 12 |
| +1 | 9.965262 | -9.937595 | 7 | 19 |
| +2 | 2.7008452 | -2.166457 | 30 | 49 |
| +3 | 1.932086 | 1.3801368 | 3 | 52 |
| +5 | 3.2826068 | 3.1257098 | 0 | 52 |
| +10 | 4.509277 | 4.496674 | 0 | 52 |

Positive z is retreat from the wall. First contact and first hole remain driving tick 1134,
zero added ticks. All four plastic exchanges pass the existing momentum account. Target
absence is 49 chunks by +2, and 52 by +3 through +10. No after-fix speed is claimed: no runtime
fix was made. MEASURED before contact: 1122 chunks, 3258 joints, zero cracked and zero absent.

MEASURED production sweep work `(bounds nodes, candidate casts, incident references, cast/contact queries)`:

| Tick | Counts |
|---|---|
| 0 | (203,34,64,46) |
| +1 | (609,40,38,47) |
| +2 | (941,90,163,130) |
| +3 | (3748,130,174,190) |
| +4 | (3638,110,66,150) |
| +5 | (3981,128,143,185) |
| +6 | (3805,107,112,151) |
| +7 | (3987,142,146,198) |
| +8 | (3057,73,110,125) |
| +9 | (3055,74,71,128) |
| +10 | (2895,61,95,112) |

DERIVED total: (29919,989,1182,1462). Temporary independent closest-contact probes and the
after-exchange geometry repetition are excluded. Broad-phase query work, internal GJK,
departure creation and structure are not counted. No timing run or capture was taken.

## Reproduction and gates

Baseline and probes use the existing tracked recipe witness:
`scripts/serial-cargo test --lib --features dev_ui,bitprobe tracked_ram_sweep_opens_within_two_ticks_and_accounts_for_momentum -- --nocapture`.
Runs use `OVERMATCH_CARGO_LOCK_WAIT_S=10800`, `OVERMATCH_UNATTENDED=1` and
`OVERMATCH_WINDOW=headless`, under the machine lock. Only the discarded observational loop
continues through +10. The shipped witness is restored. The final confirmation probe passes.
An earlier confirmation probe failed only because its reporter required an owner already
removed by production laws; the reporter was corrected. No runtime failure was observed.
No historical speed/contact-count assertion or new collision policy was added.

MEASURED final gates: formatting and CI workspace/all-target clippy pass. All 64 targeted tests
pass, with zero ignored/failing: sweep, ram, contact-transfer callers, `doc_citations`, `ui_ascii`,
`net_boundary` and `bake::`. Self-review covered all six lenses; no backlog row names either
changed document or the measurement table. Raw logs are under
`target/sweep3-{baseline,trace,cast,gap,order,confirm,final-clippy,final-gates}.log`.
