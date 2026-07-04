# Bind-window NaN: source scan of lightyear 0.28 examples + prediction crate

2026-07-04. Scan done inline (the subagent fan-out looped on self-delegation and was cut off).
Sources: lightyear repo clone @0.28.0 (scratchpad/lightyear-src), vendored crates. All cited.

## Examples / reference patterns

- `avian_3d_character/src/shared.rs:85-97`: plugin composition identical to ours (disable
  `PhysicsTransformPlugin`, `PhysicsInterpolationPlugin`, `IslandPlugin`, `IslandSleepingPlugin`).
  The placeholder-init hole exists for the examples too — they never cross it because every
  physics entity is spawned **synchronously with explicit `Position` (+`Rotation`)** in the
  bundle; no example adds colliders to a predicted entity after spawn, and none has child
  colliders under a predicted body. Our async glb bind is off the reference map (confirms the
  original map §6/§8 gap).
- `avian_3d_character/src/renderer.rs:181-198` (`disable_projectile_rollback`): predicted
  projectiles get `DisableRollback` inserted permanently after their first predicted frame —
  precedent for "this entity's components must not be state-restored again".
- `deterministic_replication/src/shared.rs:96-116`: decorated (`DeterministicPredicted`)
  entities carry `Position` from their spawn bundles (never the require-insert placeholder), and
  get `FrameInterpolate<Position/Rotation>` on decoration so `FrameInterpolationSystems::Restore`
  preserves post-rollback Position against the transform→position sync.

## Prediction-crate mechanics (the chain, fully cited)

1. **History attaches per registered type, on marker add**: `add_prediction_history<C>`
   (predicted_history.rs:217-265) fires on `Add` of any of `C`/`Predicted`/`PreSpawned`/
   `DeterministicPredicted`/`CatchUpGated` and ensures `PredictionHistory<C>` for every
   prediction-registered `C` **present on the entity** — on our decorated rig children that
   includes avian `Position`/`Rotation` (require-inserted as `PLACEHOLDER` by collider
   registration, avian backend.rs:97-98).
2. **Recording is change-based**: `update_prediction_history` (predicted_history.rs:94-123)
   records `C` into history whenever `is_changed()`, at `PredictionSystems::UpdateHistory`
   (FixedPostUpdate, chained after `PhysicsSystems::StepSimulation` by lightyear_avian
   plugin.rs:193-204). The require-insert marks the component changed — so if avian's
   child-collider pose update skipped that tick (ColliderTransform not yet propagated — the
   set-anchoring race, partially fixed), **the literal PLACEHOLDER is recorded as history**.
3. **Restore replays recorded history verbatim**: `prepare_rollback<C>` (rollback.rs:874-970),
   `Without<DisableRollback>`: non-replicated components (no `ConfirmedHistory`) restore
   `predicted_history.get_state(rollback_tick)` — the poisoned early entry — directly into the
   live component. Ticks before the first entry restore nothing ("leave current value") — benign;
   the poison case is specifically a recorded placeholder.
4. **Why the children's grace doesn't save us**: during `enable_rollback_after` the children are
   `DisableRollback` → skipped by `prepare_rollback` → the poisoned history survives untouched;
   the first rollback after the grace lifts that reaches back to the poisoned range restores it.
   Also explains crash-rate sensitivity to frame timing (how many poison ticks got recorded).

## Verdict

The rig children's `Position`/`Rotation` are **derived state** — avian recomputes them from the
root pose ∘ `ColliderTransform` every tick (`update_child_collider_position`, both avian's and
lightyear's copies). Pose history/restore on them has zero value and is the poison vector. Fix:
after decoration, remove `PredictionHistory<Position>` / `PredictionHistory<Rotation>` from the
decorated children (`add_prediction_history` only fires on `Add` events, so the removal sticks;
`prepare_rollback` uses the history component itself as its membership marker — rollback.rs:882
comment — so removal cleanly excludes exactly those two components while `ServoState`/`Reload`/
`Suspension`/`DriveState` histories keep working). Root stays fully predicted.
