# NaN / Rotation::PLACEHOLDER research — prediction rollback on newly-bound child colliders

Status: IN PROGRESS (written incrementally; sections may be partial).

Scope: source-verified read-only research into lightyear_prediction 0.28.0, lightyear_core 0.28.0,
lightyear_avian3d 0.28.0, lightyear_replication 0.28.0, lightyear_inputs 0.28.0 (all from
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`), avian3d 0.7.0, cross-referenced against
the git clone at
`/private/tmp/claude-502/-Users-Yan-Desktop-github-vikng-dev-personal-overmatch/f803d278-bcd0-481e-8ec2-771cbe2e1914/scratchpad/lightyear-src`
(HEAD `28e823d "fix bei no-std"`, one commit past the `0.28.0` tag — used only for corroboration;
crates.io vendored copy is authoritative for all line numbers cited here).

All paths below are absolute; `LP` = `lightyear_prediction-0.28.0`, `LC` = `lightyear_core-0.28.0`,
`LA` = `lightyear_avian3d-0.28.0`, `LR` = `lightyear_replication-0.28.0`, `LI` = `lightyear_inputs-0.28.0`,
`AVIAN` = `avian3d-0.7.0`, all rooted at
`/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.

Repo context read (not re-derived below, cited where used): `src/net.rs:191-411` (`decorate_rig_children`,
`plugin()`), which:
- Decorates turret/gun/muzzle/roadwheel children with `DeterministicPredicted { skip_despawn: true, enable_rollback_after: 20 }` once `Rig` + `Predicted` land on the root (`src/net.rs:211-236`).
- Registers avian `Position`/`Rotation`/`LinearVelocity`/`AngularVelocity` on the root with `.replicate().predict()` plus coarsened rollback thresholds (`src/net.rs:333-363`).
- Registers `local_rollback::<DriveState/ServoState/Reload/Suspension>()` (`src/net.rs:370-373`), which live on the decorated children.
- Mounts `LightyearAvianPlugin { replication_mode: AvianReplicationMode::Position, .. }` (`src/net.rs:329-332`), and separately re-anchors `PhysicsTransformSystems::Propagate` into `PhysicsSystems::Prepare` in `FixedPostUpdate` as an already-applied fix for a *different*, related NaN mechanism (ColliderTransform ordering, `src/net.rs:375-388`) — this existing fix is orthogonal to the mechanism identified in Q7/Q8 below (which is about `RunFixedMainLoop`, not `FixedPostUpdate` ordering).

---

## Q1 — Which components get `PredictionHistory<C>`, and when?

**Answer: every `.predict()`-registered type AND every `local_rollback::<C>()`-registered type get a
`PredictionHistory<C>` attached — including avian's `Position`/`Rotation`, since `net.rs:333-348`
calls `.predict()` on both.** There is no separate, narrower "local_rollback types only" pool.

- `ComponentRegistration::add_prediction()` (`LP/src/registry.rs:832-884`, reached via the public
  `.predict()` builder method, `LP/src/registry.rs:687-689` declares the trait method; the
  `PredictedComponentRegistration` wrapper forwards to it) calls
  `add_prediction_systems::<C>(self.app)` at `LP/src/registry.rs:874`.
- `add_prediction_systems::<C>` (`LP/src/plugin.rs:117-180`) is what actually wires history:
  - `app.add_observer(add_prediction_history::<C>)` — `LP/src/plugin.rs:159`.
  - `app.add_observer(apply_component_removal_predicted::<C>)` — `LP/src/plugin.rs:157`.
  - `app.add_observer(handle_tick_event_prediction_history::<C>)` — `LP/src/plugin.rs:158`.
  - `update_prediction_history::<C>` in `FixedPostUpdate`, `PredictionSystems::UpdateHistory` —
    `LP/src/plugin.rs:175-179`.
- Separately, `app.local_rollback::<C>()` (used by `net.rs:370-373` for `DriveState`/`ServoState`/
  `Reload`/`Suspension`, which live on the decorated children) resolves to `add_local_rollback::<C>`
  (`LP/src/registry.rs:1044-1051`), which calls `add_non_networked_rollback_systems::<C>`
  (`LP/src/plugin.rs:75-100`) — the **same** `add_prediction_history::<C>` observer
  (`LP/src/plugin.rs:81`) and the **same** `update_prediction_history::<C>` system in
  `FixedPostUpdate`/`UpdateHistory` (`LP/src/plugin.rs:96-99`).
- So both registration paths (`predict()` for replicated avian components, `local_rollback()` for
  non-networked sim components) converge on the identical `add_prediction_history` observer and
  `update_prediction_history` system — there is exactly one history-attach mechanism in the crate.

**The observer that actually attaches `PredictionHistory<C>`:** `add_prediction_history<C>`
(`LP/src/predicted_history.rs:237-326`) is registered as:
```rust
trigger: On<Add, (C, Predicted, PreSpawned, DeterministicPredicted, CatchUpGated)>
```
(`LP/src/predicted_history.rs:238-247`) — i.e. it fires when **any** of those five things is added
to an entity. Inside, it re-queries `Has<C>`, `Has<Predicted>`, `Has<PreSpawned>`,
`Has<DeterministicPredicted>`, `Has<CatchUpGated>` on the trigger entity
(`LP/src/predicted_history.rs:248-261`) and bails (`return`, no insert) unless
`catchup_gated || (has_component && (predicted || prespawned || deterministic))`
(`LP/src/predicted_history.rs:262-264`). This is exactly the "no hierarchy traversal — gates on the
trigger entity carrying `Predicted`/`PreSpawned`/`DeterministicPredicted`/`CatchUpGated` directly"
behavior the repo's own comment describes (`src/net.rs:199-201`).

If the gate passes, it queues a command (`LP/src/predicted_history.rs:274-325`) that, on the SAME
entity: inserts `PredictionHistory::<C>::default()` if not already present
(`LP/src/predicted_history.rs:308-310`), and conditionally seeds `ConfirmedHistory<C>` from an
init-message value (`LP/src/predicted_history.rs:283-324`) — irrelevant for `DeterministicPredicted`
children since they are non-networked (registry.rs:819-821 confirms `DeterministicPredicted` is
**not** wired into `add_confirmed_write`'s marker-fn seeding path; see Q4).

Because the observer trigger includes `C` itself in the `On<Add, (...)>` tuple, **order of
insertion does not matter**: if `Position`/`ServoState` etc. is inserted on the child before
`DeterministicPredicted`, the observer already fires (component present) but the re-query at
`predicted_history.rs:257-261` requires `deterministic` (or one of the other markers) to ALSO be
true at that same instant, i.e. `Add<C>` alone with no `DeterministicPredicted` yet present does
NOT attach history (this matters for the child-collider bind order — see Q2).

## Q2 — When is the FIRST value recorded, and could it be the PLACEHOLDER?

**Two separate mechanisms populate `PredictionHistory<C>`, and they run at very different times
relative to the physics step; the earlier of the two is what can capture the placeholder era.**

1. **The seed inserted at decoration time is an *empty* `PredictionHistory::<C>::default()`**
   (`LP/src/predicted_history.rs:41-47`, `Default` = empty `HistoryBuffer` with an empty
   `VecDeque`, `LC/src/history_buffer.rs:90-96`). Attaching `PredictionHistory<C>` does **not**
   itself write any value into the buffer — it only makes the buffer exist so the next system in
   (2) has somewhere to write.

2. **The FIRST actual value is recorded by `update_prediction_history::<C>`**
   (`LP/src/predicted_history.rs:94-123`), scheduled in `FixedPostUpdate`,
   `PredictionSystems::UpdateHistory` (`LP/src/plugin.rs:175-179`, `LP/src/plugin.rs:96-99` for the
   local-rollback path). It runs `for (entity, component, mut history) in query.iter_mut()` and
   calls `history.add_predicted(tick, Some(component.deref().clone()))` **only if
   `component.is_changed()`** (`LP/src/predicted_history.rs:102-105`). This means:
   - **(a) Component added AFTER decoration** (e.g. avian's `Position`/`Rotation` are inserted by
     avian's own `require`-hook machinery on the SAME frame the collider is added to the child —
     see Q7/Q8): change detection sees it as freshly-added/changed, so the very next
     `FixedPostUpdate`'s `update_prediction_history::<Position>` records whatever value `Position`
     holds AT THAT MOMENT. If avian has not yet resolved the `Rotation::PLACEHOLDER`
     require-default by the time `FixedPostUpdate::UpdateHistory` runs, **the placeholder value is
     what gets recorded as the first history entry.**
   - **(b) Component present AT decoration** (e.g. `ServoState`/`Suspension` on the turret/gun/
     muzzle children, which exist well before `DeterministicPredicted` is added by
     `decorate_rig_children`, `src/net.rs:211-236`): the `Add<DeterministicPredicted>` observer
     trigger creates the (empty) `PredictionHistory<ServoState>` immediately, and the NEXT
     `FixedPostUpdate` tick's `update_prediction_history::<ServoState>` records it — but only if
     `is_changed()` is true. Bevy's change detection treats a component as "changed" for a system
     the first time that system observes it after the component existed (via `Ref`'s change tick
     comparison against the system's last-run tick) if the component's last-changed tick is newer
     than the system's last run — in practice, since these components update every tick via the
     sim's own mechanisms (`drive_servos`, etc.), they are "changed" essentially every tick anyway,
     so they get a real (non-placeholder) value on the very next `FixedPostUpdate` regardless of
     decoration timing. **UNCERTAIN:** whether a component that happens to be perfectly stable
     (not touched by any system) at the moment of decoration would go one or more ticks without a
     history entry — the source doesn't special-case "just decorated" for non-avian components; it
     purely follows Bevy change detection. This is not implicated in the PLACEHOLDER bug since
     `ServoState`/`Suspension`/`Reload`/`DriveState` never hold avian's sentinel value.

**Does history-recording run before or after the game/avian systems that compute the real value for
that tick?** For the `AvianReplicationMode::Position` mode the repo uses, `LightyearAvianPlugin`
explicitly orders (`LA/src/plugin.rs:193-204`):
```rust
app.configure_sets(FixedPostUpdate,
    (PhysicsSystems::StepSimulation,
     (PredictionSystems::UpdateHistory, FrameInterpolationSystems::Update))
    .chain());
```
So **within `FixedPostUpdate`**, `update_prediction_history` runs AFTER `PhysicsSystems::StepSimulation`
— i.e., after avian's actual physics step for that tick, which is the correct ordering for the ROOT
body's `Position`/`Rotation` (avian's solver writes a real value to the root before history records
it). **However, the child-collider `Position`/`Rotation` used by lightyear (per `AvianReplicationMode::Position`) is written by `LightyearAvianPlugin::update_child_collider_position`, which runs in
`RunFixedMainLoop::AfterFixedMainLoop`** (`LA/src/plugin.rs:187-191`) — **a schedule that is
distinct from, and runs AFTER, all of `FixedMain` for that frame** (see Q7/Q8 for the schedule
topology proof). This means: on the very first frame a child collider exists, avian's `on_add`
hook for the collider resolves the `Position`/`Rotation` PLACEHOLDER synchronously
(`AVIAN/src/collision/collider/backend.rs:114-121` → `init_physics_transform`,
`AVIAN/src/physics_transform/transform.rs:1116-1275`) at *collider-insertion* time (whenever that
command/hook runs, typically in `Update`/`PostUpdate` or wherever the binder inserts the collider —
**UNCERTAIN** exactly which schedule the repo's binder runs in without reading `tank.rs`'s bind
system in full, but the observer's job per the task description is to run "seconds later on glb
load"). If `update_prediction_history::<Position>` for that child runs in a `FixedPostUpdate` that
falls BEFORE the next `RunFixedMainLoop::AfterFixedMainLoop` call, the recorded value is whatever
`init_physics_transform` resolved it to (a real computed value, not the sentinel — provided the
collider's `GlobalTransform`/parent chain was valid at hook time). The dangerous case is if the
collider's `on_add`/`ColliderOf::on_insert` hooks fire in an order or context where `GlobalTransform`
is not yet correct (e.g. before `TransformSystems::Propagate` has run for the newly-spawned
hierarchy) — then `init_physics_transform`'s `parent_global_transform` walk
(`AVIAN/src/physics_transform/transform.rs:1144-1151`, walking `ChildOf` and reading `Transform`,
NOT `GlobalTransform`, so it does NOT depend on propagation) still computes a value, but Q8 covers
the scenario where the `Rotation` never gets resolved before something else observes/records it.

## Q3 — Rollback RESTORE code path for a `DeterministicPredicted` entity

Restoration for ALL predicted components (root or `DeterministicPredicted` child alike) happens in
`prepare_rollback<C>` (`LP/src/rollback.rs:874-1011`), scheduled in `PreUpdate`,
`RollbackSystems::Prepare` (`LP/src/plugin.rs:167-169` for `.predict()`-registered types;
`LP/src/plugin.rs:92-95` for `local_rollback()`-registered types) — this runs BEFORE
`RollbackSystems::Rollback` (`run_rollback`, which actually replays `FixedMain`) per the chained
`configure_sets` at `LP/src/rollback.rs:121-132`.

The query is:
```rust
Query<(Entity, Option<&mut C>, &mut PredictionHistory<C>, Option<&mut ConfirmedHistory<C>>),
      Without<DisableRollback>>
```
(`LP/src/rollback.rs:883-891`). **`Without<DisableRollback>` is the ONLY entity-level gate** — an
entity currently carrying `DisableRollback` is entirely skipped by `prepare_rollback`, for every
component type registered for prediction.

Per matched entity/component:
1. `restore_state` is computed (`LP/src/rollback.rs:911-937`):
   - If `Rollback::FromState` (state rollback) and a `ConfirmedHistory<C>` exists on the entity:
     `history.get_state_at_or_before(rollback_tick)` (line 928) — **not applicable to
     `DeterministicPredicted` children**, since they have no `ConfirmedHistory<C>` (they aren't
     replicated; Q4/registry.rs:819-821 confirms `DeterministicPredicted` doesn't route through
     `add_confirmed_write`'s seeding).
   - Otherwise (including local/`DeterministicPredicted`-only components, and ALL input rollbacks):
     `predicted_history.get_state(rollback_tick)` (line 932/936) →
     `HistoryBuffer::get_state(tick)` (`LC/src/history_buffer.rs:160-168`): binary-searches
     (`partition_point`) for the newest entry `<= tick`; **returns `None` if `tick` is before the
     oldest retained entry** (`partition == 0` branch, line 164-166).
2. `predicted_history.clear()` then re-seeds with `(rollback_tick, restore_state)` if `Some`
   (`LP/src/rollback.rs:942-945`) — this always happens, regardless of branch (a)/(b)/(c) below,
   as long as the entity passed the `Without<DisableRollback>` filter at query time.
3. The component is updated by matching on `restore_state` (`LP/src/rollback.rs:963-1009`):

   **(a) Rollback target tick is BEFORE the history's first recorded tick** → `restore_state =
   None` → **`None` branch, `LP/src/rollback.rs:965-974`: does nothing — "leave the current
   component value in place."** It does NOT remove the component, does NOT restore Default, does
   NOT restore the oldest available value. This is also unit-tested:
   `test_predicted_component_initial_rollback` (`LP/src/rollback.rs:1235-1263`) explicitly asserts
   the live component value survives unchanged when `rollback_tick` predates the only history
   entry. **This is the exact "leaves the current value in place" behavior the repo's own comment
   about `None` restore anticipates, and it means a pre-history rollback is SAFE — it cannot itself
   inject the placeholder.** (If the bug reproduces even when this branch should apply, the
   placeholder must already be IN history by the time of the first rollback — consistent with Q2's
   finding that `update_prediction_history` can record the placeholder as history entry #1.)

   **(b)/(c) Rollback target tick is AT OR AFTER a recorded tick** → `restore_state =
   Some(HistoryState::Updated(value))` (or `Some(HistoryState::Removed)`) → the `Some(...)` arms
   fire regardless of whether the entity is inside or past its `enable_rollback_after` grace
   window, **because grace-window membership is not tested by `prepare_rollback` at all** — it is
   entirely encoded by whether `DisableRollback` is present (which the `Without<DisableRollback>`
   filter uses to exclude the entity from this query altogether). So:
   - **Inside the grace period** (i.e. `DisableRollback` present): the entity does not match the
     query filter at all — `prepare_rollback` never touches it, its `PredictionHistory<C>` is left
     completely alone (not cleared, not re-seeded), and its live component is left completely alone
     for this system. (Whether it's excluded from the REPLAY itself is a separate question — see
     Q4, `DisabledDuringRollback`.)
   - **After the grace period has lifted** (`DisableRollback` removed by the drain logic, see Q4):
     the entity now matches the query, and normal restore applies: `Some(HistoryState::Updated(v))`
     with `predicted_component: None` → `entity_mut.insert(correct)` (re-adds a removed component,
     `LP/src/rollback.rs:982-985`); with `predicted_component: Some(mut c)` → optionally snapshots
     `PreviousVisual` for correction smoothing (`LP/src/rollback.rs:988-1002`) then
     `*predicted_component = correct` (`LP/src/rollback.rs:1005`) — a direct overwrite with
     whatever value was in history at `rollback_tick`. **If that history entry is the recorded
     PLACEHOLDER (per Q2), this line is the exact write-back that reinjects
     `Rotation::PLACEHOLDER` into the live, now-grace-period-lifted child's `Rotation` component.**

## Q4 — `DisableRollback` / `DisabledDuringRollback` semantics

- **Is history still RECORDED while `DisableRollback` is present?** `update_prediction_history::<C>`
  (`LP/src/predicted_history.rs:94-123`) has NO filter on `DisableRollback` or
  `DisabledDuringRollback` at all — its query is plain `Query<(Entity, Ref<T>, &mut
  PredictionHistory<T>)>` (line 95). Bevy's `DefaultQueryFilters` only auto-excludes components
  registered as "disabling" (see below) — `DisableRollback` itself is **never** registered as a
  disabling component (only `DisabledDuringRollback` and `PredictionDisable` are, see next bullet).
  **So yes: `PredictionHistory<C>` keeps recording normally for an entity that has `DisableRollback`
  but not (yet) `DisabledDuringRollback`** — i.e., during ordinary (non-rollback) `FixedUpdate`
  ticks in the grace window, the child's history keeps growing with whatever `Position`/`Rotation`
  the sim computes each tick.
- **Is the entity restored during rollback replay while `DisableRollback` is present?** No —
  established in Q3: `prepare_rollback`'s query filters `Without<DisableRollback>`
  (`LP/src/rollback.rs:890`), so a `DisableRollback`-carrying entity's live component is never
  touched by `prepare_rollback`, and (see next bullet) it is also hidden from the `FixedMain` replay
  itself via `DisabledDuringRollback`.
- **Is the entity hidden from ALL world queries during rollback replay?** Yes, via a genuine
  Bevy default-query-filter swap, but the swap is scoped strictly to the replay window:
  - `PredictionPlugin::build` registers `DisabledDuringRollback` (and separately
    `PredictionDisable`) as `DefaultQueryFilters`-disabling components:
    ```rust
    let rollback_disable_id = app.world_mut().register_component::<DisabledDuringRollback>();
    let prediction_disable_id = app.world_mut().register_component::<PredictionDisable>();
    app.world_mut().resource_mut::<DefaultQueryFilters>().register_disabling_component(rollback_disable_id);
    app.world_mut().resource_mut::<DefaultQueryFilters>().register_disabling_component(prediction_disable_id);
    ```
    (`LP/src/plugin.rs:200-209`). Once registered as disabling, **every ordinary `Query<...>` in the
    whole app** (unless it explicitly opts back in with `Allow<T>`, see Q5) silently skips entities
    carrying that component — this is core Bevy `bevy_ecs::entity_disabling` behavior, not something
    lightyear reimplements per-system.
  - `run_rollback` (`LP/src/rollback.rs:1051-1174`) is the ONLY place that inserts
    `DisabledDuringRollback`, and it does so JUST before replaying, and removes it JUST after:
    ```rust
    let disabled_entities = world.query_filtered::<Entity, With<DisableRollback>>().iter(world).collect::<Vec<_>>();
    disabled_entities.iter().for_each(|entity| { world.entity_mut(*entity).insert(DisabledDuringRollback); });
    // ... for i in 0..num_rollback_ticks { world.run_schedule(FixedMain); ... }
    disabled_entities.into_iter().for_each(|entity| { world.entity_mut(entity).remove::<DisabledDuringRollback>(); });
    ```
    (`LP/src/rollback.rs:1103-1148`). So: an entity with `DisableRollback` is invisible to (almost)
    every system for the duration of the `FixedMain` replay loop, INCLUDING avian's own physics
    queries (avian systems are ordinary Bevy queries with no special `Allow<DisabledDuringRollback>`
    opt-in visible in the avian3d/lightyear_avian3d source read for this task) — meaning **a
    grace-period child collider is completely absent from the physics world during every rollback
    replay tick**: no collision response, no `ColliderTransform` recompute, nothing. Its parent BODY
    (the tank root, NOT `DisableRollback`) IS replayed normally through those same ticks. **This
    implies the child's `Position`/`Rotation` are frozen at whatever they were when
    `DisabledDuringRollback` was inserted, for the entire length of the replay** — they do not track
    the root's replayed motion during those ticks at all (avian's `update_child_collider_position`
    doesn't run during `FixedMain` regardless, per Q7/Q8, but even the ROOT-driven collider-of-root
    physics interactions for that specific child are skipped). Once `DisabledDuringRollback` is
    removed after replay, and once the grace window later lifts (removing `DisableRollback` too),
    the child reappears with whatever stale `Position`/`Rotation` it had — potentially now visibly
    detached from its parent for one or more frames until `update_child_collider_position` (which
    DOES run every frame, in `RunFixedMainLoop::AfterFixedMainLoop`, unconditionally on all
    `Without<RigidBody>` colliders with a `ColliderTransform`+`ColliderOf`, `LA/src/plugin.rs:858-888`)
    recomputes it from the (now-updated) parent + `ColliderTransform`. **UNCERTAIN:** whether this
    "frozen during replay, then instantly resnapped once `update_child_collider_position` next runs"
    sequence is itself capable of producing a transient PLACEHOLDER re-observation — it is not,
    by itself, since `update_child_collider_position` reads `ColliderTransform` (not history) — but
    see Q8 for how this interacts with the drain/lift moment.
- **When the grace period lifts, what exactly happens?** This is the `deterministic_skip_despawn`
  drain, and it lives in `check_rollback` (`LP/src/rollback.rs:355-814`), specifically the tail
  block from **`LP/src/rollback.rs:726-813`** (run only `if let Some(rollback_tick) =
  prediction_manager.get_rollback_start_tick()`, i.e. only on frames where SOME rollback is about to
  happen — **the task's guess of "~line 290" is INCORRECT; the actual drain is at lines 767-812,
  inside the same `if let Some(rollback_tick) = ...` block that starts at line 726, all still within
  `check_rollback`, which itself is in `PreUpdate`/`RollbackSystems::Check`**). Precisely
  (`LP/src/rollback.rs:790-812`, the non-forced-rollback branch, which is the normal case):
  ```rust
  let split_idx = prediction_manager.deterministic_skip_despawn
      .partition_point(|(t, _)| *t <= rollback_tick);
  let should_disable_rollback = prediction_manager.deterministic_skip_despawn.split_off(split_idx);
  should_disable_rollback.iter().for_each(|(_, e)| { commands.entity(*e).insert(DisableRollback); });
  prediction_manager.deterministic_skip_despawn.iter().for_each(|(_, e)| { commands.entity(*e).remove::<DisableRollback>(); });
  prediction_manager.deterministic_skip_despawn = should_disable_rollback;
  ```
  Recall the vec stores `(protection_tick, entity)` where `protection_tick = decoration_tick +
  enable_rollback_after` (`LP/src/rollback.rs:295-298`, `DeterministicPredicted::on_add`). So:
  entities whose `protection_tick <= rollback_tick` (i.e., **the grace window has fully elapsed
  relative to the CURRENT rollback's target tick**) are moved into `should_disable_rollback` and
  have `DisableRollback` **removed** — i.e., counter-intuitively, `should_disable_rollback` is the
  set of entities for which the protection window is now over ("should [have its] disable[d]
  [state] roll[ed] back", i.e. lifted) — the remaining (still-protected) entities are re-inserted
  with `DisableRollback` (line 805-808, effectively a churn/no-op refresh) and kept in the vec for
  future checks. **This whole block only executes when `check_rollback` has already decided
  `get_rollback_start_tick()` is `Some` for THIS frame** — i.e. the grace period doesn't lift on a
  quiet timer, it lifts (or is re-evaluated) opportunistically, only on ticks where a rollback is
  about to occur. On a tick with NO rollback pending, an expired-but-untouched
  `deterministic_skip_despawn` entry simply sits there with `DisableRollback` still attached
  indefinitely (it is only cleared the next time a rollback fires, however much later that is —
  **UNCERTAIN** whether this can meaningfully delay grace-window lifting beyond the nominal 20-tick
  window when rollbacks are infrequent; the repo's own measurement, `src/net.rs:203-210`, describes
  children surviving "~300ms later" against a nominal 20-tick/~333ms-at-60Hz window, consistent with
  this drain-on-rollback-only behavior).

  **The critical, load-bearing sequence for the bug:** the very first time `DisableRollback` is
  removed from a child (moment the grace period lifts) is **on a frame where `check_rollback` has
  ALSO just decided a rollback is needed this frame** (`rollback_tick` is set precisely because
  we're inside `if let Some(rollback_tick) = prediction_manager.get_rollback_start_tick()`). That
  means: the SAME frame that ends the child's grace period is guaranteed to be a rollback frame —
  the child goes from "invisible to `prepare_rollback`/`run_rollback`" to "fully subject to
  `prepare_rollback`'s restore-from-history logic" in one step, with zero intervening ticks where
  its history could be "laundered" by a few ticks of normal (correct) recording. If history's FIRST
  entry (or any entry at/behind `rollback_tick`) is the placeholder era recorded per Q2, this is
  precisely the moment `prepare_rollback`'s `*predicted_component = correct` (`LP/src/rollback.rs:1005`)
  reinjects it into the live component — matching the observed symptom of "real, non-NaN, but wrong
  sentinel value written into a live component" exactly.

## Q5 — Per-entity/per-component opt-out: `PredictionDisable` and `Allow<PredictionDisable>`

- `PredictionDisable` is a plain marker component (`LP/src/despawn.rs:31-33`,
  `#[derive(Component, PartialEq, Debug, Reflect)]`, no hooks). It is registered as a
  `DefaultQueryFilters`-disabling component in the SAME place as `DisabledDuringRollback`
  (`LP/src/plugin.rs:203,208-209`).
- **Semantics/purpose**: it is inserted by `PredictionDespawnCommand`
  (`LP/src/despawn.rs:35-64`) as the "soft despawn" marker for `Predicted`/`DeterministicPredicted`/
  `PreSpawned` entities (line 48-53: exactly these three marker types are accepted) — instead of an
  immediate `despawn()`, the entity is hidden from ordinary queries via `PredictionDisable` so it can
  be resurrected (components restored) if a later rollback proves the despawn was premature. It is
  the standard "predicted despawn" mechanism, **not a general-purpose "opt this component out of
  history" tool** — there is no evidence in the source of a narrower, single-component opt-out
  (e.g. no `#[derive(Component)] struct PredictionDisableFor<C>`-style API exists in this crate).
  **UNCERTAIN:** whether there is any OTHER, more targeted mechanism to exclude a single component
  (as opposed to an entire entity) from history/restore — nothing in `rollback.rs`,
  `predicted_history.rs`, or `registry.rs` exposes one; the closest thing is simply not calling
  `.predict()`/`.local_rollback()` for that component type at all (i.e. it's a compile-time/
  registration-time decision, not a runtime per-entity toggle).
- `Allow<PredictionDisable>` is Bevy's own `bevy_ecs::entity_disabling::Allow<T>` query-filter
  wrapper (not lightyear-authored) — it is the mechanism by which a query **opts back into seeing**
  entities that would otherwise be silently excluded because they carry a registered disabling
  component. It's used extensively in `lightyear_inputs::client.rs` (8 call sites: lines 297, 365,
  483, 538, 631, 787, 948, 1166) so that the client-side input-buffer bookkeeping systems keep
  processing predicted-but-disabled (soft-despawned) entities' inputs; and it's used once in
  `LP/src/rollback.rs:195` inside `RollbackPlugin::finish`'s dynamically-built `check_rollback`
  query (`builder.filter::<Allow<PredictionDisable>>()`), so state-mismatch rollback checks still
  run against soft-despawned-but-still-`Predicted` entities (the comment at
  `LP/src/rollback.rs:193-195` says this explicitly: "include `PredictionDisable` entities...we keep
  them around for rollback check"). **This is the general Bevy pattern for un-hiding a specific
  disabling marker in one query, not a lightyear-specific mechanism** — there's no equivalent
  `Allow<DisableRollback>` or `Allow<DisabledDuringRollback>` anywhere in the crates read for this
  task (`grep -rn "Allow<"` across all `lightyear_*` crates found matches only in
  `lightyear_inputs-0.28.0/src/client.rs` and `lightyear_prediction-0.28.0/src/rollback.rs`).
- **Insertion API**: `commands.entity(e).insert(PredictionDisable)` directly, or via
  `EntityCommands::prediction_despawn()` (`LP/src/despawn.rs:70-89`, the intended public entry
  point) which internally inserts it (through `PredictionDespawnCommand::apply`,
  `LP/src/despawn.rs:38-63`) rather than despawning outright, when NOT running as (host-)server.

## Q6 — Public API to seed/clear `PredictionHistory<C>`

`PredictionHistory<C>` derefs to `HistoryBuffer<C>` (`LP/src/predicted_history.rs:49-61`,
`Deref`/`DerefMut`), so essentially all of `HistoryBuffer`'s public API is directly callable through
a `&mut PredictionHistory<C>`. Enumerating the actually-public methods relevant to
seeding/clearing (all in `LC/src/history_buffer.rs` unless noted):
- `pub fn add(&mut self, tick: Tick, value: Option<R>)` — `LC/src/history_buffer.rs:226-234`
  (adds an `Updated`/`Removed` state).
- `pub fn add_update(&mut self, tick: Tick, value: R)` — `LC/src/history_buffer.rs:196-198`.
- `pub fn add_remove(&mut self, tick: Tick)` — `LC/src/history_buffer.rs:200-202`.
- `pub fn add_state(&mut self, tick: Tick, state: HistoryState<R>)` — `LC/src/history_buffer.rs:204-222`
  (lower-level; the above three all funnel through this).
- `pub fn clear(&mut self)` — `LC/src/history_buffer.rs:171-173` (drops ALL entries).
- `pub fn clear_after_tick(&mut self, tick: Tick)` — `LC/src/history_buffer.rs:265-271` (drops
  entries strictly newer than `tick`).
- `pub fn clear_until_tick(&mut self, tick: Tick)` — `LC/src/history_buffer.rs:175-192` (drops
  entries strictly older than `tick`, re-anchoring the effective state AT `tick`).
- `pub fn clear_except_tick(&mut self, tick: Tick) -> Option<HistoryState<R>>` (needs `R: Clone`) —
  `LC/src/history_buffer.rs:364-391`.
- `pub fn pop_until_tick(&mut self, tick: Tick) -> Option<HistoryState<R>>` (needs `R: Clone`) —
  `LC/src/history_buffer.rs:326-351`.
- `pub fn pop(&mut self) -> Option<(Tick, HistoryState<R>)>` — `LC/src/history_buffer.rs:274-276`
  (pops oldest).
- `pub fn set_most_recent_tick(&mut self, tick: Tick)` — `LC/src/history_buffer.rs:245-250`
  (re-anchors the tick of the newest entry without touching its value; `debug_assert!(tick >=
  *most_recent_tick)`).
- `pub fn update_ticks(&mut self, delta: i32)` — `LC/src/history_buffer.rs:253-257` (shifts every
  entry's tick, used only for `SyncEvent`/timeline-jump handling).
- `PredictionHistory` itself additionally exposes `pub fn add_predicted(&mut self, tick: Tick, value:
  Option<C>)` (`LP/src/predicted_history.rs:82-84`), a thin wrapper over `self.add(tick, value)` —
  this is what `update_prediction_history`/`apply_component_removal_predicted` call internally, and
  it is public, so user code CAN manually seed a specific tick's value (e.g. to overwrite a
  known-bad placeholder-era entry) by calling `history.add_predicted(bad_tick, Some(good_value))`
  — though `add`/`add_state`'s implementation (`LC/src/history_buffer.rs:206-222`) only handles
  overwriting the LAST (most recent) entry cleanly (it pops-and-replaces if the new tick equals the
  existing back-most tick); inserting/overwriting an ARBITRARY historical tick in the middle of the
  buffer is not directly supported by any public method — the closest available primitive is
  `clear()` + rebuild, or `clear_after_tick`/`clear_until_tick` to prune around a bad window.
- **No method to seed/clear `ConfirmedHistory<C>` was in scope for this question** (that's a
  separate type, `LC::confirmed_history.rs`, not read in depth for this task — flagged as
  **UNCERTAIN/out of scope** since `DeterministicPredicted` children never populate it, per Q3/Q4).

---
