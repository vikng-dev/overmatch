# View sweep review

Subject: the game's rendering, effects and camera. All impacts, rounds, crew and HP mentioned here
are the game's damage model and game state only. Starting tree: "materials: label the steel range in the track row's provenance (#238)";
lane `lane/sweep-view`.

## Track-t4 first

Reviewed and fixed before view work. See [track-t4-review](track-model/track-t4-review.md).
The friction-pair test now uses labelled synthetic data instead of constraining shipped materials.
The two consumers read one `BeltContact::mu`. Decision: retain the explicitly provisional 1.62
`TrackSteel` tune, which names missing grouser engagement in #146. DERIVED steel/brick pair:
about 1.026; the solver accepts it and bounds piece impulses, but this does not certify realistic
solid-contact friction. The inherited P3s and the remaining measurement need are listed there.

MEASURED on this lane: governed driving 10.684 m/s (spread 0.0023 m/s), pivot -0.1338 rad/s
(0.7100 of its kinematic ceiling). MEASURED synthetic material-pair ratio 2.000000, matching the
DERIVED ratio 2.000000. Driving laws and their material values were unchanged by this review.

## Base-truths inventory

| ID | Disposition | Current seam and remaining work |
| --- | --- | --- |
| V1 | Fixed | `camera::TurretPivot` caches the controlled entity with its offset and clears in the tankless gap. A different possessed rig recomputes from its complete skeleton. An ordered PostUpdate chain runs after every Update handoff and before orbit/captive readers. |
| V2 | Fixed | Required sourced round `effects.ember` states presence, radius, base offset, burn, fade and linear HDR colour. `FlightEffects` is present at shell spawn; `vfx::ember` consumes it independently of scene/calibre. |
| V3 | Fixed | Required sourced `effects.smoke` states presence, width, growth, lifetime, drift and birth taper. `vfx::trail` carries each emission through the existing path/bounce renderer. |
| V4 | Fixed (lane 2), replaced by the muzzle law | One view consumer, `vfx::muzzle`, reads the emission each shot carries (`Round::muzzle`, `FireShell::muzzle`); no calibre split or seed-calibre scale remains. The per-gun `muzzle` blocks are gone: the emission is the muzzle law's, from the shot's energy and the gun's muzzle shape (see "The muzzle law" below). |
| V5 | Fixed (lanes 2, 3) | Ground types state a required `dust.erodibility` and `dust.colour_srgb`; the muzzle law sizes the gas `reach_m` from the shot's energy. Puffs = the shot's rolled count × the painted grounds' summed share (weight × erodibility, `terrain_blend::dust_sources`); each puff is one source's soil, drawn by share, lit by the key light (see "Lane 3"). Erodibilities ASSUMED, ordered by hand; colours DERIVED from the packs' albedo. |
| V6 | Fixed for material selection | A volume's `SurfaceFeedback` travels through authority `Bounce`/`Bite`, public `PathImpact`, `Impact` and static-proxy response. Required sourced facts select spark presence and powder colour. Nonsparking materials produce powder without a metal flash/fan. Dimensions remain the separate new finding below. |
| V7 | Fixed | `ShellStrike` carries the exact struck primitive's response into `HullMark`; late joiners receive it with the record. Shared neutral relief maps receive per-response exposed tint, metallic and roughness factors. No fresh-steel constant selects appearance. |

Authored flight and surface emission values are explicitly ASSUMED/provisional, with `Unknown` or
`Tuned` provenance, not historical measurements. AP/APHE/APCR preserve the previous provisional
flight appearance in data; the HE and rifle-body sheets explicitly declare no base ember/smoke
(the separate rifle tracer streak remains). These are reviewable content decisions requiring
visual calibration, not new physical laws or a claim of ammunition-history research.

## Error-handling inventory

All four view P2s are fixed:

- Asset dependencies: one VFX texture catalog shared by setup and verification; authored gun report
  and engine paths verified before admission, including file/LFS/header refusal. Strong loader
  handles block admission until loaded. Audio and image failures stop every build; server-only
  compositions carry no presentation dependencies.
- Complete optic views: playable spec sheets require every `ViewKind`; runtime `TankViews` has
  named gunner/commander fields. Unarmed structural tooling fixtures do not instantiate a rig.
  Only named pre-spawn/target-sizing states retain a temporary view/aspect.
- `reticle::mask_rim` enforces its geometric invariant in every build.
- An unusable `SPIKE_RENDER_COST` output path refuses startup with the path and exit status.

The P3s, identified by the inventory's symbol/location:

| Finding | Disposition |
| --- | --- |
| Embedded branding decode | Fixed: embedded PNG test and invariant refusal; OS icon rejection is visible outside-input handling. |
| Nonfinite camera/sight pose | Fixed: finite/direction assertions; an aim exactly at the orbit pivot has a named no-bearing state. |
| Missing range table / servo spec | Fixed: complete-spawn invariants, no zero lob or invented unbounded travel. |
| Controlled singleton / hull pose | Fixed: no controlled tank is named; multiple control owners panic. Hull and gun poses require complete construction. The inspection follows the same cardinality rule. |
| Crew slot / invalid round index | Fixed: spawn-built gate/load entries required. Status strings use out-of-action game state. |
| Drive HUD radius clamp | Disagreed: the spec permits a reverse ladder longer than the forward steering table. The clamp deliberately uses the maximum forward steering radius for those reverse gears, matching transmission behavior. Table/forward-gear completeness is validated at load. |
| Drawn mark part / surface lookup | Fixed: matching indexed tables and Add-observer entity presence are invariants. |
| MarkBuilder mixed Option | Fixed semantically: no-mark verdict/calibre, standalone environment and thrown-clear ancestry are named absences; part/address/pose/packing violations panic. The bake refuses unrepresentable part counts and local coordinates before play. |
| Seven degenerate-vector sites | Fixed: invalid resolved normals/bearings panic; weighted unit-vector axes cannot cancel under their named biases; parallel/coincident spark orientation has a named perpendicular choice. Headless harnesses have no viewer; duplicate declared cameras panic. |
| Billboard lane table / CPU bytes | Fixed: startup-created data required. |
| Depth copy / upscale pending states | Fixed: unsized/unprepared targets and pending shader/import/uniform states named. Permanent upscale shader failures panic. |
| Missing audio source weapon | Fixed: a deferred presentation may outlive a despawned source; a missing slot on a live source or a duplicated slot is a bug. |
| Hull-mark inspection missing subject | Fixed: no zero-calibre seed, no skipped miss/wrong-target ray. The scenario fails visibly and names the repair. |
| LOD legend missing deviation | Fixed: switch and deviation originate in the same certificate chain. |

## Code-branching inventory

- View P2: fixed; SFX/VFX verifiers have one path across build profiles.
- `SPIKE_MESH_REMOVALS`: removed with its runtime system/state. The queue iterator remains only
  under `cfg(test)` because the independent upstream mesh-lifecycle tripwire uses it.
- `SPIKE_ABLATE=depth_copy` and `terrain-field`: removed, including parser fields, production
  branches, alternative test arms and script/docs. Main-pass depth copy and baked terrain shadow
  are the selected paths; the other pricing ablations retain tracked users.
- LOD showcase: retained as the ADR-0036 inspection instrument, selected by the one capture-plan
  input (`scene.demo: LodShowcase`). `scripts/capture/lod-showcase.ron` is its tracked recipe.
  Stage reach derives from declared map extent; unreachable switches are reported at their exact
  certificate distance. No environment switch or retired map-bound constant remains.
- Scatter prose: fixed to the current server stream and actual graybox-only cast. Real tree and
  structure ballistic volumes resolve through authority impacts and do not carry `ScatterProxy`.

## Additional findings

- **New P2, half fixed in lane 2: impact emission geometry.** `vfx::impact`'s compact/large sizes, speeds, light and
  lifetimes remain fixed recipes selected by calibre. Material selection is now honest, but the
  comments' broader physical-scale claim was false. Carry deposited energy and emitted-material
  facts into generic dressing; do not substitute viewer distance or a seed-round multiplier.
  The persistent mark also receives `Projectile::caliber` (the flying body), while penetration
  reads `Projectile::body` (the penetrator). A subcalibre construction therefore needs resolved
  contact geometry carried to the mark, rather than treating the exact wire value as proof that
  its footprint is the penetration diameter.
  Lane 2: the mark half is fixed. `ShellStrike::contact_caliber` (`ballistics::face_contact_caliber`)
  sizes a perforation's hole by the penetrator and a bounce or stop by the flying assembly; the
  record cutoff still reads the flying body. The dressing half stays open. It is not a failure site,
  so none of the four outcomes applies: it is a missing truth. The fix carries the energy the round
  lost at the face on `Impact` and adds an authored per-substance ejecta response, scaled by a
  stated law. No calibre boundary or seed multiplier.
- **New P3, fixed: late ownership scheduling.** An Update capture could precede a possession
  handoff later in that schedule. Pivot capture and optic-exit re-aim now run in one PostUpdate
  chain before the orbit and captive readers; an adversarial late-handoff test covers that frame.
- **New P3, fixed: replica servo fallback.** A malformed `RemoteServos` vector could fall through
  to authority state. `LiveServos::angle` now requires its selected role's complete vector.
- **New P3, fixed: permanent upscale failure.** Bevy's pipeline lookup returns absence for pending
  creation and permanent shader failures. The two terminal errors now stop the app; Bevy 0.19's
  retryable shader/import states remain pending.
- **New P3, fixed: mark calibre saturation.** A byte of millimetres silently capped larger rounds.
  A checked exact positive finite calibre replaces it, including deserialization refusal at the
  network edge. No upper presentation-size constant remains. Surface roughness uses `NonZeroU8`,
  so outside wire input cannot bypass the authored-file gate with a zero multiplier.

## Lane 2 review of sweep-view

Read against the three lenses and for correctness. No P1 or P2 found. Weighed:

- Flight emissions and surface response: every authored value is sourced, and ASSUMED values are
  labelled. Presence is a required `Sourced<Option<_>>`, and out-of-domain values refuse at load.
  The muzzle and ground facts in lane 2 follow the same pattern.
- PROTOCOL_REV 48 and the checked `MarkCaliber`/`NonZeroU8` domains are correct and pinned. Lane 2
  changes no wire type.
- The retired alternatives are gone. No code reads `OVERMATCH_LOD_SHOWCASE`, `SPIKE_MESH_REMOVALS`,
  `depth_copy` or `terrain-field`. Fixed: the vendored bevy_pbr patch record still named the
  counter as live.
- HUD clamp: agreed. The steering law clamps its reverse index to the last forward pair the same
  way. The HUD now says so, and panics on an empty table, which load refuses.

P3s:
- Fixed: the drive HUD read the first of several controlled tanks; it now uses `controlled_one`.
- Fixed: `HullMark::pack` turns an invalid calibre into `None` with `.ok()`, and the `expect` in
  `MarkBuilder::mark` named only coordinates and directions. It now names the load-validated
  calibre too. The local recorder's root lookup, which could fail silently, is an `expect`.
- Listed: `vfx::verify_vfx_paths` reads every texture whole just to refuse an empty file or an LFS
  pointer.
- Listed: `rounds::effects` `resolve` is left unformatted. rustfmt gives up on it, and
  `--check` still passes.

New findings:
- P2, base-truths: `ballistics::view` chooses the shell scene or the tracer streak by
  `TRACER_MAX_CALIBER`. The round's authored construction should decide.
- P3: the fill glow's alpha and boost and every sprite's playback rate stay shared renderer
  constants. They are facts of the sprite sheets, not emissions; a gun that needs its own moves
  them into its `muzzle` block.

## Validation and owner capture batch

Formatting and CI workspace clippy pass before every implementation commit. Focused groups passed:
asset/audio admission, complete camera/sight/crew construction, round/gun emission schemas and
renderers, material propagation, mesh lifecycle, render-plan parsing and LOD staging. The broad
material run passed **526 targeted tests**, including `ballistics::pen_cam`, `field_casts`, full
headless simulation, protocol/type-manifest guards and citations; opt-in measurement/survey tests stayed
ignored. The view-invariant group passed **122 targeted tests**, including kill-cam device flow and
bundled-font coverage. The final checked-wire group passed **63 targeted tests**, including
mark/diff round trips, positive surface decoding, protocol pinning, LOD staging and citations.
Protocol revision is **48**, with the type manifest re-pinned from the tripwire's computed value.
The final scheduling group passed **55 targeted tests**, including the late-handoff regression,
optic/camera invariants, kill-cam device flow, font coverage and citations.

No visible clients, captures, frame/tick pricing, deployment, pushes, Edgegap calls or Cloudflare
calls were run. GitHub issue access was read-only.
All test processes use the shared serial lock and unattended headless settings. Protocol compatibility
moves with the new material/mark wire facts; both ends must land this lane together.

The orchestrator's single capture batch should inspect:

1. Possession between rigs with different pivot/view offsets, then optic entry/exit at supported
   viewport aspects and the station out-of-action prompt.
2. Explicit traced/untraced flight appearances, different authored burn/width/lifetime values and
   trail continuity through a presented bounce.
3. Nonsparking wood/brick/rubber versus sparking surfaces: no metal fan on nonmetal regions,
   powder hues, and decals on mixed-substance moving parts. Check late join and mark eviction.
4. Mark size above the old byte ceiling using labelled synthetic data; no saturation.
5. Main-pass depth fading, baked terrain shadow reception, and the capture-plan LOD comparison at
   actual certificate ranges on the declared map extent.
6. Lane 2: main gun and MG muzzles against the previous look. Expect identical layers. The APCR
   fireball no longer shrinks with its flying body. Check the MG smoke ration and the tracer glimmer.
7. Lane 2: main-gun ground dust on gravel (now 4-6 puffs), dirt and sand (5-8), mud, and concrete
   (0-1). Also a muzzle just inside and just outside the 2.5 m reach.
8. Lane 2: an APCR hole on armour at the core's 35.7 mm, against a full-calibre dent or bounce.

Remaining implementation ownership: the impact-geometry P2's dressing half, which needs the energy
lost at the face and a per-substance ejecta response, and the `ballistics::view` calibre split. The provisional visual data needs the owner's batched eye
review and calibration. No speculative physical calibration was made in this lane.

## Lane 3: the capture session's muzzle, dust and spark findings

The 2026-11-01 capture session read items 6-7 and 3 by eye. Findings, game state only:

- **Ground dust read the same on every ground.** Three causes, all fixed. The grounds differed only
  in erodibility; colour, size and life were one shared palette and the gun's. Each puff eroded to
  threshold 1.0, but `impact_dust.png`'s signal sits mostly below 0.3: coverage halved at 17 % of
  the life and quartered at 28 % (MEASURED from the sheet's histogram), so the 2.0-2.5 s cloud was
  gone in ~0.5 s. And the palette was a fixed unlit colour. Now each ground states its soil's
  albedo (DERIVED: the mean of its pack's shipped albedo slice), the ground blast erodes to the
  sheet's ceiling 0.5 (quarter coverage at 55 % of the life), and the soil is lit by the sun's
  linear colour times irradiance on level ground over π times the camera's exposure (brightness
  multiplier 2.33 under the shipped sun and Bevy's default exposure, DERIVED). The linear colour
  is included by lane 4; the sky's share is left out. MEASURED per ground, shipped gun:
  concrete 1, dirt 5-8, gravel 4-6, mud 2, sand 5-8 puffs, 1.8 m growing to 3.8-5.0 m over
  2.0-2.5 s. Still open: a puff's mean coverage at birth is 0.09 (the sheet's 0.19 × the gun's 0.5
  opacity), so the cloud stays thin; the impact read's dust and the muzzle smoke still erode to 1.0. The muzzle smoke uses
  `smoke_atlas.png`, not the measured dust sheet. MEASURED separately in lane 4 at the smoke's
  sharpness: its mean coverage reaches one quarter of birth coverage at 0.196 of authored life
  and 5 % at 0.358. These are coverage thresholds, not despawn times.
- **No spark fan on steel.** Not a selection fault. This lane aimed the MGs square at the
  target's upper hull side from beside the player's gun; the strikes landed on the rear running
  gear (idler and the last road wheel's disc), several metres behind and below the aim point, and
  each drew the steel read's contact ping. The capture's ping sits in the same place. The compact steel read is a 0.5 m ping for
  0.05 s and two to four needle streaks 0.2-0.8 m long and 2-7 cm wide for 0.10-0.18 s: at 6-10 m
  they are a few pixels wide. Rubber road-wheel tyres state no sparks. The fan's size is the open
  impact-geometry P2 above; why the MG strikes fall so far from the aim point is unexamined.
- **The MG flash did not read at 10 m.** The capture's camera stood on the tank's left, 10 m off,
  and both MG muzzles were outside its frame (DERIVED from the plan's pose and the glb's muzzle
  nodes: 54° and 50° off the axis of a 36° half field). Framed from the right at 10 m, the coax
  flash and its light read. The data is consistent with the main gun's under one law: flash size
  scales with the cube root of the charge, and 2.5 kg against about 2.9 g (CLAIMED, 8.8 cm KwK 36
  Pzgr. 39 and 7.92 mm cartridges) gives a length ratio of 9.52 (DERIVED: cube root of 2.5 / 0.0029).
  The main gun's ASSUMED authored 3.5-4.6 m core and 4.3-6.4 m flames imply 0.37-0.48 m and
  0.45-0.67 m (DERIVED), against the MG's ASSUMED authored 0.3-0.55 m and 0.45-0.8 m.
  Unchanged. The MG's ASSUMED authored light peak is 1.2 Mlm; applying the same comparison to
  flash area gives 0.0883 Mlm and an excess factor of 13.59 (DERIVED from the main gun's ASSUMED
  8.0 Mlm divided by 9.5173 squared). **Lane disposition: leave the authored peak unchanged for the owner's call.** This
  comparison supplies no measurement of either gun's emitted light; it supports flagging the
  provisional tune, not a factual replacement. The owner must judge and calibrate the light.

## Lane 4: blind-review verification and HUD layering

Subject: the game's rendering and HUD, game state only. The lane starts at main's
"docs: point the sweep record at the blast-hit keying (#236)".

Both reviewers' sun-colour P2s are **verified**. The regression
`each_puff_is_its_grounds_soil_lit_by_the_key_light` failed before the fix: MEASURED linear
8-bit RGB `[81, 55, 19]` versus `[81, 47, 13]` for a synthetic soil under the world key.
`DustPalettes::palette` now multiplies linear soil albedo by `sun_light`'s linear colour;
the material still carries irradiance over pi and exposure. Bevy's directional-light
extraction multiplies the same linear colour by illuminance. This remains a level-ground,
key-only approximation: sky illumination, terrain shadowing and volumetric scattering are
not represented. No camera-distance scale or game-state change was added.

Claude's palette-provenance P3 is **verified**: the shadow and ageing ratios were unlabelled.
They are now explicitly ASSUMED presentation contrast choices, with no claim of measurement.
Fixed in "vfx: tint lifted soil by the sun's linear colour (#236)".

Codex's survey-lifetime P2 is **verified** as an evidence defect, not a runtime lifetime bug:
`spawn_ground_dust` already used the authored range, but the survey printed that input without
reading the spawned lifetime. A temporary mutant spawning a 0.1 s puff now fails the survey's
range assertion (MEASURED); the mutant is removed. The survey reads every puff's lifetime and
end diameter, asserts both against its emission, and reports the observed lifetime extrema.
MEASURED over the survey: spawned lives 2.003-2.499 s across the shipped grounds,
inside the authored 2.0-2.5 s range. All 22 muzzle/citation tests, formatting and CI clippy pass.
Fixed in "vfx: measure spawned dust lifetimes in the survey (#236)".

Claude's ground-colour test P3 is **verified** as a coverage gap. Production already refuses
missing soil colour and out-of-domain channels. The tests now omit each soil field and vary
red, green and blue independently. MEASURED: a temporary default-colour mutant and skipped
green/blue domain checks make both strengthened load tests fail. Both mutants are removed;
no load rule or authored ground value changes.
Fixed in "assets: guard every required soil colour channel (#236)".

Claude's capture-record arithmetic/provenance P3 is **verified**. The charge ratio recomputes
to 9.5173 (DERIVED); the affected size and light comparisons are now labelled DERIVED, the
loaded emission values ASSUMED. The old smoke statement was unsupported by its cited dust
histogram. MEASURED independently from `smoke_atlas.png` (red times alpha, mean of
`clamp((signal - threshold) * 2.6, 0, 1)`): birth coverage 0.13675, quarter coverage at threshold
0.196, and 5 % at 0.358. With erosion ending at 1.0 those thresholds equal fractions of life.
Claude's MG-light disposition P3 is **verified** as an omitted decision, not proof of the right
replacement tune. The owner-directed choice and its limits are now explicit above; no emission
asset changed. Both fixed in "docs: qualify the flash and smoke capture evidence (#236)".

The Tab-board capture finding is **verified**. In the red Bevy layout regression at
1280x720 / UI scale 2 (FIXTURE DATA), MEASURED board bounds were (256, 101)-(1025, 662) px,
status bounds (20, 20)-(492, 232) px. The board already had global layer 210 above labels at
0 (DERIVED from the spawn code); its ASSUMED 0.62-alpha backing let the lower labels show
through. Labels and fixed HUD shared the default layer, so their mutual order was incidental.

`hud::HudLayer` now stamps world labels below fixed HUD below modal boards; the board keeps
its existing place in the full-screen overlay ladder. `HudDock` roots reserve their actual
laid-out bounds on their UI camera: status/clock at the top, crew/drive at the bottom,
diagnostics at the right. Hidden panels release their space. Modal placement reads the last
completed layout before the next, so text/viewport changes settle on the following layout.
The board has opaque modal backing and clips to its reserved region; the roster scrolls in
the card's remaining height. There is no vehicle name, viewer-distance scale or pair-specific
nameplate/scoreboard exception. Fixed HUD placement itself is unchanged.

The two regression tests mount the real spawners and bundled fonts without a GPU. They pin
the semantic layers, Bevy's computed draw order, opaque backing, non-overlap with every visible
dock, a nonempty scrollable roster, status-text growth and released diagnostic space. Their
viewport/scale cases are FIXTURE DATA, not a claim of universal viewport coverage.
Fixed in "hud: order layers and reserve fixed-panel space (#236)".

## The muzzle law (#245)

Subject: the game's muzzle effects, view only (ADR-0014): no sim state, no wire change. The
owner's decision (2026-11-01, #245): muzzle visuals are taste, not realism, and what matters is
consistency. This section is the law's one home; `guns::muzzle` and `vfx::muzzle` point here.

**The law.** Two facts size every shot's muzzle emission, resolved once per (gun, round) as the gun
registry loads:

- **The shot's muzzle energy** E = ½·m·v², from the round's launched mass (`mass`, carrier
  included: what leaves the muzzle) and the gun's `muzzle_speed` for that round. It is per round,
  never per gun: an APCR shot and an AP shot from one gun draw different sizes. A chambered round
  without a positive finite mass, or a row without a positive finite speed, is refused at load,
  naming the round's file and `mass`, or the gun's file and the row's `muzzle_speed`.
- **The gun's muzzle shape** (`muzzle_shape` in its gun file, `guns::MuzzleShape`), a closed enum
  holding the set the shipped guns need: `MuzzleBrake` (the KwK 36) and `FlashHider` (the MG 34).
  The shape says where the gas goes: its core sprite and share, its forward jet's planes and share,
  its side jets (angle from the bore, planes, share) or their absence, and which of the fireball,
  fill glow, smoke and ground blast it draws. Bare was left out: no shipped gun has a bare muzzle,
  and a variant no data selects is a switch with no user. The first bare-muzzled gun adds the
  variant and its row in `shapes`.

Every magnitude lives once, in the taste file `assets/vfx/muzzle.ron`: its value for a shot of the
reference energy (1 MJ, a round number between the shipped guns' shots), each labelled TASTE. One
curve per property, a power law on E / reference whose exponent is fixed by what the property is:

| Property | Exponent | Why |
| --- | --- | --- |
| a length or diameter (core, flames, fireball, smoke, light range and radius, standoffs, dust reach, sizes and spread) | 1/3 | similarity: a gas volume's worth of work |
| a light's peak | 2/3 | similarity: the flash's area |
| a gas life (fireball, smoke, dust) | `duration_exponent`, TASTE 0.1667 | a gas life grows with energy, at half the cube root's rate, so a rifle's smoke lingers long enough to read; the file refuses it outside 0..=1/3 |
| a gas speed (smoke rise and push, dust push and rise) | 1/3 − `duration_exponent` | a length over a gas life |
| a flash life (core, flames, glow, light) | 0 | every real flash lasts milliseconds, below one frame for every gun: each is drawn for the same two to three 60 Hz frames |
| shares, ratios, counts, opacity, colour, bore bias | 0 | shape facts, not sizes |

Two derived consequences: the smoke ration is the shots a mount's cyclic rate fires in the file's
`puff_interval_s` (TASTE 0.3 s; a single-shot gun puffs every round), and a tracer no longer
brightens the muzzle light (the energy is the one scalar; the old per-gun `tracer_gain` is gone).
The per-gun `muzzle` blocks (about 25 values per gun), their parse path and their tests were
deleted; a gun file that still states one is refused at load.

**Derived numbers** (DERIVED from the shipped data and the taste file; the jitter is ±15 % on the
core and flame lengths and the fireball diameter):

| Shot | E, J | Core, m | Flames, m | Fireball | Smoke | Light | Gas reach |
| --- | --- | --- | --- | --- | --- | --- | --- |
| KwK 36, 88mm_AP and 88mm_APHE_ring (10.2 kg, 773 m/s) | 3.047e6 | 3.08-4.17 | forward 2.03-2.75, two side jets 3.14-4.25 | 6.16-8.34 m, 0.205 s | 2.17→4.06 m, 1.45 s | 8.41 Mlm, range 36.2 m | 2.61 m |
| KwK 36, 88mm_HE (9.30 kg, 820 m/s) | 3.127e6 | 3.11-4.20 | forward 2.05-2.77, side 3.17-4.29 | 6.21-8.41 m, 0.206 s | 2.19→4.09 m, 1.45 s | 8.55 Mlm, range 36.6 m | 2.63 m |
| KwK 36, 88mm_APCR (7.3 kg, 930 m/s) | 3.157e6 | 3.12-4.22 | forward 2.06-2.78, side 3.18-4.30 | 6.23-8.44 m, 0.206 s | 2.20→4.11 m, 1.45 s | 8.61 Mlm, range 36.7 m | 2.64 m |
| MG 34, 7.9mm_AP (11.8 g, 755 m/s) | 3.363e3 | 0.19-0.26 | three forward 0.31-0.41 | none (flash hider) | 0.22→0.42 m, 0.46 s, every 4th shot | 0.090 Mlm, range 3.75 m | 0.27 m |

DERIVED from the law: the MG shot's gas reach is about 0.27 m, and the main-gun shots' reaches
are about 2.61–2.64 m. These do not certify a mounted muzzle's height at an unspecified pose or
gun elevation. Ground coupling reads the presented muzzle's height over known terrain: within
the resolved reach it may lift soil, and higher or unknown ground lifts none.
The MG's light is 1/94 of the main gun's (the two-thirds power of the
energy ratio 906, DERIVED), and its 3.75 m range lights little beyond its own mount. Every shape
share, every reference value and the duration exponent are taste for the owner to judge on the
capture stills; the assets' digests change, so the new look reaches players with the next release.


## Lane 5: blind-review verification, compact board and requested stills

| reviewer | lane | finding | verified / not a defect | fixed in (subject) |
| --- | --- | --- | --- | --- |
| Codex, `review-sweep-view-4-codex.md` | sweep-view-4 | P2: narrow board clips score columns | verified | `hud: keep narrow board columns reachable (#236)` |
| Claude, `review-sweep-view-4-claude.md` | sweep-view-4 | P3: HUD rungs outside the overlay ladder | verified | `hud: compact the Tab board and reserve the live inset (#236)` |
| Claude, `review-sweep-view-4-claude.md` | sweep-view-4 | P3: layer documentation promises opaque backing | verified | `hud: compact the Tab board and reserve the live inset (#236)` |
| Claude, `review-sweep-view-4-claude.md` | sweep-view-4 | P3: inset/card clearance gap | verified | `hud: compact the Tab board and reserve the live inset (#236)` |
| Codex, `review-muzzle-law-codex.md` | muzzle-law | P2: scaled emission overflow | verified | `guns: refuse unrepresentable muzzle emissions at load (#245)` |
| Codex, `review-muzzle-law-codex.md` | muzzle-law | P2: fireball opacity/boost remain in code | verified | `vfx: author shared muzzle sprite taste in one file (#245)` |
| Codex, `review-muzzle-law-codex.md` | muzzle-law | P2: shape test misses duplicated flanks/angle | verified | `vfx: guard both muzzle jet flanks at the resolved angle (#245)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P2: unacknowledged legacy reference-look anchor | verified (provenance; owner accepts the taste) | `docs: record the view-5 review and stills evidence (#236)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: muzzle stills absent | not a defect | existing session `caps-muzzle/index.md` and PNGs; no new muzzle captures |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: shape flag tests miss independent combinations | verified | `guns: refuse unrepresentable muzzle emissions at load (#245)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: shape test pins shipped taste | verified | `guns: refuse unrepresentable muzzle emissions at load (#245)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: jet test rejects a legal forward side angle | verified | `vfx: guard both muzzle jet flanks at the resolved angle (#245)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: scalar sprite taste is misdescribed as playback/caps | verified | `vfx: author shared muzzle sprite taste in one file (#245)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: finite-input energy/count assertions are false | verified | `guns: refuse unrepresentable muzzle emissions at load (#245)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: dust survey deduplicates distinct ammunition rows | verified | `vfx: author shared muzzle sprite taste in one file (#245)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: gun-authored reach/cloud wording is stale | verified | `vfx: author shared muzzle sprite taste in one file (#245)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: energy ladder spans twelve decades, not six | verified | `guns: refuse unrepresentable muzzle emissions at load (#245)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: AP/APHE light range rounds incorrectly | verified | `docs: record the view-5 review and stills evidence (#236)` |
| Claude, `review-muzzle-law-claude.md` | muzzle-law | P3: unconditional muzzle-height claim lacks evidence | verified | `docs: record the view-5 review and stills evidence (#236)` |

Subject: the game's rendering and HUD; every mention of rounds, hits, crew or knockouts is the
game's damage model and game state only. Review files are the session's `scratchpad/codex/`
artifacts named individually above. Verification starts from each supplied concrete input or
mutant; these are correctness checks, not frame/tick pricing. No muzzle taste value is retuned.

**Breaking inputs and regressions.**

- The narrow-board fixture is FIXTURE DATA: target 960×1080, effective UI scale 2, diagnostics
  visible. The old table could exceed its clipped root without any horizontal navigation. The
  board now scrolls both axes, keeps columns on one line, pages rows with Page Up/Down and columns
  with Left/Right. The real-font layout and input regressions cover content bounds and paging to
  each end. MEASURED against the old handler: horizontal position stayed 0 instead of the
  requested 400 logical pixels. This is independent of the compact board's typography.
- The rung and backing findings are documentation/ownership defects: `HudLayer` stamps only z;
  it cannot guarantee a background. Named world-label/fixed-HUD rungs now live and are pinned in
  `Overlay`; the opaque card colour is the named `ui_font::MODAL_CARD_BG`. The existing real
  draw-order/alpha regression remains the consumer guard.
- The live hit inset plus held Tab is the clearance counterexample. Its existing public
  `CornerRig` marker joins its UI root to the right dock from `hud`, preserving the inset's own
  z rung; its world meshes/lights/cameras do not dock. The fixture mounts the integration marker
  and actual corner dimensions and checks non-overlap. Temporary encounter/death cards are
  deliberately exempt: the held board covers that overlay content under `Overlay::SCOREBOARD_Z`.
  Reserving the full-screen death panel would eliminate the board's usable region. No file in
  `src/damage_cam/` changes.
- FIXTURE DATA `reference_energy_j = 1e-40` with an ordinary shot produced infinite emission
  magnitudes. MEASURED: `scaling_outside_the_emissions_domain_is_refused` failed on the old
  implementation. The law now returns a field-naming admission error for an unrepresentable
  ratio, scaled field, derived end size or smoke count; the registry adds both file/row context
  and the repair. FIXTURE DATA mass/speed pairs `(1e30, 1e5)` and `(1e-30, 1e-10)` expose the false
  finite-input assertion; a huge cyclic rate exposes its count counterpart. Energy is computed
  with wider intermediates and admitted only when representable as positive finite f32. The
  registry regression checks the actual outside-input path and its source names.
- The scalar-taste counterexample is an owner wanting a different fireball alpha/boost: the old
  schema had no field, and the renderer forced constants. All named scalar boosts, opacities and
  sharpness values now live in `muzzle.ron` and reach material uniforms; sheet playback/erosion
  rules and population caps remain renderer contracts. Their shipped values are identical.
- A duplicated `[across, across]` flank survived the old count/dot test. The revised rendered-axis
  test checks the resolved angle and equal plane counts on both opposite flanks, and exercises
  legal forward/backward angles (FIXTURE DATA: 45°, 100°, 135°). It no longer imposes a 60° taste
  restriction. MEASURED: the duplicated-flank mutant fails with `(4, 0)` planes instead of
  `(2, 2)`; the mutant is removed. Shape parsing/resolution and emission tests exercise every independent Boolean
  flag combination with synthetic statements, comparing emitted layers with those statements;
  legal edits to the shipped shape flags do not fail them.
- The old dust survey's gun-key deduplication removed other ammunition rows. The survey retains
  every `(gun, round)` and reports that pair, so the cloud assertions reach every shot's resolved
  emission. Reach/cloud/fireball comments now describe resolved shot geometry. The energy
  ladder is DERIVED: its speed spans six decades and its squared energy spans twelve.
- The record's light range is DERIVED: `25 × (0.5 × 10.2 × 773² / 1e6)^(1/3) = 36.2451... m`, rounded to 36.2 m. The
  blanket muzzle-height claim is removed in favour of the actual height/reach condition; no
  unspecified elevation or rig pose is certified by an energy-derived reach.

**Accepted taste and provenance.** The comparison with the deleted gun blocks is valid. DERIVED
at the AP reference shot: dust birth about 1.81 m versus the old 1.8 m, smoke about 2.17→4.06 m
versus 2.2→4.2 m, light about 8.41 Mlm versus 8.0 Mlm. The reference smoke/light/dust values
numerically preserve much of that provisional legacy look, with rounding; they are not an
independent physical calibration. The deleted blocks are visible in
`guns: size every muzzle from the shot's energy and the gun's shape (#245)`. This provenance
was missing from the record. The owner explicitly accepts the current taste as a start, so this
lane corrects the attribution and keeps every accepted magnitude and exponent.

The missing-muzzle-stills finding is not reproducible at verification; the session artifact
directory differs from the two repository-relative locations the review searched. MEASURED here:
the session's
`caps-muzzle/` already contains its `index.md` and 92 PNGs. That evidence needs no new capture.

**Board decision.** Held Tab may use more of the middle beside fixed panels, while staying clear
of their actual boxes. A first small still exposed the current repair readout's extra rows: a
full-width top-band reservation left no roster. `hud: place the board beside tall fixed panels
(#236)` selects the largest clear rectangle from the measured panel boxes, and the real-font
fixture now includes the full repair status. Panel growth is checked for non-overlap, rather than
assuming that every growth must move the board downward. The inset's separate clearance fixture
states its short-vitals state explicitly. A target completely occupied by fixed panels still has
an explicitly empty modal region.

ASSUMED typography: 14 px Barlow Condensed body on a 16 px line. MEASURED real-font capacities:
six complete standings at 1280×720 / effective UI scale 2, and all thirty at 1920×1080 / scale 1.
The tests also check real rendered line count/height and that the scroll content reaches the full
roster. Thirty standings plus a header cannot fit a 360-logical-pixel-high target at that line
height even without HUD/card furniture (DERIVED), so the small fixture intentionally scrolls.

The only player-facing count string found was `Column::Knockouts`'s `KNOCKOUTS` header in
`src/net/battle_hud.rs`; it now says `Kills` in combat and results. Code identifiers, diagnostics
and domain prose retain knockout vocabulary.

**Gates and stills.** Formatting and workspace CI clippy passed before each implementation
commit. Focused green groups: MEASURED 40 gun/muzzle/citation tests for admission; 43 for scalar
sprite taste and prewarming; 42 for the jet guard; 15 for board paging; 117 for the compact board,
HUD/overlay/capture modules and whole-tree guards; 67 for the full-repair placement fix and guards.
The final capture batch passed formatting, CI clippy and all 43 citation/font/network/bake
guards. MEASURED: eight PNGs verify with the requested dimensions; clients completed and local
servers stopped. Capture-server logs retain no-RenderApp and late-input diagnostics, documented
in the stills index; excluded netcode was not investigated. GPU-less UI harnesses emit their existing no-RenderApp/logger warnings; their assertions
pass. No frame/tick pricing, pushes or issue comments were made.

The session's `scratchpad/caps-view5/index.md` owns the requested stills, recipes and logs: the
original sand-dust sequence and the final board at both fixtures. The board stills were retaken
after the full-repair placement fix. The owner must judge the dust's prominence, soil colour and
persistence, and the compact board's readability and scrolling. The ground cloud is thin in this
exact dust pose and reads most clearly in the early post-shot frame; no taste was retuned.
