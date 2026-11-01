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
