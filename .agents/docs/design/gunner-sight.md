# Design sketch: gunner's sight (System B) & gunner-view aim control

**Status: SPEC for the in-progress build (2026-06-26).** Decided in design discussion; being
implemented now. Graduates to an ADR if it survives contact. Vocabulary from `.agents/GLOSSARY.md`
(Sight, Bore axis, Bore point, Aim point) and `.agents/skills/codebase-design`.

## The sighting system: B (of A/B/C/D)

Player-solved, **coaxial, no parallax** — the camera sits on the **Gun node** (later: a dedicated
sight node). Rejected: A (game-solved elevation, WoT — arcade), C (offset sight + lateral parallax
— differentiator, payoff only <~150 m, parked), D (offset + game-solved — pointless: auto-solving
wastes the offset). Parallax math + the A/B/C/D rationale: chat 2026-06-26.

### The three lines (the core relationship)

```
intent      ← committed world (hull-local) aim POINT, steered by the mouse   (see §2026-07-10)
sight line  ← the gun's BASE lay; camera looks along this; reticle centre   (= intent, lagged)
bore        ← sight line + superelevation(range)   ← the barrel; sits ABOVE the reticle
```

- Camera orientation = **sight line = bore − superelevation**. The camera is positioned at the
  Gun node but **must NOT inherit the barrel's superelevated pitch** — else ranging tilts the view
  off-target. Compute it: `gun_forward` pitched DOWN by `superelevation(range)` about the gun's
  right axis.
- Gun-pitch servo target = `intent_pitch + superelevation(range)`, so the barrel physically
  elevates and firing is automatically correct (shooting reads the Muzzle's elevated bore; the
  shell arcs back onto the sight line at the dialed range).
- **Ranging = scroll wheel** (manual; Tiger has no rangefinder — LRF is modern-only). Sets
  `superelevation` via the gravity solution `θ ≈ g·R / (2·v²)` (88 mm, v≈773 m/s → ~8 mrad @ 1 km).

## Aim control: WoT third-person + WT gunner (hybrid)

- **Third-person (commander):** unchanged — free sight leads, gun chases (the current `aim.rs`).
- **Gunner view (WT):** camera locked to the gun's reality; **world-space position-control**
  intent. Mouse *deltas* (cursor already `Locked`) accumulate into a committed hull-local
  yaw/pitch intent. The turret/gun servos chase it at their RON-authored slew rate; the camera
  (= sight line) lags, so the intent reticle **drifts back to centre and settles**. Dead-stop on
  release — hold still and the gun arrives and STOPS (continuous slew needs continuous hand
  motion). NOT rate control (a screen-pinned cursor would emergently produce rate — rejected).
- **Toggle: Lshift.** On entering gunner view, seed `intent` from the gun's current lay (no jump).

## Implementation seam

- New `sight.rs`: `SightMode{ThirdPerson,Gunner}` + `Ranging{range}` resources; `toggle_sight`
  (Lshift); `adjust_range` (scroll, gunner only); `drive_gunner_aim` (mouse→intent→ServoCommand
  targets); `superelevation(range)`.
- `camera.rs`: when Gunner, position at Gun node along the sight line + narrow FOV; skip orbit.
- `aim.rs`: gate the existing third-person `aim` to `ThirdPerson`; gunner mode drives the same
  `ServoCommand` targets from `sight.rs` instead. One writer per mode — no conflict.
- Both write the existing `ServoCommand.target` (hull-local yaw / pitch), so the rig + `drive_servos`
  chase mechanism is reused, not rebuilt.

## 2026-07-10 revision: the intent is a resolved POINT, measured from the mount

The original spec's "committed hull-local aim direction" shipped as a bare direction re-encoded as
a 10 km far point from the HULL-FRAME ORIGIN, while third person committed a resolved world point.
The two forms met at every mode transition, and every conversion changed the observer origin
(hull origin ≈ ground level vs gun mount 2.2 m up vs orbit camera ~5 m up) without re-resolving —
a parallax error class scaling with 1/distance (~2.5° at 50 m, most of the 3.1° optic radius),
invisible at the horizon where the feel checks ran. Three regressions in one day came from it.

Revised model (implemented; see `aim::CommittedAim`'s four-invariant doc block, the doctrine):

- **Both modes commit resolved world points** into the one `CommittedAim` memory — third person by
  raycasting from the camera, the optic by raycasting **from the gun mount** along its sight line
  (terrain or another tank's armor — the shell's own `Terrain | Armor` mask, own tank excluded —
  far fallback in the sky). No point↔direction conversion exists anymore.
- **One origin per frame convention:** the optic's yaw/pitch working form is the bearing of
  `point − mount`, the same per-servo-from-its-own-pose decomposition `drive_aim_servos` uses; the
  resolve `mount + dir·t` inverts it exactly, so resume↔resolve round-trips without drift.
- **Zero-input identity** (kept, and still necessary): the two modes resolve from different origins,
  which can see different geometry (crest occlusion), so the optic never re-resolves an inherited
  commitment until actual mouse input (`sight::resume_commit`).
- **Mode exit re-aims the orbit camera at the committed point** (`camera::reaim_orbit_on_optic_exit`):
  pivot, camera body, and point are collinear, so the white reticle lands on the committed point and
  an RMB-up recommit re-picks the SAME point — the transition is identity on the aim in both
  directions.

## 2026-08-17 revision: one camera on a knob, and the glass is drawn

The five gunner *schemes* (A `BoundOptic`, B `FreeReticle`, C `DecoupledOptic`, D `ElasticBore`,
E `LeadOptic`) collapsed. A and E commanded the gun **identically** — both through
`sight::drive_gunner_aim` — and differed only in where the camera rode, so they were never
alternatives: they are the two ends of one continuous knob.

- **`sight::GunnerBlend(k)`** is that knob. The optic camera's look is a geometric blend, fraction
  `k`, between the gun's **sight line** and the **committed intent**, taken as a plain interpolation
  of the two bearings in the HULL's frame (shortest-angle on yaw — a continuous turret's yaw
  coordinate wraps). `k = 0` is A, `k = 1` is E, and both are pinned as regressions in `camera`'s
  tests. Instantaneous, stateless, bounded: no damping and no spring, so the aperture cannot
  overshoot or wobble. Damping is a later decision, not a lost one.
- **`V` cycles `k`** through a ladder, defaulting to the interior `0.5`. (The ladder was re-cut
  — §playtest.)
- **B and C are cut.** They were a genuinely different commit path — the mouse steering the camera
  and the gun chasing it, with no optic circle — and the free-look commit, its servos param, its
  camera and its wide gunnery FOV went with them. D's spring is cut too, recoverable from git.
- **The optic mask** (`sight::reticle`) draws the surround: an opaque field with a circular hole
  centred on the gun's **sight line reprojected** — not screen centre, which it only coincides with
  at `k = 0`. Its angular radius is `sight::OPTIC_RADIUS_FRACTION × fov/2`, the SAME number that
  bounds the cursor's deflection, carried to pixels through the camera's actual projection. That
  shared number plus the blend's position between the two bearings is why the intent can never leave
  the drawn glass — the property the mask's coherence rests on. (The rim moved to the viewport; the
  shared number survives that — §settlement, §playtest.)

## 2026-08-17 revision: a view is authored as the instrument it is

A view carries `spec::Optics`, not a bare angle: `Magnified(magnification, field_deg)` for an
instrument, `Unaided(°)` for the naked eye a commander's hatch gives. Two variants because they are
two different eyes, and a hatch has no magnification to state.

**An optic authors both of its numbers, off the data sheet, and neither derives the other.** The
Tiger's TZF 9b is `2.5×` over a `25°` true field, both measured. Deriving the field from the
magnification through one game-wide eyepiece constant was rejected — real instruments share no such
constant — and nothing cross-checks the pair, because there is no reference left to check it
against. Magnification is authored truth about the instrument, field is authored truth about what it
frames, and `spec::Optics::vertical_fov` is the one place the authored degrees become radians.

Only `validate()` reads the magnification today, and that is correct. It is the instrument's
identity and the seat a selectable ladder sits in; a consumer invented before that ladder exists
would be a consumer invented to satisfy a lint.

Everything downstream already read the field rather than a stored angle, so nothing needed a second
edit: the cursor's deflection bound was `OPTIC_RADIUS_FRACTION × fov/2` (3.1° → 11.25°, since
re-derived through the projection — §playtest), the drawn
glass is that bound measured through the camera's own projection, and the mask's on-screen radius is
invariant under the field — the two field terms cancel, leaving only the projection's `tan`
(MEASURED: under 2% of the radius across a 5.2°–62.5° spread).

The range scale is angles and did not move, but it now sits in a field 3.7× wider than the old
authored angle: the whole 200–4000 m graticule spans **3.37° ≈ 145 px on a 1080-tall viewport**
(MEASURED, 88 mm at 773 m/s), inside a glass 970 px across. It used to overflow the glass and be
clipped; it now fits with room over. The one consequence to watch is the numbered 400 m graduations,
which near the dialed range stand ~10–15 px apart — legible marks, crowded labels. Thinning the
numbering (or shortening the scale) is a sight-design decision.

## 2026-08-17 revision: the mask is a style, and one style is only framing

**Superseded by the section below**, which settles the style and rewrites the bound; kept for the
containment argument it introduced.

War Thunder's surround is not a scope aperture. It is a circle spanning most of the display's
**larger axis** with a broad blurred edge, and it says nothing about where the gun can be laid.
Which of the two reads better is a question for play, so both ship behind one knob.

- **`reticle::MaskStyle`**, cycled by **`B`**, is the knob. It is presentation-only and private to
  the presentation half: no style is an input to the aiming law, and there is no path by which one
  could become one.
  - **`Aperture`** (default) is the mask as built above — rim on the deflection bound through the
    camera's own projection, ~1 px feather at 720p (0.4486 of the viewport height at 2.5×, DERIVED).
  - **`Framed`** takes its radius from the viewport instead: the drawn circle spans
    `FRAMED_SPAN_FRACTION` (0.9) of the larger axis, feathered by 10% of its own radius — a broad
    gradient, which is what reads as a blur rather than an anti-alias.
- **`Framed` deliberately breaks the shared-number invariant** the section above rests on. A circle
  off the larger axis is bigger than the angular bound (78% bigger at 16:9, DERIVED), so its glass
  stops indicating where the intent stops and becomes framing. `sight::OPTIC_RADIUS_FRACTION` is
  still the ONE aiming bound, identical in both styles; what a style owns is the rim.
- What survives the break is **containment**: every style's rim must sit outside the bound. Asserted
  in `MaskStyle::rim` and tested at every blend rung across four aspect ratios, rather than argued
  from a constant only one style shares. Its floor is a viewport no wider than it is tall, where
  `Framed` spans 0.9 of the height against a bound the projection's `tan` keeps under it — 0.3% of
  clearance at 2.5× (DERIVED), which is why the assertion is worth having.
- **Ultrawide is the honest cost of the larger-axis rule**: at 21:9 the framed circle is 2.13× the
  viewport height (DERIVED), so only the corners darken. That is what the style *is* — framing, not
  an aperture — and a per-axis cap would be a second law bought for nothing.

## 2026-08-17 settlement: the mask settles

Three playtest rulings, taken together because they are one sight picture. Two of them — the blend
splitting per axis and the bound becoming an ellipse — were reversed by the next playtest and are
recorded in §playtest below; only the mask survives here.

### The mask collapses to the framed circle

Playtested against the aperture: **framed, with the soft feather, is the sight.** Options collapse
once one is truth — the same ruling that killed the five schemes — so `MaskStyle`, its `Aperture`
arm, its ladder and its `B` binding are gone, and `Framed`'s geometry is simply the mask. The circle
spans `MASK_SPAN_FRACTION` (0.9) of the viewport's larger axis as a DIAMETER, feathered by 10% of
its own radius. `B` is freed and stays free (§playtest).

The aperture's job — a rim that *is* the aiming bound — does not survive the collapse, and does not
need to: §playtest puts the bound back on the shared number by a different route.

## 2026-08-17 playtest: the bound goes back to a circle, the blend back to one knob

Two rulings on the settlement above. Both REMOVE machinery.

### The bound is a circle again, sized by the LARGER axis

The ellipse was built, played, and judged **too restrictive** — "equal deflection according to the
larger axis". So `optic_margin` answers with one angle again and `OpticMargin` is gone. Do not
rebuild it.

```
margin = atan(OPTIC_RADIUS_FRACTION · max(aspect, 1) · tan(fov/2))
```

`max(aspect, 1)` is not a special case bolted on. It is precisely what makes the bound equal the
DRAWN RIM, whose radius is `0.45 × max(width, height)`: **the travel circle IS the drawn glass at
every aspect, exactly** — the invariant the design had before the ellipse, now true by equality
rather than by inscription. Because `sight::optic_margin` and `reticle::optic_glass` provably
compute the same number, the containment `debug_assert` between them is a real check again instead
of the vacuous one the ellipse left behind.

At the Tiger's 25° the cursor reaches **19.53° in every direction at 16:9** (MEASURED), and 11.28°
on a portrait or square viewport, where the height is the larger axis. The reach is isotropic in the
sight's yaw/pitch ANGLES; the perspective divide maps that circle to a curve exactly on the rim at
the four axes and 1.6% inside it on the diagonals (MEASURED).

**Accepted consequence, ruled and not to be guarded against:** 19.53° is wider than the screen is
tall (±12.5° at a 25° field), so the bound no longer keeps the intent inside the VIEWPORT. The gun's
own elevation stop binds first, and at full elevation the cursor can sit slightly past the top edge.
The surviving containment property is "the intent never leaves the drawn GLASS"; the viewport
assertion is retired.

`optic_margin` takes the viewport aspect as well as the field, so `drive_gunner_aim` carries a
camera query; the pre-bind fallback is a square viewport.

`camera::GUNNER_FOV_FALLBACK` was left alone deliberately. It is not a vehicle's field — it is a
conservative "narrowest field" bound that `view::ViewFacts::default` and `world::terrain_lod_view`
consume to seed both LOD ladders, and widening it fires two LOD tests.

### The blend collapses back to one knob

`0.5` felt best on BOTH axes, so the split earned nothing and goes — the same ruling that killed the
five schemes and the `Aperture` mask. `GunnerBlend` carries one `k`, `camera::blended_look` takes
one `k` for the yaw and pitch lines alike, and **`B` is free again**.

The ladder extends DOWNWARD, because `0.5` was the bottom rung reachable when it was judged and may
be a floor artifact rather than an optimum: **`[0.0, 0.35, 0.5, 0.65, 0.8, 1.0]`, default `0.5`,
cycled by `V`.** `0.0` is on it so the whole spectrum can be felt, which puts the two endpoints back
to being the two cameras the five schemes collapsed into. Still stateless: the camera reads the
resource fresh each frame and holds nothing. No time-domain filter and no spring on top — a stateful
low-pass is the proportional blend RO2 shipped, and its documented failure is overshoot.
