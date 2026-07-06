# lightyear 0.28: CorrectionPolicy is sealed (private fields, no builder)

**Target:** lightyear 0.28 · **Severity for us:** LOW (API ergonomics; bypassed) · **Status:** unfiled

## Suggested title

CorrectionPolicy cannot be customized: private fields, no constructor beyond presets

## Mechanism

`CorrectionPolicy` (prediction correction decay tuning) exposes only preset constructors; its
fields are private and there is no builder. A game that wants, e.g., an error-magnitude-adaptive
decay with a correction-velocity cap (to bound how fast corrections move the rendered pose)
cannot express it — the choice is between the presets' fixed exponential decays or
`instant_correction()`.

## Consequence for us

We wanted Fiedler-style adaptive decay (retain 0.95/frame at ≤0.25 m blending to 0.85/frame at
≥1 m) under a 3 m/s / 120 deg/s correction-velocity cap, with a hard teleport threshold. Not
expressible → we set `instant_correction()` (the sim snaps in one frame) and built the smoothing
entirely outside lightyear as a render-space error layer (src/net/render_error.rs, commit
597ec21). That turned out to be the better architecture anyway (sim/view split — the sim is
always tick-truth), but the API gap forced the design rather than allowing it.

## Suggested upstream fix

A builder (decay function or per-magnitude curve, velocity cap, snap threshold) — or simply
public fields. Alternatively, document `instant_correction()` + user-side render-offset
smoothing as the intended extension point; it composes well with FrameInterpolation.

## Our workaround

Permanent (render-error layer is now core architecture, ADR-0014/0015). Report is a suggestion,
not a request we depend on.
