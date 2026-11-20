# Freeze reach and collision masks

Subject: the game's building-debris model (pieces, joints and loads).

Invariant: freeze/thaw reach admits a collider pair only when both collision masks permit
contact; the authoritative debris settling system relies on this to avoid phantom reach.

## Baseline witness

Started from main commit `7b046e22cf48ee2fa77c34a6e13e593f44e5d6e5`, adding only the regression to
`src/debris/freeze.rs` before changing production code. Ran:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe a_mover_without_mutual_contact_permission_does_not_thaw_a_piece
```

MEASURED result, exit 101:

```text
test debris::freeze::tests::a_mover_without_mutual_contact_permission_does_not_thaw_a_piece ... FAILED
empty source filter, update 0: a non-contacting mover must leave the piece frozen
test result: FAILED. 0 passed; 1 failed
```

The preceding query-only sensor case passed. The cited collider in
`construction::breakage::spawn_prepared_as` is a sensor as well as having an empty contact filter, so
that specific shipped pattern was already excluded. The failing non-sensor mover made no Avian
contact pair but thawed the piece. Support is read separately from constraint-producing contacts;
this witness demonstrates false reach, not false support.

The test mounts the shipped `debris::sim_plugin` through `settling`, uses anonymous cube/slab/mover
fixtures, and covers unilateral refusal in either direction against the shipped piece layers, each
by a mover whose second, permitting collider sits out of reach. The empty-mask cases were deleted:
`interacts_with` refuses them with or without the early skip, so they killed no mutant. The
existing positive reach test guards waking before permitted contact.

MEASURED, once, under a temporary runtime mutant switch in `Reach::of` (since removed), with each
case run to its own verdict: gate reverted, both cases red (`update 0: a non-contacting mover must
leave the piece frozen`); body-pooled masks, both red; only source filter against target
membership, "target refuses source" red; only the reverse, "source refuses target" red; unmutated,
both green.

## Change and behaviour

Rewritten against the baseline, without cherry-picking. Reach carries each source collider's
layers beside its travel bounds, rejects empty source masks before querying, and requires mutual
permission before admitting a hit. The census fixture enables contact after its contact-free
setup, preserving its timer and population exclusions.

Verified against vendored Avian source: `collision/collider/backend.rs` requires layers;
`collision/collider/layers.rs::interacts_with` checks both directions;
`collision/broad_phase/bvh_broad_phase.rs` applies that gate; the AABB callback in
`spatial_query/system_param.rs` performs no layer filtering.

DERIVED patch size: production +21/-10 lines; tests +84/-0 lines, including the census fixture.
No wire types change; `PROTOCOL_REV` remains 107.

No rendered game session or timing measurement was performed.
DERIVED effect on today's game: track side plates, which production masks exclude from debris,
can no longer supply phantom reach where their bounds alone reach a piece. Eligible debris may
freeze or remain frozen there; genuine hull and soft-run contacts retain their reach. The cited
query-only sensors' behaviour is unchanged.

## Validation

MEASURED: all commands below exited 0; the targeted run passed all 14 freeze tests, including the
new regression and the existing permitted-contact, support and deterministic-order witnesses.

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe debris::freeze::tests
cargo fmt --all --check
scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings
scripts/serial-cargo check --no-default-features --bin overmatch-server --features dev_ui,bitprobe
```

The no-audio server check reported dead-code warnings for audio fields in unchanged
`src/tank/model.rs`. Both test builds reported the macOS linker's unwind-table size warning.
Neither warning prevented the commands from completing.

Independent read-only reviews covered all six lenses. The sole report finding, a repeated
baseline-observation sentence under brevity, was removed and the reviewer confirmed it closed.
