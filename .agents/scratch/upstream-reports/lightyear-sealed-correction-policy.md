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

## What fixing this unlocks for us

**Nothing — and this one should be filed saying so.** `net/render_error.rs` is not scaffolding waiting
for an upstream fix; ADR-0015 classifies it as *"permanent-but-looks-like-scaffolding"*: multiplayer
reintroduces legitimate mispredictions forever (you cannot predict the other tank's input), and this
layer is how *any* correction is presented. A `CorrectionPolicy` builder would not retire a single line
of it, and we would not move the smoothing back into lightyear if it did exist — `instant_correction()`
plus a render-space error layer is the sim/view split (ADR-0014), i.e. the arrangement we would choose
anyway.

The honest content of the report is therefore about **API shape, not about us**: the gap forced a design
that turned out right, which is luck, not ergonomics. The only thing a builder would buy us is a cheap
A/B — lightyear-side adaptive decay vs our render-error layer — that we currently cannot run without
rewriting either side. Marginal, and speculative: we have no complaint the layer does not already answer.
Everything else this fix buys goes to games that want a Fiedler-style adaptive decay with a
correction-velocity cap and no view layer of their own.
