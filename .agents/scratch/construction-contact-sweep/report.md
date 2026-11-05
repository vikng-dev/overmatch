# Sweep checkpoint

Subject: the video game's environment destruction model and server work; game state only.
Branch: `lane/sweep`. The design narrative lives in
`.agents/docs/design/construction-destruction.md`, “Bodies in contact”.

## Status

The prescribed per-chunk force criterion is implemented, but **the tracked ram acceptance gate
fails**. This is a checkpoint for review, not an accepted ram result. No asset, material strength,
attachment value or drive setting was changed to obtain an outcome.

## Evidence

All quantities in this table are MEASURED by the named tests, except the explicitly DERIVED row.

| Witness | Result |
|---|---|
| Focused sweep tests | Six pass: translated/oblique occupancy, rotating occupancy, slow/strong hold, linear plastic exchange, off-centre angular/energy exchange, and the production pre-physics/ledger/cancellation composition. |
| Production cube fixture | One swept chunk, one candidate cast, two incident-joint visits. Source speed 128 -> 127.764275 m/s; exchanged momentum approximately 235725.08 N s. The deliberately staged obsolete belt reaction is cancelled. |
| DERIVED hand account | A 100 kg body at (2, 0, 3) m/s sharing with three centred 10 kg chunks leaves at (1.5384615, 0, 2.3076923) m/s. Initial/final kinetic energies are 650/500 J; plastic loss is 150 J. |
| Shipped recipe target at rest | 1122 chunks, 3258 joints, zero cracked joints, zero absent chunks. This uses the existing initial equilibrium, before the parts lane merges. |
| Ram first occupied contact | Driving tick 1134, incoming speed 10.885337 m/s; the recipe's minimum is 9.983797 m/s. Twelve intersected chunks are tested. |
| Ram demand/capacity | Required mean forces 5792.43–20697.13 N; corresponding intact capacities 16904.19–46326.79 N. Every chunk holds. |
| Ram release | Zero swept chunks through the contact tick and the next two ticks. The source then slows against the remaining solver contacts. The acceptance assertion fails. |
| Ram work over those three ticks | 18060 bounds checks, 6622 candidate chunk casts, 98 incident-joint visits, 6645 calls into cast/contact query APIs. Internal engine GJK operations are not counted. |

The force criterion uses all intact incident joints of each intersected chunk. The strongest
first-contact demand/capacity ratio is DERIVED approximately 0.566: 15150.466 / 26759.186.
Under that criterion the asset must hold; lowering strength to force a breach would contradict
the data. A follow-up must resolve whether the intended load path is the moving patch's cut set:
connections between chunks moving together have no relative motion. That is an unimplemented
law question, not a measured validation of another model.

The operation count also exposes a cost limitation: any nonzero spin currently admits a whole
rotation-radius bound before exact casts. It is conservative but much wider than a small-angle
sweep. These counts do not establish work proportional to the exact swept-volume membership;
tight angular-travel bounds remain unfinished. No timing or capture was taken.

## Removed and retained

Removed: contact rest-speed splitting and capped body blows; source-mass shares; body pulse-work
adapters; give-back; cross-owner return batches; delayed launch capping and its pending-launch
plugin; and their exclusive tests. Remaining solver contacts supply whole mean forces to the
structure check. Field impulses retain their existing fracture-work and occupied-body launch.

The departure boundary is `breakage::sweep_intake::sweep_departure`: stable construction/chunk
identity, world velocity/spin and tick. It still routes through the existing server debris body
path until decor replaces that routing. Its per-chunk live-state copying and collider creation
are additional costs, not included in the sweep counters.

## Merge seams

- `construction.rs`: registers the new law module and builds its immutable geometry cache from admitted data.
- `breakage.rs`: adds the pre-physics hook/reconciliation, explicit changed-owner intake, whole contact-force conversion, and removes body-return plumbing and launch-finalization ordering.
- `fracture.rs`: removes the retired body origin/share and simplifies field-work debit accordingly; retain parts' replacement structural calculation.
- `contact_transfer.rs`: removes split/mass-budget readers and adds cancellation of staged forces on retired colliders.
- `debris/launch.rs`: deletion of retired contact exchange bookkeeping only; occupied mass and field launch remain. Reconcile this deletion with decor's routing changes.
- Track and headless callers: tests now read whole forces. `net::ram` exposes its real recipe parser/planner only under tests; no wire type changes.

## Gates

Formatting and CI workspace/all-target clippy pass on the checkpoint Rust source.
The focused tests pass. The explicit ram acceptance test fails as reported above.
MEASURED broader module/caller run: 285 passed, eight ignored, with one boundary failure:
the headless test named the operator module. The witness now lives under `net::ram::tests`
and reuses the headless host through test-only helper visibility. No boundary allowlist was widened.
MEASURED final post-correction run: 59 passed, one ignored, zero failed, including the boundary,
documentation, UI and bake guards, recipe tests and all focused sweep tests.
The ram witness remains an explicit on-demand acceptance failure; its relocated body is unchanged.
Raw logs are retained under `target/sweep-{focused,ram,clippy,gates,final}.log`.
DERIVED overall Rust diff removes 1805 lines, including the retired contact mechanics and their tests.
