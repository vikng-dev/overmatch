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
  enable_rollback_after` (`LP/src/rollback.rs:295-298`, `DeterministicPredicted::on_add`).
  **Direction of the split — CORRECTED on second-pass audit (`split_off` returns the TAIL):**
  `partition_point(|(t, _)| *t <= rollback_tick)` (`LP/src/rollback.rs:791-793`) is the index of the
  first entry with `protection_tick > rollback_tick`, so `split_off(split_idx)`
  (`LP/src/rollback.rs:794-796`) moves the **still-protected** entities
  (`protection_tick > rollback_tick`) into `should_disable_rollback`, and those get
  `DisableRollback` **inserted** (`LP/src/rollback.rs:797-801`). The entities that remain in the
  original vec (`protection_tick <= rollback_tick`, i.e. **the grace window has fully elapsed
  relative to the CURRENT rollback's target tick**) get `DisableRollback` **removed**
  (`LP/src/rollback.rs:802-809`), and the vec is then replaced with only the still-protected set
  (`LP/src/rollback.rs:810-811`), so expired entries drop out of tracking permanently.
  (An earlier draft of this section had the two sets swapped; the conclusions below are unchanged —
  protection is stamped while `protection_tick > rollback_tick` and lifted on the first rollback
  frame whose `rollback_tick` has reached `protection_tick`.) **This whole block only executes when `check_rollback` has already decided
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

## Q7 — `lightyear_avian3d`'s per-tick write order for child-collider `Position`/`Rotation` under `AvianReplicationMode::Position`

The enum is named exactly `AvianReplicationMode` with variant `Position` (`LA/src/plugin.rs:98-104`,
`#[default]`) — confirmed matching the repo's usage at `src/net.rs:330`.

### Every system this mode adds/configures that can write child-collider `Position`/`Rotation`

Under `AvianReplicationMode::Position` (`LA/src/plugin.rs:159-218`), with
`update_syncs_manually: false` (the repo's default, `src/net.rs:329-332` doesn't set it):

1. **`transform_to_position`** (avian-native, `AVIAN/src/physics_transform/mod.rs:187-237`) — added
   to `RunFixedMainLoop` in `PhysicsTransformSystems::TransformToPosition`
   (`LA/src/plugin.rs:572-610`, `LightyearAvianPlugin::sync_transform_to_position`). Copies
   `GlobalTransform` → `Position`/`Rotation` for entities that have `Transform` and are not already
   "changed" more recently than `LastPhysicsTick`. **Applies to any entity with `Position`+
   `Rotation`+`GlobalTransform`, not root-only** — so it CAN touch a child collider if its
   `GlobalTransform` differs from its current `Position`/`Rotation` by more than the tolerance
   (`AVIAN/src/physics_transform/mod.rs:202-205`). Runs BEFORE `PhysicsSystems::Prepare` in
   `RunFixedMainLoop` per the `configure_sets` chain at `LA/src/plugin.rs:169-174` (`PhysicsSystems::
   Prepare.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop).before(FrameInterpolationSystems::
   Restore)`), and ALSO configured to run inside `FixedPostUpdate`'s `PhysicsSystems::Prepare`
   (`LA/src/plugin.rs:574-583`, `sync_transform_to_position` configures BOTH `FixedPostUpdate` and
   the caller-supplied `schedule` — here `RunFixedMainLoop`, per the call at line 163).
2. **`position_to_transform`** (avian-native, `AVIAN/src/physics_transform/mod.rs:317-349` for 3D) —
   added to `PostUpdate` in `PhysicsSystems::Writeback` (`LA/src/plugin.rs:613-651`,
   `sync_position_to_transform`, called with schedule=`PostUpdate` at line 175). Writes `Position`/
   `Rotation` → `Transform`, filtered to `Or<(With<RigidBody>, With<ApplyPosToTransform>)>`
   (`AVIAN/src/physics_transform/mod.rs:254-257`) — a plain (non-`RigidBody`) child collider only
   gets this if it independently has `ApplyPosToTransform`, which is required-registered onto any
   entity with `Position` or `Rotation` (`LA/src/plugin.rs:620-623`) — **so YES, child colliders DO
   get `ApplyPosToTransform` and thus DO get this system's writeback**, but this is `Position`/
   `Rotation` → `Transform`, the opposite direction from the placeholder-injection concern.
3. **`Self::add_transform`** (`LA/src/plugin.rs:765-851`) — companion to (2), adds a computed
   `Transform` when `Position`/`Rotation` are both present but `Transform` isn't yet
   (`AVIAN/src/plugin.rs` query filter `Without<Transform>`, `LA/src/plugin.rs:766`); also runs in
   `PostUpdate`. Not itself a Position/Rotation writer.
4. **`sync_received_position_to_transform`** — adds the same `position_to_transform` + `add_transform`
   pair again in `PreUpdate` after `ReplicationSystems::Receive` (`LA/src/plugin.rs:653-661`); this
   is for freshly-received REPLICATED Position/Rotation (interpolated/root entities), not
   specifically child colliders, though the query filter is the same and would match a child if it
   somehow had `RigidBody`/`ApplyPosToTransform` and just received a replicated `Position` (children
   here are NOT replicated individually, only the root is, so this system is largely inert for
   `DeterministicPredicted` children specifically — **UNCERTAIN** whether it could still fire for
   them via broader query matching if `ApplyPosToTransform` alone satisfies the filter — checking
   the filter again: `PosToTransformFilter = (Or<(With<RigidBody>, With<ApplyPosToTransform>)>,
   Or<(Changed<Position>, Changed<Rotation>)>)`, `AVIAN/src/physics_transform/mod.rs:254-257` — since
   `ApplyPosToTransform` is required-registered for ANY entity with `Position` (line 620-623 above),
   **every child collider DOES match this filter's first clause**, so this system runs for children
   too whenever `Position`/`Rotation` "changed" — again, writing Transform FROM Position, not the
   reverse).
5. **`LightyearAvianPlugin::update_child_collider_position`** (`LA/src/plugin.rs:858-888`) — **THE
   system that actually computes child-collider `Position`/`Rotation` FROM the parent body's
   `Position`/`Rotation` and the collider's `ColliderTransform`.** Added to `RunFixedMainLoop`,
   `RunFixedMainLoopSystems::AfterFixedMainLoop` (`LA/src/plugin.rs:187-191`). Query:
   `Query<(&ColliderTransform, &mut Position, &mut Rotation, &ColliderOf), Without<RigidBody>>` —
   explicitly `Without<RigidBody>`, i.e. it only touches non-body colliders (children), looks up the
   parent via `ColliderOf`, and computes:
   ```rust
   position.0 = rb_pos.0 + rb_rot * collider_transform.translation;
   *rotation = (rb_rot.0 * collider_transform.rotation.0).normalize().into();
   ```
   (`LA/src/plugin.rs:876,881-886`). This is the ONLY system among the five that writes a child's
   `Position`/`Rotation` from "parent pose + local offset" — it is the intended replacement for
   avian's own (disabled) internal per-frame child-position sync, per the doc comment right above it
   ("In avian, this is done in `PhysicsSystems::First`, so we need to manually run it after
   `PhysicsSystems` run", `LA/src/plugin.rs:853-857`).
6. **`configure_sets` block, `FixedPostUpdate`**: `(PhysicsSystems::StepSimulation,
   (PredictionSystems::UpdateHistory, FrameInterpolationSystems::Update)).chain()`
   (`LA/src/plugin.rs:193-204`) — governs ordering of history recording relative to physics stepping
   WITHIN `FixedPostUpdate`, but says nothing about `RunFixedMainLoop`.
7. **`configure_sets` block, `PostUpdate`**: `(FrameInterpolationSystems::Interpolate,
   RollbackSystems::VisualCorrection, PhysicsSystems::Writeback, TransformSystems::Propagate).chain()`
   (`LA/src/plugin.rs:205-217`) — visual-only, post-simulation.

### Does anything initialize/write a placeholder pose for freshly-added colliders?

**No — `lightyear_avian3d` itself never writes `Position::PLACEHOLDER`/`Rotation::PLACEHOLDER`; that
sentinel is exclusively an avian-core concept** (required-component default,
`AVIAN/src/collision/collider/backend.rs:97-98`, resolved by avian's own `on_add` hook,
`AVIAN/src/collision/collider/backend.rs:114-121` → `init_physics_transform`,
`AVIAN/src/physics_transform/transform.rs:1116-1275`, all read in Q8 below).
`lightyear_avian3d`'s `update_child_collider_position` (item 5 above) is the only lightyear-side
system that writes a plausible, non-placeholder pose to a child, and it does so **unconditionally
overwriting whatever `Position`/`Rotation` currently holds** — it has no placeholder-detection or
skip-if-parent-not-ready logic; if `rb_pos`/`rb_rot` (queried via `Query<(&Position, &Rotation),
(With<RigidBody>, With<Children>)>`, `LA/src/plugin.rs:869`) resolve successfully, it always writes.
If the PARENT itself doesn't match that query (e.g. parent transiently lacks `Childrenue` for one
frame, or is mid-bind), the child's `Position`/`Rotation` are simply left untouched for that frame
(the `let Ok(...) = rb_query.get(...) else { continue; }` at `LA/src/plugin.rs:872-874` skips
silently) — meaning **a child can go through one or more `RunFixedMainLoop::AfterFixedMainLoop`
passes without this system ever correcting a still-placeholder value**, if the parent doesn't yet
satisfy the query.

### Does it handle child colliders differently from root bodies at all?

Yes, exactly via the `Without<RigidBody>` filter on `update_child_collider_position` — that is the
ENTIRE differentiation. Root bodies (`With<RigidBody>`) get their `Position`/`Rotation` exclusively
from avian's own physics step (`PhysicsSystems::StepSimulation`, inside `FixedPostUpdate`/
`FixedMain`) plus (if manually moved) `transform_to_position`; child colliders get theirs
recomputed every frame, once, from the parent + `ColliderTransform`, in `RunFixedMainLoop::
AfterFixedMainLoop` — **a schedule that runs exactly once per render frame, OUTSIDE `FixedMain`, and
is therefore NEVER re-entered by `run_rollback`'s manual `world.run_schedule(FixedMain)` replay
loop** (verified from `bevy_app` source: `RunFixedMainLoop` "runs the `FixedMain` schedule in a
loop" as its OWN top-level schedule, `bevy_app-0.19.0/src/main_schedule.rs:94-104`; `FixedMain` "runs
first"→`FixedFirst`→...→`FixedLast` chain configured separately,
`bevy_app-0.19.0/src/main_schedule.rs:106-160,316-345`; `RunFixedMainLoopSystems::AfterFixedMainLoop`
doc: "Runs after the fixed update logic. ...runs exactly once per frame, regardless of the number of
fixed updates," `bevy_app-0.19.0/src/main_schedule.rs:401-404,470-491`). This is the single most
important structural fact for Q8.

---

## Q8 — Ranked plausible mechanisms writing a literal `Rotation::PLACEHOLDER` (or NaN) into a live child component around bind-time rollback

### Mechanism 1 (highest confidence): placeholder recorded into `PredictionHistory<Rotation>` before `update_child_collider_position` ever resolves it, then reinjected by `prepare_rollback` the moment the grace window lifts on a rollback frame

**Chain of evidence, fully cited:**
1. Avian's `require`-hook gives every fresh `Collider` a synchronous `Rotation::PLACEHOLDER` the
   instant the collider component is added (`AVIAN/src/collision/collider/backend.rs:97-98`,
   `try_register_required_components_with::<C, Rotation>(|| Rotation::PLACEHOLDER)`).
2. Avian's `on_add` hook for that same collider (`AVIAN/src/collision/collider/backend.rs:114-121`)
   calls `init_physics_transform` (`AVIAN/src/physics_transform/transform.rs:1116-1275`)
   synchronously, in the SAME `DeferredWorld` hook — this is the ONLY place the placeholder is
   normally resolved to a real value for a child, and it happens once, at collider-insertion time
   (via the entity's `ChildOf`/`Transform` chain, NOT `GlobalTransform` propagation, so it doesn't
   depend on `TransformSystems::Propagate` having run — `AVIAN/src/physics_transform/
   transform.rs:1144-1151`).
3. **If this resolution is skipped, delayed, or the entity briefly re-observes the placeholder
   before/without this hook completing** (e.g. `RigidBody` special-case at
   `AVIAN/src/collision/collider/backend.rs:118-121`: `if
   !world.entity(ctx.entity).contains::<RigidBody>() { init_physics_transform(...) }` — **this
   SKIPS `init_physics_transform` entirely for an entity that has BOTH `Collider` and `RigidBody`
   at insertion time**, deferring to whatever inserted `RigidBody` to handle its own pose; a child
   collider normally does NOT carry `RigidBody`, so this branch should not apply to children — but
   it IS the one conditional gap in an otherwise-unconditional resolution, and is worth flagging as
   the most likely SOURCE of a still-placeholder `Rotation` surviving past collider-insertion if the
   binder's insertion order ever puts `RigidBody` on a "child" transiently, or if bevy's required-
   component insertion order interacts unexpectedly with multi-component batch inserts — see the
   Fix Candidates section), the child's `Rotation` remains `PLACEHOLDER` until
   `update_child_collider_position` (`LA/src/plugin.rs:858-888`) next runs, which (per Q7) is in
   `RunFixedMainLoop::AfterFixedMainLoop` — once per frame, and ONLY if the parent body
   simultaneously satisfies `Query<(&Position, &Rotation), (With<RigidBody>, With<Children>)>`
   (`LA/src/plugin.rs:869`).
4. **`update_prediction_history::<Rotation>` (`LP/src/predicted_history.rs:94-123`) runs in
   `FixedPostUpdate`, every tick, unconditionally on `is_changed()`** — since the fresh collider's
   `Rotation` insertion is itself a change, the very next `FixedPostUpdate` after collider-insertion
   records history entry #1 with whatever `Rotation` currently holds. **If that `FixedPostUpdate`
   happens before the next `RunFixedMainLoop::AfterFixedMainLoop` pass has (re-)computed a correct
   child pose — which is entirely possible since `FixedPostUpdate` is nested inside `FixedMain`,
   which itself is nested inside `RunFixedMainLoopSystems::FixedMainLoop`, which runs BEFORE
   `AfterFixedMainLoop` in the SAME `RunFixedMainLoop` pass, meaning on the FIRST frame after
   collider insertion, `FixedPostUpdate` (and thus `update_prediction_history`) systematically
   fires strictly earlier than `update_child_collider_position` for that same frame** (`bevy_app-
   0.19.0/src/main_schedule.rs:401-491`: `BeforeFixedMainLoop` → `FixedMainLoop` (runs `FixedMain`
   zero-or-more times) → `AfterFixedMainLoop`, in that fixed order every single frame) — **history
   entry #1 for a freshly-bound child's `Rotation` is recorded from within the SAME
   `RunFixedMainLoop` pass that will only later, in `AfterFixedMainLoop`, overwrite the placeholder
   with a real value.** If `init_physics_transform`'s synchronous resolution (step 2) already fixed
   it by then, entry #1 is fine; if not (step 3's gap, or any other stall), **entry #1 is
   `Rotation::PLACEHOLDER`, permanently, in history.**
5. **`prepare_rollback::<Rotation>`'s `Without<DisableRollback>` filter (`LP/src/rollback.rs:890`)
   PROTECTS this child from having that bad entry #1 restored back into the live component for the
   ENTIRE `enable_rollback_after` grace window** (default 20 ticks, `LP/src/rollback.rs:264-274`) —
   the repo's own choice of `skip_despawn: true` (`src/net.rs:217-220`) is precisely what creates
   this window (Q3/Q4). **The window ends, unconditionally, at
   `check_rollback`'s`deterministic_skip_despawn` drain (`LP/src/rollback.rs:790-812`), and — as
   established in Q4 — that drain fires ONLY on a frame where `get_rollback_start_tick()` is
   already `Some`, i.e. a rollback is happening THIS SAME frame.** The very first tick
   `DisableRollback` is removed is therefore always immediately followed (same `PreUpdate` pass) by
   `prepare_rollback` now matching the (newly un-filtered) entity, calling `predicted_history.
   get_state(rollback_tick)` (`LP/src/rollback.rs:936`, since this is a `Rollback::FromInputs` or
   non-completed `Rollback::FromState` case for a non-replicated component — `Rotation` here IS
   replicated on the root but the CHILD doesn't carry `ConfirmedHistory<Rotation>` since it's not
   individually replicated, so it always takes the `predicted_history.get_state(rollback_tick)`
   branch, `LP/src/rollback.rs:930-933`), and if `rollback_tick` resolves to (or before, per
   `HistoryBuffer::get_state`'s `partition_point`-based "newest entry ≤ tick" lookup,
   `LC/src/history_buffer.rs:160-168`) history entry #1 — **the PLACEHOLDER — `*predicted_component
   = correct` (`LP/src/rollback.rs:1005`) writes `Rotation::PLACEHOLDER` directly into the live,
   now-active child `Rotation` component, at the EXACT moment the child becomes a "real" rollback
   participant.** This is bit-for-bit consistent with the reported symptom: "a real, non-NaN, but
   wrong sentinel value written into a live component."

**Why this ranks #1:** every link in the chain is independently sourced to a specific line range
(steps 1–5 above), the timing argument (`RunFixedMainLoop` vs `FixedMain` ordering) is drawn
directly from `bevy_app`'s own schedule-ordering documentation/implementation (not inferred), and
the mechanism reproduces BOTH observed variants from the task description:
- **NaN-computed-position variant** (children without their own `Position`, going through
  `ColliderTransform`-relative math): if `ColliderOf::on_insert`
  (`AVIAN/src/collision/collider/collider_hierarchy/mod.rs:82-148`) computes `ColliderTransform`
  from a `GlobalTransform::reparented_to` (line 104) while the PARENT's `GlobalTransform` is itself
  still stale/placeholder-derived (because the parent's own `Position`/`Rotation` hasn't been
  resolved from ITS placeholder yet, in a freshly-spawned batch where parent and children are all
  new in the same frame), `ColliderTransform` itself can carry NaN/garbage, and
  `update_child_collider_position`'s `rb_rot * collider_transform.translation` (`LA/src/
  plugin.rs:876`) then propagates that into the child's computed world `Position` — a downstream
  NaN, not a sentinel, consistent with the first observed variant.
- **Literal-PLACEHOLDER variant** (children given their own explicit `Position`/`Rotation`): the
  above 5-step chain reads back the RAW placeholder value (not a NaN derived from it), consistent
  with the second observed variant, exactly.

### Mechanism 2 (medium-high confidence): stale `ColliderTransform` computed from a not-yet-propagated parent `GlobalTransform`, independent of history/rollback

`ColliderOf::on_insert` (`AVIAN/src/collision/collider/collider_hierarchy/mod.rs:82-148`) computes
`ColliderTransform` from `collider_global_transform.reparented_to(body_global_transform)` (line
96-104), reading `GlobalTransform` on BOTH the collider and the body — if either hasn't yet been
propagated (i.e. `TransformSystems::Propagate` for THIS newly-spawned hierarchy hasn't run since the
binder's insert), this computes from whatever `GlobalTransform` defaults to (identity, or a stale
value from before the bind). This is a real, independently-triggerable source of a wrong
`ColliderTransform` that doesn't require any rollback/history involvement at all — it can corrupt
the child's computed pose on the very first `update_child_collider_position` pass regardless of
prediction. **This ranks below Mechanism 1** because it does not, by itself, explain why the
corruption specifically survives the `enable_rollback_after` grace window and then reappears as
the LITERAL sentinel bit-pattern (a stale/garbage `GlobalTransform`-derived value is not
necessarily `f32::MAX`-exact) — the repo's own experiment (children given explicit `Position`/
`Rotation`, observing the EXACT `Rotation::PLACEHOLDER` sentinel) is much better explained by
Mechanism 1 (history recording literally the sentinel byte pattern) than by an accumulated-transform-
math corruption, which would more likely be a garbage-but-non-sentinel quaternion or a NaN. Still
plausible as a contributing/parallel cause for the NaN variant specifically.

### Mechanism 3 (lower confidence, but structurally real): the `RigidBody`-present skip branch in avian's collider `on_add` hook

`AVIAN/src/collision/collider/backend.rs:118-121`: `init_physics_transform` is explicitly SKIPPED
"to avoid doing this twice for rigid bodies added at the same time" whenever the collider entity
ALSO carries `RigidBody` at `on_add` time. This is a real conditional gap in the placeholder-
resolution path, but **empirically ruled out for this repo**: `RigidBody::Dynamic` is inserted
exactly once, only on the root tank entity, in `spawn_tank` (`src/tank.rs:364-373`, specifically
line 372); child colliders are attached via `ColliderConstructorHierarchy` on `*_Collider`-suffixed
glTF nodes (`src/tank.rs:609-619`), and roadwheels/servos/muzzle get their components via plain
`entity.insert((...))`/`commands.entity(id).insert((...))` calls (`src/tank.rs:598-603,
639-649,671-687`) that never include `RigidBody`. So no rig part other than the root ever carries
`RigidBody`, and this mechanism cannot be firing in the observed bug. Kept in the ranking only as a
documented, ruled-out alternative (in case future refactors of the binder change this).

### Ranking rationale

1 > 2 > 3, based on: (1) how many links in each chain are backed by an exact `path:line` citation
vs. inference, (2) how precisely each mechanism reproduces the EXACT reported sentinel value (not
just "a wrong value"), and (3) how well each explains the specific timing detail that the bug
appears "within a frame of bind" and correlates with the `enable_rollback_after`/rollback-frequency
knobs the repo already varied (per `src/net.rs`'s own comments about the skip_despawn amendment).

---

## Ranked mechanisms + fix candidates

### #1 — Placeholder recorded into `PredictionHistory<Rotation>` (and `Position`) before `update_child_collider_position` resolves it; reinjected by `prepare_rollback` when the grace window lifts on a rollback frame

**Fix candidate A (targeted, lowest-risk): keep the child protected until at least one KNOWN-GOOD
value has been recorded, not just until a fixed tick count has elapsed.**
`DeterministicPredicted::enable_rollback_after` is a plain tick-count
(`LP/src/rollback.rs:264-266`, `u8`) with no concept of "has this child's Position/Rotation ever
been observed non-placeholder." There is no built-in hook for "wait for a condition," so this
requires app-level code: before calling `decorate_rig_children`'s insert, or in a follow-up system,
manually clear/reseed `PredictionHistory<Position>`/`PredictionHistory<Rotation>` for the child
using the PUBLIC `PredictionHistory::add_predicted(tick, Some(value))` (`LP/src/
predicted_history.rs:82-84`) once you've confirmed (by directly reading the live component) that it
no longer equals `Position::PLACEHOLDER`/`Rotation::PLACEHOLDER`. Concretely: add a one-shot system
that runs AFTER `LightyearAvianPlugin::update_child_collider_position`
(`RunFixedMainLoopSystems::AfterFixedMainLoop`) and BEFORE the next `FixedPostUpdate`'s
`update_prediction_history`, which detects `DeterministicPredicted` children whose `Rotation ==
Rotation::PLACEHOLDER` (or `!is_finite()`) and either (a) calls `history.clear()` on their
`PredictionHistory<Rotation>`/`<Position>` so a later `update_prediction_history` call starts a
fresh, correct history once the value resolves, or (b) delays inserting `DeterministicPredicted`
itself until the first tick where both `Position` and `Rotation` are confirmed finite/non-
placeholder (i.e., move the decoration query's `Added<Rig>` gate to also require a "pose resolved"
marker you stamp yourself). **Tradeoff:** (b) is the cleanest but adds a one-tick (or more) extra
delay to when rollback participation begins, and requires a new marker component + system; (a) is
a smaller patch (clear-on-detect) but a `history.clear()` call that races with `update_prediction_
history` in the same tick needs careful system ordering (`.before`/`.after`) to avoid re-recording
the bad value on the same frame.

**Fix candidate B (broader, addresses Mechanism 2 as well): force
`LightyearAvianPlugin::update_child_collider_position`-equivalent resolution to run once,
synchronously, at bind time, before `DeterministicPredicted` is ever inserted** — i.e., in
`decorate_rig_children` (`src/net.rs:211-236`) or a system ordered immediately before it, directly
compute and write the child's `Position`/`Rotation` from `ColliderTransform` + the (already-
propagated, by this point, seconds after spawn) parent `Position`/`Rotation`, THEN insert
`DeterministicPredicted`. This guarantees no PLACEHOLDER (or its downstream NaN) is ever live on the
child at the moment `PredictionHistory` starts recording. **Tradeoff:** duplicates
`update_child_collider_position`'s math in application code (drift risk if lightyear_avian3d changes
that formula); still doesn't address the `RigidBody`-skip gap (Mechanism 3) or a bad
`ColliderTransform` computed before parent `GlobalTransform` propagation (Mechanism 2) unless you
ALSO force a `TransformSystems::Propagate` pass first.

**Fix candidate C (most surgical, addresses the reinjection specifically, not the recording):**
override `enable_rollback_after` to a much larger number is NOT sufficient by itself — Q4
established the drain only fires opportunistically on a rollback frame, so a larger grace period
only delays, but does not prevent, the eventual reinjection of a bad entry #1 if it's still the
oldest entry in history by then. Instead: after the grace window naturally lifts (observe via
polling `DisableRollback`'s removal — there is no event for this, so this requires a custom system
that runs every tick checking `RemovedComponents<DisableRollback>` and, on removal, checks whether
`PredictionHistory<Rotation>::oldest()` (`LC/src/history_buffer.rs:117-119`, public) is the
placeholder and if so calls `.clear()` immediately, in the same `PreUpdate` pass, BEFORE
`RollbackSystems::Rollback` runs. **Tradeoff:** requires ordering your custom system between
`RollbackSystems::RemoveDisable`/the drain point inside `RollbackSystems::Check`, and
`RollbackSystems::Rollback` — tight, fragile ordering against lightyear internals not designed to
be hooked at that granularity (the drain lives inside `check_rollback`, a single monolithic system,
not split into a separately-orderable step, `LP/src/rollback.rs:355-814`).

**Recommended first fix to try: Candidate A(a)** — cheapest, most local, and directly targets the
proven mechanism (bad history entry #1) rather than trying to prevent the placeholder from ever
existing (which fights against avian's own required-component design) or trying to intercept the
crate-internal drain timing (fragile). Concretely: add a `FixedPostUpdate` system, ordered
`.after(PredictionSystems::UpdateHistory)` for one tick delay OR (better) ordered to run in
`RunFixedMainLoop::AfterFixedMainLoop` `.after(LightyearAvianPlugin::update_child_collider_position)`
that, for any `DeterministicPredicted` child whose live `Rotation`/`Position` are currently
placeholder/non-finite, calls `.clear()` on both `PredictionHistory<Position>` and
`PredictionHistory<Rotation>` (accessible since they're `pub` types with `pub` `Deref`/`DerefMut` to
`HistoryBuffer`, and `HistoryBuffer::clear` is `pub fn clear(&mut self)`,
`LC/src/history_buffer.rs:171-173`) — guaranteeing history only ever contains genuinely-resolved
poses by the time the grace window lifts.

### #2 — Stale `ColliderTransform` from not-yet-propagated parent `GlobalTransform`

**Fix candidate:** ensure a `TransformSystems::Propagate` (or avian's own
`PhysicsTransformSystems::Propagate`, since `PhysicsTransformPlugin` is disabled per `src/
net.rs:375-388`'s own comment) pass runs and completes for the newly-bound hierarchy BEFORE
`ColliderOf` is inserted on any child — i.e. insert colliders (and thus trigger `ColliderOf::
on_insert`) only after an explicit `world.run_schedule`/flush that propagates transforms for the
just-spawned subtree. If the binder currently does `commands.spawn(...).insert(Collider)` inside a
single frame alongside reparenting, consider deferring the `Collider`/`ColliderOf` insertion by one
`Update` tick (spawn hierarchy + `Transform`s first tick, insert colliders next tick after
`TransformSystems::Propagate` has run at least once) — mirrors how avian's own
`init_physics_transform` prefers walking `Transform` up `ChildOf` (not `GlobalTransform`) specifically
to sidestep this ordering hazard (`AVIAN/src/physics_transform/transform.rs:1144-1151`), so if the
binder can guarantee `Transform` (not `GlobalTransform`) is correct on insert, this whole mechanism
is moot regardless of propagation timing. **Tradeoff:** adds a frame of latency to bind; requires
restructuring the binder's insertion order, which the task said not to analyze in depth (only
grepped, not read in full) — recommend the repo owner read `tank.rs`'s bind system before committing
to this fix.

### #3 — `RigidBody`-present skip branch in avian's collider `on_add` hook

**Fix candidate:** audit the binder (`tank.rs`, not fully read for this task) to confirm no
non-root rig part (turret/gun/muzzle/roadwheel) ever momentarily carries `RigidBody` alongside
`Collider` in the same `commands.entity(...).insert((...))` bundle or across the same command-flush
boundary. If confirmed absent, this mechanism can be ruled out entirely with no code change. If
present (e.g. as an artifact of a shared spawn-bundle helper), split the bundle so `RigidBody` is
only ever inserted on the true root entity, in a separate `insert()` call that cannot race with a
child's `Collider` insertion. **Tradeoff:** none if the audit clears it; otherwise a straightforward
bundle-splitting refactor with no runtime cost.


---

## Audit addendum (second pass, independent source re-verification)

A second full pass over the vendored sources confirmed Q1–Q6 and Q8's overall ranking, but found
two substantive corrections and one missing fix candidate. Line citations use the same `LP`/`LC`/
`LA`/`AVIAN` roots as above.

### Correction 1 (Q4): direction of the `deterministic_skip_despawn` split

Fixed in place above (`split_off` returns the tail = still-protected set, which gets
`DisableRollback` INSERTED; the head = expired set gets it REMOVED; `LP/src/rollback.rs:790-812`).
Conclusions unchanged.

### Correction 2 (Q7/Q8): avian's own `ColliderTransformPlugin` is NOT disabled and writes child poses INSIDE the physics step — including during rollback replay

Q7's claim that `LightyearAvianPlugin::update_child_collider_position`
(`RunFixedMainLoop::AfterFixedMainLoop`) is "the ONLY system that writes a child's
`Position`/`Rotation` from parent pose + local offset" is **incomplete**. The repo disables exactly
`PhysicsTransformPlugin`, `PhysicsInterpolationPlugin`, `IslandPlugin`, `IslandSleepingPlugin`
(`src/net.rs:478-481`). **`ColliderTransformPlugin` stays enabled**, and its `build`
(`AVIAN/src/collision/collider/collider_transform/plugin.rs:41-64`) adds:

- `propagate_collider_transforms` in `FixedPostUpdate`, `PhysicsTransformSystems::Propagate`
  (`AVIAN/src/collision/collider/collider_transform/plugin.rs:48-53`) — recomputes
  `ColliderTransform` for new/moved collider subtrees (this is the system the repo's earlier
  set-anchoring fix, `src/net.rs:410-424`, re-anchored into `PhysicsSystems::Prepare`).
- **avian's own `update_child_collider_position` INSIDE the `PhysicsSchedule`, at
  `PhysicsStepSystems::First`**
  (`AVIAN/src/collision/collider/collider_transform/plugin.rs:55-63`; function body at
  `AVIAN/src/collision/collider/collider_transform/plugin.rs:66-95`, identical math to the
  lightyear copy: `position.0 = rb_pos.0 + rb_rot * collider_transform.translation`).

Since the `PhysicsSchedule` runs inside `FixedPostUpdate`'s `PhysicsSystems::StepSimulation`, this
means: **on every physics tick — including every `FixedMain` replay tick during a rollback — a
child collider's `Position`/`Rotation` are recomputed from body pose ∘ `ColliderTransform` at the
START of the step, provided the child is (a) visible (not `DisabledDuringRollback`/
`PredictionDisable`) and (b) wired: it has `ColliderTransform` + `ColliderOf` resolving to a body
matching `Query<(&Position, &Rotation), (With<RigidBody>, With<Children>)>`.** Consequences:

1. `PredictionSystems::UpdateHistory` runs after `StepSimulation` (`LA/src/plugin.rs:193-204`), so
   on any *wired* tick the recorded history value is a real, step-start-recomputed pose — the
   Mechanism-1 poison window is therefore NOT "every tick until `RunFixedMainLoop::
   AfterFixedMainLoop`"; it is only the tick(s) where the child exists with `Position`/`Rotation`
   but is **not yet wired**. That un-wired window is real and structural: `ColliderOf` is inserted
   by a **commands-based observer** (`ColliderHierarchyPlugin`,
   `AVIAN/src/collision/collider/collider_hierarchy/plugin.rs:15-40`) — at least one command-flush
   after the collider insert — and `ColliderTransform` arrives as `ColliderOf`'s required component
   (`AVIAN/src/collision/collider/collider_hierarchy/mod.rs:49`, `#[require(ColliderTransform)]`)
   with `ColliderOf::from_world` defaulting to `body: Entity::PLACEHOLDER`
   (`AVIAN/src/collision/collider/collider_hierarchy/mod.rs:58-65`), which fails the body lookup and
   makes both copies of `update_child_collider_position` skip the child silently
   (`AVIAN/src/collision/collider/collider_transform/plugin.rs:79-81` / `LA/src/plugin.rs:872-874`).
2. Because avian's in-step copy writes through `Mut` unconditionally (no change-gate), a wired
   child's `Position`/`Rotation` are flagged changed every tick, so `update_prediction_history`
   keeps refreshing history with real values every wired tick
   (`LP/src/predicted_history.rs:102-105`). A placeholder-era entry can remain the value returned
   by `get_state(rollback_tick)` (newest entry ≤ tick, `LC/src/history_buffer.rs:160-168`) only if
   (a) the rollback target tick lands inside the un-wired window itself, or (b) the child stayed
   un-wired/hidden for the whole stretch between the poison entry and `rollback_tick`. **UNCERTAIN:**
   which of (a)/(b) matched the wheels observation — settling it needs a log of `ColliderOf`
   insertion tick vs. decoration tick vs. the restoring rollback's target tick.
3. During replay, a *grace-protected* child is hidden (`DisabledDuringRollback`), so avian's in-step
   copy skips it and its stale pre-rollback pose persists through the replay (as Q4 stated). A
   *post-grace* child IS re-simulated: `prepare_rollback` restores its pose from history, and
   avian's in-step copy then overwrites it from the replayed body pose each replay tick — so
   post-grace children track the body correctly during replay (better than Q4's "frozen" wording,
   which applies only to protected children).

### Missing fix candidate D (Q8/#1) — remove `PredictionHistory<Position/Rotation>` from decorated children (the repo's `strip_child_pose_history`, already in tree)

The repo already implements the cleanest per-entity-per-component opt-out, and it deserves to be
ranked as the primary fix rather than omitted: **removing `PredictionHistory<C>` from an entity
fully excludes that component from both recording and restore**, because the history component is
itself the membership key of both systems:

- `prepare_rollback<C>`'s query requires `&mut PredictionHistory<C>` (`LP/src/rollback.rs:883-891`)
  — no history component, no restore, ever.
- `update_prediction_history<C>`'s query requires `&mut PredictionHistory<C>`
  (`LP/src/predicted_history.rs:95`) — no history component, no recording.
- The children's poses are DERIVED state (recomputed every wired tick by avian's in-step
  `update_child_collider_position`, per Correction 2, replay included), so pose history on them has
  zero rollback value — nothing is lost by stripping it.
- Safety of the removal window: between decoration and the strip, the worst case is a rollback
  hitting a child whose history exists but is still EMPTY (records only happen in
  `FixedPostUpdate`, which is after `PreUpdate`'s rollback machinery in the same frame) —
  `get_state` on an empty buffer returns `None` (`LC/src/history_buffer.rs:160-168`) and
  `prepare_rollback`'s `None` arm leaves the live value in place (`LP/src/rollback.rs:965-974`), so
  the one-frame polling window of `strip_child_pose_history` (`src/net.rs:255-273`, scheduled in
  `Update`, `src/net.rs:426`) is benign *provided the strip lands before the first `FixedMain` tick
  that records* — which the current `Update` scheduling does NOT strictly guarantee against a
  decoration that lands mid-`FixedMain` (observer commands flush inside `FixedMain`; that same
  `FixedMain` run's later ticks can record before `Update` runs).
- **Hardening (recommended):** convert the polling system into an observer
  `On<Add, (PredictionHistory<Position>, PredictionHistory<Rotation>)>` gated on
  `With<DeterministicPredicted>` + a rig-part marker, which removes the history in the same command
  flush that inserted it — closing the record-before-strip window entirely. Caveat either way:
  lightyear's `add_prediction_history` observer re-attaches history on any later `Add` of `C` /
  `Predicted` / `PreSpawned` / `DeterministicPredicted` / `CatchUpGated` on that entity
  (`LP/src/predicted_history.rs:238-247`), so re-decoration or component re-insertion re-poisons;
  the observer form self-heals against exactly that, the polling form heals one frame later.

**Revised recommendation:** fix candidate D (strip via observer, hardening the in-tree system) as
primary — it removes the entire mechanism-1 class (poison recording AND reinjection) with public,
stable APIs (`EntityCommands::remove`, component types are `pub`), no dependence on lightyear-
internal drain timing, and no duplicated pose math. Candidate A(a) (placeholder-detect +
`history.clear()`) remains a good diagnostic guard; candidates B/C are superseded by D.
