# Ballistics sweep review

Subject: the game's ballistics and damage model, game state only. Crew are HP pools at stations;
fields deposit dose, and a pool emptied leaves its component knocked out.

## Review of sweep-ball-1

Reviewed the commits after `ff571fe0` through `2cff0cf5` against the error-handling, code-branching
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

Reviewed `8f63e9c3^..4381e25c` against the three lenses and for correctness.

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
Reviewed `f1647a4f^..0f4a7493` against the three lenses and the part-3 brief.

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

## Whole-flight source transparency: measured, retained for the owner

Subject: the game's damage model, game state only. The premise needed to delete `not_own_volume`
is false: actual authored muzzle starts are not all outside their own solid volumes. The lane
removed the predicate's filtering entirely, ran the shot suites and CI-scale fuzzer, and compared
every mounted round at neutral servos under zero and full authored recoil. The temporary audit
used the real spawned rig/muzzle pose, certified birth membership, `FireShell` and the real march;
it was removed with the losing experimental arm. No geometry or physical law was changed.

MEASURED software results: baseline 451 tests passed; without filtering 450 passed and only
`a_shell_ignores_the_tank_that_fired_it` failed (it explicitly pins the retained rule). The fuzzer,
pen-cam corpus, field-cast pin, recorder, public-path/network and headless shot gates were unchanged.
Of the ten mounted-round/recoil cases, exactly these three changed:

| Mounted fixture configuration | Own volumes at birth, MEASURED | Unfiltered outcome, MEASURED |
|---|---|---|
| Coax / 7.9mm_AP, zero recoil | Coax_MG_Barrel | Stops after 0.0142822 m; barrel reduced by one shown HP point |
| Coax / 7.9mm_AP, full authored recoil | Coax_MG_Barrel | Stops after 0.0142822 m; barrel reduced by one shown HP point |
| HullMG / 7.9mm_AP, full authored recoil | Hull_MG_Ball and Hull_MG_Mantlet | Stops after 0.0158691 m; no shown pool loss |

With filtering each travels 93.2575836 m in the measured march window. The other seven cases
(neutral HullMG, and both recoil endpoints of each mounted main-gun round) have clear starts and
unchanged outcomes. These are fixture measurements of the current data, never a vehicle-specific
pass condition. The earlier comment claiming a coax/mantlet start did not describe these measured
neutral-servo configurations; it is replaced by the generic retained-rule invariant.

The missing facts are authored bore/launch clearance through solid barrel, ball and mantlet
volumes over the recoil envelope, rather than ownership as a physical property. Moving the start
past a guessed distance would hide those facts again. The fuzzer's rays carry no shooter and
already admit all volumes, so it cannot establish own-muzzle clearance. MEASURED: the existing
`a_round_that_strikes_a_tank_again_records_each_encounter` fixture still resolves a wall-reflected
path back into the same compound tank. DERIVED: own-source identity supplies no geometric reason
to exclude such a later encounter; the retained predicate would do so. Ground return is currently
stopped by the unresolved B6 ground law. Decision: retain the rule pending the owner's bore and
clearance contract, explicitly including its whole-flight self-intersection defect. The source
comments now state authority-only walking and the current attribution readers. No experiment
switch or second path remains.

Restored-rule verification: format and workspace/all-target clippy passed. MEASURED: 108 march,
recorder, pen-cam, fuzzer and citation tests passed; four ignored. The temporary audit and unfiltered
arm are absent from the tree. The history regression's capture-history comment is also replaced
by its current role and resync invariant (the review's P3 comment finding is fixed).

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
- **Claude P3s, verified, not fixed** (files outside this lane's changes): `shooting::fire`'s
  replica pose gap skips trigger, readiness and round bookkeeping that fused own fire could still do without
  the pose (the netcode lane owns the client resync path); `potency`'s test-only `Band::Nothing`
  colour arm and its assertion guard no shipped value; the `rounds` module doc still names a fuze
  `position`. The `aim` cardinality P3 was a verification gap, already fixed forward on main by
  "give the UI ownership app the player view and tank specs a live client has".

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
holding a discovery start". The release capture reports discovery inside one certified shell;
it records neither that collider's name/owner nor the round's pose or pending playback. The last
knockout and pictured detached turret do not identify the shell. Its exact identity and route
into membership therefore remain unestablished by the retained evidence.

The outside-start invariant is false by construction: a new body's birth geometry can contain
an already flying round; another body can also move over a path while its previously frozen
crossing plays. Discovery now reads initial membership from the same certified crossing stream
as later entries, contacts held material immediately, and seeds the ordinary material walk in
that contact scene. No body is excluded and no round is dropped. Relative rest retains its
heading; an exhausted discovery interval still gives a finite immediate contact.

MEASURED: `a_body_born_around_an_in_flight_round_resolves_its_material` failed before the fix
with the capture's "discovery starts outside every collider" panic. It now requires the new
body's actual material crossing. The shallow-start and exhausted-boundary checks also pass.
Format and workspace/all-target clippy passed. MEASURED: the targeted motion, recording,
pen-cam, field-cast, headless and citation run passed with 90 tests and five ignored instruments.
No capture or pricing was run; the full Battle cycle remains for the orchestrator's release gate.
