# Element-law netcode design (codex, 2026-07-18) — ACCEPTED direction for the promotion arc


# Netcode shape for per-element track grip state

## As built at REV-14/15

The REV-15 game path now runs the per-element law; the sandbox-only finding below is historical.
`TrackGripElements` is constructed synchronously from tank data at authority spawn, seeds an owner's
join-in-progress root through replicate-once initialization, and then rewinds through local rollback
history. The server publishes a per-tick wrench/digest anchor and sends exact sparse checkpoints on
rest entry, explicit request, and a DERIVED 256-tick fallback cadence at the MEASURED 64 Hz
configuration (DERIVED 4 s). Validated checkpoints install at the state-entering tick through forced
rollback; a digest mismatch alone never rolls back.

`TrackGrip` kept its name because it still serves the aggregate offline compatibility path and is
derived telemetry in element mode; it left the wire at REV 15. The join-in-progress ordering has
both direct first-force-tick coverage and a real loopback-UDP replicate-once gate. Batch D's
MEASURED cumulative-belt-phase curves concluded that moving fields self-heal, but the periodic
checkpoint remains until Phase-4 multiplayer evidence justifies removing it. The settled contract
is [[0027-element-grip-netcode]]; future-tense statements below are the historical design record and
are superseded where they conflict with this section or that ADR.

## Decision

Recommend a hybrid:

1. Keep the complete per-element strain field as root-resident local rollback state.
2. Replicate a cheap authoritative effect anchor containing the traction wrench, belt reactions, and a coarse field digest—not only the four aggregate force sums.
3. Send exact, sparse per-element checkpoints:

   - in the initial replication snapshot / join-in-progress;
   - whenever a side becomes non-healing, especially when parked;
   - on wake-worthy contact topology changes;
   - on client request after a persistent anchor mismatch;
   - optionally at a slow moving cadence until measurements prove request-only repair is sufficient.

This closes the parked-tank hole that disqualified aggregate `local_rollback`, preserves ordinary rollback correctness, and avoids continuous field replication.

This is not mathematically equivalent to authoritative per-tick field replication. If the invariant is strengthened to “every correction must begin from the server’s exact element field at that tick,” then structured per-tick replication is the only honest answer. A macro summary cannot recover the field’s unobservable spatial modes.

## Repository findings

Historical finding, SUPERSEDED at REV 15: the shipped law integrated one aggregate `TrackGrip`
resultant and distributed it in load proportion, while the prototype carried world-space `Vec3`
values keyed by material link and column and remained sandbox-only in the game adapter
([forces.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/forces.rs:106),
[sim.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/sim.rs:257)). The as-built
game path now uses that element field.

Two prototype details need resolution before promotion:

- The brief budgets two floats per element, but the live prototype uses three. DERIVED: promoted unchanged, the raw field is `2 × 97 × 3 × 3 × 4 = 6,984` bytes, not `4,656` bytes.
- The current code resets every element not touched in the current tick ([forces.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/forces.rs:462)). Because contacts are omitted when damped `load <= 0`, one separating/noisy tick can erase grip even while elastic penetration remains. That is unsafe for parking.

Lightyear is pinned to version `0.28` ([Cargo.toml](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/Cargo.toml:41)). Its local rollback mechanism attaches a `PredictionHistory<C>` and records changed values in `FixedPostUpdate` ([plugin.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/plugin.rs:74)). During state rollback, a local-only component without confirmed history is restored from that local history ([rollback.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/rollback.rs:911)). This is exactly the required temporal rewind mechanism.

## Cost model

All bandwidth figures below are DERIVED application payload before entity/component headers, netcode framing, acknowledgements, loss, or retransmission.

| Representation | Snapshot payload | Payload at the measured 64 Hz configuration |
|---|---:|---:|
| Full two-axis `f32` field | 4,656 B | 291 KiB/s |
| Current three-axis prototype | 6,984 B | 436.5 KiB/s |
| Current four-float aggregate | 16 B | 1 KiB/s |
| Sparse exact two-axis `f32`, with explicit occupancy | about 1,289–1,529 B | about 80.6–95.6 KiB/s |
| Sparse signed 16-bit axes, with occupancy | about 689–809 B | about 43.1–50.6 KiB/s |
| Sparse packed 12-bit axes, with occupancy | about 539–629 B | about 33.7–39.3 KiB/s |
| Sparse signed 8-bit axes, with occupancy | about 389–449 B | about 24.3–28.1 KiB/s |

The sparse figures use the brief’s MEASURED estimate of 25–30 grounded stations per side, three columns, and an explicit 582-bit occupancy map. Explicit identity is worth its roughly 73-byte cost: applying bytes according to a contact set re-derived from corrected pose and phase risks assigning strains to the wrong material links whenever the client and server disagree at a contact threshold.

DERIVED rollback memory for the two-axis field at the brief’s maximum 20-tick window is at least:

`4,656 × 20 = 93,120 B = 90.9 KiB` per predicted tank.

The live `Vec3` prototype raises that lower bound to DERIVED 136.4 KiB. Both are reasonable for one owner-predicted tank, although cloning heap-backed `Vec`s every changed tick is avoidable overhead.

Lightyear `0.28` normal replication is Replicon `OnChange`. The old Lightyear delta-compression builder is explicitly inert on the Replicon backend; `replicate_diff()` works only if mutations are manually expressed as diffs ([replication.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_replication-0.28.0/src/registry/replication.rs:213), [replication.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_replication-0.28.0/src/registry/replication.rs:375)). Since most grounded strains change during slip, “delta compression” is not a free order-of-magnitude reduction.

## Strategy analysis

### A. Local elements plus the four-float aggregate

This satisfies rollback coherence but not authoritative convergence.

On every rollback, Lightyear can restore the element field from its local `PredictionHistory`, so replay does not use future-state bristles. That fixes the classic “force state was not rolled back at all” poison.

It does not make those bristles equal to the server’s bristles. At a park, deterministic local evolution stops, so any existing mismatch persists indefinitely.

#### Hidden distribution and torque

Let the two fields produce per-element forces \(f_i\) and \(f'_i\), with the same aggregate:

\[
\sum_i f_i = \sum_i f'_i.
\]

Their force difference has zero resultant, but its torque is:

\[
\Delta\tau = \sum_i (r_i-r_0)\times(f_i-f'_i).
\]

If every element obeys \(|f_i|\le c_i=\mu N_i\), and the contact patch fits within radius \(R\) of \(r_0\), a conservative bound is:

\[
|\Delta\tau| \le 2R\sum_i c_i = D\,C,
\]

where \(D=2R\) is the contact-patch diameter and \(C=\sum_i\mu N_i\).

That is a finite bound, but not a useful small-error bound: it is on the order of friction budget times track-contact length. Front and rear elements can carry equal and opposite lateral forces, giving zero aggregate force and a large yaw couple. Reversing that distribution produces the opposite couple with the same four-float summary.

On non-flat contact, the current scalar longitudinal/lateral sums are even weaker: different element distributions can produce different world resultants because their local traction directions differ.

Therefore:

- Pose, velocity, and four-float aggregate agreement do not imply matching acceleration.
- Matching aggregate does not make the parked distribution observationally irrelevant.
- A load or contact redistribution can expose a previously hidden mode after the comparison tick.

#### Can renormalization fix it?

A common scale cannot:

- create a nonzero aggregate from a zero-sum field;
- change the resultant direction;
- alter the front/rear or inner/outer moment;
- correctly handle partially saturated elements.

A load-proportional additive correction can force a desired two-axis sum in the unsaturated linear region, but leaves most spatial modes untouched and can corrupt the physical strain history.

A constrained least-squares projection against a full wrench can match the instantaneous rigid-body effect, but still has a large nullspace. It is an optional transient mitigation, not convergence.

Verdict: aggregate-only is acceptable for sandbox experimentation, but not as the shipped reconciliation mechanism.

### B. Quantized or structured per-tick replication

This is the only strategy here that can provide an authoritative element representation at every correction tick without reconstructing history.

#### Eight-bit quantization

For axes spanning `[-K, K]` with MEASURED \(K=75\) mm:

- DERIVED axis step: \(150/255 = 0.588\) mm.
- DERIVED maximum rounded axis error: 0.294 mm.
- DERIVED maximum two-axis vector error: 0.416 mm.
- DERIVED maximum linear-zone force error: approximately 0.56% of that element’s \(\mu N\).

That is plausible as a network correction error.

It is not safe as the simulation’s canonical storage resolution. Re-quantizing after every tick creates a DERIVED strain-increment deadband corresponding to about 18.8 mm/s of slip at the measured 64 Hz configuration. Small presliding motion would stop accumulating—the very regime this law exists to model.

Higher-resolution canonical integer state is more credible:

- DERIVED 12-bit half-step corresponds to about 1.17 mm/s per tick.
- DERIVED signed 16-bit half-step corresponds to about 0.073 mm/s per tick.

If bit-exact client/server grip state is required, the server must simulate the same canonical integer representation it transmits. Quantizing only the wire snapshot means the client deliberately resumes from a nearby state that the server never occupied.

#### Determinism consequences

Three different claims must not be conflated:

- Local replay can be bit-exact from a deterministically dequantized checkpoint.
- A quantized client seed need not be bit-equal to the server’s unquantized field.
- A hash over quantized projections can agree while exact `f32` fields differ.

If eight-bit network checkpoints are used, keep separate exact-local and wire-quantized hashes. The anchor rollback threshold must exceed the force/torque envelope introduced by quantization or the correction itself will cause another correction.

Verdict: viable if DERIVED roughly 25–30 KiB/s per predicted tank-recipient is acceptable, or as a fallback if the hybrid fails. Use explicit element occupancy/identity.

### C. Exact state when parked

This directly closes the previous `local_rollback` objection.

The ideal parked behavior is not “replicate at a low frequency.” It is:

1. Server declares a new authoritative rest epoch.
2. Server publishes one exact field checkpoint.
3. Replication continues retransmitting until acknowledged.
4. No further field bytes are generated while the state remains unchanged.

Replicon mutations use an unreliable latest-state channel but resend unacknowledged latest mutations by default ([Bevy Replicon](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_replicon-0.41.1/src/lib.rs:571)). Thus an unchanged rest checkpoint is eventually delivered without a heartbeat.

“Parked” must be a server-owned game state, not Avian sleeping. It should require a stable contact topology and stable element state for a dwell period. Healing must be assessed per side from material turnover, not merely hull speed:

- A tank can move or yaw with a locked belt while the same material pads remain grounded.
- One side can remain nearly stationary during a brake turn.
- For a non-turning side, contact dwell tends to infinity even while the vehicle is maneuvering.

#### Transition semantics

The checkpoint should represent state entering an explicitly named fixed tick. A simple convention is:

- capture the end-of-tick field in `FixedPostUpdate`;
- label it as the state entering the next tick;
- force rollback to the BASELINE tick before it (implementation correction — see the
  Rollback flow section: Lightyear restores the requested tick as baseline and replays
  from the tick after, so the entering-\(T\) field installs at \(B = T - 1\)).

On wake:

- do not gate or freeze the force law on the replicated rest bit;
- let local slip immediately evolve the exact parked field;
- bump the rest epoch on command, impulse, phase motion, contact-topology change, or threshold escape;
- send another checkpoint only if the side later becomes non-healing again.

Verdict: strongly recommended as part of the hybrid. Alone, it does not repair a moving field that diverges faster than it self-heals.

### D. Reconstruct from pose, velocity, aggregate, and phase

Not viable as an exact rollback mechanism.

With 75–90 grounded elements per side, the field has roughly 150–180 active scalar degrees of freedom per side. The proposed aggregate provides two constraints per side. Even a complete rigid-body wrench and belt reactions provide only a handful of constraints. The inverse is underdetermined.

Load-proportional redistribution is especially bad during a pivot. Pivot strain is approximately antisymmetric along the footprint: front and rear lateral strains oppose each other. Their aggregate can be near zero while their yaw moment is the principal desired effect. Reconstructing from the aggregate replaces that field with nearly zero yaw mode and recreates “turns on ice” during replay.

Exact reconstruction is theoretically possible only by replaying each material element from its last contact-entry reset through the complete authoritative slip/load/contact history. That history may exceed the rollback window and still needs a full seed for join-in-progress and parking. It moves the state cost into a longer event history rather than eliminating it.

Verdict: never use macro reconstruction for a predicted owner. It is acceptable only for a non-simulated remote presentation proxy.

### E. Wrench anchor plus exact checkpoints

This is the recommended improvement.

The minimal summary of the element field’s current effect on this model is:

- total traction force on the rigid body: three floats;
- total traction torque about center of mass: three floats;
- longitudinal ground reaction for each belt: two floats.

DERIVED payload: eight floats, or 32 bytes. Add a tick, epoch, and coarse field digest for a payload around DERIVED 44 bytes, about 2.75 KiB/s before protocol overhead at the measured tick rate.

Unlike the current aggregate, that summary captures everything the traction applications do to the single rigid body and both belt coordinates during that tick.

It still cannot reconstruct the field. Use it to detect when an exact checkpoint is required.

Recommended reconciliation policy:

- Effect mismatch above threshold: request an exact checkpoint immediately.
- Coarse digest mismatch while parked or non-turning: request immediately.
- Coarse digest mismatch while a side is turning over: allow at most a MEASURED contact-dwell interval; request if it persists.
- Periodic exact checkpoint: Batch D's MEASURED cumulative-belt-phase curves concluded that moving
  fields self-heal. The DERIVED 256-tick fallback at the MEASURED 64 Hz configuration (DERIVED 4 s)
  remains until Phase-4 multiplayer evidence justifies removing it.

Do not make a digest mismatch alone trigger a Lightyear rollback. A rollback without corrective field data can restore the same divergent local history and create a loop. The grip-attributed rollback should happen when an exact checkpoint is ready to install.

## Recommended Lightyear mechanics

### Components and messages

`TrackGripElements`

- Fixed-size root-resident simulation state.
- Constructed synchronously at tank spawn from data.
- Contains every element’s strain plus force-affecting contact activity/generation state.
- Registered with `local_rollback`.
- Included in the exact determinism hash in side/link/column order.
- No `HashMap`, runtime resize, or iteration-order ambiguity.

For join-in-progress, transmit an exact once-only initial value. Lightyear `0.28` provides an initial confirmed-write path specifically for once-replicated, subsequently local-rollback state. After catch-up activation, remove the one-time `ConfirmedHistory<TrackGripElements>` so later ordinary rollbacks use local prediction history, matching the reason behind the current `TankSim` stripping code ([protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:1047)). REV-15 rider DISCHARGED: direct first-force-tick cases and a real loopback-UDP replicate-once gate verify that a tank does not run one fixed tick with a default empty field.

`TrackGripEffect`

- Local per-tick output from the force-law module.
- Contains the eight-float effect summary and the coarse field digest.
- Locally historied so a received server anchor can be compared at its producing tick.

`NetTrackGripAnchor`

- Plain server-to-client replicated state with explicit producing tick and rest epoch.
- Do not depend on an uninterrupted sequence; mutation replication can skip a lost intermediate value and deliver a newer one.
- Compare against historical local `TrackGripEffect`.

`GripCheckpointChunk`

- Owner-private, server-to-client, unordered reliable message.
- Contains tank identity, epoch, state-entering tick, chunk index/count, explicit occupancy/element IDs, exact strains, contact generations, and a whole-checkpoint hash.
- Apply only after all chunks validate.
- Keep chunks below transport packet limits; Replicon deliberately avoids splitting one entity’s mutation across messages even if it exceeds the usual packet size ([channels.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_replicon-0.41.1/src/shared/backend/channels.rs:117)).

`GripResyncRequest`

- Owner-to-server reliable message.
- Rate-limited and deduplicated by tank and epoch.
- Server responds with a fresh current checkpoint, not a stale historical one.

### Rollback flow

When an exact checkpoint for state-entering tick \(T\) is fully assembled (CORRECTED at implementation, adversarial review 2026-07-20: Lightyear treats the requested rollback tick as the restored BASELINE and begins replay at the tick after it — [rollback.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/rollback.rs:1208) — so requesting \(T\) itself would install the entering-\(T\) field as if it were end-of-\(T\) and tick \(T\) would never consume the correction; the flow below uses baseline \(B = T - 1\)):

1. Wait until normal replication has confirmed macro state through \(B = T - 1\), so pose, velocity, and drive history are available at the baseline.
2. Stage the checkpoint in a non-rollback pending-correction resource.
3. Call `StateRollbackMetadata::request_forced_rollback(B)`. This public mechanism is intended for externally deposited state that must be replayed from a specific tick ([manager.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/manager.rs:241)).
4. After Lightyear restores ordinary histories (`RollbackSystems::Prepare`), but before replay begins, overwrite `TrackGripElements` with the checkpoint and replace the local history entry at \(B\), so replay's first tick \(T\) consumes the corrected field and the divergent value cannot resurrect.
5. Let `FixedPostUpdate` record the corrected field into ordinary prediction history.
6. Clear the pending correction only after its epoch has been applied.

If \(T\) is older than retained rollback history, request a fresh checkpoint. Do not apply a stale moving snapshot to the present. A parked snapshot may be reusable only if the server explicitly confirms the same rest epoch, contact fingerprint, and unchanged field.

### Rollback conditions

Keep existing conditions for position, rotation, velocities, and `TrackDrive`.

For grip reconciliation, use a physical-effect metric:

\[
e_v = \Delta t\,|\Delta F|/m
\]

\[
e_\omega = \Delta t\,|I^{-1}\Delta\tau|
\]

\[
e_{b,s} = \Delta t\,|\Delta R_s|/I_{\text{belt},s}.
\]

Compare these to the existing velocity, angular-velocity, and belt-speed correction policies. These thresholds are DERIVED netcode policy and then must be MEASURED under injected mismatch; they are not friction constants.

Recommended behavior:

- Effect error crossing threshold requests a checkpoint.
- Epoch/checkpoint arrival causes the forced rollback.
- Coarse digest mismatch alone never causes repeated automatic rollback.
- Exact checkpoint contents have no tolerance: applying a new epoch is authoritative.

This avoids the current failure mode where a threshold trip can cause a rollback but supplies no authoritative replacement for the divergent field.

### Protocol impact

The current code reports MEASURED `PROTOCOL_REV = 13` ([protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:41)). If no intervening protocol change lands, this design requires DERIVED `PROTOCOL_REV = 14`. (As landed: the transmission replication batch took rev 14 first, and this design shipped as `PROTOCOL_REV = 15` — the extra bump because the old replicated `TrackGrip` left the wire in the same change; in element mode it is derived telemetry whose rollback would be the correction-free loop this document forbids.)

The same change must update:

- ordered `WIRE_SURFACE`;
- `WIRE_SURFACE_HASH`;
- `WIRE_TYPES_HASH`;
- wire-type definition coverage;
- channel and message registrations;
- handshake fixtures/tests.

Historical advice, DISCHARGED at REV 15: the old replicated `TrackGrip` should not silently change
meaning from elastic state to telemetry. The implementation instead removed it from the wire while
retaining the name for aggregate offline compatibility and derived element-mode telemetry.
`TrackGripEffect` is the output summary; `TrackGripElements` is the simulation state.

## Failure modes and required tests

### Parked divergence

Inject two fields with:

- identical pose and velocity;
- identical four-float force aggregate;
- opposite front/rear lateral strain couples.

Verify:

- their torque anchor differs;
- an exact checkpoint repairs the field;
- the client remains settled afterward;
- no park/correct/park rollback loop occurs.

Also inject a hidden distribution that matches the full current wrench, then alter load distribution slightly. This tests the anchor’s remaining nullspace and the checkpoint path.

### Rollback during pivot

Force corrections:

- at maximum yaw resistance;
- around material-link phase wrap;
- with opposite belt directions;
- with one nearly stationary side;
- on a curb/cross-slope where contact frames differ.

Compare the complete field, emitted wrench, belt reactions, pose, and canonical hash after replay.

### Join-in-progress

Join against:

- a slope-parked tank with nonzero strain;
- a tank holding a yaw couple at zero aggregate;
- a moving tank near phase wrap.

Assert that the first predicted force tick begins with the authoritative field. There must be no one-tick zero-grip impulse.

### Packet loss

Drop:

- isolated anchors;
- an anchor burst;
- individual checkpoint chunks;
- the checkpoint completion/ack;
- the rest-to-wake notification.

The anchor path must tolerate skipped ticks. Checkpoint assembly must be atomic and idempotent. Wake delivery must not gate local physics.

### Self-healing

Inject a field mismatch while moving and measure error against cumulative belt phase, separately for each side. Do not plot only wall-clock time. Test locked-belt sliding and one-side-stationary turns, where the brief’s usual dwell argument does not apply.

### Quantization, if retained

Test:

- worst half-LSB field;
- near-zero presliding accumulation;
- correction-induced anchor error;
- repeated quantize/dequantize cycles;
- exact-local versus wire-quantized hashes.

## Per-element-law hazards

### World versus contact-local frame

The live prototype’s world-space strain is safer than storing raw coordinates in a hull-attached contact frame. A hull-local vector would rotate the remembered force when the hull rotates, without integrating corresponding ground slip.

However, the prototype’s operation:

```rust
j -= j.dot(normal) * normal;
```

is projection, not objective parallel transport. When the plane normal rotates, projection shortens the bristle and dissipates stored energy. Worse, the current “normal” is the belt/link direction; `TerrainOracle` supplies depth but not a terrain surface normal ([oracle.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/oracle.rs:21)).

Before reducing state to two floats, define the frame precisely:

- ground-attached contact-entry frame;
- deterministic tangent basis and surface identity;
- minimal-rotation transport when the terrain normal changes;
- behavior across block/rounded-edge transitions.

Without that extra interface/state, the honest representation is world `Vec3`, and the brief’s bandwidth should be multiplied by DERIVED 1.5.

### Contact-loss reset

A single-tick `touched` bit is too brittle. Contact lifetime should be force-affecting state with rollback and checkpoint coverage.

Use:

- elastic penetration/load, never damped-load sign, to determine membership;
- separate enter and leave thresholds;
- a short deterministic loss dwell or geometric material-departure condition;
- continued stored strain while engagement falls toward zero;
- reset only when the pad definitively leaves the ground contact generation.

Otherwise engage-ramp flutter can delete the exact parked strain intended to hold the tank.

### New generalized bristle modes

The aggregate stability result does not cover the new law. Per-element modes visible to the rigid body are governed by the generalized tangent stiffness:

\[
K_g = \sum_i \frac{\mu N_i}{K} J_i^\mathsf{T}P_iJ_i,
\]

including translational, yaw, pitch, roll, and belt coordinates.

Compute the maximum eigenvalue of \(M^{-1}K_g\) over the actual footprint/load cases and verify its discrete-time margin at the measured fixed tick. In particular:

- lateral front/rear strain creates yaw stiffness;
- longitudinal front/rear strain creates pitch stiffness;
- outer/center/inner columns create roll/yaw coupling;
- engage/load switching makes the stiffness time-varying.

The current statement that the summed translational stiffness matches the aggregate is necessary but not sufficient ([forces.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/forces.rs:403)).

### Material identity at phase boundaries

The prototype separately computes an `f32` station offset and an `f64` floor-based material wrap. Near an exact pitch multiple, rounding can make those decompositions disagree for one tick.

Use one phase-decomposition function that returns both:

- canonical integer wrap count;
- canonical residual offset with explicit carry if rounding reaches one pitch.

Test next-representable values on both sides of positive and negative pitch multiples and after long accumulated travel.

### Low-load strain accumulation

An element with tiny load can accumulate nearly full strain and later receive substantial load, producing a sudden force. Decide whether shear integration is:

- full while contact generation is active;
- engagement-weighted;
- or delayed until a firm-contact threshold.

Whichever choice is made must share the same hysteresis used for reset. Load should still scale force through `load_elastic`, as the shipped aggregate findings require.

### Force curve and damping

Specify what `sat(|j|/K)` means. A hard linear-to-cap function is not the exponential Janosi–Hanamoto curve; it has a tangent discontinuity at the cap. The current rational Dupont/Dahl update plus projection implements the former.

Per-element damping must:

- scale consistently with element load;
- share the isotropic friction cap with elastic force;
- remain dissipative under the selected frame transport;
- be included in the effect anchor even though it is not stored strain.

### Isotropic steering change

Removing the MEASURED `lateral_ratio = 0.55` policy restores full lateral \(\mu N\) capacity. That is not merely a better state representation; it changes the steering law and can greatly increase pivot resistance. The sandbox result must therefore be judged as both:

- per-element versus aggregate memory;
- isotropic circle versus the shipped anisotropic ellipse.

Otherwise the A/B confounds two independent design changes.

## Bottom line

The four-float aggregate is too shallow a correction anchor: the exact turning mode being added lies in its nullspace. A full traction wrench plus belt reactions is the smallest useful effect summary, but even that cannot reconstruct hysteretic element state.

Use local per-tick rollback history for temporal correctness, exact sparse checkpoints for authoritative convergence, and server-owned rest/non-turnover epochs to close the indefinitely parked case. Keep structured per-tick replication as the fallback if injected-divergence tests show that checkpoint latency or moving nullspace exposure is visible.

The underlying distributed shear rationale is consistent with Wong and Chiang’s track-ground steering model, which explicitly bases steering resistance on the spatial shear stress–displacement relationship rather than a mean patch slip ([Wong & Chiang, 2001](https://doi.org/10.1243/0954407011525683)).
