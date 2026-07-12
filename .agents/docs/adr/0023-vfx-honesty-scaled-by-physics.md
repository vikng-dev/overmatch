# VFX honesty: an effect is scaled by the physics, never by the viewer

Research on the 88's terrain impact recommended a ~10 m dirt column, sized so that a hit would subtend enough pixels to be read at 1000 m. The owner rejected it (2026-07-12): *"i don't want to fake columns — if there's a small splash at 1km, then it needs to remain a small splash. no fake assistance. just the impact needs to be real and scaled to the shells power and the surface it hits."* That is now doctrine, and this ADR states it as a law of the view layer: **an effect's size, duration and behavior derive from the physics — shell energy, caliber, the surface struck, real reference footage — and NEVER from viewer distance, screen-space readability targets, or camera position.** No range-scaled effect sizes. No minimum-apparent-size clamps. No assistance for the shooter. Shipped with the 88 splash (`924609d`).

## The rule

The `Impact` event carries what the *physics* knows — the round's `caliber` and the `ImpactSurface` it struck — and `vfx::impact` branches on those two axes and nothing else. It does not know how far away the camera is, and it must never learn.

The 88 (`caliber: 0.088`) on **terrain** lands the full soil-strike splash: a contact flash, 8–12 dirt ejecta streaks, 1–2 tall dirt plumes, a low dust ring, a lingering brown haze (3.0 s), and a flat disturbed-earth ground scar. The same round on **armor** lands a categorically different read — white-hot contact flash, a dense fast spark fan, a small gray spall puff, plus a flame lick *only* when it bit into the steel (`Impact.penetrated`) — and **no plume, no dust ring, no ground scar, no brown cloud**, because armor is not dirt and steel is not gouged like soil. The MG (`caliber: 0.0079`, under `TRACER_MAX_CALIBER`) keeps its compact read on both.

The plume's numbers come from period gun-camera and range footage of large-calibre AP soil strikes: **dirt fountains in the ~4–10 m band with ~1.5–2 s hang**. We ship the middle of that band — `SPLASH_PLUME_SIZE.1` 5.5 m × `SPLASH_PLUME_ASPECT.y` 1.4 ≈ **7.7 m**, rising 4.5 m/s over 1.8 s. Mid-band, honest, **not scaled to screen or range**. The provenance of every constant is the reference, and the code says so where a future reader would otherwise reach for a distance term.

## What was rejected, and the arithmetic that was actually wrong

The recommendation was a readability target: make the column tall enough to subtend ~10 px at 1000 m, on the grounds that a realistic ~1.5 m puff subtends barely a pixel. The owner rejected readability-tuning outright, and that decision stands on its own — but the arithmetic behind the recommendation was **also** computed against the wrong view, and the correction is worth recording because it removes the temptation to relitigate.

Angular size does not care about the effect; it cares about the optic. Our gunner sight is authored at a **0.12 rad vertical FOV** (`tiger_1.tank.ron`, `views.Gunner` — ≈ 6× magnification; the commander view is 0.785 rad). At 1000 m, on a 1080-px-tall viewport:

| | commander (0.785 rad) | **gunner optic (0.12 rad)** |
|---|---|---|
| a 1.5 m puff | ~2 px | **~13 px** |
| our honest 7.7 m plume | ~11 px | **~69 px** |

The "sub-pixel at 1 km" figure holds only for an *unmagnified* view. In the view a gunner actually spots fall-of-shot from, the honest plume is already an unmistakable ~70 px, and even a small 1.5 m puff clears the threshold the research was chasing. **The readability the exaggeration was meant to buy is bought by the optic and by accurate scale.** There was never a trade to make.

## Honest means are still allowed — the rule is against *inventing*, not against *reading well*

Real large-calibre soil strikes DO throw dirt several metres up and hang for ~2 s; rendering that is not assistance, it is accuracy. The tracer ember is the sharpest case: the 88's Pzgr. 39 was APCBC-HE-**T** — a ~13 g base tracer burning ~2 s to ~1500 m, and it existed *specifically* as the gunner's fall-of-shot aid. So `vfx::ember` puts a glowing point at the shell's base, burns it for `EMBER_BURN` 2.0 s and fades it, and that is a documented historical fact rendered faithfully, not a gameplay affordance bolted on. (It is deliberately a point at the *base*, never a whole-shell glow — the "glowing telephone-pole" failure mode is itself a form of dishonesty.)

The line: **readability won through accuracy is fine; readability won through exaggeration is not.** When the real thing helps the player, render the real thing and take the help.

## The corollary that already bit us: an effect must not assert what the authority never resolved

Honesty is not only about scale. An effect is a claim about the world, and the view layer may not make a claim the sim never sanctioned.

[[0021-fire-replication-architecture]] hit this directly. When a net client's shell contacts armor but no server verdict ever arrives, the pre-slice fallback drew a **neutral spark** at the client-computed contact point. But a lost verdict and a pose-divergent *miss* are indistinguishable to the client — and in the miss case that spark asserts a contact the authority never resolved: fabricated geometry, fabricated event. So the shell now **dissolves quietly** — despawned, its trail simply stopping, no spark (`ballistics`, the F3(ii) decision; `observer_hold_expires_to_quiet_dissolve` and `own_shell_keyframe_lost_dissolves_quietly` pin it). Correctness never depended on delivery (0021's invariant 3), so silence is the honest degradation and a spark is a lie.

Same doctrine, second axis. Do not invent the *magnitude* of an event, and do not invent the *event*.

## Consequences, named not buried

- **Fall-of-shot at long range is genuinely hard.** That is accepted gameplay, not a UX bug, and it is the intended texture of the gunnery loop this game is built around. The player's aids are the ones the crew actually had: the optic's magnification, the tracer ember, the range dial. If long-range spotting is later judged *too* hard, the legitimate levers are the optic and the reference (is the plume actually mid-band? is the ember bright enough to bloom?) — never a distance term.
- **Effects may be tuned; tuning must not become a back door.** Sprite count, erosion, alpha, LUT — all fair game, and all tuned. But a knob whose *input* is camera distance or screen-space size is not a tuning knob, it is this decision being reversed quietly. There is no such knob today; adding one requires reversing this ADR in the open.
- **Symmetry is a free consequence.** Because nothing scales by viewer, the shooter, the victim and a spectator all see the same impact at the same size. No effect exists that is bigger for the person who benefits from seeing it.

## What this ADR does not say

It does not forbid *view-layer* effects that have no sim counterpart — the view layer is allowed to be cosmetic ([[0014-sim-view-split]]); recoil kick, camera shake, bloom and the smoke ribbon are all pure presentation, and none of them fabricate an event or misreport a magnitude.

It does not claim the current constants are final. They are the current best reading of the reference. Move them by bringing better reference, not by bringing a screenshot of how small they look.

It is not a performance doctrine. Eviction rings, sprite caps and the independent `GROUND_MARK_CAP` bound the cost of an impact storm; culling an effect that is off-screen or budget-evicting an old one is not viewer-scaling, because it changes *whether* we can afford to draw, not *what the effect claims about the world*.

## Related decisions

[[0014-sim-view-split]] — the sim/view split is what makes this decision *possible* to state as a law: with truth and presentation on separate planes, "the view may not invent" is enforceable rather than aspirational. This ADR is the view plane's design law. · [[0021-fire-replication-architecture]] — the quiet dissolve (invariant 2, the fail-closed/keyframe composition) is this doctrine applied to the *existence* of an effect rather than its size; 0021 already cites the honesty doctrine by name, and this is the ADR it was citing. · [[0020-fire-mode-mechanism-enum]] — caliber and fire mode are authored sim facts, and the impact read branches on them; the effect asks the physics what happened, and asks nothing else.
