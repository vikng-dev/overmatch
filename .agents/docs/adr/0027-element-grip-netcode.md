# Element grip converges through local rollback plus authoritative checkpoints

At DECLARED `PROTOCOL_REV = 16`, the networked element law keeps the complete
`TrackGripElements` field as root-resident simulation state. The authority constructs it
synchronously from tank data at spawn. Its exact current value crosses to the owning client once in
the initialization snapshot; after that, prediction rewinds local history and authoritative repair
arrives through sparse exact checkpoints. The field is never reconstructed from an asset, attached
late to an already-replicated authority entity, or replicated every tick.

This settles the hybrid from `element-netcode-design.md`: a cheap authoritative effect anchor finds
meaningful divergence, while exact checkpoints provide the missing state required for correction.
The aggregate `TrackGrip` component remains available to the offline compatibility law and as
derived telemetry in element mode, but it is not network authority and stays off the wire.

## Authority and repair

Every completed authority tick publishes `NetTrackGripAnchor`: total traction force, traction
torque about the hull center of mass, both belt reactions, a coarse field digest, its producing tick,
and the current rest epoch. The effect values are compared with the owner's locally historied
`TrackGripEffect`. An anchor produced by completed tick `N` keeps label `N`, matching Lightyear's
end-of-tick prediction-history label; if it arrives before the owner has completed `N`, comparison
waits. This is distinct from the entering-next-tick checkpoint convention below. The digest is
evidence for requesting bytes; it never causes rollback by itself.

The authority captures the current exact field into owner-private, unordered-reliable
`GripCheckpointChunk` messages when a side enters a stable non-turnover/rest state, when an admitted
`GripResyncRequest` asks for current truth, or at the moving fallback cadence. That cadence is
DERIVED 256 ticks at the MEASURED 64 Hz configuration (DERIVED 4 s) and remains until Phase-4
multiplayer evidence supports removing it. Rest classification and moving digest persistence derive
from the force law's single contact-loss dwell. Commands, contact-topology changes, and explicit
hull impulses advance wake/rest bookkeeping without gating the local force law.

Chunks address the tank by stable `CombatantId`, then carry explicit side/element identity, exact raw
`f32` strain values, contact-generation state, and a whole-checkpoint hash. They never use mapped ECS
entity identity: a client defers a chunk until that combatant resolves to exactly one local controlled
replica. The defer queue is bounded at DERIVED 256 chunks and expires unresolved chunks with a warning
after DERIVED 128 ticks (DERIVED 2 s at the MEASURED 64 Hz configuration). Assembly is bounded,
validates every chunk, and exposes no partial field. A request uses the same stable identity and is
advisory about the client's observed epoch; the authority resolves ownership, admits and rate-limits
against its own current epoch, and always captures fresh state.

The strain validator accepts the producer's complete numerical domain without rewriting authority
bytes. Its DERIVED squared-length bound accounts for binary32 rounding in the producer's three-term
length dot, square root, division, per-component rescale, and the validator's three-term dot. With
unit roundoff `u` and `gamma_3 = 3u / (1 - 3u)`, the bound is
`K^2 * (1 + gamma_3)/(1 - gamma_3) * (1 + u)^4/(1 - u)^2`, rounded outward once. Non-finite or truly
out-of-domain vectors are rejected with side, element, axes, measured squared length, and bound.

## Correction semantics

A checkpoint names the fixed tick whose entry state it represents. For state-entering tick `T`,
the client waits for ordinary confirmed history through baseline `B = T - 1`, stages the validated
checkpoint outside rollback state, and requests forced rollback to `B`. After Lightyear restores
ordinary history but before replay starts, the client installs the field and replaces the local
history entry at the baseline. Replay's first tick is therefore `T`, which consumes the corrected
field. A stale moving checkpoint outside retained history is rejected in favor of a fresh request.

Effect thresholds may request a checkpoint, and the arrival of a new exact epoch may cause the
forced rollback. Neither an aggregate mismatch nor a digest mismatch may trigger a correction-free
rollback: without authoritative field bytes, replay can only resurrect the same divergent history.

Before forcing rollback, the client compares a fully assembled checkpoint with the retained exact
field history at rollback baseline `B` by raw float bits and dwell bytes. The live field is not the
comparison target: it may have legitimately evolved after `B`. If the baseline and checkpoint are
identical, the checkpoint is a completed no-op repair: no rollback fires, and request evidence for
that `(tank, epoch, digest)` is SPENT until epoch or digest changes. Repeating a field checkpoint for
unchanged evidence is protocol-invalid because it cannot repair a non-field effect/history
discrepancy. `SPIKE_TRACE` records every anchor comparison and request with both effects and computed
error metrics, and records whether checkpoint application changed any field bit, so the remaining
effect divergence can be classified from one capture.

## Evidence and consequences

- Replay/parity gates compare canonical element hashes and the full simulation state bit-for-bit
  across flat, slope, neutral-steer, phase-wrap, and correction cases.
- Join-in-progress has DERIVED three adversarial first-force-tick fixtures plus a real loopback-UDP
  replicate-once gate; the replicated root does not attach its sim body until the exact, correctly
  sized field and complete transmission state are present.
- Checkpoint codec, unordered assembly, validation, admission, rest/wake, rollback-install, and
  loss/reordering behavior have focused batteries. Batch D's MEASURED cumulative-belt-phase curves
  concluded that moving fields self-heal; they do not yet justify removing the periodic multiplayer
  fallback.
- Exact local storage and checkpoints preserve raw float state. Quantized per-tick replication and
  macro reconstruction remain rejected because neither is the authority state this force law
  actually evolves.

## Related

[[0014-sim-view-split]] · [[0015-divergence-doctrine]] ·
[[0016-replicate-causes-derive-consequences]] ·
[[0018-wire-surface-fingerprinted-and-refused]] ·
[[0026-static-friction-strain-regime]]
