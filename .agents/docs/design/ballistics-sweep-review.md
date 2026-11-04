# Ballistics sweep review

Subject: the game's ballistics and damage model, game state only. Crew are HP pools at stations;
fields deposit dose, and a pool emptied leaves its component knocked out.

## Review of sweep-ball-1

Reviewed the commits after "trees: require sourced root-plate overturning coefficients (#236)"
through "spec: refuse a requirement member coefficient outside (0, 1] at load (#219)" against the error-handling, code-branching
and base-truths lenses and the sweep brief. No earlier ballistics sweep record was present in
`.agents/docs/design/` at this base.

### Standards findings

- **B1 remains P1:** the drag parameters in `round_types.ron` are bit-preserving and honestly
  labelled `Tuned`, including the original calibration and absence of an aerodynamic source.
  This makes the assumption visible; it does not supply the missing aerodynamic evidence. The
  universal relationship between the hit-nose factor and aerodynamic drag remains an assumption.
  Do not call B1 fixed or replace it with another unsupported coefficient.
- **R1, P2, fixed:** `fuzz::probe_rounds` used `round_by_key`, whose uniqueness requirement the gun
  registry does not establish. Another gun chambering an existing round would panic the
  instrument, and the stowage class-invariant test would reject that valid registry. Enumerate
  every `(gun, round)` configuration at its own muzzle speed; reports retain both identities.
  The synthetic shared-ammunition test exercises distinct speeds through the same round key.
- **P3:** `ShellReadout` duplicates projectile state for test-only readers. The `ShellVisual`
  comment is already generic at this base; the inventory's stale-calibre-comment claim is no
  longer current. The fuzzer still names
  one tank in its target/report. The gun parser validates then discards `names` and `caliber`.
- **P3 / B11 follow-up:** the pen cam still loads one scene and scales it by record calibre. Its
  record already carries a round key on the wire; the presentation can use that identity to read
  authored body metadata without adding another protocol field.

- **R2, P2, fixed:** the server-composition gate still required two calibre classes and a round
  surviving to the end of a frame. A valid tank with one weapon class, or a round stopping in its
  first tick, could fail the gate. Sample actual projectile births and removals instead; require
  a shot to exercise the gate and reject render components independently of calibre or lifetime.

The other reviewed changes introduce no further P1/P2 findings: authored body and streak presence
reaches world rendering; model metadata is checked against glb bounds and flying diameter; the
replacement registry/body tests express class invariants; the damage queries now assert their
spawn/relationship invariants; member coefficients are refused outside their fractional domain.
The codegen workflows use one feature set and cache family; the asset door still runs the Rust
bake consumer it verifies. The disk sweep's size readout remains diagnostic while reap failures
remain visible.

### Spec review

The implemented registry, visual and error-handling portions match their brief, subject to R1
above. B1 is incomplete, as are the explicitly handed-off pen-cam and impact consumers. The visual
model declarations are authored stand-ins, not a claim of measured round-specific shape.

### Verification

R1: `cargo fmt --all --check`, workspace/all-target clippy with `dev_ui,bitprobe`, and the targeted
`guns::`, `ballistics::fuzz::`, `doc_citations` tests passed. MEASURED: 17 passed, one ignored
on-demand instrument, no failures. No captures or pricing runs.

## Field admission (B5)

Fixed: a throw's candidates are every spatially reached ballistic or witness volume, independent
of `VolumeOf`. The throw carries no owning-tank filter. Source-volume exclusion still prevents
counting its own line dose twice; other tanks and unowned matter can both receive and occlude the
field. Attribution retains ownership. The existing recorder contract explicitly opens records
for encounters of the round's path with a tank; a field-only recipient opens no new encounter
(`damage_cam::recorder` module invariant). This change preserves that presentation contract.

Replaced the old ownership-exclusion assertion with owned/unowned source plates, three recipients
(owning tank, neighboring tank, unowned matter), and a foreign-owned occluder. The runtime/gate
agreement tests now compare recorded line energy directly, independently of field dose, rather
than relying on ownership to silence fields. MEASURED: all 41 resolver/citation tests passed;
format and workspace/all-target clippy passed.
The cross-platform field-cast pin also passed without re-pinning.

## Remaining internal failures (B28–B38)

- Already fixed by the walk redesign: B28 witness-walk failures, B29 failed walks, B30 uncertified
  starts, B31 entry-force errors, B33 field-direction errors, B34 witness-pass errors and B38
  crossing-count exhaustion now panic through their classified walk invariant. B8's overrun
  envelope is gone; containment closes against finite loaded geometry instead.
- Fixed B32/B35: a derived burst or spall field outside its configured law is an unconditional
  bug, naming its validated inputs. Non-finite/negative strength and an invalid source direction
  also panic instead of disappearing at the quantization gate or taking a default heading.
- Fixed B37: every pool appended by `ShotDose::settle` has its emptying deposit's context. The
  consumer now requires it; the step endpoint no longer invents an entry or direction.
- Fixed B36: instantaneous rest advances time and preserves heading; see the rest gate below.

MEASURED: all 98 targeted march, resolver, field-law and citation tests passed. Format and
workspace/all-target clippy passed. The law's existing refusal tests exercise invalid field
ratios; march tests cover both line and field emptying contexts.

## Instantaneous rest (B36)

Fixed: a zero free-flight step preserves the incoming sampling heading, advances an armed delay
for the stationary interval and publishes its resulting state. The next gravity step can move the
round instead of repeating the old upward velocity forever. A delay expiring there bursts at its
remaining wait. Non-finite velocity/speed is an invariant failure in every build. The march no
longer returns an optional step that the caller can silently discard.

MEASURED: all 53 targeted fuze, march and citation tests passed, including exact-apex cases with
an inert count, a continuing armed count, an expiring count and deliberately invalid velocity.
Format and workspace/all-target clippy passed.


## One projectile state (readout follow-up)

Fixed: removed the unused readout component, its authority and replica writes, and its fixture
bundles. The tests read the bound projectile body and velocity directly; view dressing remains
independent of simulation state. MEASURED: all 219 targeted march, public-path, pen-cam,
field-cast and citation tests passed; three on-demand instruments stayed ignored. Format and
workspace/all-target clippy passed; no field-cast pin moved.

## Authored bodies in the pen cam (B11 follow-up)

Fixed: the plan resolves its body from the record's existing round key. The inset replaces its
scene and material by model identity; the x-ray caches by that identity too. Both use the body's
native dimensions, and a trail ends at that body's tail. An authored body absence draws no body
or model-material pieces; an unregistered synthetic clip also has no invented body. The diagram's
calibre still sizes its trail, rings and framing. No record or wire layout changed, so no protocol
revision bump is needed.

The new synthetic controls switch scenes/materials between distinct models at unchanged record
calibre and vary diagram calibre without resizing either model. The model-bound gate now has an
independent synthetic control for parent scale rather than borrowing the first shipped model's
numbers. The corpus still tests the recorded path of a round drawn without a body.

Verified against the installed Bevy 0.19 source: `WorldAssetRoot` changes are handled by
`world_instance_spawner`, which despawns the previous instance before instancing the new handle;
`WorldInstanceReady` invalidates the stage's mask copies. MEASURED: 192 targeted pen-cam,
round-effects, headless-composition and citation tests passed; six on-demand instruments stayed
ignored. Format and workspace/all-target clippy passed. No captures.

## Recoil readout (B3, owner decision)

The game's damage model: MEASURED software readouts from the headless
`hull_recoil_is_an_opposed_linear_response_to_shot_momentum` test, using the gun registry's
launched assembly mass and muzzle speed. These are impulse magnitudes along the negative bore,
not external physical measurements. The shot-momentum reference is `mass × muzzle_speed`; the
current response is the actual `recoil_impulse` function applied by shooting. No recoil behavior,
including `RECOIL_FEEL`, changed. No frame/tick timing was measured.

| Gun / round (fixture data at this revision) | Shot momentum (N s), MEASURED | Current hull impulse (N s), MEASURED |
|---|---:|---:|
| kwk36_l56 / 88mm_AP | 7884.5996094 | 19711.5000000 |
| kwk36_l56 / 88mm_APCR | 6789.0000000 | 16972.5000000 |
| kwk36_l56 / 88mm_APHE_ring | 7884.5996094 | 19711.5000000 |
| kwk36_l56 / 88mm_HE | 7626.0000000 | 19065.0000000 |
| mg34 / 7.9mm_AP | 8.9090004 | 22.2725010 |

The synthetic control tests opposition to the bore and linearity in mass and speed, without
pinning the ratio the owner is deciding. Format, workspace/all-target clippy, the readout and
citation gates passed. MEASURED: four tests passed, none failed.

## Instrument names and dressing follow-up

Fixed: the fuzzer's target entity and Markdown heading read the selected tank's authored display
name. Gun labels read every authored designation and nominal calibre, while retaining canonical
gun/round identities for reproduction. Nominal calibre is a designation; no chamber-fit rule is
inferred from it. The world-body marker describes authored model dressing, and the unreachable
reinsert-insurance branch is removed: projectiles are dressed once at complete birth.

MEASURED: 19 targeted gun, fuzzer, dressing and citation tests passed; one on-demand instrument
stayed ignored. Format and workspace/all-target clippy passed.


R2 verification: format, workspace/all-target clippy, the real server-composition test and the
citation gates passed. MEASURED: four tests passed. The gate no longer relocates a neighboring
tank or waits for particular calibre classes to survive a frame.

## Retired crew-sweep comparison

The game's damage-model sweep now measures the current witness law only. Removed the reconstructed hard-substance crew law and its pen-cam convenience hook; the resolution survey and owner-held F2 knockout-rule comparison remain live. Station outcomes and reports use knocked out / reduced / untouched. No production damage behavior changed.


## Authored contact dressing

Fixed the impact follow-up: removed the calibre cutoff. Every round requires a sourced compact or
layered presentation recipe, bound at complete projectile spawn and carried into authority and
replica contact events. Scatter reads the same recipe. Surface feedback still controls powder and
spark admission. The current recipes retain their output and RNG order; their dimensions and
emissions are explicitly ASSUMED art tuning, not a measured physical emission model. Dev capture
cues now require `style: Compact` or `style: Layered`, with no invented flying body.

The contact event is local. Fire reconstruction still resolves the same gun/round identity from
the registry; no record or wire layout changes and no protocol revision bump. The armaments
fingerprint already includes all authored round data. Synthetic controls select either recipe at
either flying diameter, refuse an omitted recipe and carry both recipes through a real fired
round's contact. MEASURED: 106 targeted round, impact, scatter, march, replica-path, debug, capture
plan, battle, headless-composition and citation tests passed; three on-demand instruments stayed
ignored. Format and workspace/all-target clippy passed. No captures.

## Test-only march seams

Fixed the code-branching follow-ups in the march: the alternate cone, burst and discovery-ceiling
resources and their reads are compiled only into tests. Shipped marches take the loaded law and
work bound directly. The continuous field-density oracle is test-only too. Removed `finish`'s
unread law argument and the fixture wrapper's corresponding argument: all law decisions this
stage consumes are already bound in its certified transit request and mechanism.

## Unread fuze position and coefficient units

Removed the unread fuze-position enum and its parsed fields. Cited placement claims remain as
asset comments; no current law, UI or instrument consumed that field. The trigger and selectable
modes remain required. Corrected the crew-line coefficient prose: HP dose is disc-mean chord times
the stored coefficient times `HP_PER_REFERENCE_MM`. The legacy scale's identifier is retained;
no coefficient, conversion or HP behavior changes.

## Derived frame invariants

The game's damage model now requires a checked fire bore to construct its sampling frame, and
requires a validated field stratum in its orthonormal frame to construct a finite nonzero
direction. Neither internal failure substitutes a world basis or an axial field ray. The broken
field-frame control panics instead of depositing dose along an invented direction.

## Unfinished physical contracts and owner calls

- **B1, P1, unresolved:** moving the hand-set aerodynamic anchors into honestly labelled data
  did not establish their aerodynamic basis. No damage-topic web research was authorized here.
- **B4, P1, unresolved:** `launch_turrets_on_cookoff` still prescribes linear and angular launch
  velocities. `Ammo` identifies only a volume with HP and ownership. Per-weapon stowage counts
  exist but do not assign contents to ammunition volumes; the reserve is currently infinite.
  Round filler energies exist, but neither propellant energy nor confinement, attachment failure
  work, a release force path or per-volume energetic contents are supplied. `TurretSpec` supplies
  mass and inertia extents only. A conserving release also needs the hull's inherited motion and
  its mass/inertia after detachment. These facts must be supplied before deriving a release;
  relabelling the current kick as an energy coefficient would leave the finding intact.
- **B6, P1, unresolved:** the ground contact still functions the fuze with unbounded resistance
  and stops the round immediately. The old `stops_unresolved` catch-all is already gone. The
  height grid supplies a surface, while `GroundType` supplies print, friction and dust responses;
  neither gives the material's ballistic constitutive response or solid intervals for the
  generic march. Supply the ground response and its geometric adapter together. No arbitrary
  soil strength was invented in this lane.
- **B3:** measured above, unchanged; the owner decides the recoil response.
- **B2 and B13:** sampling and the protection contract remain untouched, as instructed.

Remaining code-branching P3 work outside the fixes above includes the other pen-cam CPU oracles,
recorder/delivery dead readers, `SpallCone` pin-only source fields, dev-only runners and switches,
trace-only state and its FNV copy, and record test helpers. The old `MarchMode::Demo`, fallback
`Held::age` and perspective-slice switch are absent at this base; their inventory claims are stale.
The open F2 and axis-rule experiments retain their users.

Final frame-invariant validation: format and workspace/all-target clippy passed. MEASURED: 313
targeted march, field-law, wire, field-cast, pen-cam, protocol, bake and source-guard tests passed;
four on-demand instruments stayed ignored. This includes `doc_citations`, `ui_ascii`,
`net_boundary` and `bake::`. No field-cast pin or wire definition changed.

R2 gate follow-up: frame-end sampling also reads every surviving projectile after deferred birth
commands. A canary inserts visibility after a clean birth and requires the sampler to report it.
Birth and removal samples still cover rounds whose whole lifetime fits inside a tick.

R2 final validation: format and workspace/all-target clippy passed. MEASURED: 45 targeted tests
passed, including the real server composition, the late-dressing canary and all required
whole-tree citation, font, network-boundary and bake guards. The broader final batch above
also passed the protocol checks, pen-cam records and unchanged cross-platform field-cast pin.

## Review of sweep-ball-2

Reviewed from "ballistics: probe every gun and round configuration (#219)" through
"ballistics: sample surviving server rounds after deferred dressing (#219)" against the three
lenses and for correctness.

- **P1, fixed (`owned_losses`):** once fields reach every volume, a burst or cone near the firing
  tank can dose its own pools, and hit confirmation counted every tank-owned pool: a self-dose
  raised `ShellDamage`, the shooter's marker read as a hit. It now excludes the shooter's tank.
- **Field admission (B5) is the physical reading.** A field now doses the firer's own exterior and
  any neighbour where the old filter silenced it; hard volumes still stop each direction at
  entry, so interior crew are reached only where the geometry lets a direction in. Attribution is
  unchanged. New finding (base-truths, P2): the round's own line stays transparent to its firer's
  volumes for the whole flight (`not_own_volume`), a per-object rule where the truth is the
  muzzle's clearance; a ricochet back into the firer passes through it.
- **Verified, no finding:** the pen cam's body resolves from the record's round key, which decode
  takes from the shipped registry's vocabulary, so `Plan::new`'s panic is a bug outcome; that
  commit touched no wire file. The contact recipe is a required authored enum on a local event.
  Every law-override resource and its insert sites are test-only. The retired crew-sweep arm
  leaves a test-only module. At rest, `Fuze::at_speed(0)` keeps the last positive clock speed,
  so `wait_at_rest` spends the armed delay in seconds. B1, B4 and B6 were rightly left.
- **P3:** `prepare_round` keeps the last model's material when a body-less record shows
  (harmless: pieces are gated on a body); an unregistered synthetic clip draws no body; the field
  horizon now unions every reached hard collider (unpriced).

## Part 3: error-handling and code-branching follow-ups

Fixed, each its own commit: the shooting state (B09–B18, B20–B23: spawn invariants are expects;
a client's missing gate and loads for another player's tank and its `Static` replica body are
named absences); aim, walk, field reach, trace and atlas invariants (B27, B42, B47, B55,
B64–B67); a malformed `OVERMATCH_XRAY` window refuses the start (B69); certified hard bodies
fail loudly (B61); the trace hasher feeds the one FNV-1a; test-only record readers, CPU shader
mirrors and their constants compile into tests alone; each trace lever names its user.

Already fixed at this base: B01 (a burst core's facts are required), B19, B24–B26, B41,
B43–B46 (the walk redesign), B51–B54, B70 (the pen-cam demo binary is gone).

Also fixed: the ricochet's `reflect` and `bend_toward`'s turn keep unit length by construction
and say so (opposed directions, which name no turning plane, stay a named case), and the slice
frame's plane is required rather than defaulted.

**Client resync (P1, fixed):** the collider history ran on clients, whose replica hulls lose
`Position` for a frame at an interpolation resync; `place_children` panicked. A client marches
no round (ADR-0037), so the history now runs only where the walk runs.

Left for part 4: B02/B03 (asserting a dose's sign first needs the line energy proven
non-negative); record/codec validation B56–B60; staging B62–B63, B68, B71; B72/B73 (the
recorder may pose a despawned tank, an absence to name first); the P3 singleton groups and the
remaining direction fallbacks (`resolve`, `walk`); `RecordedShot::tank`/`shooter`;
`SpallCone`'s pin-only fields; `potency::band` (its `Band::Nothing` has no other constructor);
the `ballistic_fuzzer` CLI's user; the `Cargo.toml` comment; `RoundFile`'s
`#[serde(default)]` optional blocks (new); `SPIKE_TRACE_SIM_FIELDS`, whose only user is the
ADR-0029/0030 capture (owner: keep, or delete with those ADR lines).

## Review of sweep-ball-3

Subject: the game's ballistics and damage model, game state only.
Reviewed from "ballistics: confirm no hit on the shooter's own pools (#219)" through
"ballistics: correct the part-4 list in the sweep record (#219)" against the three lenses and
the part-3 brief.

- **R3, P1, fixed:** `shooting::fire` required a replica root's `Position`/`Rotation` through
  its tank query before checking whether the weapon fires. Lightyear's `queue_history_presence`
  can remove either interpolated component during resync. The tank identity remains required;
  a replica's temporary pose gap now waits without consuming readiness or round bookkeeping.
  An authority missing its spawn-complete pose still panics. Controls cover both missing pose
  components, recovery, and the authority refusal.
- **History fix verified:** `ProjectileMarchWorld` is the only production history reader, and
  `integrate_projectiles` returns on replicas before calling any of its geometry/history methods.
  The mounted history has one authority role condition and no alternate client walk. The existing
  regression exercises normal authority intervals and a client resync; an authority does not have
  an interpolation timeline to resync. It does not replay a real transport resync on both ends.
- **Verified:** hit confirmation excludes only the shooter's own pools; field admission is unchanged.
  Weapon slots and bound fuze modes follow their construction invariants. Remote private gates
  and loads have named absence. Test-only readers and shader mirrors are gated to tests; the
  remaining Band constructor is part 4's cleanup. The new invariant failures name their premise.
- **P3:** the history regression comment tells capture/edit history instead of just its invariant;
  the x-ray window parser reports invalid input visibly but represents its error as `Option`.
  The previously recorded body-less material retention and field horizon pricing remain owner work.

R3 verification: format and workspace/all-target clippy passed. MEASURED: all 23 targeted
shooting and citation tests passed, including both resync-pose gaps and authority refusal.

- **R4, P2, fixed:** the named private gate/load absence in `tick_weapon_gate` and `fire`
  still accepted missing state on authority and controlled roots. Only an uncontrolled client
  tank may lack both private components; partial private state or absence on the authority or
  controlled tank now fails its construction invariant. The control runs both systems on all
  three role/ownership cases and checks that no failed case consumes round bookkeeping.

R4 verification: format and workspace/all-target clippy passed. MEASURED: all 24 shooting and
citation tests passed. No other P1/P2 introduced by sweep-ball-3 was found in the reviewed diff.

## Part 4: dose-domain invariants (B02/B03)

Subject: the game's damage model, game state only. Both quantizers now require finite,
nonnegative contributions before a float-to-integer cast can erase invalid input. The line law
already forms a nonnegative kinetic-energy loss, partitions it with nonnegative cost weights,
and bounds each subinterval loss at zero; no new physical clamp was added. The independent edge
control spans rest, gaps, tiny chords, tied overlaps and finite material resistance, checking
nonnegative deposits and their total against the incoming kinetic energy. Invalid dose/energy
controls require failure rather than a zero deposit. Positive accumulator saturation is retained:
it caps an overlarge valid contribution, while pool settlement still caps at the available pool.

B02/B03 verification: format and workspace/all-target clippy passed. MEASURED: 115 targeted
quantization, mechanism, march, pen-cam, fuzzer and citation tests passed; four instruments ignored.

## Complete record poses and delivery (B56–B58, B72/B73)

Subject: the game's damage model, game state only. An empty offline loading frame needs no
vocabulary; a finished authority record requires the blueprint-derived vocabulary and successful
encoding. Neither missing readiness nor an internal codec failure is logged and dropped.
Every simulated servo pose is now explicit: the recorder distinguishes a despawned tank from
an existing servo-less rig, requires the registered slots' state, and preserves remote-state
precedence. A despawned tank receives no finished record. The encoder refuses an omitted declared
angle rather than inventing rest; synthetic rest-pose clips remain local. No serialized layout
changed in this producer fix.

Complete-record verification: format and workspace/all-target clippy passed. MEASURED: 90
record, delivery, codec, recorder-integration, pen-cam and citation tests passed; four ignored.
The first new readiness test lacked its fixture clock; after supplying it, the intended refusal
and the legitimate empty loading frame both passed.

## Semantic record admission (B59/B60)

Subject: the game's damage model, game state only. Local record parsing, authority encoding and
network decoding now share semantic admission before presentation: finite domain-correct values,
unit directions, ordered paths and events, origin/departure/burst path consistency, valid field
descriptors, component references and HP-pool bounds. Local synthetic clips retain explicit
missing vehicle/servo/dose-placement allowances, while obeying numeric and path invariants.
Network semantic refusals have a typed codec variant, and local parsing distinguishes syntax from
semantic failures. Protocol revision deliberately advances from 55 to 56 for the stricter admission
contract; serialized record fields and quanta are unchanged. Independent malformed local and
network controls exercise their own edges rather than relying on encoder/decoder agreement.

Semantic-admission verification: format and workspace/all-target clippy passed. MEASURED: 262
record, codec, pen-cam, recorder-integration, explanation-transport, protocol and citation tests
passed; four ignored. The existing deliberately unplaced burst-dose clip is now explicitly
synthetic. All recorded corpus reports pass the new edge without changing their data.

## Collider-exit birth and bounded source clearance

Subject: the game's damage model, game state only. Mounted rounds are born at the forward bore
exit from their source's physics collision proxies. The flash, report and pen-cam muzzle anchor
remain at the muzzle node. `FireShell` and the fire message carry both positions; authority flight,
client catch-up and the presented march start at birth. This deliberately advances `PROTOCOL_REV`
to 62 with its fire-type pin: the lane wrote 61, which "driving: type every weapon and battle span
and refuse paused rounds (#238)" had taken on main, and the orchestrator's merge moved it on ("net:
pin the merged wire types at PROTOCOL_REV 62 (#247)").

The earlier MEASURED construction audit established why deleting all initial source clearance
would stop mounted rounds in their own barrel or mount. Its fixture evidence remains:

| Mounted fixture configuration | Own volumes at birth, MEASURED | Unfiltered outcome, MEASURED |
|---|---|---|
| Coax / 7.9mm_AP, zero recoil | Coax_MG_Barrel | Stops after 0.0142822 m; barrel reduced by one shown HP point |
| Coax / 7.9mm_AP, full authored recoil | Coax_MG_Barrel | Stops after 0.0142822 m; barrel reduced by one shown HP point |
| HullMG / 7.9mm_AP, full authored recoil | Hull_MG_Ball and Hull_MG_Mantlet | Stops after 0.0158691 m; no shown pool loss |

The source is now ignored only until the round's whole disc first leaves its box: it stays
transparent while any sample start is inside the box. An incomplete playback cursor is not a
discovery disc: its captured admission survives tick boundaries until certified starts resume.
A decided chain ends at every reflection to spend its real flight time, or at the decision after
which the source is solid: a straight one that hands on every start outside the box with none held,
or a bounce whose way in took every sample out, so a return meets the source solid wherever the
bounce resumes. Starts handed on outside the box keep the membership their sweep read, since no
source volume can hold them. The box is the union of all source-body
collider AABBs, including ballistic barrel volumes and physics proxies, transformed into the
firing rigid body's local frame at both captured interval endpoints. It follows that body's pose;
a tick-end world-space box could move away before the round's march begins. Clearance uses the
same local endpoint-chord model as moving-collider discovery, splits open flight at the last
sample's exit, and uses the shared boundary nudge before rediscovery. A spawn-time latch keeps the
source solid thereafter, including later ticks and returns. The whole-flight predicate and its
pinned test are gone; source identity remains for attribution.

MEASURED software checks: the muzzle-birth mutant missed the intervening plate and failed the
mounted-fire origin test. With collider-exit birth, foreign-hull and unowned-wall fixtures meet
and pay their plates from outside; their deliberately thick fixture plates stop the rounds.
The real reflected-source fixture failed under whole-flight filtering and strikes its source
under bounded clearance. A source moving farther than its box width in one tick retains clearance
while the round stays inside the body-local box. Geometry, speed and pools are labelled fixture
data; these are class invariants, with no vehicle-specific pass condition.

The headless presentation check samples both sides of the muzzle within the first tick: the
replayed path begins at birth, and its early tracer geometry includes the birth-to-muzzle prefix;
the event's visual muzzle remains separate through cursor release. DERIVED: a five-metre prefix
at 773 m/s lasts about 6.47 ms, less than the 15.625 ms fixed step. The trace therefore includes
that short barrel-length segment; no capture or raster-occlusion claim was made.

The birth is the bore's first open point. The weapon declares its bore-start distance behind the
muzzle, in the recoiling weapon's frame, with provenance. From that start the bore line is walked
forward through the intervals of the source's convex Vehicle-layer proxies; the first point
outside all of them, one nudge past the face it leaves, is the birth. A mount no proxy holds is born at the mount, and a
proxy past a gap does not move the birth: a wall in that gap is the march's to meet.

Deferred alternative in #247: colliding barrels, a hollow bore and a round born at the breech.

## Required staging facts (B62/B63, B68, B71)

Subject: the game's damage-model view, game state only. Staging requires the certified tank asset
named by the decoded vocabulary; a failed load can no longer disable the view. Once a scene's
hull appears, its declared servo nodes and transforms are required. The existing asset consumer
contract checks `TankSpec::node_references` against the model; staging now enforces that same
premise instead of drawing a missing servo at rest. A descriptor reaching shader packing must
have passed record admission; an invalid law can no longer become a zero/unlit lane. Slice
materials require the retained original glTF material after its dependencies loaded. Synthetic
stage fixtures now retain actual materials instead of relying on the deleted fallback.

Staging verification: format and workspace/all-target clippy passed. MEASURED: 194 pen-cam,
field-cast, headless and citation tests passed; six ignored. The field-cast pin is unchanged.
Missing-node and malformed-descriptor controls fail explicitly; the shared stage fixture now
spawns every servo its fixture spec declares.

## Remaining derived directions

Subject: the game's damage model, game state only. The resolver requires the ricochet and transit
axes constructed by its checked entrance; neither substitutes the incoming direction. A walk
requires finite normalized entrance axes/normals after its named vanishing-aggregate case.
Sampling-frame construction refuses invalid axes or references; transport requires valid flight
axes and a lateral reference that survives its unit rotation. It never re-anchors or returns the
old basis on failure. The verified glam normalization uses the same reciprocal-and-multiply
operations as the previous successful path. Clipped corridor endpoints explicitly name their
missing face; any interior boundary missing its collected normal is a bug.

Direction verification: format and workspace/all-target clippy passed. MEASURED: 361 walk,
resolver, march, fuzzer, pen-cam, field-cast, headless and citation tests passed; seven ignored.
The existing field-cast pin stayed bit-identical. The endpoint-convention comment then received
a fresh format, clippy and citation check (three passed); no behavior changed after the broader run.

## Explicit round construction blocks

Subject: the game's damage model, game state only. Every `RoundFile` now requires `nose`,
`penetrator`, `burst` and `fuze` to be authored as `None` or `Some(..)`. Removing `serde(default)`
alone would still let serde default an omitted `Option`; the explicit deserializer disables that
implicit arm. Missing blocks refuse the named round file, name the field and tell the author to
supply `None` or `Some(..)`. All shipped round files and the round/gun fixture builder now state
their choices. `nose: None` explicitly selects the authored type nose, `penetrator: None` selects
the whole body, and an inert round explicitly has no burst or fuze. No fired parameter changes.

Explicit-block verification: format and workspace/all-target clippy passed. MEASURED: 259 round,
gun, bake, pen-cam, field-cast, headless and citation tests passed; six ignored. Every omitted
optional-block control refuses its named field and gives the explicit-authoring fix; the field-cast
pin remains unchanged.

## P3 record diagnostics, shader oracle and instrument users

Subject: the game's damage model, game state only. Removed unread `RecordedShot::tank` and
`shooter`, and the recording's duplicate source retained only to write the latter. The recorder
integration tests now identify recipients by the production `TankLife` reader. Removed the emitted
`SpallCone`'s pin-only spend, energy and endpoint speeds; its strength stays because the recorder
reconstructs fields from it. The pin now hashes only retained field data. DERIVED expected schema
change: six shot-metadata hashes among the pin's 2009 records, deleting four diagnostics from
each emitted cone; all other record hashes and every shot's ray ends, contacts and HP data remain
identical to the saved MEASURED baseline after stripping exactly those diagnostics.

The potency classifier and its invisible band are CPU shader oracles compiled only into tests.
The shipped visible bands return their colours directly. Removed the stale Cargo dependency
comment. The fuzzer CLI has a tracked user, `scripts/ballistics/fuzz.sh`, for asset authors' bake
sweeps and ray replay, using the required hidden/unattended composition and machine queue.

**R5, new P2, fixed:** the CI fuzzer's anti-vacuity census named a fixed list of fixture stations
and ammunition volumes. It now derives that expected set from the selected spec's declared
component facets. This fixes the named-asset gate without changing B13's owner-held protection
floor or its admission rule.

P3 diagnostic verification: format and workspace/all-target clippy passed. MEASURED: 239 recorder,
fuzzer, pen-cam, codec, delivery, field-cast and citation tests passed; five ignored. All 2009 pin
records passed; the six emitted-shot payloads matched the saved baseline after removing exactly
the four diagnostics, and the other 2003 pin records were unchanged. The tracked fuzzer runner
completed 10000 rays with no walk errors, invariant violations or admitted protection-floor route.

## P3 presentation cardinality and named pending states

Subject: the game's damage-model view, game state only. Aim HUD furniture and its active player
camera/window are required, while controlled-tank absence uses the shared possession classifier.
A pending viewer before mounting or during teardown is distinct from duplicate viewer declarations;
mask/detail/stage/kill-cam readers now share that distinction. Mounted corner and kill-cam furniture
and stage camera/round roots are required. The kill-cam UI explicitly idles while unmounted.
The removed demo, demo-world and demo kill-cam singleton groups were already fixed at this base.

The review's x-ray-input P3 now has a typed syntax/number/domain refusal at its input edge; malformed
windows still refuse process startup. Local non-synthetic records also refuse an unregistered round
before body planning, preserving synthetic clips' explicit unknown-round allowance.

Presentation verification: format and workspace/all-target clippy passed. MEASURED: 288 targeted
presentation, recorder, protocol, pen-cam, field-cast, headless and citation tests passed; seven
ignored. The teardown controls establish the named unmounted state: both stage roots are absent
and the corner is clear; partial roots or an open record without them fail. MEASURED: all 43
whole-tree citation, font, net-boundary and bake guards passed. The field-cast pin is unchanged.

## Cone-source producer closure and final disposition

Subject: the game's damage model, game state only. The emitted-cone diagnostic deletion also
removes its now-unread source entry speed, coverage and copied substance list, their producers,
and the redundant post-grouping body labels. The grouping keys still distinguish material sets,
exit kind and ordinal; the resulting members carry the actual geometry. The remaining source
exit speed feeds fuze continuation, energy and strength feed fields, and exited volumes protect
against dosing the same body twice. Tests compare retained energy, residual speed and geometry
rather than requiring diagnostic storage. A reached exit must belong to an advanced stretch;
that construction invariant cannot silently discard a cone.

Disposition: B02/B03, B56–B60, B62/B63, B68, B71, B72/B73, remaining resolver/walk directions,
record and cone diagnostics, the potency oracle, fuzzer CLI user, dependency comment and explicit
round construction blocks are fixed. The removed demo singleton groups were already fixed at
the base. The surviving presentation groups now name unmounted states and reject duplicates.
The shader oracle stays test-only; the fuzzer stays behind its tracked author runner. Synthetic
unknown-round clips deliberately omit a body. Body-less records retain a material handle which
no active piece reads; no fallback appearance is drawn. No alternate runtime path or experiment
switch remains from this lane.

Owner decisions held unchanged: B1 aerodynamic facts, B2 sampling, B3 recoil, B4 release/construction,
B6 ground law, B13 the protection-floor contract, the whole-flight source exclusion pending authored
bore/clearance, and `SPIKE_TRACE_SIM_FIELDS` with ADR-0029/0030. Field-horizon pricing is still for
the orchestrator; this lane ran no capture or pricing. The predecessor review's R3 and R4 and the
new generic-fuzzer R5 are fixed; no other P1/P2 was found in that predecessor diff.

Producer-closure verification: format and workspace/all-target clippy passed. MEASURED: 386 walk,
resolver, march, fuzzer, recorder, pen-cam, field-cast, headless and citation tests passed; seven
ignored instruments. MEASURED: all 43 whole-tree citation, font, net-boundary and bake guards
passed again. The field-cast pin is unchanged by producer cleanup. All lane changes are committed
on the lane branch; no push, capture, pricing or deployment was performed.

## Review of sweep-ball-4

Subject: the game's damage model and its shot records, game state only. Two blind reviews read the
twelve commits from "preserve the client's legitimate firing-pose resync gap" to "remove unread
cone-source producer state". Each finding was decided against the code; each P1 has a test that
builds the state and failed on main.

- **R6, P1 (Codex), verified, fixed** in "admit a round at rest inside overlapping plates". A round
  that stops inside two overlapping armour volumes of one tank leaves both without an exit (the walk
  exposes tied strongest contacts in each); admission accepted a retaining plate only last, and the
  authority's delivery `expect` panicked. A plate without an exit now belongs to a record whose
  round rests; a perforated plate inside a retaining one may follow it in path order.
- **R7, P1 (Codex), verified, fixed** in "admit a departure between path nodes of a shared
  crossing". A crossing spanning overlapping tanks ends the first tank's record `Left` where its own
  volumes end, between the crossing's entrance and rest nodes; admission required a node. A
  departure now lies on the path polyline within the same DERIVED wire bound. A burst keeps its
  node requirement: a burst starts a leg of the path. The plan already projects a departure.
- **R8, P1 (Claude), verified, fixed** in "name a husk's launched turret instead of panicking its
  record". A cook-off launch removes the turret's `ServoIndex` while `TankServos` keeps its slot,
  so a later round into the husk tripped `servo_angles`' slot/node assert. `servo_angles` now
  returns `ServoPose::Detached` while a `LaunchedTurret` names the tank; that encounter finishes
  without a record. An encounter opened while the rig was mounted keeps its pre-launch pose. The
  launched turret despawns with its root (`tank::integrity::sweep_launched_turret_on_root_despawn`),
  so the detached state lasts the husk's life (DERIVED from that observer).
- **R9, P2 (Codex), verified, fixed** in "refuse a local record that omits a declared servo". Local
  admission let a simulated record omit a declared servo, which presentation drew at rest. No
  shipped loader reads local records today (only the corpus tests), so it had no effect yet.
  `PenetrationReport::load`, the one local gate that resolves the vehicle, now refuses a simulated
  record whose servos differ from its tank file's, naming the servo; synthetic clips keep rest.
- **Claude P3 dispositions:** the replica firing-pose gap was already fixed by "retain presented
  samples through interpolation resyncs" before this review range; it is not open. The unused
  potency colour arm is fixed in "remove the unused pen cam colour arm", and the stale fuze
  field documentation is fixed in "state the current fuze and firing-pose contracts". The `aim`
  cardinality P3 was a verification gap, already fixed forward on main by "give the UI ownership
  app the player view and tank specs a live client has".

The same class, swept: every `assert`/`expect` the reviewed range added on the record and delivery
path holds by construction against husks, launched turrets, overlapping tanks, a despawned shooter
and a resync. The mounted-rig slot assert holds because spawn gives every servo entry a node with
its slot and only the launch removes one. The posed-before-finish `expect` holds because
`pose_struck` runs after the march and before the same tick's finish, and a despawned tank returns
first. The offline vocabulary `expect` holds because `build_vocabulary` runs in `PreUpdate` once
the blueprint exists, before any fixed tick of a tank spawned from it. The build `expect`s in
offline delivery and `net::shot_explanation` hold because every remaining encoder refusal is met by
the producer: keyed shots and combatant lives on every authority tank, i32 mm/µs ranges, one
consequence per emptied pool, cones thrown in the shipped shape, burst fields thrown from the
burst point, and the semantic checks above. A despawned shooter only drops its projection's
recipient; a resync is a client-side typed refusal (`Receipt::Refused`).

Owner decision: a husk struck after its turret launched now opens no pen cam. Showing it would need
the record to state a detached node (its pose, or its absence from the hull) on the wire and on the
stage; no angle states it.

Review-fix verification: format and workspace/all-target clippy passed. MEASURED: before the
fixes, the three P1 tests failed on main with "only the last plate can retain the round", "the
departure point must be a path point" and the slot/node assert (left 1, right 2). After them,
MEASURED: 288 damage_cam, pen-cam, recording, shot-explanation, citation and whole-tree guard
(font, net-boundary, bake) tests passed; four ignored. Every corpus record loads through the new
servo edge. No capture or pricing.

## Release-gate discovery membership

Subject: the game's damage model and shot records, game state only. Fixed in "resolve material
holding a discovery start". The release capture reports discovery inside one certified shell
whose key names one entity as both volume and primitive. By the spawn code that is a collider
that is its own volume: a split multi-substance tank volume (`tank::spawn`'s `split` arm), a
construction element, a tree segment or a breakage chunk, never the child primitive of a
single-substance tank node. The log's last respawn came 5 s before the panic (MEASURED: the
server log's last respawn at 08:53:39.36Z; the panic follows lines stamped 08:53:44.50Z), so the
capture's state is not the newborn-body regression's. The likelier route is the second one
below, a body moving over the round (a falling tree segment or a chunk): DERIVED from the key
class and the timing, not established. The log still names neither the collider nor the round's
pose or playback.

The outside-start invariant is false by construction: a new body's birth geometry can contain
an already flying round; another body can also move over a path while its previously frozen
crossing plays. Discovery now reads initial membership from the same certified crossing stream
as later entries, contacts held material immediately, and seeds the ordinary material walk in
that contact scene. No body is excluded and no round is dropped. Material holding a round at
relative rest gives the crossing no motion and no clock: the round keeps its world velocity to
the tick boundary (or an armed delay's end) and its next step reads that membership again. An
exhausted discovery interval still gives a finite immediate contact.

MEASURED: `a_body_born_around_an_in_flight_round_resolves_its_material` failed before the fix
with the capture's "discovery starts outside every collider" panic. It now requires the new
body's actual material crossing. The shallow-start and exhausted-boundary checks also pass.
Format and workspace/all-target clippy passed. MEASURED: the targeted motion, recording,
pen-cam, field-cast, headless and citation run passed with 90 tests and five ignored instruments.
No capture or pricing was run; the full Battle cycle remains for the orchestrator's release gate.

## Review of sweep-ball-5

Subject: the game's damage model and shot records, game state only. Finding identifiers below
name the findings in the blind Codex and Claude reviews, including their disagreement.

- **R10, P1 (Codex), verified, fixed** in "record each tank's resolved departure on its path".
  Two overlapping tanks moving transversely together share a crossing that slows the round.
  `Playback::point` translates each point by the carrier displacement at that point's resolved
  time. The mark's hull frame is one frame translated at its mark time, so reading those points
  in that frame does not straighten the resulting path. The target's last exit can lie off the
  entrance-to-end chord. The producer now includes that target's actual departure as a path
  node; admission keeps its geometric contract. No wire fields or protocol vocabulary changed.
  **Claude's contrary soundness claim is disagreed with:** its chord argument covers the static
  case, but the moving-state test fails main's admission with "the departure point must lie on
  the path". The test uses the actual march, playback and recorder, then parses each record.
- **R11, P3 (Claude), verified, fixed in the same subject.** `RoundEnd::Left` now describes a
  target's own departure inside a shared crossing. The corpus helper accepts a departure on a
  path segment and any retaining plate, including overlapping retaining plates. It no longer
  imposes plate order or an unsupported fixed distance from the final plate's entry. Both the
  overlapping-retention and shared-departure regression tests now exercise that helper.

MEASURED: the moving-state regression failed on main before the fix and passes with the producer
change. Format and workspace/all-target clippy passed. MEASURED: 269 damage-cam, recording,
pen-cam, field-cast, headless and citation tests passed; six ignored instruments. No capture or
pricing was run.


### Living-target encounter admission

Subject: the game's damage model and shot records, game state only. The owner decision is fixed
in "open shot encounters only for living targets": `MarchRecorder::note` refuses a new encounter
when the struck tank already has `TankKnockedOut`. This runs before the current tick's knockout
consequences, so the knockout shot records. An existing encounter keeps its captured strike
angles and can finish after knockout. Direct `StruckTank::capture` follows the same living-target
rule. The round's simulation and deposits are unchanged.

- **R13, P3 (Claude), verified, removed by the same subject.** The old launch-outcome test did
  not exercise the real launch's retention of `TankRoot`. That was a gap while husk records
  depended on it. New husk encounters no longer query servo angles, so no record depends on
  that launch detail. `ServoPose`, its detached arm, the launch detector and their test arms
  are deleted; there is no remaining consumer. `LaunchedTurret` remains in the physical launch
  and its presentation/replication, independent of recording.

MEASURED: before the gate, the living-target test failed because a mounted husk produced a
record. After it, strikes on husks with and without a turret produce no record, the knockout
shot records its strike angles and knockout cause, and the remaining ammunition pool takes dose
in every case. A later-frame test also caught an unnecessary servo reread after knockout; it now
requires the saved angles and a filled later hull frame, and refuses a fresh capture of that
husk. Format and workspace/all-target clippy passed. MEASURED: 270 targeted damage-cam,
recording, pen-cam, field-cast, headless and citation tests passed, six ignored; the final five
focused capture/encounter/citation checks also passed. No capture or pricing was run.

### Local-file admission guard

- **R12, P3 (Claude), verified, fixed** in "test servo admission through the local file loader".
  Subject: the game's damage-model record admission. The old test called `check_vehicle_servos`
  directly, so removing the file loader's call left the test green. The replacement writes a
  record file and calls `PenetrationReport::load`: a complete simulated record loads, missing
  and undeclared servo names are refused, and a synthetic clip retains its named rest allowance.
  Each temporary file is removed before the result is asserted. Removing the loader's admission
  call now makes the rejection assertions fail.

MEASURED: the shared final run passed 331 targeted damage-cam, recording, pen-cam, field-cast,
headless, round-data, citation, font, net-boundary and bake tests; six ignored instruments.
Format and workspace/all-target clippy passed. No capture or pricing was run.

### Potency colour cleanup

Subject: the game's damage model and pen cam potency bands, game state only.

- **R15, P3 (Claude's ball-4 review), verified, fixed** in "remove the unused pen cam colour
  arm". The `Band::Nothing` colour arm and its colour assertion existed only in tests and
  guarded no shipped colour. The variant, arm and assertion are removed. The shader oracle
  represents below-floor intensity as no drawn band (`None`); its callers still check the
  reduced and knockout boundaries and their actual colours. The floor contract, colour values
  and shader equations are unchanged.

MEASURED: the shared 331-test final run included pen-cam, field-cast and headless checks with
no failures. After the final test-comment correction, format and workspace/all-target clippy
passed again, and all 43 dedicated whole-tree citation, font, net-boundary and bake guards
passed. No capture or pricing was run.


### Current fuze and firing-pose prose

Subject: the game's damage model and shot records, game state only.

- **R14, P3 (Claude), verified stale report, corrected** in "state the current fuze and
  firing-pose contracts". `shooting::fire` already reads required `Position` and `Rotation`
  directly from the weapon root's query, with no replica pose-gap skip. "retain presented samples
  through interpolation resyncs" fixed the code before the reviewed range. The earlier review's
  open-P3 line is corrected above; no firing-code change is required.
- **R16, P3 (Claude's ball-4 review), verified, fixed in the same subject.** `rounds` now documents
  only the fuze fields its authoring parser declares: trigger, modes and default mode. The
  obsolete `position`/nose/base sentence is removed.

MEASURED: the final shared run passed 331 targeted damage-cam, recording, pen-cam, field-cast,
headless, round-data and whole-tree guard tests, with six ignored instruments. Format and
workspace/all-target clippy passed again after the final source-comment edit; all 43 dedicated
citation, font, net-boundary and bake guards passed. All verified review findings are closed.
The full Battle cycle remains the orchestrator's release gate; the capture cannot establish the
identity of its inside-start collider. No capture, pricing, deployment or push was run, and no
new owner decision is needed.

## Review of sweep-ball-6

Subject: the game's damage model and shot records, game state only. Each finding of the blind
Codex and Claude reviews is verified against the code, by a test that fails before its fix
where one can be written.

- **P2 (Codex), verified, fixed** in "rest a round born inside material that shares its
  velocity". Material holding a round at relative rest (zero-resistance material newly around a
  round that shares its carrier velocity) gave `Begin::Miss` a positive clear span over zero
  speed, and `add_stretch` panicked on the zero clock. The round now keeps its world velocity to
  the tick boundary or an armed delay's end, and its next step reads the membership again.
  `a_round_at_relative_rest_inside_new_material_builds_no_zero_clock` failed before the fix
  with that panic.
- **P2 (Codex), verified, fixed** in "open a tank's encounter when the round reaches it". A
  shared crossing's entrance mark opened encounters for every tank in it, so a tank knocked out
  between the entrance and the round's arrival was still recorded. Playback now notes each tank
  when the round reaches its volume's entry (`PenetrationMarks::reached`), and the living-target
  gate reads it then. `a_shared_crossing_opens_each_tank_when_the_round_reaches_it` recorded the
  husk before the fix.
- **P2 (Claude), verified, fixed** in "keep a launched-turret target a husk on clearing the
  range". `sandbox::clear_shots` relabelled a tank whose turret had launched as living, so the
  next round posed n-1 servo nodes against n slots. Clearing now keeps such a tank a husk (R
  rebuilds it). `clearing_the_range_keeps_a_launched_turret_tank_a_husk` failed before the fix.
- **P3 (Claude) `on_line` comment, verified, fixed** in "restate the record contracts the
  sweep-ball-6 review found stale", with the next two: the authority's departures are path nodes;
  the between-node tolerance serves records read from the wire or a file.
- **P3 (Claude) `assert_self_consistent`, verified, fixed**: the last perforated plate's exit
  arm is back, within the DERIVED sqrt(3) mm of independently quantized points.
- **P3 (Claude) moving contained branch, verified coverage gap, fixed** with the relative-rest
  fix: a body moving over an in-flight round now has a test
  (`a_moving_body_born_around_an_in_flight_round_crosses_it_in_its_frame`) that asserts the
  crossing's heading and speed in the body's frame. It passes on the reviewed code; the review's
  zero-velocity mutant was not run.
- **P3 (Claude) the record's membership paragraph, verified, corrected** above, in "Release-gate
  discovery membership".

The crew sweep's F2 rule comparison is gone with F2's losing arm: the sweep measures the one
knockout rule (`damage::knockout_from_counts`, #227).

MEASURED: the three new P2 tests failed on main and pass with their fixes; 333 targeted
recording, damage-cam, pen-cam, field-cast, headless, sandbox, score, stats and citation tests
passed with nine ignored instruments (two recording tests updated for the fixture seats
re-passed), and 55 whole-tree guard tests passed. Format and workspace/all-target clippy passed.
No capture or pricing was run.


## Review of sweep-ball-7 and source-clearance follow-through

| Reviewer | Finding | Disposition | Fixed in (commit subject) |
|---|---|---|---|
| Codex | P1: muzzle birth skips a wall or foreign hull | verified | shooting: start rounds at the source collider exit (#247) |
| Codex | P1: whole-flight source transparency | verified | ballistics: make the source tank solid after box clearance (#247) |
| Codex | P1: relative rest skips other collisions | verified | ballistics: discover other bodies during relative rest (#247) |
| Codex | P2: husk counter depends on tick partitioning | verified | damage: count husk contacts once per round and tank (#227) |
| Codex | P3: knockout module contract states the removed rule | verified | spec: refuse tanks with fewer than two crew stations (#227) |
| Claude | P2: sheets below the two-crew floor load | verified | spec: refuse tanks with fewer than two crew stations (#227) |
| Claude | P3: knockout module contract states the removed rule | verified | spec: refuse tanks with fewer than two crew stations (#227) |
| Claude | P3: design sections 7 and 8 state the removed rule | verified | ballistics: make the source tank solid after box clearance (#247) |
| Claude | P3: the knockout test cannot detect cause replacement | verified | spec: refuse tanks with fewer than two crew stations (#227) |
| Claude | P3: knockout-shot continuation marks count as husk strikes | verified | damage: count husk contacts once per round and tank (#227) |

Subject: the game's damage model, game state only. Every review finding is verified; the first
two Codex findings describe work explicitly left unstarted by sweep-ball-7, rather than an
unreported regression. The source-clearance section above owns the birth/return design narrative.

- Relative rest: `relative_rest_still_discovers_a_separate_plate_in_the_same_tick` was red with
  no separate-plate crossing. Exact fixture positions make the carrier velocity representable;
  at relative rest the old branch spent the rest of the tick without discovery. The fix excludes
  that collider only until another crossing changes velocity; the zero-speed regression also passes.
- Husk statistics: the direct `MarchRecorder::note` regression was red at three strikes for one
  round/tank across three steps. A projectile now remembers each tank's first contact, including
  automatic fire. A living first contact cannot be reclassified by later marks after knockout.
  The counter's deliberate unit is first contacts with husks, at most one per round and tank.
- Crew floor: the one-station fixture passed `validate` before the gate. Both zero- and one-station
  files now refuse at the asset door with the `.tank.ron` filename, `volumes.*.crew` and the repair.
  The synthetic bake sheet and geometry obey the same rule; the scale test derives its expected
  refusals from the declared volumes. No shipped geometry or asset data changed.
- Knockout prose: the implemented input `(one in action, no cook-off)` gives the crew-loss label;
  the module and design claims of an all-crew threshold were false. They now state the under-two
  rule. Cook-off empties every crew pool and therefore satisfies that rule as a cause.
- Cause-latching coverage: the original five rows could not distinguish an independent cook-off
  label from the crew rule. The test now starts with a crew-loss label, then processes a later
  cook-off through the real consequence chain and checks the scoring label stays crew loss.

MEASURED gates: relative-rest/citation checks passed (five); recording/contact checks passed
(35); spec, bake, consequence and citation checks passed (72); birth, replay, fire transport,
protocol pins, pen cam, field casts, headless and citation checks passed (359, six ignored).
The final source-clearance and whole-tree run passed 363 tests (seven ignored), including
`doc_citations`, `ui_ascii`, `net_boundary`, `bake::`, pen cam, field casts and headless checks.
Formatting and workspace/all-target CI clippy with `dev_ui,bitprobe` passed for each code commit.
The fire-field plumbing required mechanical VFX fixture initialization and accurate tracer-anchor
comments; VFX behavior was unchanged. No captures, pricing, pushes or issue comments were made.
B1, B2, B3, B4, B6, B13 and `SPIKE_TRACE_SIM_FIELDS` remain owner calls.


## Review of sweep-ball-8 and the transit membership (#254)

Subject: the game's damage model, game state only.

**A reproduced transit-membership failure while investigating #254.** Before bent-ray
certification, `walk::begin` supplied each transit sample with membership read on its incoming
ray at its hand-off, while the transit corridor swept the bent ray from the same start. A sample that hands off on a face sits off it by its
rounding, and a ray that grazes that face turns the offset into a distance along the ray: the
rounding over the graze's sine. The two rays can then place the face on opposite sides of the
start, beyond both the band the seed declines and the corridor's clamp. Neither owns the face, and
the shell's exit arrives with nothing open.

MEASURED reproduction: a seeded headless Battle (29 bots, one parked client) panicked twice at the
same crossing after about 140 s (sample 11, exit triangle 106, t = 0.1819 m). A round had perforated
a cupola. One ring sample entered the hatch's face 59 0.355 m along its incoming ray, 0.30 m deeper
than the faces the rest of the disc entered, and handed off there. Its bent transit ray then left
the hatch with no entry. A collider-history checker found no stale pose or history in about
7 680 ticks. The struck primitive in this reproduction was a tank's hatch volume. That does
not identify the original incident's body or rule out the server-perf-2 lead: its original log
records sample 2, triangle 11 and t = 0.6176264910893549 m (MEASURED), a different crossing.
No with/without bisect of the original incident was run. No `src/track/` or `src/net/` change
was needed for the reproduced mechanism.

The reproduced mechanism was fixed in "ballistics: read a transit's membership on its own rays (#254)".
`resolve::transit_starts` certifies every transit start on the bent ray, with the probe that
resumed flights already use. `certified_samples` hands back the offsets it was given, so the probe
and the corridor form each start in one arithmetic. The regression test
`a_hand_off_face_the_bent_axis_grazes_is_owned_once` sweeps a block's overhanging side face from
1 to 16 degrees of graze, near the world origin and 1.6 km from it. With the incoming-ray seeds its
first pose fails with `UnexpectedExit` (MEASURED). The seeded Battle, rebuilt with the fix, ran
332 s without a panic (MEASURED). The transit request now carries only hand-off offsets;
incoming membership was removed in "ballistics: keep only bent-ray transit membership (#254)".
The production-driver mutant and analytic-fixture follow-through are recorded below.

| Reviewer | Finding | Disposition | Fixed in (commit subject) |
|---|---|---|---|
| Codex | P1: queued decisions keep source transparency after box clearance | verified | ballistics: end queued discovery at source-box clearance (#247) |
| Codex | P2: the reverse ray skips earlier open bore segments | verified | shooting: start a mounted round at its bore's first open point (#247) |
| Codex | P2: a bore that misses every proxy panics at fire | verified | shooting: start a mounted round at its bore's first open point (#247) |
| Codex | P3: this record states PROTOCOL_REV 61 | verified | docs: record the sweep-ball-8 review and the transit membership (#247, #254) |
| Claude | P2: the birth is not the bore's first open point; an outside mount panics | verified | shooting: start a mounted round at its bore's first open point (#247) |
| Claude | P2: the source box is rebuilt for every cleared round | verified | ballistics: build no source box for a cleared round (#247) |
| Claude | P3: the tracer assertion never reads the drawn streak | verified | vfx: test that a round born behind its muzzle streaks from its birth (#247) |
| Claude | P3: the disclosure doc states the deleted whole-flight rule | verified | damage_cam: state the disclosure rule after box clearance (#247) |
| Claude | P3: `FireShell::of_round` defaults `birth` to the muzzle | verified | ballistics: require the physical birth when constructing a shot (#247) |
| Claude | P3: blank lines after `birth:` in `src/net` literals | verified | ballistics: require the physical birth when constructing a shot (#247) |

- Birth: the old reverse ray from beyond the envelope returned the foremost exit of the whole
  line, so a proxy past a gap moved the birth past a wall in the gap; and a bore line that met no
  proxy hit its `expect`. Both are DERIVED from that code. `a_gap_in_the_envelope_ends_the_walk_at_its_first_open_point`
  and `a_mount_outside_the_envelope_is_born_at_the_mount` state each case. The mount reaches
  `outside_source` as a distance along the bore. The current start is the negative of the
  required, load-validated bore length, independent of the tank root and recoil travel.
- Source box: a cleared round never reads its box again, so it now builds none. Behaviour is
  unchanged and the cost was not measured.
- Tracer: `a_streak_born_behind_its_muzzle_is_drawn_from_its_birth` reads the drawn streak in the
  real sim, view and streak world. It checks that the tail sits on the birth while the streak is
  shorter than nominal. The recording test keeps only its path assertions.
- Follow-through: the queued-source finding and the two net call-site P3s were left open by
  sweep-ball-9. Their verified fixes and gates are recorded below.


## Review of sweep-ball-9 and queued source clearance

Subject: the game's damage model, game state only. Both blind reviews were present and read
again before this table was written. Each finding was checked against the code; the inherited
constructor and net-literal P3s are included.

| Reviewer | Finding | Verified / not a defect | Fixed in (commit subject) |
|---|---|---|---|
| Codex | P1: queued decisions retain source exclusion after clearance | verified | ballistics: end queued discovery at source-box clearance (#247) |
| Codex | P2: a fixed weapon infers its mount from an arbitrary body origin | verified | shooting: author the bore start independently of the body origin (#247) |
| Claude | P2: the transit regression bypasses the runtime driver | verified | ballistics: keep only bent-ray transit membership (#254) |
| Claude | P2: incoming transit membership remains as a losing alternative | verified | ballistics: keep only bent-ray transit membership (#254) |
| Claude | P2: an outside-envelope birth meets its own source volumes | not a defect | no source-type exemption; see the box-law disposition below |
| Claude | P2: independent GJK rays can lose the reverse intersection | verified | shooting: clip source envelopes on one local bore line (#247) |
| Claude | P3: probe, corridor and playback renormalize the transit axis differently | verified | ballistics: keep only bent-ray transit membership (#254) |
| Claude | P3: mount/muzzle ordering is checked late under recoil | verified | shooting: author the bore start independently of the body origin (#247) |
| Claude | P3: the canonical hand-off narrative states incoming-only membership | verified | ballistics: keep only bent-ray transit membership (#254) |
| Claude | P3: the reproduced hatch crossing is attributed to the original #254 incident | verified | docs: record verified sweep-ball-9 findings (#247, #254) |
| Claude (inherited) | P3: the constructor defaults physical birth to the muzzle | verified | ballistics: require the physical birth when constructing a shot (#247) |
| Claude (inherited) | P3: blank lines follow net birth literals | verified | ballistics: require the physical birth when constructing a shot (#247) |

MEASURED regressions and dispositions:

- `queued_decisions_meet_source_armour_after_leaving_its_box` failed before its fix. Its
  foreign body has a first plate inside the source box, an outside reflector, and a later plate
  beyond the source on the reflected path. The return now resolves source armour with positive
  cost and deposits dose in the source. The source-clearance section above owns the rule.
- `a_fixed_weapon_above_the_envelope_fires_with_its_root_ahead_of_its_muzzle` failed at the
  old mount-order assertion. A fixed bore now uses required construction data. Missing,
  non-finite and non-positive bore lengths are refused at load. The asset's model projections
  are DERIVED; its choice of pivot as bore start is explicitly ASSUMED in that data, pending a
  measured breech distance. No code infers a start from a servo or tank root.
- `grazing_a_source_proxy_never_loses_one_end_of_its_interval` reproduced the reverse-cast
  `expect` on finite convex-cube fixture geometry. It now passes using the walk's face clip.
- Restoring incoming seeding in the production `decide` made
  `a_hand_off_face_the_bent_axis_grazes_is_owned_once` fail with `UnexpectedExit`. The runtime
  mutant is removed. The strengthened test reaches `decide`; the request no longer has incoming
  membership to pass back, and analytic fixtures certify their bent rays.
- The outside-envelope claim conflates individual collider/proxy AABBs with `source_box`:
  that box encloses every collider registered to the source body, including its ballistic
  volumes. Being outside each collider's AABB does not establish being outside their enclosing
  box. A birth actually outside the enclosing
  box has already cleared it under the chosen law; material it later meets, including its own,
  is ordinary material. The inaccurate mount inference is removed by the authored bore start,
  without adding an exception to clearance.
- The original #254 log was checked against its issue and local server log. Its crossing differs
  from the reproduced hatch crossing, so the earlier attribution and dismissal of the
  server-perf-2 lead are withdrawn. The original incident's body and cause remain unestablished.

MEASURED gates: source-clearance checks passed 235 tests; the shared-clip checks passed 239;
authored-bore/load/net checks passed 623; transit/walk checks passed 353; explicit-birth,
replay/net checks passed 681. Ignored instruments were left ignored. Each code commit passed
formatting, workspace/all-target CI clippy with `dev_ui,bitprobe`, and citation checks. The final
whole-tree guards passed 44 tests (MEASURED): `doc_citations`, `ui_ascii`, `net_boundary` and
`bake::`; formatting and workspace/all-target CI clippy passed again.

Wire definitions are unchanged, so PROTOCOL_REV deliberately remains 63. No `src/track/`,
`ShotClock`, `WeaponClock` or `Respawn::Opens::tick` definition changed. The shared seams are
`Weapon` in `src/tank/model.rs` and mechanical weapon-fixture initialization in `src/net` and
VFX; no tick-type overlap was edited. No captures, pricing, pushes or issue comments were made.

## Review of sweep-ball-10 and whole-disc source clearance

Subject: the game's damage model, game state only. Both blind reviews were present when this lane
started and were read again before this table was written; neither was missing.

| Reviewer | Finding | Verified / not a defect | Fixed in (commit subject) |
|---|---|---|---|
| Claude | P1: main does not build: `bore_length` exists only on the file-side weapon spec | verified | spec: carry the bore length onto the parsed weapon (#247) |
| Codex | P1: the same build break | verified | spec: carry the bore length onto the parsed weapon (#247) |
| Claude | P1: a queued chain hands on lagging samples inside the source's own volume with the source made solid | verified | ballistics: end launch clearance when the whole disc leaves its source box (#247) |
| Codex | P1: an exit followed by an immediate return keeps the source transparent | verified | ballistics: end launch clearance when the whole disc leaves its source box (#247) |
| Claude | P3: the clearance break ends a chain while a sample still holds material | verified | ballistics: end launch clearance when the whole disc leaves its source box (#247) |
| Claude | P3: the transit request's unit axis is held only by a doc line | verified | ballistics: end launch clearance when the whole disc leaves its source box (#247) |
| Claude | P3: a recovered round invents its birth at the muzzle | verified | not fixed: `src/net/` is outside this lane; for the orchestrator to route |

MEASURED regressions and dispositions:

- Both P1s had one cause: clearance tested a single point, the axis start at the decision's end,
  against the source box. The source now stays transparent while any sample start is inside the
  box; a chain ends at the decision after which none is (with none held), or at a bounce whose way
  in took every sample out. The source-clearance section above owns the rule. The open-air exit
  keeps its swept starts instead of laying a fresh square disc, which could set an oblique disc's
  samples back inside the box.
- `a_staggered_disc_clears_its_whole_source_box_before_the_source_turns_solid` and its moving
  variant (an 88 perforating a foreign plate at 30° inside its own box, its barrel volume reaching
  the box face) failed before the fix: the round's line crossed its own barrel, static and moving
  alike (the static case charged it rather than failing its collector). They pass after it. The
  foreign plate's spall cone still doses the barrel, as any field may.
- `a_bounce_just_past_the_source_box_returns_into_a_solid_source` (Codex's DERIVED geometry: an
  axis-only 7.9 mm round reflecting at x = 1.0001 off a box ending at x = 1, outgoing
  x = −sin 15°) failed before the fix, the return passing through the source's armour; after it
  the return resolves that armour with positive cost.
- The transit request carries its bent axis as a `Dir3`, the checked bend `begin` computes; no
  `new_unchecked` remains on it.

Wire definitions are unchanged, so PROTOCOL_REV deliberately remains 63.


## Review of law-walk-angles and playback boundaries (sweep-ball-11)

Subject: the game's damage model, game state only. Both blind reviews were present at the start
and read again before this table was written; neither was missing. Each claim was checked against
the current code and records. The angle law's form is retained for the owner.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex | P1: an endless reflected chain overflows the playback clock | verified | ballistics: keep discovery on the playback clock (#247, #255) |
| Codex | P1: partial playback clears the source on an invented planar disc | verified | ballistics: keep discovery on the playback clock (#247, #255) |
| Claude | P2: source clearance is lost across a mid-crossing tick boundary | verified; same cause as the preceding P1 | ballistics: keep discovery on the playback clock (#247, #255) |
| Codex | P2: sharp internal reflections break the stage's drawn body | verified | damage_cam: present every reflected leg in the pen cam (#255) |
| Claude | P2: stale s5 and s9 records conceal that stage failure | verified | damage_cam: present every reflected leg in the pen cam (#255) |
| Claude | P2: mirror reflection differs from the documented restitution-zero slide | verified; owner decision, form retained | not changed; canonical angle-law section records the DERIVED consequences |
| Claude | P2: the whole crossing contributes thickness beyond the round's reach | verified; owner decision, form retained | not changed; canonical angle-law section preserves the choices |
| Claude | P3: this record states revision 62 while the tree is at 63 | verified | ballistics: correct the angle-law review bounds (#255) |
| Claude | P3: encoded sizes and the sub-1-KB claim use stale records | verified | damage_cam: present every reflected leg in the pen cam (#255) |
| Claude | P3: the thin-slab bounce test is cited as a universal dose bound | verified | ballistics: correct the angle-law review bounds (#255) |
| Claude | P3: normalization omits the requested potency and thickness/calibre dependence | verified; scope gap documented, owner decision | law form retained; no unchosen dependence introduced |
| Lane follow-through | P2: spatial proximity assigns a folded path's second cone the wrong exit speed | verified by the refreshed s9 front-speed test | damage_cam: present every reflected leg in the pen cam (#255) |

MEASURED regressions and dispositions:

- `a_closed_box_of_reflectors_returns_to_the_flight_clock` reproduced the finite-positive-clock
  panic with a fuze-less, axis-only fixture round reflecting between steel faces at 45°. Discovery
  now ends at a reflection; playback spends its real flight time before the next contact, with no
  bounce-count cap. Existing potency loss is unchanged.
- `partial_playback_keeps_launch_clearance_until_its_samples_resume` failed because the source
  latch became solid while an oblique perforation was still playing. It now retains that admission
  across ticks, then resumes from the crossing's certified sample starts. The source-clearance
  section above owns the rule; both reviewers' clearance findings share this fix.
- Re-recording changed only s5 and s9. The original stage checks then failed: s5's tail stood
  5.1 mm behind its struck plane and its nose trailed travel. The refreshed stage checks every
  reflection, keeps the body leading, and clears its latest struck plane. Event arcs and cone
  exit-speed selection use record time; the follow-up verification below finds that the writer
  still assigned that time by global spatial proximity. A default scenario test compares the live
  march's interaction sequence with the
  stored records, including reflected faces, crossed volumes and field counts.
- At that review step, s5 showed one rack reflection and rest in the hull side; its rack line
  dose was 13.5 HP (MEASURED). Its turn is 92.0°, correcting the review's 88° claim (DERIVED from recorded vectors).
  s9 showed four reflections, rest in the turret bottom, a commander-station line dose of
  21.1 HP, and a second cone of 2000.6 J (MEASURED). Its first turn is 123.7° (DERIVED). The
  canonical angle-law section owns the full scenario account and the current law's consequences.
- The complete serializer table was measured again: s5 is 858 B for the victim and 786 B for the
  shooter; s9 is 1728 B and 1680 B, with two cones totalling 1223 B (MEASURED). Every other row
  is unchanged. The false universal message-size claim is removed.
- The bounce-dose bound is per struck plate: `cos² θ_c = t² / (t² + k²)` rises toward one as
  thickness grows (DERIVED). The follow-up verification below removes the cross-plate fixture ordering because the
  thickness term remains an open owner choice. No physical law or dose changed.

Owner decisions remain the reflected direction versus the restitution-zero slide; a floor on
critical incidence, a thickness read bounded by the round's reach, or the current form; and the
normalization input gap. For the 28.1° reference incidence the mirror turns 123.8° and retains
along-face velocity 0.222 v, while a slide turns 61.9° and retains 0.471 v (DERIVED). With
`N = 7/24`, potency 32.6 mm and grip 9.42, thicknesses 456 and 912 mm give critical incidences
11.1° and 5.6° (DERIVED). These are consequences to decide, not new rules in this lane.

MEASURED gates: the P1 step passed 285 selected tests; the stage and refreshed-record step
passed 286, then 226 final stage/record/wire/citation checks. The serializer-size instrument passed.
The final run passed 83 tests, including the strengthened plate-bound comparison, all resolve
checks and the whole-tree `doc_citations`, `ui_ascii`, `net_boundary` and `bake::` guards.
Formatting and locked workspace/all-target CI clippy with `dev_ui,bitprobe` passed for every code
commit. Ignored pricing and capture instruments were left ignored.

Wire definitions are unchanged: PROTOCOL_REV deliberately remains 63. The protected track,
construction, contact-transfer and net modules were not edited. No captures, pricing, pushes or issue
comments were made.

## Verification of the sweep-ball-11 review (sweep-ball-12)

Subject: the game's damage model, game state only. The lane's single Claude review was present
and checked against code; the angle-law form remains the owner's open choice.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | P2: a folded path assigns a cone an earlier leg's record time; the stage comparison repeats that choice | verified | damage_cam: time events on their source crossings (#255) |
| Claude | P2: the ricochet resume skips the unswept normal lift | verified | ballistics: resume ricochets at reached samples (#255) |
| Claude | P3: the thick-plate assertion commits the test to an open plate-term choice | verified | ballistics: keep plate-bound tests independent of the open law choice (#255) |
| Lane follow-through | P2: a latest-face-only pose releases the full model through the preceding nearby face at the next apex | verified | ballistics: resume ricochets at reached samples (#255) |

MEASURED: the DERIVED folded-path fixture failed with a cone at `0.034642003 s`, before its
source crossing at `0.036097 s`. The writer now restricts each hard event to its mark's own path
nodes, excluding approach and run-out. Witnesses without their own node use the adjacent source
crossing/flight span. The stage guard also reads a synthetic folded log through the actual writer
and compares against an independently DERIVED source time and its raw mark's exit speed; matching
the plan's own nearest-time calculation cannot pass it. Re-recording refreshes event times in
the existing scenarios without changing interaction sequences or wire definitions.

MEASURED clock-step gates: 269 selected tests passed, with four pricing/capture instruments left
ignored. Formatting and locked workspace/all-target CI clippy with `dev_ui,bitprobe` passed.
At the clock step, the refreshed s9 second cone was at `0.036387894 s`, within its source
crossing (MEASURED).

At the initial bay's green stop, the game's damage-model lift fixture had reproduced the defect:
after a reflection it skipped a 2 mm neighbour across a 7.2 mm gap (fixture data), with no chord charged. The unlanded hand-off
prototype passed that regression and 190 resolver/walk/collector/recording checks, but its final
cross-module run passed 516 tests but still failed the reflected-body view guard and the
refreshed field-cast check (MEASURED). The cast records themselves matched: their aggregate
`PINNED_TOTAL` constant had not been updated. The view guard still checked the previous plane
at the following reflection's apex; at that stop the endpoint still needed verification.
That prototype was not green and was excluded from the initial lane commits. The s5/s9
records at that stop retained the existing march's geometry, with only the clock-step corrections;
they still showed the lift-driven rests identified by the review. The corrected walk,
those two failures and its gates were deferred to the continuation below.

The plate-bound test now checks both sides of each struck plate's own critical angle and its
own energy bound, without ordering the doses of unrelated plates. At that step, the angle law's
form, the owner's floor/reach-bound choice and PROTOCOL_REV 63 were unchanged.

MEASURED final green-step gates: 83 resolver and whole-tree guard tests passed, including
`doc_citations`, `ui_ascii`, `net_boundary` and `bake::`. Formatting and locked workspace/all-target
CI clippy with `dev_ui,bitprobe` passed. Implementation stopped at the user's measured 60-minute
limit; the already-queued green-step checks completed afterwards. That initial branch contained
the clock fix and plate-bound test only; the unlanded walk, its provisional re-records and cast
pins were excluded. No protected modules, wire definitions, captures, pricing, pushes or issue
comments were changed or made.

The continued game's damage-model lift verification (sweep-ball-12b) is fixed in
`ballistics: resume ricochets at reached samples (#255)`. Reflected samples resume at their
reached hand-offs, with no normal lift or outgoing nudge; their offsets survive tick boundaries.
The thin-neighbour regression charges the 2 mm wall across its DERIVED 7.2 mm gap (fixture data).
The newer buffered collider re-enable path is retained unchanged.

MEASURED: the unchanged stage guard reproduced a tail depth of `0.15633692 m` at s5 bounce 1,
`0.009987354 m` after its apex, at the following reflection. Selecting the preceding plane failed
at the new apex instead: the full model spans both nearby faces. The pose now constrains the
body against the reached planes within its trailing span, preserving its nose position and
length. The guard's interval, endpoints and `0.0001 m` tolerance are unchanged. A synthetic
parallel-face fixture also checks the shared apex and its adjacent f32 values; an independent
corner checks simultaneous bounds from two distinct normals.

The final re-records and refreshed complete encoded-size table are in
`pen_cam: record ricochets without the unchecked lift (#255)`. MEASURED from the actual march
and postcard serializer:

| record | reflected faces, in order | rest | cones | victim B | shooter B | cone B |
|---|---|---|---|---|---|---|
| s5 | Ammo_L_1, Hull_Side_Upper_L, Ammo_L_1, Hull_Side_Upper_L | Ammo_L_1 | 1 | 926 | 854 | 539 |
| s9 | Breech, Turret_Side, Turret_Side, Turret_Bottom, Turret_Bottom, Turret_Bottom | Hull_Top | 3 | 1661 | 1617 | 962 |

s5 now reads the narrow rack/hull gap as flight and repeated contacts, rather than lifting into
the hull side. s9 continues through six reflections and three source-timed cones rather than
lifting into its turret-bottom rest; Ammo_L_1 receives only `0.0000228 HP` of raw field dose,
which converts to zero pool units (MEASURED). The other complete serializer rows are unchanged.
The field-cast blessing tool re-pinned 2009 records and printed `PINNED_TOTAL = 0x127879570b5d23ea`
(MEASURED on macOS arm64); the aggregate constant now matches it.

The angle-law form and the owner's open choices are unchanged. No wire definitions changed;
main's PROTOCOL_REV 65 (bumped by the interleaved #228 commits) is retained. No protected modules, captures, pricing, pushes or
issue comments were changed or made.

MEASURED continuation gates: 589 targeted tests passed across `ballistics::`, `pen_cam`,
`field_casts`, `damage_cam`, `recording_tests::` and `headless_test`; 11 capture/pricing tools
remained ignored. All 44 whole-tree guards passed (`doc_citations`, `ui_ascii`, `net_boundary`,
`bake::`). Formatting and locked workspace/all-target CI clippy with `dev_ui,bitprobe` passed.
The source comments describing the removed bounce nudge now state the reached-hand-off invariant.

## Verification of the sweep-ball-12 review (sweep-ball-14)

Subject: the game's damage model, the shell walk's ricochet resume and the damage cam's view of
it; game state only. The angle law's form stays the owner's open call.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex | P1: a reflection resumes every sample at its own incoming hand-off, so depth spread becomes lateral offset that compounds | verified: a stepped-face fixture puts a sample 0.0716 m off the outgoing axis against r = 0.044 m (MEASURED) | open |
| Codex | P2: s9's re-record empties the loader station's HP pool while `shows` says reduced | verified against the record | open (follows the P1 re-record) |
| Codex | P2: `face_safe_body` panics at a trihedral corner tighter than the model | verified (DERIVED infeasible) | damage_cam: draw a leading body at a corner tighter than the model (#255) |
| Codex | P2: launch clearance reads a fresh square disc while launch resumes carried starts | verified | ballistics: admit launch clearance on the starts launch resumes (#255) |
| Codex | P3: the contact-time outgoing membership probe is never read; a Required continuation can cross a tick unread; `launched()` clears membership silently | verified | tick boundary and assertion: ballistics: admit launch clearance on the starts launch resumes (#255); the unread probe is open with the P1 |
| Codex | P3: `collect_convex`'s accepted-exit exception is unnamed | verified | ballistics: name the convex collector's accepted-exit exception (#255) |
| Codex | P3: the record says PROTOCOL_REV 64 | verified: 65 at the time, 66 on main now | this record |

The P1 is not landed. A disc of radius r about the outgoing axis, its samples on the struck
face's plane through the axis hand-off and moved forward out of the struck volume, makes the
fixture and the thin-neighbour test pass and keeps every other record. On the re-recorded s9 it
reflects off Breech, Turret_Side, Turret_Side and Turret_Bottom, rests in Turret_Side and
throws two cones. The gunner station's HP pool is emptied and the breech knocked out; the
commander station is reduced (14.4 of 30 HP taken) and so is the loader station (13.4 HP taken,
field dose only) (MEASURED). s5 keeps its four reflections, its rest and its one cone. Two view
guards fail on that s9: `the_shell_never_sinks_into_the_face_it_glances_off` (the path's nose
6.2 mm inside the Turret_Bottom face 16 mm past the apex) and `each_front_runs_at_its_own_speed`.
After the Turret_Bottom reflection a rim sample's 1.17 mm chord into Turret_Side ends the
flight. The wire definitions and PROTOCOL_REV 66 are unchanged.

## One round through a skid, step 1 (#260)

Subject: the game's damage model, what the walk does with a round after a skid; game state only.

The exit is a slide: the striking direction with its normal part removed, restitution zero, keeping
`v sin θ` (a `StandIn` and an upper bound) and the coverage blend. The mirror is gone. Every leg
after a skid lays the round's own circle about its own axis, so no earlier leg's offset survives.
The round rests on the face: the axis turns on its incoming ray where it stands a body radius plus
`REST_CLEARANCE` off the read plane, nothing lifted. A plate or step in that disc's path is met and
decided by the contact law. At full coverage a face within the friction angle (`contact_friction`,
`StandIn`) holds the round, which ends a spent round's path by a law: DERIVED, `v sin θ ≤ μ v cos θ`
is angle only. The losing form, continuous Coulomb `v(sin θ − μ cos θ)`, would lower retained speed
on every skid against the kept `v sin θ`; it is the owner's call if the cliff at `atan μ` shows.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | P1: samples drift 0.2 m off a 0.044 m axis | verified in the old Ricochet arm, which inherited each offset | ballistics: lay a round's own disc on every leg after a skid (#260) |
| Codex | P2: s9's crew outcome against `shows` | on the current record the loader station is reduced as stated, not emptied: s9 no longer skids under the pressure law. `shows` omitted the commander station and now states each outcome | same |
| Claude | F1: a spent round has no rest path | traced, then fixed: `a_spent_round_trapped_in_a_closed_box_comes_to_rest` | same |
| Claude | F2: `end: Left` for a round that never left | verified: the unwaited stage test failed on the old s5 record (`the body is off the last leg`) | same |

MEASURED, s5 before: 11 skids alternating Ammo_L_1 and Hull_Side_Upper_L, `end: Left` inside the
tank, 926 B. After: one skid off Ammo_L_1 at 252.5 → 126.3 m/s, then rest within 3.6 mm of
steel-equivalent chord in the hull plating (Hull_Rear, Hull_Side_Upper_L), because the gap to the
inner hull face (7.2 mm) is narrower than the calibre, `end: Rest`, 877 B. The box's HP pool is
emptied in both (72 of 72 HP), and its line dose moves from 86.7 to 81.3. s9 before and after: no
skid, the same outcomes (breech 142 of 142 HP taken, gunner station 30 of 30, loader station 15.95
of 30, commander station 5.02 of 30, Ammo_R_1 1.67 of 72); only `shows` changed.

Left for a later lane: a disc partly held at its start reads its entrance at its free samples' first
face. In s5 that face is 0.75 m on, and the crossing is decided by its normal though the round stops
short of it; the record's `timeline` skips that node as a named adaptation, and the walk's read
remains.
