# Current upstream drafts

Local proposals against the pinned published crates. Nothing here has been sent.
Every embedded patch is checked against the pristine release; evidence from an older release is
labelled as historical measurement, never the current pin.

Lines to retire are MEASURED added + removed source lines from `diff --minimal -ruN` against pristine,
including fork-only source files. Counts cover the retained functional patch, including its
comments. Diagnostic-only deltas remain separate: broad phase 18, moved bounds 13, size metrics
16; the batch ancestry re-export's 2 lines are also outside these proposals.
The known-ID nearest-contact ranking is game policy, served by the fork's `ContactMatcher` hook;
only the identity correctness fix is proposed upstream. Zero means the application already uses
a public seam and has no crate patch to retire.

| Draft | Crate | Kind | Fork lines it would retire (MEASURED) | Status |
|---|---|---|---:|---|
| [avian-child-collider-pose-churn.md](avian-child-collider-pose-churn.md) | avian3d 0.8.0 | public scheduling seam | 6 | not sent |
| [avian-collider-propagation-floor-counts-roots.md](avian-collider-propagation-floor-counts-roots.md) | avian3d 0.8.0 | bug fix | 6 | not sent |
| [avian-collider-of-reinsert.md](avian-collider-of-reinsert.md) | avian3d 0.8.0 | bug fix | 23 | not sent |
| [avian-mass-recompute-redundant-work.md](avian-mass-recompute-redundant-work.md) | avian3d 0.8.0 | bug fix | 41 | not sent |
| [avian-pairless-broad-phase.md](avian-pairless-broad-phase.md) | avian3d 0.8.0 | bug fix | 10 | not sent |
| [avian-sleeping-velocity-increments.md](avian-sleeping-velocity-increments.md) | avian3d 0.8.0 | bug fix | 75 | not sent |
| [avian-moved-aabb-prune.md](avian-moved-aabb-prune.md) | avian3d 0.8.0 | bug fix | 59 | not sent |
| [avian-body-size-metrics-once-per-body.md](avian-body-size-metrics-once-per-body.md) | avian3d 0.8.0 | bug fix | 45 | not sent |
| [avian-prune-points-unsigned-depth.md](avian-prune-points-unsigned-depth.md) | avian3d 0.8.0 | bug fix | 156 | not sent |
| [avian-unknown-contact-feature-identity.md](avian-unknown-contact-feature-identity.md) | avian3d 0.8.0 | bug fix | 26 | not sent |
| [avian-island-rest-history.md](avian-island-rest-history.md) | avian3d 0.8.0 | bug fix | 23 | not sent |
| [bevy-invisible-mesh-removal-churn.md](bevy-invisible-mesh-removal-churn.md) | bevy_pbr 0.20.1 | bug fix | 10 | not sent |
| [bevy-mesh-removal-channel.md](bevy-mesh-removal-channel.md) | bevy_pbr 0.20.1 | bug fix | 37 | not sent |
| [bevy-directional-shadow-factor-per-light.md](bevy-directional-shadow-factor-per-light.md) | bevy_pbr 0.20.1 | API gap | 0 | not sent |
| [lightyear-netcode-denied-reason.md](lightyear-netcode-denied-reason.md) | lightyear_netcode 0.31.0 | bug fix | 12 | not sent |
| [lightyear-netcode-admission-recheck.md](lightyear-netcode-admission-recheck.md) | lightyear_netcode 0.31.0 | bug fix | 11 | not sent |
| [lightyear-avian-child-collider-pose-churn.md](lightyear-avian-child-collider-pose-churn.md) | lightyear_avian3d 0.31.0 | bug fix | 0 | not sent |
| [avian-ccd-sensors-swept-and-counted.md](avian-ccd-sensors-swept-and-counted.md) | avian3d 0.8.0 | bug fix | 7 | not sent; query ships, observer/thickness proposal compile-only |
| [avian-ccd-decide-per-collider.md](avian-ccd-decide-per-collider.md) | avian3d 0.8.0 | bug fix | 15 | not sent; includes the small lazy-shape patch |

MEASURED index census: 19 current draft rows, 13 targeting Avian.

Historical mechanism pointers beside these drafts keep current source citations usable. Their
investigations live under `.agents/scratch/fork-moves-history-2026-11-16/`; they are outside this
proposal queue. Obsolete proposals without a current source or patch-record consumer were removed.

Current complete fork deltas, MEASURED again against the cached published archives with
`diff --minimal -ruN src/` (added + removed lines, fork-only source included): avian3d **871**,
bevy_pbr **47**, lightyear_netcode **159**, lightyear_avian3d **0**; total **1077**.
The per-proposal rows above exclude game divergences and the diagnostic-only lines listed here.

## Avian draft compile checks

MEASURED: each patch below was applied independently to a fresh extraction of the cached
published avian3d 0.8.0 crate, then its library compiled offline through `scripts/serial-cargo
check`, with all default features (including 3d), except the CCD drafts, which use
`3d,parry-f32` with default features disabled. A tiny path-dependency harness leaves the
published manifest unchanged and excludes example-only dev dependencies: direct crate-root
checks cannot resolve the uncached `bevy_mod_debugdump 0.17` offline. These are library compile
checks, not example builds or upstream test runs.

| Draft | Library compile result |
|---|---|
| avian-body-size-metrics-once-per-body.md | compiled |
| avian-child-collider-pose-churn.md | compiled |
| avian-collider-of-reinsert.md | compiled |
| avian-collider-propagation-floor-counts-roots.md | compiled |
| avian-island-rest-history.md | compiled |
| avian-mass-recompute-redundant-work.md | compiled |
| avian-moved-aabb-prune.md | compiled |
| avian-pairless-broad-phase.md | compiled |
| avian-sleeping-velocity-increments.md | compiled |
| avian-unknown-contact-feature-identity.md | compiled |
| avian-ccd-sensors-swept-and-counted.md | compiled (`3d,parry-f32`); thickness proposal compile-only |
| avian-ccd-decide-per-collider.md | compiled (`3d,parry-f32`), including lazy shape |

`avian-nearest-warm-start-contact.md` records game policy and contains no upstream patch.
