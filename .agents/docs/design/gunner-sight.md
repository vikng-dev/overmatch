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
