# Static friction: a brush anchor gated by a stick speed; throttle release commands a hold

> **Status: retired with [[0025-belt-force-locomotion]]** (2026-07-17). The brush anchor patched the raycast model's velocity-proportional friction and died with it — nothing structural carried over into the belt model (owner call at cutover). Slope-parking creep is currently an accepted gap; if playtest demands hill-hold it returns as a per-element bristle *inside* the belt model's slip law, for which the Karnopp/LuGre analysis below remains the reference.

A roadwheel holds the tank at rest with a **brush anchor**: below a small contact speed (the **stick speed**) the wheel plants a world anchor point and applies a spring-damper pulling its contact back toward it, capped at the friction circle (μ × load); above the stick speed friction is kinetic — the existing skid / coast-down model. This is a Karnopp-style zero-velocity gate wrapping a Dahl/LuGre-style bristle. It exists because velocity-proportional friction has zero force at rest, so a parked tank crept — in fact ran away — down any slope.

Gameplay stance: **releasing the throttle commands a hold, not a neutral coast.** Across eras a stopped driver holds station on the brakes (older clutch-and-brake, modern in-gear), so "no throttle = hold on the brakes" is faithful rather than an arcade concession — and it lets us skip modelling the gearbox/clutch entirely. The hold is not an instant stop: on release the longitudinal axis first applies a light engine-brake / coast-down (the heavy-glide feel dial) while the tank is still rolling, and the static anchor only grips once it slows past the stick speed. Neutral roll-away becomes an explicit later mechanic (a clutch/neutral input), not the default.

## Considered Options

- **Pure velocity-proportional (Coulomb-viscous) friction** — what the seed shipped with; simplest, but zero force at rest means no hill-hold: the tank slides, then runs away, on any slope. Rejected as a missing model, not a tuning fault.
- **Karnopp velocity band that zeroes velocity in the dead-band** — cheap and numerically robust, but it works by *setting velocity*, which fights the force-based rigid body (`apply_force_at_point`). We keep Karnopp's speed gate but apply a force (the anchor spring) inside it rather than editing velocity.
- **Explicit brake input over a default neutral coast** — physically honest as a default, but makes every slope a manual hold and needs a brake key to be playable. Deferred: an explicit brake/handbrake earns its key only once braking-while-manoeuvring (handbrake turn, hold during neutral steer) is wanted.

## Consequences

- Hill-hold, auto-stop on release, and honest sliding past the grip limit all emerge from one per-contact model; the kinetic driving regime (skid steer, neutral steer) is unchanged — the anchor only engages near rest.
- New tuning surface in `driving.rs`: `STICK_SPEED` (the static↔kinetic gate), `BRUSH_STIFFNESS` / `BRUSH_DAMPING` (the hold spring), and `ROLLING_RESISTANCE` repurposed as the engine-brake / coast-down feel dial. All principled placeholders.
- The grip anchor lives at the same contact station as suspension and drive — one ray, now three jobs (support, drive, hold) — extending the support/drive split of [[0005-raycast-roadwheel-locomotion]].
- A future explicit **neutral/clutch** (roll-away) and **brake/handbrake** are deliberately unbuilt; this ADR records that "release = hold" is the default they would layer onto.
