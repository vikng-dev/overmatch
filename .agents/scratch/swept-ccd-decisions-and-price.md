# Swept CCD decisions and repeated release price

Subject: the multiplayer tank game's rigid-body driving physics and environment destruction
model; game state only.

Continue `lane/swept-ccd` on top of `602c0a57`. This report supersedes the decisions in
[swept-ccd.md](swept-ccd.md) and [swept-ccd-sensors-and-price.md](swept-ccd-sensors-and-price.md);
their observations remain historical evidence.

## Per-collider decision

The per-collider gate lands. The previous AABB probe's 24.15 / 55.40 mm peaks at 8 / 10 m/s
were ordinary first-step contact: DERIVED at 64 Hz, travel past the fixture's 0.1 m gap is
25 / 56.25 mm. The earlier zero peaks came from thin track-side slices selecting the unrelated
hull for CCD. They were not an engine obligation.

The body early-out remains. Each supported collider then compares the conservative body motion
bound with its own thickness before mass properties, swept bounds or trees. Reusing the body's
bound includes offset rotation and preserves upstream single-collider eligibility. No default,
threshold or speculative-contact law changes.

## Verification record

MEASURED: all five current CCD tests pass. Each mutant below was applied alone, run through
`scripts/serial-cargo test --lib --features dev_ui,bitprobe <filter> -- --nocapture`, observed red
(exit 101), then restored. No mutant remains in shipping source.

| Mutant | Test filter | Failure |
|---|---|---|
| Remove the per-collider gate | `a_thin_child_does_not_force` | Thick child clamped at its wall |
| Skip all moving colliders after support selection | `a_fast_thin_child_and_a_single_slab` | Thin single collider crossed its wall |
| Replace the collider motion bound with COM translation | `an_offset_rotating_child` | Offset child rotated through its wall |

Logs: `/private/tmp/ccd4-mutant-{no-collider-gate,no-thin-sweeps,translation-only}.log`;
structured exit results: `/private/tmp/ccd4-mutants.json`. The unmutated run is
`/private/tmp/ccd4-green.log`.

MEASURED: both independent published-crate drafts compile offline, with unchanged published
manifests and small path-dependency harnesses (`default-features=false`, `3d,parry-f32`).
Both diffs dry-apply cleanly with `patch -p1` to a pristine extraction. Commands and archive
SHA-256 are in the drafts. Logs: `/private/tmp/ccd4-{per-collider,sensor}-draft.log`.
The sensor observer/thickness proposal is compile-checked only; it is not a shipped runtime claim.

At the per-collider batch boundary, MEASURED source inventory against published 0.8.0:
890 added + 236 removed = 1,126 lines, including 78 in the fork-only file; CCD 14+2=16,
metrics 98+27=125. Subsequent batches remove the thickness change and remeasure the merged tree.

Each commit runs formatting, CI clippy and the affected CCD/metrics tests. Probe/track tests
run when those sites change. Independent landing review remains the orchestrator's gate.

MEASURED per-collider commit gates: `cargo fmt --all --check`; CI clippy with
`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`; targeted
`avian_swept_ccd` (5 passed) and `avian_body_size_metrics` (4 passed), all through
`scripts/serial-cargo`, plus `git diff --check`. Log: `/private/tmp/ccd4-d1-gates.log`.
