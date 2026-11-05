# Carrier-tree experiment

Subject: the video game's environment destruction model, game state only.

## Verdict and predeclared gates

**KILL this implementation as an exact linear replacement.** Carrier accumulation fits the Mac
allowance; preserving today's plane-section verdict does not. This is a failed prototype, not a
proof that every all-plane algorithm is impossible. No server or scheduling change lands here.

The owner's criteria were stated before implementation:

| Kill criterion | MEASURED value | Verdict |
|---|---|---|
| A singleton verdict differs beyond an exact rounding tie | Zero observed differences; positive holders exercised 94 carrier-reader calls in the shipped loops. All 1,815 paired weight and 1,815 push-opening section verdicts match. | Pass on this corpus; no tie exception needed. |
| A full pass costs over 0.27 ms on the Mac | Carrier intake, accumulation and bridges: median 0.204458 ms, max 0.219833 ms. Exact plane checks alone: median 8.448500 ms, min 8.353875 ms. | Carrier fits; a complete exact extension fails even before ring/rest work. |
| Plane checking grows faster than size at the double | Chunks and joints each double; plane median grows 3.75343x and counted visits 3.78470x (DERIVED ratios). | KILL |
| Carrier choice decides a shared load | Zero: non-bridges never supply a singleton verdict; planes use exact components from all live edges. All 8,897 plane topology queries match today's searches. | Pass, at the unacceptable plane cost above. |

## Primary source and computation

The `cfg(test)` prototype was beside `fracture.rs`, in `carrier_prototype.rs`. Its primary source
is the lane commit subject **construction: prototype carrier totals and census (#279)**.
The corrected scaling fixture and batched timings are in **construction: verify carrier scaling
on level-ground copies (#279)**. The report commit removes the losing code and all temporary adapters. Git retains the experiment;
no alternate runtime path or historical-output test remains.

The fixture selector enumerates admitted shipped construction files and chooses the largest
chunk graph. Its ID selects no law or pass condition. MEASURED selected data: `house_1`,
1,122 chunks and 3,258 joints. These are the observed input's sizes, rather than the different
sizes quoted in the research sessions. `ConstructionLibrary::load` supplies its admitted
`ConstructionKind::structure` at unit scale, the same shared checked structure the server's
`construction::spawn` supplies to `Live`; `State::new` supplies the same pools, fates and cracks.
No mesh or replacement construction generator supplies sim data.

A breadth-first carrier forest roots at every ground anchor and buried chunk, plus the existing
law's reference for each moving group. A reverse sweep accumulates mass, first mass moment,
sphere-plus-parallel-axis inertia, gravity/press/peak/mean-force wrenches and impulse wrenches
separated by `Load::felt`'s responses. A free group's translation and angular response enter once
per response class and once per chunk, including spin and the sphere couples' integrated torque.
Materials and `Navier` are reused. The analytic hinge, lift and fracture-work laws are unchanged;
body-source pair work still reads each body's mass and hinge lever. A summed impulse cannot
replace that nonlinear operation.

Bevy APIs were checked in pinned `bevy_math-0.19.1`, its `glam-0.32.1` reexports and
`bevy_ecs-0.19.1` (`Entity::PLACEHOLDER`). No new Avian API was written.

## One-time comparison

The weigh-cost lane's design census was read with `git show`; its branch was neither merged nor
cherry-picked. Each evaluation ran the shipped `structural` loop followed by `weigh`, first with
the original readers, then with aggregate readers. The comparison covers ordered opened-joint
and cause sequences, unassessed chunks and final state. It does not assert bit equality of
reassociated demand magnitudes. Ring/rest laws, bearing-contact deferrals, candidate ordering and
the repeated-failure control flow remain in the census. This compares arithmetic inside the
existing scheduler, rather than claiming an integrated replacement has shipped. Temporary
adapters refresh the prototype at every stage; an independent topology census compares singleton
bridge groups and exact plane components to `held` searches.

The MEASURED corpus comprises:

- 12 grounded and 12 moving inputs, the latter alternating zero and nonzero spin. Contact, body
  and field sources cover peak/no-peak responses, directions and force/impulse magnitudes derived
  from the graph's mass, strengths and fracture work.
- 12 snapshots of a law-selected first-break sequence. Five stages fail, then the chain settles;
  the last seven inputs are settled states, not additional failure stages.
- 12 positive-singleton inputs. The intact graph has no singleton holder, so the census selects
  its largest non-buried carrier subtree from data (96 chunks) and cracks every alternative
  bonded route across its boundary except one joint. Masses, patches and materials remain the
  shipped data. Cracked contacts still participate in the shipped bearing-contact deferral.
- The actual initial scheduled-break input formula from `net::loadtest::drive`: sorted buried
  and ground-anchor chunks each receive a centroid line with energy `2 * volume * crush` and
  zero momentum. `strike` produces the dusted/cracked lists used by both checks. MEASURED:
  160 supports, 160 dusted chunks and 544 initial cracked joints.

**MEASURED: 49/49 evaluations identical**, including failure sequence/cause, unassessed set and
final state. All 145,311 singleton topology queries, 8,897 plane topology queries, 1,815 weight
section readings and 1,815 push-opening verdicts match. Positive singleton totals were read 94
times by the production-loop adapters. There are zero observed verdict differences to explain
or excuse as rounding ties. These finite observations do not prove every source combination or
exact axial/strength tie. In particular, the aggregate push prototype uses a zero-moment hinge
absence instead of the expanded point stream's count-dependent roundoff guard; a replacement
would need a justified bound for its new arithmetic before shipping.

## Shared spans

Today's `held` refuses a singleton if another bonded route reaches a ground anchor, buried chunk
or moving reference. `weigh` also defers a group when a cracked seam or intact rest still bears
compression onto grounded matter (`bears_on_contact`). Touched chunks left unassessed or deferred
appear in its unassessed set.

"All shared spans are unassessed" is too broad: `sections` also groups coplanar joints. Removing
an entire plane can disconnect a group even though none of its joints is a bridge. Today's code
can assess that whole plane. Skipping every non-bridge would lose those verdicts; choosing one
carrier to decide their load would invent a law.

One iterative low-link pass uses edge identities, including parallel anchors and implicit
soil/reference edges. An actual bridge's unsupported side is independent of carrier choice;
non-bridges supply no singleton decision. A generic two-route span, reversed carrier order and
duplicate anchors witness those cases. For planes, the prototype removes every joint in the cut,
visits the ground component first, and accumulates each remaining rooted component using all live
edges. It preserves the verdict independently of the tree, but repeats at least S(N+J) graph
work for S planes, plus component sorting and section reads. A bridge pass alone does not answer
these simultaneous multi-edge cuts. The doubled input doubles S as well as N and J, exposing the
superlinear work. No exception patches around that finding.

## Timings and limits

MEASURED Apple M4, `edit` profile (crate opt1, dependencies opt3, LTO off), CI features
`dev_ui,bitprobe`. `scripts/serial-cargo` held the machine lock for each run; no compile overlapped
the test. Every run exported `OVERMATCH_UNATTENDED=1`, `OVERMATCH_WINDOW=headless` and
`OVERMATCH_MUTE=1`. No game, screen, capture or tick-pricing run was started.

| Operation | Min / median / max, ms | Workload |
|---|---:|---|
| Full carrier pass (MEASURED) | 0.199875 / 0.204458 / 0.219833 | 1,122 chunks / 3,258 joints; includes bridges |
| Changed grounded press, per update (DERIVED) | 0.000046059 / 0.000046387 / 0.000094238 | Divide each measured 256-update batch by 256; 26 nodes including ground |
| 256 press updates (MEASURED) | 0.011791 / 0.011875 / 0.024125 | Every delta exposes its writes |
| One bridge failure (MEASURED) | 0.000666 / 0.000708 / 0.002500 | 96 hanging chunks and two ancestors; cache copies excluded |
| Exact planes, source (MEASURED) | 8.353875 / 8.448500 / 8.789708 | 186 planes; 1,453,962 counted visits |
| Exact planes, double (MEASURED) | 31.407667 / 31.710833 / 32.716042 | 352 planes; 5,502,816 counted visits |

Tree/press/plane timings use five warm-ups, then 101 / 1,001 / 31 samples respectively. Each
press sample batches 256 alternating deltas and exposes the cache after every update. Dividing
by the batch size amortizes clock cost. Bridge failure uses 101 samples without an explicit
warm-up. An initial positive/negative-pair probe did not expose its writes, and a subsequent
single-call probe lay near clock resolution; their timings are discarded.

The carrier bracket includes compact frame intake, adjacency allocation, carrier selection,
reverse accumulation and low links. It excludes plane enumeration/checks, ring/rest checks and
state construction. Plane brackets include exact component detection, aggregation and the
unchanged Navier weight reading, excluding section enumeration and carrier setup. Even this
incomplete part exceeds the full-pass budget. No timing assertion is left behind.

The double is two disjoint horizontally translated copies of the admitted structure, with
addresses and area-share footprint coordinates translated consistently: 2,244 chunks, 6,516
joints, 352 planes. Height and buried status are preserved; shared horizontal planes stay shared.
The probe has one grounded press, gravity on all chunks and reads every candidate plane. It is
a scaling fixture, not a second shipped asset or changed material. The initial all-axis copy
moved buried chunks above grade; that double's timing is discarded, not used for the verdict.

A standing press update assumes unchanged topology and reference. A moving press changes rigid
acceleration and potentially the reference; this prototype performs full intake. The failure
bracket enumerates a genuine bridge subtree and subtracts its cached totals along its ancestors;
it excludes release, reframing the detached owner, non-bridge rerouting and multi-joint
reassessment. Scratch is still indexed by the full construction, so small-owner cost is unproven. Body-source
hinge work still scans sources per candidate; dense source sets have no linear-cost proof here.
No hosted two-core or worst-tick claim follows from these isolated measurements.

## Session 2

Do not replace the server path on this evidence. First supply an exact all-plane method with a
linear work argument and a passing double-size measurement. Changing the section law instead
would require an owner decision and would cease to be this refactor.

If that gate clears, owner-local carrier state replaces repeated `ground_path`, `held`,
`crossing` and candidate membership searches. Compact frame responses replace the expanded
`in_group_frames` stream. Retain materials, `Navier`, source-specific pair work, ring/rest laws
and bearing-contact deferrals until their replacements are proved. Resolve the new arithmetic's
roundoff bound and compact owner addressing before claiming the replacement complete.

Replace the repeated failure loops in `structural`/`weigh` with at most one failure stage per
owner per tick and a named pending-reassessment state. The stage allowance belongs to the owner
across both checks, rather than one allowance for each check. It is `Resolution`, justified by
bounded tick cost. Preserve parallel owner evaluation and the ordered effect commit.

Before editing the server path, run twice:

```sh
scripts/perf/collapse.sh --target local --ram scripts/perf/ram.json --out target/carrier-before-a
scripts/perf/collapse.sh --target local --ram scripts/perf/ram.json --out target/carrier-before-b
```

After its gates, repeat with `carrier-after-a` and `carrier-after-b` output folders. Compare raw
fixed ticks, worst tick, the 15.625 ms limit, owner-stage costs, impact admission and populations.
These runs belong to the orchestrator's pricing session, as does hosted two-vCPU verification.

## Gates and artifacts

The corrected archived prototype passed 249 construction tests (seven measurement beds ignored), 48
whole-tree guards (`doc_citations`, `ui_ascii`, `net_boundary`, `bake::`), its explicit census beds,
`cargo fmt --all --check`, `git diff --check` and locked workspace/all-target CI clippy with
`dev_ui,bitprobe`. Final removal passes 247 construction tests (three pre-existing measurement
beds ignored), the same 48 whole-tree guards, formatting, diff whitespace and the same strict CI
clippy command. No matching fracture/prototype backlog rows were found.

Raw evidence stays in `target/carrier/`: `census.log`, `extra-final.log`, `scaling-final.log`,
`corrected-{construction,guards,clippy,fmt}.log` and the final gate logs. The report is the only
net addition to the branch. Start: 2026-11-05 10:44:34 +07; final gates completed at 11:23:18 +07,
within the owner's one-hour limit.
