# Current upstream drafts

Local proposals against the pinned published crates. Nothing here has been sent.
Every embedded patch is checked against the pristine release; evidence from an older release is
labelled as historical measurement, never the current pin.

Lines to retire are MEASURED added + removed source lines from `diff -ru` against pristine,
including fork-only source files. Counts cover the retained functional patch, including its
comments. Diagnostic-only deltas remain separate: broad phase 18, moved bounds 13, size metrics
16; the batch ancestry re-export's 2 lines are also outside these proposals.
The identity-only proposal is a subset of the combined nearest-contact fix, so it has no separate
retirement count. Both fixes must land to retire that combined patch. Zero means the application
already uses a public seam and has no crate patch to retire.

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
| [avian-unknown-contact-feature-identity.md](avian-unknown-contact-feature-identity.md) | avian3d 0.8.0 | bug fix | part of 93 below | not sent |
| [avian-nearest-warm-start-contact.md](avian-nearest-warm-start-contact.md) | avian3d 0.8.0 | bug fix | 93 | not sent |
| [avian-island-rest-history.md](avian-island-rest-history.md) | avian3d 0.8.0 | bug fix | 22 | not sent |
| [bevy-invisible-mesh-removal-churn.md](bevy-invisible-mesh-removal-churn.md) | bevy_pbr 0.20.1 | bug fix | 10 | not sent |
| [bevy-mesh-removal-channel.md](bevy-mesh-removal-channel.md) | bevy_pbr 0.20.1 | bug fix | 37 | not sent |
| [lightyear-netcode-denied-reason.md](lightyear-netcode-denied-reason.md) | lightyear_netcode 0.31.0 | bug fix | 12 | not sent |
| [lightyear-netcode-admission-recheck.md](lightyear-netcode-admission-recheck.md) | lightyear_netcode 0.31.0 | bug fix | 11 | not sent |
| [lightyear-avian-child-collider-pose-churn.md](lightyear-avian-child-collider-pose-churn.md) | lightyear_avian3d 0.31.0 | bug fix | 0 | not sent |

Historical mechanism pointers beside these drafts keep current source citations usable. Their
investigations live under `.agents/scratch/fork-moves-history-2026-11-16/`; they are outside this
proposal queue. Obsolete proposals without a current source or patch-record consumer were removed.

Current complete fork deltas, MEASURED again against the cached published archives with
`diff -ruN src/` (added + removed lines, fork-only source included): avian3d **729**,
bevy_pbr **357**, lightyear_netcode **159**, lightyear_avian3d **0**; total **1245**.
The per-proposal rows above exclude game divergences and the diagnostic-only lines listed here.
