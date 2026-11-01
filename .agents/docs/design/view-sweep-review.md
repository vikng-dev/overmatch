# View sweep review

Subject: the game's rendering, effects and camera. All impacts, rounds, crew and HP mentioned here
are the game's damage model and game state only. Starting tree: `7ca401d3`; lane `lane/sweep-view`.

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
| V4 | Fixed (lane 2) | Each gun file states a required sourced `muzzle` block (`guns::MuzzleEmission`): core, flames, fireball, glow, smoke ration, light, ground blast. It rides `Round::muzzle` and `FireShell::muzzle` to one `vfx::muzzle` observer; no calibre split or seed-calibre scale remains. Shipped values are the previous provisional look, ASSUMED. |
| V5 | Fixed (lanes 2, 3) | Ground types state a required `dust.erodibility` and `dust.colour_srgb`; the gun states its gas `reach_m`. Puffs = the gun's rolled count × the painted grounds' summed share (weight × erodibility, `terrain_blend::dust_sources`); each puff is one source's soil, drawn by share, lit by the key light (see "Lane 3"). Erodibilities ASSUMED, ordered by hand; colours DERIVED from the packs' albedo. |
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
  irradiance on level ground over π times the camera's exposure (2.33 under the shipped sun and
  Bevy's default exposure, DERIVED); the sky's share is left out. MEASURED per ground, shipped gun:
  concrete 1, dirt 5-8, gravel 4-6, mud 2, sand 5-8 puffs, 1.8 m growing to 3.8-5.0 m over
  2.0-2.5 s. Still open: a puff's mean coverage at birth is 0.09 (the sheet's 0.19 × the gun's 0.5
  opacity), so the cloud stays thin; the impact read's dust and the muzzle smoke erode to 1.0 on
  the same kind of sheet and still show about a quarter of their authored lives.
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
  Pzgr. 39 and 7.92 mm cartridges) is a ratio of 9.6; the main gun's 3.5-4.6 m core and 4.3-6.4 m
  flames give 0.36-0.48 m and 0.45-0.67 m, against the MG's authored 0.3-0.55 m and 0.45-0.8 m.
  Unchanged. The MG's light peak, 1.2 Mlm, is not on that law: by flash area it would be about
  0.09 Mlm.

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
