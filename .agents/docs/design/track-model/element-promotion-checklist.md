# Phase-2 offline gate + REV-14 landing checklist (codex, 2026-07-18)

> **Post-merge review addendum (codex, same day):** the `step_side` runtime size-repair
> branch (`forces.rs`, the clear-and-zero-resize of both `GripElements` slabs on length
> mismatch) is a CONFIRMED REV-14 blocker with a concrete failure: a predicted root that
> runs one driving tick before its authoritative seed records a zeroed field into local
> history, and replay follows it; a snapshot with one mismatched slab erases valid strain
> instead of surfacing the invariant violation. Phase 2 must construct both slabs
> synchronously at spawn (link_count × 3) and REPLACE the resize branch with a fixed-size
> invariant check. `Vec` is acceptable; runtime resizing is not.


# Element grip follow-up

## Q1 — Local-first Phase 2 gate

### Verdict

There is a genuine offline composition, but no current executable reaches it:

- `GamePlugin` mounts local physics, `SimPlugin`, the two-Tiger spawn, and `ClientPlugin`—no Lightyear client/server plugins ([lib.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/lib.rs:328)).
- The shipped executable always calls `run_client()` ([main.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/main.rs:9)); the targeted search found no `GamePlugin` call site.
- Therefore comments claiming single-player is already a runtime mode are aspirational/stale.

The smallest honest alternative is a process-start-only `--offline-elements` route, or a dev binary, which mounts `DefaultPlugins + GamePlugin`.

### Exact gate

Insert a private, startup-latched resource such as `ElementGripFeelTest` only in that offline composition:

```text
element path   iff ElementGripFeelTest exists
aggregate path otherwise
```

The branch belongs in `apply_track_forces`, where the adapter currently always passes `None` to `step_side` ([sim.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/sim.rs:243)). Pass `Some(&mut elements.sides[si])` only under that resource; pass `None` everywhere else.

Construct both side arrays synchronously from the Tiger’s `link_count` during root assembly—not by the prototype’s first-tick resize ([forces.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/forces.rs:424), [spawn.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/tank/spawn.rs:513)).

### Why `Connected` is not the gate

Do not gate on absence of Lightyear’s `Connected` marker:

- The net client installs `ClientPlugins`, shared protocol, rollback support, and the real `SimPlugin` before attempting connection ([client.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/client.rs:97)).
- It spawns a `NetcodeClient` connection entity unconditionally ([client.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/client.rs:229)).
- `AppState::Playing` is asset-gated, not connection-gated ([mod.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/mod.rs:53)).
- A missing link is an indefinitely reconnecting MP session ([client.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/client.rs:468)), not offline play.
- Lightyear’s prediction systems themselves run only while connected ([plugin.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/plugin.rs:59)); true SP never mounts them at all.

Because neither the net client nor server inserts `ElementGripFeelTest`, both continue passing `None` during ordinary ticks and rollback replay. Unregistered element state is never read or mutated, so it cannot enter MP rollback.

### Mid-session connection failure

The supported offline composition cannot connect mid-session because it has no connection entity or connection driver.

If hot-connect is later added, require full world teardown/restart. Flipping from elements to aggregate on `Connected` would reinterpret the element path’s four-float telemetry sum as aggregate elastic state ([forces.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/forces.rs:629)). REV 13 has neither authoritative element checkpoints nor element prediction history, so subsequent corrections would replay from unreconciled hidden state. That is precisely the rollback poison this gate must prevent.

---

## Q2 — REV 14 mechanical landing checklist

### 1. Promote the sim state without changing the wire

- Add fixed-order `TrackGripElements` and derived `TrackGripEffect` beside the current drivetrain types ([sim.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/sim.rs:46)).
- Build `TrackGripElements` synchronously from `link_count × 3` in root construction; include strain and every force-affecting activity/generation field.
- Replace the adapter’s `None` with the per-side arrays and populate the eight-float wrench/belt-reaction effect.
- Retain `TrackGrip` temporarily only as the aggregate compatibility path; do not change its meaning in place.

Touch points: [forces.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/forces.rs:417), [sim.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/sim.rs:176), [model.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/tank/model.rs:20), [spawn.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/tank/spawn.rs:513).

### 2. Move the canonical hash to elements

- Replace `TrackGrip` in `hash_tank_state` and its callers with `TrackGripElements`.
- Keep the JSON field `hblt`, but feed it in canonical `side → material link → column → field` order using raw bits.
- Include contact activity/generation; exclude `TrackGripEffect`, which is derived output.
- Replace the aggregate-grip ULP fixture with an element ULP test and an activity/generation tripwire.

Touch points: [trace.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/trace.rs:180), [trace.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/trace.rs:237), [trace.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/trace.rs:560), [trace.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/trace.rs:975), [headless_test.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/headless_test.rs:690).

### 3. Register the REV 14 surface in this exact order

Use one new bidirectional `UnorderedReliable` `GripChannel`.

```text
NetTank
NetBot
CombatantId
ServoAngles
NetCrew
NetTankStatus
LaunchedTurretPose
NetBelts
NetTrackGripAnchor
TrackGripElements
FireChannel
OutcomeChannel
DamageChannel
GripChannel
FireVisualBatch
FireEvent
RicochetKeyframe
ImpactConfirm
DamageConfirm
GripCheckpointChunk
GripResyncRequest
TankCommand
Position
Rotation
LinearVelocity
AngularVelocity
TrackDrive
```

Specific registrations:

```rust
app.component::<NetTrackGripAnchor>().replicate();

app.component::<TrackGripElements>()
    .replicate_once()
    .local_rollback()
    .add_confirmed_write();

app.local_rollback::<TrackGripEffect>();
```

`GripCheckpointChunk` is server-to-client; `GripResyncRequest` is client-to-server. Remove the replicated/predicted `TrackGrip` registration entirely—`TrackGripEffect` does not replace it on the wire.

Touch points: [protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:767), [protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:839), [protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:868), [protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:973), [protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:986).

Lightyear APIs: `replicate_once` ([replication.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_replication-0.28.0/src/registry/replication.rs:204)); `local_rollback` and `add_confirmed_write` ([registry.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/registry.rs:697), [registry.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/registry.rs:776)).

API correction to the prior report: in 0.28’s implementation, `add_confirmed_write` routes mutation writes only while the entity is `CatchUpGated` ([registry.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/registry.rs:811)). Init-message values on a predicted root are seeded separately into `ConfirmedHistory` ([predicted_history.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/predicted_history.rs:217)). A mutation-style JIP reveal must therefore actually hold `CatchUpGated`; the builder call alone is insufficient.

### 4. Land JIP and checkpoint rollback

- Preserve the initial `ConfirmedHistory<TrackGripElements>` through JIP activation.
- Do not reuse the immediate `strip_confirmed_history::<TankSim>` observer for elements ([protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:1003)).
- After activation has restored the authoritative seed, `try_remove::<ConfirmedHistory<TrackGripElements>>()`; later rollbacks then restore local `PredictionHistory`.
- Assemble checkpoint chunks atomically in non-rollback client state. Once macro history covers tick `T`, call `StateRollbackMetadata::request_forced_rollback(T)`.
- Apply the staged elements after Lightyear’s rollback preparation and before `SimPhase::DrivingForces` at `T`; clear them only after that application.

Lightyear records local history in `FixedPostUpdate` ([plugin.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/plugin.rs:92)), restores confirmed/local history during preparation ([rollback.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/rollback.rs:866)), and exposes the forced rollback API here ([manager.rs](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lightyear_prediction-0.28.0/src/manager.rs:241)). The existing scheduling precedent is [watchdog.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/watchdog.rs:35), with the call at [watchdog.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/watchdog.rs:216).

### 5. Update fixtures and semantic tripwires

- Spawn fixture: assert every `Tank` has correctly sized elements at `On<Add, Tank>`—never an empty vector awaiting first tick ([headless_test.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/headless_test.rs:29)).
- Trace fixtures: update digest signatures, queries, samples, and `hblt` localization tests ([trace.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/trace.rs:277), [trace.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/trace.rs:872)).
- Protocol tripwire: require the exact `replicate_once().local_rollback().add_confirmed_write()` chain and assert `TrackGrip` is absent from registrations.
- JIP test: first predicted driving tick observes the authoritative nonzero field; `ConfirmedHistory` exists through activation and is absent afterward.
- Resync test: out-of-order/duplicate chunks do not partially mutate state; the completed checkpoint requests rollback at its entering tick and applies before driving.
- Keep `tests/net_boundary.rs` green: sim/track files must not name Lightyear ([net_boundary.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/tests/net_boundary.rs:131)).

### 6. Re-pin and bump in the final protocol commit

1. Update actual registrations, then make `WIRE_SURFACE` match them.
2. Run `plugin_registrations_match_wire_surface` ([protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:1242)).
3. Run `wire_surface_is_pinned`; copy its printed value into `WIRE_SURFACE_HASH` ([protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:1110)).
4. Remove `TrackGrip` from `WIRE_TYPE_DEFS`; add every new wire type and embedded definition. Do not add local-only `TrackGripEffect` ([protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:1273)).
5. Run `wire_types_are_pinned`; copy its printed value into `WIRE_TYPES_HASH` ([protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:1412)).
6. Change `PROTOCOL_REV` from `13` to `14` in the same commit ([protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:41)).
7. Run `fingerprint_couples_every_pinned_wire_manifest_value` ([protocol.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/net/protocol.rs:1442)). Client, server, and UDP fixtures already consume `PROTOCOL_FINGERPRINT`; there is no separate numeric fingerprint fixture to edit.