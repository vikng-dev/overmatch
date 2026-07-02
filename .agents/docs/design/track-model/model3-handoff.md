# MODEL 3 (box-belt) — fresh-start handoff

Handoff for a fresh session to build **Model 3** of the track sandbox from scratch. A first
attempt (increment 1) was built, tested, and **deleted at the user's request** — it was never
committed; everything worth keeping from it is distilled here.

## Read first (don't re-derive)

- `.agents/docs/design/track-model/HQ.md` — the living command post. Read the whole status log;
  the Model-2 steps (12–15b) and the Model-3 design entries (step 16 area) are the direct context.
- `src/track_sandbox/` — `mod.rs` (shared rig/course/registry/viz/drivetrain), `model1.rs` (frozen
  belt-primary baseline), `model2.rs` (link-belt, the model 3 forks from). All ~self-contained;
  mounted by `src/bin/track_sandbox.rs`, NOT by GamePlugin. Run: `cargo run --bin track_sandbox`.
- Memory dir (`MEMORY.md` index): working-style-overmatch, visualization-first, lower-proactivity
  (discuss/propose before editing), question-style (one plain-text question at a time),
  scope-guard-vertical-slice.

## Repo state

- Bevy 0.19 + Avian3D 0.7, Rust edition 2024. fmt + clippy must be green (CI gate, no warnings).
- Tags: `checkpoint/track-model-1` (belt-primary), `checkpoint/track-model-2` @ `ea54a48`
  (link-belt final: advected fixed-count link ring, plate casts + pressure-centroid contact,
  Verlet chain solve, T-34 benchmark, damped load gauge). Model 2 is the verified baseline and the
  DEFAULT; `M` cycles registered models live on identical terrain.
- Already in the tree, ready for model 3 (kept from the deleted attempt):
  - `model2.rs` state types are `pub(super)` (BeltPhase get/advance, ChainMemory + ChainSideMemory
    fields + get_mut, LinkLoads + fields + get_mut, `clipped_linear_piece`) so a sibling model file
    can share them.
  - `switch_model`/`reset_rig` clear BeltSpeed, BeltPhase, ChainMemory, **LinkLoads** (the gauge's
    identity shift goes stale across teleports/switches otherwise — was a real bug).

## The settled design (user-driven, three refinements deep — do not re-litigate)

Each track link is a real box — the T-34 shoe, **500 mm wide × 172 mm pitch × ~40 mm thick** —
hung symmetrically on the **pin line**:

1. **Pin line is the chain.** Pins join links at mid-thickness; the pin line is the true pitch
   line (172 mm IS pin-to-pin; sprocket engagement is at pitch radius). The chain solve, joints,
   pitch, advection, and link count all live on it. Three parallel offsets of one solved curve:
   - inner face (pin − t/2): wheels ride here → wheel-circle constraints = wheel radius + t/2;
   - pin line: the state;
   - outer face (pin + t/2): terrain contact; forces applied out here (lever includes the shoe).
2. **Box casts.** Terrain detection per link = oriented box cast (`cast_shape` takes any collider;
   thin/thick boxes cast robustly) — first touch anywhere on the outer face, full-width.
3. **Width via edge columns.** Detection is full-face (the cast); the force *distribution* reuses
   Model 2's closed-form 1D pressure profile at the plate's two lateral edge columns (each owns
   half the per-metre coefficients), lateral position = load-weighted centroid of the columns.
   Lateral resolution knob = column count (2 = edges; more never likely needed).
   This is what buys: roll torque from a curb under one track edge, honest cross-slope contact,
   partial support half-off a ledge.
4. Boxes **scissor freely at pins** on bends (real castings interlock — the mesh's job, not
   physics). Parked consciously: link self-collision, end-face contact, guide horns.

Increments (each verified by the user before the next): (1) pin-line refactor + box cast on the
centerline; (2) real 500 mm width + edge columns + lateral centroid; (3) box rendering (drawn
plates — wrap fan-out around sprocket/idler emerges because the outer-face arc is longer).

## Implementation learnings from the deleted attempt (all verified-working mechanics)

- **PinBelt resource is required**: the pin-line perimeter is ~π·t longer than the belt-line loop;
  reusing model 2's `LinkCount`/`BeltLength` silently eats most of the slack budget. Compute own
  `length = polyline_len(belt_loop(pin_circles(), None)) + TRACK_SLACK` and
  `count = round(length / CONTACT_SPACING)` at startup. `pin_circles()` = `rest_circles()` with
  radii + t/2.
- **The outer-face pen convention is automatic**: casting the box (centered on the pin segment)
  from `center − out·CONTACT_PROBE` along `out`, `pen = CONTACT_PROBE − hit.distance` measures
  penetration past the *outer face* — the face offset cancels in the travel distance. No offset
  bookkeeping in the contact math.
- **Box orientation basis**: local X = lateral = `axis.cross(out)`, Y = `out`, Z = `axis` along
  the pin segment; `Quat::from_mat3(&Mat3::from_cols(lat, out, axis))`;
  `Collider::cuboid(width, TRACK_THICKNESS, len)` (full extents).
- **Endpoint rays** for the pressure profile must probe from the pins' *outer-face* points
  (`w + out·t/2 − out·PROBE`), clamped `.min(pen_max)`.
- **Apply force + draw the contact at the terrain surface**, not the sunken reference outer face:
  `p = wa + axis·(moment/area) + out·(t/2 − pen_c)` with `pen_c ≈ (pen_a.max(0) + pen_max)/2`.
  (The penalty penetration is virtual compliance; the reference line rides ~5 cm inside terrain at
  rest and the dots visibly rendered underground until this fix.)
- **Conform fork**: wheel circles + t/2; terrain planes hold pins t/2 inside the contact plane
  (`(p − q)·m ≥ t/2`); everything else (Verlet, gravity-in-hull-frame, drive anchor, rigid lengths,
  35° angle cap — the T-34 sprocket wrap needs ~31°/joint — slack bookkeeping via belly_extra) is
  model 2's unchanged.
- **`articulate_wheels` needs a per-model surface offset**: model 3 wheels rest on the *inner
  face* = chain + t/2 (candidate centre = chain_y + t/2 + √(R²−dz²)).
- **Draw the outer face** as a dimmer companion line (offset each conformed sample by its local
  normal × t/2) — makes the thickness read immediately; user verified "darker blue rides the
  ground, wheels ride light blue — looks logical".

## Open issues observed in the deleted attempt (unresolved — watch for them in the redo)

1. **Flat-ground gizmo jitter** returned in model 3 (model 2 was calm after its damped-gauge fix).
   Element never diagnosed (user was asked dots-vs-chain-lines but the session ended first). Known
   suspects, in order: box-cast coplanar tie-breaking noise feeding the profile; the outer-face
   companion line amplifying normal noise (computed from neighbor tangents of the conformed
   samples); a Verlet anchor-vs-terrain-plane residual. Diagnose element-first (that method worked
   twice before: ask WHICH gizmo moves).
2. **Washboard slap-down at crawl speed**: links visibly drop off bump corners one at a time.
   Analysis says largely honest (a link's contact plane is continuous while the corner is under
   it, then drops to the flat when its trailing edge clears; the rear joint falls over ~100 ms —
   real tracks clatter exactly like this), but the user found it "jumpy"; magnitude/feel needs a
   pass — candidate levers: Verlet damping/drive, plane hand-off blending, or accepting it.

## Vehicle + drivetrain constants (shared, in `mod.rs` — both models drive the same T-34)

26.5 t; road wheels r 0.415; wheel spacing 0.96 (3.85 m contact); sprocket/idler r 0.32 (smaller
than road wheels, like the real thing); pitch 0.172; `TRACK_SLACK` 0.13 (loose track — no return
rollers, return run drapes onto the road wheels via the chain solver's wheel circles); support
680k/m, damping 80k/m (~5 cm sink); constant-power drivetrain `engine_available(v) =
min(120 kN, 186.5 kW / |v|)` per track; MAX_BELT_SPEED 15; belt inertia 8 t. `TRACK_THICKNESS`
was 0.04 in the attempt (T-34 shoe).

## Avian 0.7 API notes (verified against vendored source, ~/.cargo/registry/.../avian3d-0.7.0)

- `SpatialQuery::cast_shape(shape, origin, rotation: Quat, dir: Dir3, &ShapeCastConfig, &filter)`;
  `ShapeCastConfig { max_distance, .. }`; `ShapeHitData { distance, point1 = on the HIT shape
  (terrain), point2 = on the CAST shape, .. }`.
- **Coplanar contact points are degenerate**: parry picks arbitrarily among tied points and the
  pick flips tick-to-tick. Never hang force position or gizmos on `point1` for face-face contact —
  that's what the pressure-profile/centroid machinery is for (this bug was found and fixed in
  model 2, step 13b).
- Colliders default to friction 0.5: any backstop collider needs
  `Friction::ZERO.with_combine_rule(CoefficientCombine::Min)` (already on hull + drive wheels).
- Bevy 0.19: `TextFont.font_size` is `FontSize::Px(..)`.

## Workflow contract (user's explicit expectations)

Work end-to-end: after each change `cargo fmt` + `cargo clippy --bin track_sandbox` (green, no
warnings), `pkill -f target/debug/track_sandbox`, relaunch in the BACKGROUND, hand off with
specific test criteria; the USER screenshots and reports. Update HQ.md each step (append
chronologically at the bottom, before the parking lot — beware: repeated "NEXT:" lines make bad
edit anchors; the log got scrambled once). Commit only when asked; checkpoint tags on request.
One model per file, model-specific systems gated on `ActiveModel`; frozen baselines (model 1,
model 2) are not edited except shared-visibility changes. Discuss/propose at design forks; one
question at a time; keep every mechanic visible in the sandbox (gizmos).

## Registration checklist (what wiring "add a model" takes, from the deleted attempt)

`model3.rs` (fork of model2's two systems + own consts) · `mod model3;` + imports in mod.rs ·
`Model::BoxBelt` variant + `label()` arm + `MODELS` entry + default → BoxBelt · startup
`init_pin_belt` · FixedUpdate physics gated `model_is(BoxBelt)` + `sim_running` · conform system
in the Update visual chain, same gates · per-model wheel-surface offset in `articulate_wheels` ·
outer-face companion line in `draw_rig_gizmos` (+ `BELT_OUTER_COLOR`).
