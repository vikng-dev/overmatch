# lightyear 0.28 step-7 map (Overmatch spike)

Research date: 2026-07-03. Follow-on to `lightyear-spike-map.md` (increments 1-6, connectivity
through predicted glb rig). This doc answers the six open questions blocking step 7 (wiring
`SimPlugin` for real: `driving`/`aim`/`shooting`/`damage` under prediction) — see
`lightyear-spike-log.md`'s "Increment 6 verdicts" for the open rollback-rate item this doc
resolves (§1).

Source roots (all under
`/private/tmp/claude-502/.../scratchpad/`, this session's copy symlinked from the prior session's
clone): `lightyear-src/` — clone of `cBournhonesque/lightyear` @ tag `0.28.0`, commit `28e823d`
(same commit the spike map verified). `replicon-src/` — `bevy_replicon` v0.41.0. Vendored cargo
copies at `~/.cargo/registry/src/*/lightyear*-0.28.0/` and `~/.cargo/registry/src/*/avian3d-0.7.0/`
cross-checked for a few spots (noted inline).

Grounding read from the Overmatch repo: `src/net.rs` (current protocol registration + rollback
conditions), `src/lib.rs` (`SimPlugin` composition), `src/shooting.rs` (`fire`, the
`FireShell`-triggering system — the `is_in_rollback` audit target), `src/ballistics.rs`
(`on_fire_shell` observer, `plugin` fn), `src/driving.rs` (`DriveState`, `Suspension`),
`src/tank.rs` (`ServoState` — confirmed at `tank.rs:210-237`, `#[derive(Component)]` only, no
`Clone`/`PartialEq`/`Debug`/`Reflect`; lives on `Turret`/`Gun` child entities per `sight.rs:476-477`
queries `Query<&ServoState, (With<Turret>, Without<Gun>)>`, NOT on the tank root).

---

## 1. Rollback-rate reduction on contact-rich bodies

**Verdict up front**: the 430-vs-13 rollback gap is **not** caused by child colliders leaking into
the prediction/rollback comparison — that's structurally impossible per (a) below. It's real
solver noise on the root body's `Position`/`Rotation`/velocities, amplified by the 16-contact rig,
tripping the same 1 cm / 0.01 rad thresholds the reference examples use unmodified regardless of
body complexity. The fix is threshold/policy tuning, not a registration bug.

### (a) What exactly enters the per-tick comparison

**Only entities carrying the `Predicted` marker (or `PreSpawned`/`DeterministicPredicted`/
`CatchUpGated`) ever get a `PredictionHistory<C>` component, full stop** — this is enforced at the
observer that attaches history, which filters on `On<Add, (C, Predicted, PreSpawned,
DeterministicPredicted, CatchUpGated)>` and then re-checks `Has<Predicted> || Has<PreSpawned> ||
Has<DeterministicPredicted>` (or `CatchUpGated`) on the *same trigger entity* before inserting
`PredictionHistory<C>`.
source: `lightyear-src/crates/replication/prediction/src/predicted_history.rs:237-265`
(`add_prediction_history`)

Child colliders in our rig are separate entities (turret, hull, roadwheels — each with their own
avian-managed `Position`/`Rotation` mirroring world-space pose) that never receive the `Predicted`
marker; only the tank root does. `.component::<Position>().predict()` is a **type-level**
registration (it wires up the observer/systems generically for the `Position` type), but the
observer's own entity-level gate means a `Position` on a non-`Predicted` entity simply never gets
`PredictionHistory<Position>` attached, and therefore never enters `update_prediction_history`
(which only iterates entities that already have both `T` and `PredictionHistory<T>`) or the
rollback-check comparison (`check_rollback_for_unchanged_component`, which explicitly requires both
`ConfirmedHistory<C>` and `PredictionHistory<C>` present, returning false/skip otherwise).
source: `lightyear-src/crates/replication/prediction/src/predicted_history.rs:94-123`
(`update_prediction_history`, unfiltered query but membership requires the history component);
`:316-378` (`check_rollback_for_unchanged_component`, requires both history components present)

**Conclusion: child colliders do not, and structurally cannot, participate in rollback
comparison via this path.** `update_child_collider_position` (the system that keeps child
collider poses correct through rollback replay, `lightyear-src/crates/integration/avian/src/plugin.rs:859-880`)
is a stateless recomputation from parent pose + fixed local offset — it has no history, no
comparison, and runs regardless of rollback state.

### (b) Thresholds in the actual avian examples — complexity does not change them

Checked every avian example in the repo, not just `avian_3d_character`:

- `examples/avian_physics` (2D, single-collider bodies): Position `(a.0-b.0).length() >= 0.01`,
  Rotation `angle_between >= 0.01`, `LinearVelocity`/`AngularVelocity` registered with **no**
  custom condition (defaults to `PartialEq::ne`, exact bit equality).
  source: `lightyear-src/examples/avian_physics/src/protocol.rs:103-109`
- `examples/avian_3d_character` (3D, character capsule + auxiliary child colliders — the closest
  reference to our multi-collider rig): Position `>= 0.01`, Rotation `>= 0.01`, **and** this one
  gives `LinearVelocity`/`AngularVelocity` explicit `>= 0.01` conditions too (matching our own
  `net.rs` registration, already applied per the increment-5/6 bugfix log).
  source: `lightyear-src/examples/avian_3d_character/src/protocol.rs:104-118`

**No example loosens thresholds for a more complex body.** The 1 cm / 0.01 rad bar is applied
uniformly regardless of collider count — lightyear's own reference material does not treat
"more contacts" as a reason for coarser tolerance. This confirms our thresholds are already
"correct" per the reference pattern; the 430 rollbacks are the honest cost of that bar on a
contact-rich body, not a mis-set knob.

### (c) PredictionManager / rollback policy knobs that bound cost

`PredictionManager` (a component on the client's connection entity, alongside `Client`/`Link`)
carries a `RollbackPolicy`:
```rust
pub struct RollbackPolicy {
    pub state: RollbackMode,   // Always / Check / Disabled — default Check
    pub input: RollbackMode,   // Always / Check / Disabled — default Check
    pub max_rollback_ticks: u16, // default 100 — hard cap on re-simulation depth
}
```
source: `lightyear-src/crates/replication/prediction/src/manager.rs:47-78` (`RollbackPolicy`),
`:80-100` (`PredictionManager` fields, incl. `rollback_policy`, `correction_policy`,
`earliest_mismatch_input`, `deterministic_despawn`/`deterministic_skip_despawn`)

Relevant knobs for our problem:
- **`rollback_policy.state = RollbackMode::Disabled`** would stop state-mismatch rollbacks
  entirely for the tank, relying only on input-based rollback (i.e., trust prediction unless the
  *input* history disagrees, never re-simulate purely because Position/Rotation drifted past
  threshold). This is a real, coarse-grained lever — not per-component, whole-policy.
- **`max_rollback_ticks` (default 100)** bounds worst-case re-simulation depth per rollback event
  — already generous headroom for our 100 ms/~7-tick scenario, not the limiting factor here.
- **`DisableRollback` marker** — a per-entity opt-out (excludes an entity from rollback checks
  and effects entirely; during a rollback the entity is tagged `DisabledDuringRollback` and
  skipped by the replay's queries).
  source: `lightyear-src/crates/replication/prediction/src/rollback.rs:303-306`
- **No debounce/hysteresis exists.** Rollback fires on the first tick where any registered
  component's condition trips (OR'd across components) — there's a mismatch-mask
  (`StateRollbackMetadata.mismatch_mask`) to avoid re-checking an already-flagged tick, but no
  "N consecutive mismatches required" mechanism anywhere in the source.
- **`CorrectionPolicy`** (`decay_period`/`decay_ratio`/`max_correction_period`, defaults 200 ms /
  0.5 / 600 s) is orthogonal — it only smooths the *visual* snap after a rollback has already
  happened; it does not reduce rollback count, only its visibility. Already effectively in play
  via our `.add_linear_correction_fn()` calls.
  source: `lightyear-src/crates/replication/prediction/src/correction.rs:195-235`

**Practical recommendation for our tank**: coarsen the Position/Rotation/velocity thresholds
specifically (e.g. 0.03-0.05 m, ~0.03 rad) rather than reaching for `RollbackMode::Disabled` —
the latter throws away state-based correctness entirely (a genuine desync would never
self-correct), which is a bigger behavior change than the CPU problem warrants. Threshold
tuning is the mechanism the reference examples themselves expose via `with_rollback_condition`,
and it's the narrowest fix.

### (d) Rollback CPU metrics beyond `PredictionMetrics.rollbacks`

Yes — richer instrumentation exists, all currently unused by our spike:
- **`PredictionDiagnosticsPlugin`** registers Bevy `Diagnostics` entries: `ROLLBACKS` (count),
  `ROLLBACK_TICKS` (sum of re-simulated tick depth across all rollbacks), and an implied
  `ROLLBACK_DEPTH` (ticks/rollbacks average) — flushed on a 200 ms interval, queryable via
  Bevy's standard `Diagnostics` resource (no new plugin needed, `PredictionPlugin::build` already
  mounts this per the spike log's earlier finding).
  source: `lightyear-src/crates/replication/prediction/src/diagnostics.rs:14-89`
- **Per-phase timers**: `TimerGauge::new("prediction/rollback/check")` around the rollback
  decision system, `TimerGauge::new("prediction::rollback")` around the actual `FixedMain` replay
  loop — both integrate with lightyear's own metrics backend (`lightyear_metrics`), not just logs.
  source: `lightyear-src/crates/replication/prediction/src/rollback.rs:379` (check),
  `:1053` (execute)
- **Tracing spans per replayed tick**: `debug_span!("rollback", tick = ?rollback_tick)` inside
  the replay loop, target `"lightyear_debug::prediction"`, structured fields (`local_tick`,
  `confirmed_tick`, `rollback_tick`, `delta`) — usable with any `tracing` subscriber for
  per-tick timing breakdown, though there's **no per-component or per-entity granularity**: all
  re-simulation for a given rollback still runs inside one `world.run_schedule(FixedMain)` call,
  so you can't isolate "how much of this rollback's cost was the tank's contact solver" from the
  trace alone — you'd need external profiling (e.g. `tracy`) layered on top for that.
  source: `lightyear-src/crates/replication/prediction/src/rollback.rs:1115-1143`,
  `:1172-1173` (`PredictionMetrics` increment site)

**UNCERTAIN**: no source evidence of a way to get "physics-solver-only" cost isolated from
"lightyear's own bookkeeping" cost inside a single rollback tick — the `FixedMain` schedule call
is opaque from lightyear's side. If per-rollback CPU cost needs finer attribution, that's an
external-profiler task, not something lightyear's own diagnostics surface.

---

## 2. Predicted projectile spawning — the shell problem (most important question)

**Verdict up front**: `PreSpawned` is exactly the mechanism designed for this, and the correct
pattern is the **opposite** of `is_in_rollback` gating: always call the spawn code (never gate
`fire`/`on_fire_shell` on `not(is_in_rollback)`), and tag the spawned shell with
`PreSpawned::new(explicit_hash)`. Gating the spawn would actively break replay — lightyear's own
rollback machinery pre-emptively despawns unmatched/stale `PreSpawned` entities before every
replay pass specifically so the (ungated) spawn code can safely re-create exactly one canonical
entity per logical fire event.

### 1. Exact `PreSpawned` API in 0.28

Plain `Component`, not an observer/event:
```rust
#[derive(Component)]
#[component(on_add = PreSpawned::on_add)]
#[reflect(Component, Default)]
pub struct PreSpawned {
    pub hash: Option<u64>,
    pub user_salt: Option<u64>,
    pub receiver: Option<Entity>,
}
```
source: `lightyear-src/crates/replication/replication/src/prespawn.rs:267-286`

Constructors:
```rust
impl PreSpawned {
    pub fn new(hash: u64) -> Self { ... }               // explicit hash — what we want
    pub fn default_with_salt(salt: u64) -> Self { ... }  // default archetype hash + salt
    pub fn for_receiver(self, entity: Entity) -> Self { ... }
}
```
source: `prespawn.rs:288-313`

`PreSpawned::default()` (no explicit hash) computes one from `(spawn tick, sorted component-type
list, optional salt)` via `SeaHasher` — but the doc comment explicitly warns this "only works
currently for entities that are spawned during `FixedMain`" and is fragile to
rollback-replay/component-timing differences, which is exactly why real examples avoid it (see
§2 below).
source: `prespawn.rs:504-539, 544-606` (`compute_default_hash`); `:561-563` (fragility caveat)

Companion type `PreSpawnedReceiver` — a `Component` on the connection/link entity holding the
hash→entity matching tables.
source: `prespawn.rs:317-337`

Prespawned entities **must** be spawned inside `FixedMain` (documented constraint, matches our
`fire`/`on_fire_shell` being `FixedUpdate` systems already).
source: `prespawn.rs:252`

### 2. The spaceships demo's actual bullet-spawn code

One unified system, `shared_player_firing` (`FixedUpdate`, both client and server run it — no
separate "predict-spawn" vs "authoritative-spawn" code path; `is_server`/`is_local` bools only
gate the *final* replication-marker insertion, not the spawn itself):

```rust
// A bullet is uniquely identified by the owner and the simulation tick
// that fired it. Use an explicit hash instead of the default
// archetype-based hash so rollback replay/component timing cannot make
// the local prespawn disagree with the server spawn.
let prespawn_hash = bullet_prespawn_hash(player.client_id, current_tick);
let prespawned = PreSpawned::new(prespawn_hash);
```
```rust
fn bullet_prespawn_hash(owner: PeerId, tick: Tick) -> u64 {
    let mut x = owner.to_bits() ^ ((tick.0 as u64) << 32) ^ tick.0 as u64;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}
```
Keyed purely on `(owner client/peer id, fire tick)` — no spawn-order index, no other salt.
source: `lightyear-src/demos/spaceships/src/shared.rs:341-346` (comment + hash construction),
`:395-402` (`bullet_prespawn_hash`)

Spawn call, **identical on client and server**:
```rust
let bullet_entity = commands
    .spawn((
        Position(bullet_origin),
        LinearVelocity(bullet_linvel),
        ColorComponent((color.0.to_linear() * 5.0).into()),
        BulletLifetime { origin_tick: current_tick, lifetime: FIXED_TIMESTEP_HZ as i32 * 2 },
        BulletMarker::new(player.client_id),
        PhysicsBundle::bullet(),
        bullet_mass_properties(),
        prespawned,
    ))
    .id();
```
Only the server then adds replication markers:
```rust
if is_server {
    commands.entity(bullet_entity).insert((
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::All),
    ));
}
```
source: `demos/spaceships/src/shared.rs:348-362` (spawn), `:385-391` (server-only replication
markers)

**No `is_in_rollback` gating anywhere in this system.** It's gated only by gameplay-state
conditions (fire button held + cooldown check) — the same shape our own `fire`'s
`triggered`/`reload.remaining > 0.0`/`requirement_met` gate already has. Cross-checked against
`examples/fps/src/shared.rs:573-631` (same unconditional-spawn-both-sides pattern, `grep` confirms
zero `is_in_rollback` occurrences in that file).

### 3. What happens when the server disagrees (predicted fire never actually happens)

Two distinct, both source-confirmed, cleanup paths:

**(A) Timeout, no server spawn message ever arrives.** `pre_spawned_player_object_cleanup`
(`PostUpdate`, `PreSpawnedSystems::CleanUp`) despawns any locally-spawned, still-unmatched
`PreSpawned` entity older than `tick - 50`:
```rust
let past_tick = tick - 50;
let split_idx = manager.prespawn_tick_to_hash.partition_point(|(t, _, _)| *t < past_tick);
let expired = manager.prespawn_tick_to_hash.drain(..split_idx).collect::<Vec<_>>();
for (_, hash, entity) in expired {
    manager.remove_unmatched_entity(hash, entity);
    if let Ok(mut entity_commands) = commands.get_entity(entity) {
        entity_commands.despawn();
    }
}
```
source: `prespawn.rs:87-102` (system registration), `:165-200` (body); confirmed by test
`test_prespawn_local_despawn_no_match` (`lightyear-src/crates/tests/src/client_server/prediction/prespawn.rs:596-637`)

**(B) Rollback-driven cleanup (state mismatch reveals the fire shouldn't have happened).**
`PreSpawnedReceiver::despawn_prespawned_after` — called unconditionally from the rollback-check
system, **before** the replay loop, for every `PreSpawned` entity (matched or not) spawned at a
tick ≥ the rollback target:
```rust
/// Despawn all local PreSpawned entities spawned at a tick >= Tick.
/// This includes already matched entities, because rollback replay must be
/// able to recreate entities spawned after the rollback tick instead of
/// leaving the previous matched instance live.
pub fn despawn_prespawned_after(&mut self, tick: Tick, commands: &mut Commands) {
    self.despawn_prespawned_after_with(tick, |_| false, commands);
}
```
Called from the rollback trigger site:
```rust
if let Some(rollback_tick) = prediction_manager.get_rollback_start_tick() {
    debug!(?rollback_tick, "Rollback! Despawning all PreSpawned/DeterministicPredicted entities spawned after the rollback tick");
    prespawned_receiver.despawn_prespawned_after_with(
        rollback_tick + 1,
        |entity| protected_prespawn_entities.contains(&entity) || ...,
        &mut commands,
    );
```
source: `prespawn.rs:396-451` (`despawn_prespawned_after`/`_with`),
`lightyear-src/crates/replication/prediction/src/rollback.rs:726-749` (call site)

So: if replay of the fire tick now evaluates the trigger condition as false (server-confirmed
state disagrees), `fire`/`on_fire_shell` simply doesn't call `commands.spawn(...)` again on that
replayed tick — and since the stale entity was **already despawned** by (B) before the replay
loop started, no replacement is created. **The shot silently vanishes with no explicit "rejected"
message** — it's implicit non-recreation.

### 4. Is `is_in_rollback` gating right, or does it break replay determinism?

**It breaks replay determinism. Do not gate the spawn.** Full reasoning, source-backed:

- Gating the *original* (non-replay) tick's spawn on `not(is_in_rollback)` is indeed a no-op for
  that tick, as reasoned — `is_in_rollback` is only true during the replay loop.
- The real failure mode is on **replay** of the same tick: `RollbackSystems::Check` (which runs
  in `PreUpdate`, *before* the replay loop, ordered via `.chain()`) **unconditionally despawns**
  any `PreSpawned` entity — matched or not — whose spawn tick is ≥ the rollback tick (quoted in
  §3(B) above). This happens regardless of what your own spawn system's run condition is. So if
  `fire`'s spawn call is gated `.run_if(not(is_in_rollback))`: the original tick-N shell gets
  despawned pre-replay (its spawn tick ≥ rollback_tick), then replay reaches tick N with the gate
  active, skips the spawn — **and the shell never comes back**, even though the fire input at
  tick N is still `true` and would otherwise re-evaluate identically. This is confirmed directly
  by test `test_matched_prespawn_despawned_on_rollback_before_spawn_tick`
  (`lightyear-src/crates/tests/src/client_server/prediction/prespawn.rs:296-381`), which explicitly
  re-adds the (ungated) spawn system before triggering rollback and asserts exactly one fresh
  replacement entity exists afterward ("rollback replay should spawn exactly one replacement
  prespawn", line 375).
- **The correct pattern**: always call the spawn code unconditionally (same code path, original
  tick and every replay of that tick). `PreSpawned`'s hash is what prevents duplicates:
  1. Original tick N: `fire` spawns shell A, `PreSpawned::new(hash(tick, owner))`.
  2. A later rollback targets an earlier tick ≤ N: `despawn_prespawned_after_with` despawns A
     (spawn tick ≥ rollback_tick+1) *before* replay starts.
  3. Replay reaches tick N again. `fire` re-runs unconditionally, re-evaluates gate state
     (reload/requirement/trigger, now from correctly-restored predicted state — see §3 of Q3 for
     what "correctly restored" requires): either it still fires → spawns shell B with the *same*
     hash A had (A is already gone, no duplicate, this is the intended single-logical-shot
     outcome) — or it no longer fires (corrected state says it shouldn't have) → nothing spawns,
     and since A was already despawned in step 2, the shot correctly and silently vanishes.
  4. Two live entities for one logical tick's shot cannot occur: `ActivePreSpawnedSignatures`
     rejects a second *simultaneously live* entity claiming an already-active hash — but this
     never arises in the replay flow because A is always torn down before B is created (never
     coexist). Confirmed by `test_prespawn_reuses_hash_after_unmatched_local_despawn`
     (`prespawn.rs:166-227`): after entity 1 (hash `1`) is despawned, a second local spawn with
     the *same* hash `1` succeeds and matches the server entity — hash reuse across sequential
     (non-concurrent) spawns is the designed, supported pattern.
     source (duplicate rejection): `prespawn.rs:133-141, 148-159`

**Design directive for our code**: do not gate `fire` or `on_fire_shell` on `is_in_rollback`. Add
`PreSpawned::new(explicit_hash)` to the bundle `on_fire_shell` spawns
(`src/ballistics.rs:419-439`), with `explicit_hash` a deterministic function of `(firing tank's
network/peer id, fire tick, per-weapon slot if a tank can fire two guns same tick)` — mirroring
`bullet_prespawn_hash` exactly, not the default archetype hash (too fragile per the doc caveat).
lightyear's own `RollbackSystems::Check` ordering guarantees the stale entity is torn down before
`fire` gets a chance to re-trigger — no manual "is this a replay" detection needed in our spawn
code at all.

**Observer-pattern note**: our `fire` calls `commands.trigger(FireShell {...})`
(`src/shooting.rs:133-139`), and a separate observer `on_fire_shell` does the actual
`commands.spawn(...)` (`src/ballistics.rs:419-439`) — an extra indirection vs. the demos' direct
inline spawn. This is orthogonal to the rollback story: Bevy observers fire synchronously within
the same schedule execution as any other system, so triggering during a `FixedMain` replay pass
behaves identically to the original pass. The `PreSpawned` component just needs to land on
whatever `commands.spawn(...)` call actually creates the entity — i.e., inside `on_fire_shell`,
not on the `FireShell` event itself.

**UNCERTAIN**: no lightyear source directly exercises the observer-trigger-spawns-entity pattern
(all real examples spawn directly in a plain system, not via an event→observer indirection). The
ordering guarantee (`RollbackSystems::Check` before `RollbackSystems::Rollback`, both in
`PreUpdate`, upstream of `FixedMain`) should apply identically regardless of *how* the spawn is
invoked within `FixedMain`, but this specific shape (trigger → observer → spawn) hasn't been
proven empirically anywhere in the reference material — validate with an actual forced-rollback
spike once `FireShell`/`PreSpawned` wiring lands, same as increment 5/6 validated other claims
empirically.

---

## 3. `local_rollback::<C>()` for `ServoState`/`DriveState`/`Reload`/`Suspension`

**Verdict up front, most consequential finding of this doc**: `local_rollback::<C>()` is strictly
entity-local — it requires `C` to live on the SAME entity that carries `Predicted` (or
`PreSpawned`/`DeterministicPredicted`/`CatchUpGated`). **There is no hierarchy-aware mechanism
anywhere in the prediction crate.** Of our four target types, only `DriveState` (lives on the tank
root itself) is a drop-in fit. `ServoState` (turret/gun children), `Reload` (weapon/muzzle child),
and `Suspension` (per-roadwheel children) all live on entities that do **not** carry `Predicted`
today — as designed, `local_rollback` silently does nothing useful for them (their history is
simply never attached, no error, no panic).

### 1. Exact API + trait bounds

Two entry points, both in `lightyear_prediction::registry`:

**(a) `App::local_rollback::<C>()`** (the one every real call site uses):
```rust
pub trait PredictionAppRegistrationExt {
    fn local_rollback<C: SyncComponent>(&mut self) -> LocalRollbackComponentRegistration<'_, C>;
}
impl PredictionAppRegistrationExt for App {
    fn local_rollback<C: SyncComponent>(&mut self) -> LocalRollbackComponentRegistration<'_, C> {
        LocalRollbackComponentRegistration::new(add_local_rollback::<C>(self))
    }
}
```
```rust
pub trait SyncComponent: Component<Mutability = Mutable> + Clone + PartialEq + Debug {}
impl<T> SyncComponent for T where T: Component<Mutability = Mutable> + Clone + PartialEq + Debug {}
```
source: `lightyear-src/crates/replication/prediction/src/registry.rs:1009-1012, 1053-1056`;
`lightyear-src/crates/replication/prediction/src/lib.rs:53-54`

Required bounds: `Component<Mutability = Mutable>` (not `#[component(immutable)]`) + `Clone` +
`PartialEq` + `Debug`. **No `Reflect`, no `Default`.**

**(b) `ComponentRegistrator::local_rollback()`** (builder-chain variant, looser bound —
`Component<Mutability = Mutable> + Clone` only, no `PartialEq`/`Debug` — used for internal avian
resources like `ContactGraph` that don't implement `PartialEq`).
source: `registry.rs:699-701, 726-733`

Both funnel into:
```rust
pub fn add_non_networked_rollback_systems<C: Component<Mutability = Mutable> + Clone>(app: &mut App) {
    app.world_mut().register_component::<PredictionHistory<C>>();
    app.world_mut().register_component::<ConfirmedHistory<C>>();
    app.add_observer(apply_component_removal_predicted::<C>);
    app.add_observer(add_prediction_history::<C>);
    app.add_observer(handle_tick_event_prediction_history::<C>);
    app.add_systems(PreUpdate, prepare_rollback::<C>.in_set(RollbackSystems::Prepare));
    app.add_systems(FixedPostUpdate, update_prediction_history::<C>.in_set(PredictionSystems::UpdateHistory));
}
```
source: `lightyear-src/crates/replication/prediction/src/plugin.rs:75-100`

**Our types**: `DriveState` (`throttle: f32, steer: f32`) needs `Clone + PartialEq + Debug` added
— trivial derives. `ServoState`, `Reload`, `Suspension` need the same derives, but see §3 below —
deriving the traits doesn't help if they're on the wrong entity.

### 2. Where registration lives

`local_rollback::<C>()` internally checks for `PredictionRegistry` and no-ops (skips
observer/system registration) if `PredictionPlugin` (client-only) hasn't run yet:
```rust
fn add_local_rollback<C: SyncComponent>(app: &mut App) -> ComponentRegistration<'_, C> {
    if app.world().get_resource::<PredictionRegistry>().is_none() {
        return ComponentRegistration::<C>::new(app);
    }
    register_prediction_metadata::<C>(app);
    add_non_networked_rollback_systems::<C>(app);
    ComponentRegistration::<C>::new(app)
}
```
source: `registry.rs:1044-1051`

This means it's **safe to call unconditionally from a shared `ProtocolPlugin`** mounted by both
client and server — it silently no-ops on the server (no `PredictionPlugin` there), matching how
`examples/deterministic_replication/src/protocol.rs:107-136` calls it unconditionally from shared
code. No explicit ordering rule vs. `ClientPlugins`/spawning the connection entity is enforced in
code (unlike `.replicate()`'s ordering rule) — it just needs `PredictionPlugin` to have already
inserted `PredictionRegistry`, i.e. called after `ClientPlugins` is added. Standard practice
(every real call site) is during plugin `build()`.

### 3. THE CRITICAL QUESTION — does it work for components on CHILD entities?

**No. Definitively no, confirmed three independent ways:**

**(a) The history-attach gate checks only the trigger entity, no hierarchy traversal:**
```rust
pub(crate) fn add_prediction_history<C: Component + Clone>(
    trigger: On<Add, (C, Predicted, PreSpawned, DeterministicPredicted, CatchUpGated)>,
    query: Query<(Has<C>, Has<Predicted>, Has<PreSpawned>, Has<DeterministicPredicted>, Has<CatchUpGated>)>,
    mut commands: Commands,
) {
    let Ok((has_component, predicted, prespawned, deterministic, catchup_gated)) = query.get(trigger.entity) else { return; };
    if !catchup_gated && !(has_component && (predicted || prespawned || deterministic)) { return; }
    ...
```
source: `predicted_history.rs:237-264`. `trigger.entity` is checked directly, no `ChildOf`/
`Children` lookup anywhere. If `C` is added to a child that never itself gets `Predicted` (or
`PreSpawned`/`DeterministicPredicted`/`CatchUpGated`), this observer returns early — that child's
`C` never gets `PredictionHistory<C>`, permanently, silently.

**(b) The read/write systems match on "has both `C` and `PredictionHistory<C>` on the same
entity", never on ancestry:**
```rust
pub(crate) fn update_prediction_history<T: Component + Clone>(
    mut query: Query<(Entity, Ref<T>, &mut PredictionHistory<T>)>,
    timeline: Res<LocalTimeline>,
) { ... }
```
source: `predicted_history.rs:94-97` — no `With<Predicted>` filter needed because membership is
already fully determined by (a).

**(c) Cross-check against the general pattern**: `snap_to_confirmed_during_rollback` (for
replicated predicted components) explicitly filters `With<Predicted>` directly on the query — the
crate's universal pattern for "does this apply to a predicted entity" is always a same-entity
marker check, never hierarchy traversal.
source: `predicted_history.rs:382-390`

**Exhaustive grep across the whole prediction crate: zero `ChildOf`/`Children` usage.** The only
`ChildOf`/`Children` usage anywhere near this problem is in `lightyear_avian3d`, for
`update_child_collider_position` — and that system is **not a rollback mechanism**, it's a
stateless kinematic re-derivation:
```rust
pub fn update_child_collider_position(
    mut collider_query: Query<(&ColliderTransform, &mut Position, &mut Rotation, &ColliderOf), Without<RigidBody>>,
    rb_query: Query<(&Position, &Rotation), (With<RigidBody>, With<Children>)>,
) {
    for (collider_transform, mut position, mut rotation, collider_of) in &mut collider_query {
        let Ok((rb_pos, rb_rot)) = rb_query.get(collider_of.body) else { continue; };
        position.0 = rb_pos.0 + rb_rot * collider_transform.translation;
        ...
```
source: `lightyear-src/crates/integration/avian/src/plugin.rs:859-880` (identical in
`~/.cargo/registry/.../lightyear_avian3d-0.28.0/src/plugin.rs:859-880`)

This works precisely *because* child collider pose has zero independent dynamics — it's
`parent_position + parent_rotation * static_local_offset`, recomputed fresh every tick, no
history needed. **This does not generalize to `ServoState`**: its `current`/`previous`/`velocity`/
`captured` fields are integrated over time with their own dynamics (spring-damper toward a
target), not a pure function of the tank root's `Position`/`Rotation`. If an upstream input change
during rollback replay affects the turret's target angle, there's no "recompute from parent"
formula — the servo's own integration state must itself be rolled back and re-simulated
tick-by-tick, which requires `PredictionHistory<ServoState>`, which requires `ServoState`'s entity
to carry `Predicted` (or equivalent) directly.

**No alternative "children of a predicted root also participate" mechanism exists anywhere in the
codebase** — no propagation of `Predicted` to descendants, no cascading ownership marker. Only
opt-**out** exists (`DisableRollback`/`DisabledDuringRollback`, for entities that already have
history and want to skip it) — never opt-in-via-ancestry.

**This changes our step-7 design, as flagged.** Two real options given the actual API surface:

- **Option A — mark children `Predicted` directly.** Insert `Predicted` (or `CatchUpGated`, if
  that fits our catch-up semantics better — not otherwise researched here) on the turret/gun/
  muzzle/roadwheel child entities at spawn time, mirroring how avian's own per-collider rollback
  state (`ColliderAabb`, `EnlargedAabb`, `CollidingEntities`) works — those are legitimately
  per-collider-**entity** and use `local_rollback` directly on the collider entities themselves
  (which, notably, DO carry appropriate markers in lightyear_avian3d's own setup — this is
  precedent that "give the child its own marker" is the supported pattern, not a workaround).
  Requires our spawn/binder code to insert `Predicted` on these children itself — nothing
  automatic does it — and requires client/server agreement on which children get marked (same
  concern as any other spawn-time replication decision).
- **Option B — denormalize onto the root.** Store `ServoState`/`Suspension`/`Reload` as a single
  aggregate component on the tank root (e.g. keyed by child entity or a stable index), so one
  `local_rollback::<C>()` call on the root captures everything in one `PredictionHistory`. Bigger
  refactor (breaks the current "component lives where the mechanism lives" shape that `sight.rs`/
  `damage.rs` query against), but avoids multiplying `Predicted` markers across the rig.

Option A is more idiomatic given the avian-integration precedent and preserves our existing
component-per-node shape; it's the one to prototype first in step 7.

### 4. `is_in_rollback` — import path + canonical usage

Confirmed current: `lightyear_core::timeline::is_in_rollback` (the prior map's citation is
correct and still valid in 0.28.0):
```rust
pub fn is_in_rollback(client: Query<(), With<Rollback>>) -> bool {
    client.single().is_ok()
}
```
source: `lightyear-src/crates/core/core/src/timeline.rs:236-239`

Both `lightyear::core::timeline::is_in_rollback` and `lightyear_core::prelude::is_in_rollback`
resolve to the same function and are used interchangeably in the tree (e.g.
`crates/inputs/input_bei/src/plugin.rs:28` vs. `demos/spaceships/src/client.rs:6`).

Real call sites:
```rust
// frame_interpolation — perf gate, not correctness
app.configure_sets(FixedLast, FrameInterpolationSystems::Update.run_if(not(is_in_rollback)));
```
"We don't run UpdateVisualInterpolationState in rollback because that would be a waste to do it
for each rollback frame." source: `lightyear-src/crates/core/frame_interpolation/src/lib.rs:115-121`

```rust
// input_bei — genuine correctness gate: replay must use HISTORICAL input, not live device state
app.configure_sets(FixedPreUpdate, (
    EnhancedInputSystems::Update.run_if(not(is_in_rollback)), // do not run Update during rollback as we already know all inputs
    InputSystems::BufferClientInputs,
    EnhancedInputSystems::Apply,
).chain());
```
source: `lightyear-src/crates/inputs/input_bei/src/plugin.rs:161-171`

```rust
// avian integration — positive gate: only needed DURING rollback, to rebuild broadphase before replay
app.add_systems(PreUpdate, Self::restore_collider_tree_from_enlarged_aabbs
    .after(RollbackSystems::Prepare).before(RollbackSystems::Rollback).run_if(is_in_rollback));
```
source: `lightyear-src/crates/integration/avian/src/plugin.rs:374-380`

**Decision framework** (grounded in: rollback restores whatever has `PredictionHistory<C>`, then
re-runs `FixedMain` only; a system re-running against *correctly restored* state produces correct
re-simulation, not a duplicate side effect — the only things needing `not(is_in_rollback)` gating
are true externalities with no history-backed restoration):

- **VFX/audio spawns (muzzle flash, gunshot sound)** — **gate.** `Commands::spawn` with no
  `PredictionHistory` participation; replay would spawn a second flash/sound for a shot that
  already visually happened, and rollback never retroactively removes the first one. Same class as
  the `frame_interpolation` example above.
- **HUD writes** — **gate only if non-idempotent (accumulate/append).** A HUD readout that's
  simply overwritten each tick from current predicted values (e.g. "ammo count = Reload.remaining")
  needs no gate — replay reproduces the correct value from correctly-restored state. A kill-feed
  push or hit-marker flash trigger is a one-shot externality like VFX — gate it.
- **One-shot recoil impulse (`Forces::apply_linear_impulse` in `fire`)** — **no gate needed**,
  provided `LinearVelocity`/`AngularVelocity` are themselves rollback-participant (already true,
  per our `net.rs` registration) and the firing decision is derived from replayed, deterministic
  state (restored `Reload`, replayed `TankCommand`) rather than a one-shot side channel. The
  impulse is applied to a component whose pre-impulse value was itself correctly restored before
  replay, so reapplying the same delta-v on replay reproduces the same post-impulse value — this
  is correct re-simulation, not a double-apply.
- **Reload timer decrement** — **no gate needed, IF `Reload` is registered via
  `local_rollback::<Reload>()`** (which — per §3 above — requires first solving the
  child-entity problem, Option A or B). Once registered, `Reload.remaining`'s rollback-tick value
  is restored from `PredictionHistory<Reload>` before replay, so the decrement system re-running
  during replay operates on the correct value and reproduces the original result tick-by-tick —
  gating this would actually be a **bug** (would leave the value stale through replay). If
  `Reload` is left unregistered (the child-entity problem unsolved), it is NOT restored, and the
  decrement system re-running during replay would double-decrement from an un-rolled-back value —
  in that fallback case `is_in_rollback` gating is a stopgap, but it masks the deeper problem
  (state genuinely not history-tracked) rather than fixing it; solving §3 is the real fix.

**General rule**: gate with `not(is_in_rollback)` only when a side effect (a) isn't captured by
any `PredictionHistory<C>`-tracked component, and (b) would be wrong/duplicated/stale if re-run
against replayed ticks. Everything computed purely from rollback-participating state needs no
gating — that's the entire point of rollback+replay.

---

## 4. `is_in_rollback` gating — which of our system classes need it (summary table)

| System | Gate? | Why |
|---|---|---|
| `on_fire_shell` shell spawn | **No** — tag `PreSpawned` instead | §2 Q4: gating breaks replay; hash-dedup is the correct mechanism |
| Muzzle flash / fire sound (future) | **Yes** | one-shot externality, no history |
| HUD ammo/reload readout | No | idempotent overwrite from restored state |
| HUD kill-feed / hit-marker push | Yes | accumulate/append, not idempotent |
| Recoil impulse (`apply_recoil` kick) | No | feeds into rollback-participant velocity components |
| `Reload.remaining` decrement | No, once `local_rollback::<Reload>()` lands (§3) | correct re-simulation once history-tracked |
| `ServoState` integration (`drive_servos`, not yet read) | No, once §3 Option A/B lands | same reasoning as Reload |
| `DriveState` ramp (`ramp_drive`) | No, register via `local_rollback::<DriveState>()` | lives on root already — straightforward fit |
| `Suspension` spring solve | No, once §3 Option A/B lands | per-wheel dynamics need history like ServoState |

---

## 5. Input delay & prediction config

### 1. Full `InputDelayConfig` definition

```rust
#[derive(Debug, Clone, Copy, Reflect)]
pub struct InputDelayConfig {
    /// Minimum number of input delay ticks applied, regardless of latency. Almost always 0.
    pub minimum_input_delay_ticks: u16,
    /// Maximum input delay (ticks) applied to cover latency before prediction kicks in.
    /// Default 3 (~50ms @ 60Hz).
    pub maximum_input_delay_before_prediction: u16,
    /// How far ahead the client is allowed to predict (bounds max rollback ticks).
    /// Default 7 (~100ms @ 60Hz).
    pub maximum_predicted_ticks: u16,
}
```
source: `lightyear-src/crates/core/sync/src/timeline/input.rs:142-173`

Three fields, all `u16` tick counts — no direct single "delay ticks" field, but see `2` below for
the constructor that produces exactly that effect. The actual per-tick computed delay lives
separately in `InputTimeline.input_delay_ticks`, recomputed each sync event by
`InputDelayConfig::input_delay_ticks()`.
source: `crates/core/sync/src/timeline/input.rs:58,66,75-76,93` (`InputTimeline` fields),
`:222-259` (`input_delay_ticks()` computation)

### 2. Constructors — including the one we want

```rust
impl InputDelayConfig {
    pub fn balanced() -> Self { Self { minimum_input_delay_ticks: 0, maximum_input_delay_before_prediction: 3, maximum_predicted_ticks: 7 } }
    pub fn no_input_delay() -> Self { Self { minimum_input_delay_ticks: 0, maximum_input_delay_before_prediction: 0, maximum_predicted_ticks: 100 } }
    pub fn no_prediction() -> Self { Self { minimum_input_delay_ticks: 0, maximum_input_delay_before_prediction: 0, maximum_predicted_ticks: 0 } }
    pub fn fixed_input_delay(delay_ticks: u16) -> Self {
        Self { minimum_input_delay_ticks: delay_ticks, maximum_input_delay_before_prediction: delay_ticks, maximum_predicted_ticks: 100 }
    }
    pub fn is_lockstep(&self) -> bool { self.maximum_predicted_ticks == 0 }
}
```
source: `crates/core/sync/src/timeline/input.rs:175-220`

**`fixed_input_delay(delay_ticks: u16)` is exactly the "delay input by 1-2 ticks to shrink the
prediction window" knob** — pins both min/max input-delay fields to `delay_ticks`, leaves
`maximum_predicted_ticks: 100` so anything beyond the fixed delay still falls back to prediction
rather than growing input delay further. Real usage: `demos/spaceships/src/main.rs:78`
(`fixed_input_delay(10)`), `examples/deterministic_replication/src/main.rs:95`, `examples/
avian_3d_character/src/main.rs:72` (`fixed_input_delay(0)`), `examples/projectiles/src/server.rs:469`.

### 3. Attach point

Not a resource, not a `PredictionManager` field. Wrapped in `InputTimelineConfig` (a `Component`,
`#[require(InputTimeline)]`), inserted on the **`Client` entity** — the same entity that already
carries `Client`/`Link`/`PredictionManager`/`NetcodeClient` in our current setup:
```rust
fn configure_input_delay(client: Single<Entity, With<Client>>, mut commands: Commands) {
    commands.entity(client.into_inner()).insert(
        InputTimelineConfig::default().with_input_delay(InputDelayConfig::no_input_delay()),
    );
}
```
source: `lightyear-src/examples/simple_box/src/client.rs:37-41`; entity composition confirmed at
`examples/common/src/client.rs:51-58` (`Client::default(), Link::new(...), ..., PredictionManager::default()`
all on one entity — `InputTimelineConfig` lands there via a later additive `.insert()`).

For our step-7 tuning: `commands.entity(client_entity).insert(InputTimelineConfig::default()
.with_input_delay(InputDelayConfig::fixed_input_delay(1)))` (or `2`), inserted once after spawning
our client connection entity, same file/timing as our existing `PredictionManager::default()`
insert in the client bin.

### 4. Tradeoff, confirmed in doc comments

> "Input delay can be ideal in low-latency situations to avoid rollbacks and networking artifacts,
> but it must be balanced against the responsiveness of the game. Even at higher latencies, it's
> useful to add some input delay to reduce the amount of rollback ticks that are needed. (to
> reduce the rollback visual artifacts and CPU costs)"

> "If you set `maximum_input_delay_before_prediction` to 50ms and `maximum_predicted_time` to
> 100ms: 30ms ping → 30ms input delay, no prediction; 120ms ping → 50ms input delay + 70ms
> prediction/rollback; 200ms ping → 100ms input delay + 100ms prediction/rollback"

source: `crates/core/sync/src/timeline/input.rs:150-171`. Directly relevant to §1's rollback-rate
problem: a small fixed input delay (1-2 ticks, ~15-30 ms @ 64 Hz) is a second, complementary lever
to threshold tuning — it shrinks how often prediction needs to run ahead at all, independent of
per-component thresholds.

---

## 6. Pause/time manipulation under lightyear

**Verdict up front**: lightyear has no pause primitive, no example implements a networked pause
menu, and our single-player `Time<Physics>`-pause pattern (`state.rs`'s `client_plugin`) will not
survive connecting to a server as-is — it needs to become a client-only presentation overlay
(blur/dim + input-suppression), not an actual simulation freeze, once networked.

### 1. Whole-repo search

`grep -rn -i "pause"` across the entire clone (no `book/` directory in this checkout) surfaces
only: an unrelated internal flag (`PAUSED_DURING_ROLLBACK: bool = true`, controls whether a
*timeline* advances during rollback replay — nothing to do with gameplay pause), doc comments
about "pausing replication" (removing `Replicate` to stop sending updates for one entity — a
different concept), and one unrelated log string in a bot-automation test harness.
source: `lightyear-src/crates/core/core/src/timeline.rs:46`;
`lightyear-src/crates/core/lightyear/src/lib.rs:197`;
`lightyear-src/crates/replication/replication/src/send.rs:99`

**No example or demo has a pause menu, `Pause` event/resource, or any `Time<Physics>`/
`Time<Virtual>` pause call anywhere in `examples/` or `demos/`** (confirmed via exhaustive grep,
zero hits). **UNCERTAIN/NOT FOUND is the honest, complete answer here** — this isn't a gap in our
search, it's the actual state of the reference material.

### 2. What happens if a client locally pauses time while connected

`TimelinePlugin` only sets `Time<Fixed>`'s **timestep** (period) — tick *advancement* is Bevy's
stock `run_fixed_main_schedule` driver, gated by `Time<Virtual>`'s accumulated delta. Lightyear's
own doc comment confirms `Time<Virtual>` is the real driver of `FixedUpdate` cadence and tick
increments:
> "This timeline is synced with the server timeline, and is the main driving timeline: any speed
> adjustments applied to this timeline will also be applied to the `Time<Virtual>` timeline. (and
> will therefore affect how fast the FixedUpdate loop runs, and how ticks are incremented)"

source: `crates/core/sync/src/timeline/input.rs:264-266`

Lightyear's `update_virtual_time` system only ever *writes* `relative_speed` to `Time<Virtual>`
for clock-sync speed adjustment — it never reads or clears `Time<Virtual>::is_paused()`.
source: `crates/core/sync/src/timeline/sync.rs:279-302` (`set_relative_speed` call, line 300)

So: calling `Time<Virtual>::pause()` client-side would genuinely stop `FixedMain` (hence
`LocalTimeline`, `Time<Fixed>`, all our `FixedUpdate` sim systems) from advancing, while the
server keeps ticking. **There is no timeout/disconnect logic keyed on "client tick stopped
advancing"** — searched `sync.rs` for resync/timeout mechanisms and found only the continuous
RTT-based clock-sync loop (`SyncConfig::error_margin`/`max_error_margin`/
`consecutive_errors_threshold`, driving either a gradual `SpeedAdjust` or a hard `resync` tick
jump). Since `FixedMain` itself isn't running while paused, no rollback replay happens either (it
requires `PreUpdate`/`FixedMain` to be ticking) — the "growing rollback queue" failure mode
doesn't apply; instead, on unpause, the accumulated tick gap would very likely blow past
`max_error_margin` and trigger a **hard resync (visible tick/state snap)**, not a smooth
catch-up and not a disconnect.
source: `crates/core/sync/src/timeline/sync.rs:107-119` (error-margin config),
`:45, 370-393` (`SyncEvent`/hard resync mechanism)

### 3. Any "shared pause" concept

**Not found.** No `Pause` event/resource/config anywhere, no pause UI in any example
(single-player-style or otherwise). The architecture assumes continuous `Time<Virtual>`/
`Time<Fixed>` advancement on both peers, with clock-sync (speed adjustment + hard resync) as the
only tool for handling divergence — not a pause primitive. This is the answer the task
anticipated as legitimate if that's what the source showed, and it is: **networked lightyear
games don't pause simulation; a pause menu has to be a client-only overlay that doesn't touch
`Time<Virtual>`/`Time<Fixed>`/`Time<Physics>`.**

### 4. Is `avian3d`'s `Time<Physics>` even still relevant under `LightyearAvianPlugin`?

Yes, still consulted — `LightyearAvianPlugin` disables only `PhysicsTransformPlugin` and
`PhysicsInterpolationPlugin` (confirmed, no change from the original spike map); `Time<Physics>`
is never referenced anywhere in the avian-integration crate (zero grep hits). Avian's own
`run_physics_schedule` (in vendored `avian3d-0.7.0`) still gates stepping on
`Time<Physics>::is_paused()`:
```rust
fn run_physics_schedule(world: &mut World, mut is_first_run: Local<IsFirstRun>) {
    let _ = world.try_schedule_scope(PhysicsSchedule, |world, schedule| {
        let is_paused = world.resource::<Time<Physics>>().is_paused();
        ...
        if !is_paused { world.resource_mut::<Time<Physics>>().advance_by(timestep); ... }
        *world.resource_mut::<Time>() = world.resource::<Time<Physics>>().as_generic();
        if !world.resource::<Time>().delta().is_zero() { schedule.run(world); }
        ...
```
source: `~/.cargo/registry/.../avian3d-0.7.0/src/schedule/mod.rs:235-273`; registered in
`FixedPostUpdate` by default (`:44-45, 110-112`), which is part of Bevy's stock `FixedMain` group.

Lightyear's rollback replay calls `world.run_schedule(FixedMain)` directly and does not bypass or
reorder `FixedPostUpdate` — so `Time<Physics>::is_paused()` genuinely is still honored during
replay, in principle. **But lightyear's own maintainers flag this as unresolved**, with an
explicit TODO at the exact point rollback rewinds time resources:
```rust
let time_resource = *world.resource::<Time>();
let current_fixed_time = *world.resource::<Time<Fixed>>();
*world.resource_mut::<Time<Fixed>>() = rollback_fixed_time(&current_fixed_time, num_rollback_ticks);
// TODO: should we handle Time<Physics> and Time<Subsets> in any way?
//  we might need to rollback them if the physics time is paused
//  otherwise setting Time<()> to Time<Fixed> should be enough
//  as Time<Physics> uses Time<()>'s delta
```
source: `lightyear-src/crates/replication/prediction/src/rollback.rs:1090-1101`

**Net implication for us**: `Time<Physics>` pause is not rewound by lightyear's rollback
mechanism — only `Time<Fixed>`/generic `Time` are. lightyear's own team calls the paused-physics
interaction with rollback correctness an open question, unvalidated by any example. Given §3's
"no shared pause exists" conclusion anyway, the actionable takeaway is: **don't pause
`Time<Physics>` (or `Time<Virtual>`) at all once networked** — use a client-only UI/input-gating
pause (blur overlay, suppress `TankCommand` gather, keep simulation and replication running
underneath) instead of trying to freeze the sim, which is both unsupported by lightyear's design
and explicitly flagged as unvetted by lightyear's own TODO.

---

## Step-7 recommendations (ordered)

1. **Solve the child-entity rollback-participation gap first (§3).** This blocks `ServoState`/
   `Reload`/`Suspension` correctness under any rollback at all — before wiring `SimPlugin` in for
   real, decide Option A (mark turret/gun/muzzle/roadwheel children `Predicted`) vs. Option B
   (denormalize onto the root) and prototype the cheaper one (Option A) against a single child
   type (`ServoState` on the turret) as a spike, since it's the architecture-determining
   unknown flagged as most consequential. `DriveState` (root-resident) is a safe, independent
   `local_rollback::<DriveState>()` call regardless of that decision — do it now, it's free.
2. **Wire predicted shell spawning per §2 before enabling prediction on `shooting`/`ballistics`.**
   Add `PreSpawned::new(hash(peer_id, fire_tick[, weapon_slot]))` to `on_fire_shell`'s spawn
   bundle; do NOT add `is_in_rollback` gating to `fire`/`on_fire_shell`. This is a
   correctness-before-optimization item — get it right before the rollback-rate tuning in step
   3 makes rollbacks rare enough to hide a latent bug here.
3. **Apply the §1 rollback-rate mitigations to the real rig** once §1/§2 land: coarsen
   Position/Rotation/velocity thresholds on the tank root first (cheapest, matches the reference
   pattern's own tuning knob), and layer in `InputDelayConfig::fixed_input_delay(1)` or `(2)`
   (§5) as a second, complementary lever if threshold tuning alone doesn't bring the rate down
   enough. Avoid `RollbackPolicy::state = Disabled` — too coarse a hammer for a CPU problem.
4. **Audit every `FixedUpdate` system in `driving`/`aim`/`shooting`/`damage` against the §4
   decision table** as `SimPlugin` comes online — most need no `is_in_rollback` gate (they
   become correct automatically once §1 lands and their state is history-tracked); only true
   cosmetic/one-shot externalities (muzzle flash, fire sound, kill-feed) need the gate.
5. **Defer the pause redesign (§6) to whenever the pause-menu UI is touched again** — no urgency
   for the spike itself (headless bins don't have a pause menu), but flag it now so `state.rs`'s
   `client_plugin` doesn't get carried into the networked build unchanged: the eventual fix is
   "pause becomes a client-only overlay + input suppression, never touches
   `Time<Physics>`/`Time<Virtual>`," not a networking-layer change.

---

## 7. `DeterministicPredicted` + child-entity decoration (follow-up research)

**Verdict up front**: the decoration design (net-side observer inserts `DeterministicPredicted` +
`local_rollback::<C>()`-registered components on rig children, no state-sync/comparison/
replication) is **safe to build largely as specified**, with two amendments: (1) don't rely on
`DeterministicPredicted`'s `skip_despawn`/`enable_rollback_after` fields for our async-bind
timing — set `skip_despawn: false` (the default) since our own analysis shows the empty-history
case degrades gracefully, not catastrophically, so the protection isn't needed and misusing it
just delays real rollback participation; (2) confirm before shipping that our lightyear version's
`prepare_rollback` component path already contains the November-2026 upstream fix for
"empty-history misread as removal" (§7.3) — it does, in the exact commit we're pinned to, but this
was a **live bug in this exact code path** until 9 days before our checkout, which is close enough
to warrant re-verifying on any future lightyear bump.

### 1. `DeterministicPredicted` exact semantics

**Definition** — plain marker `Component` with an `on_add` hook, not an observer/event, not
gated by `ReplicationMode`:
```rust
#[derive(Component, PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
#[component(on_add = DeterministicPredicted::on_add)]
/// Marker component used to indicate this entity is predicted (it has a PredictionHistory),
/// but it won't check for rollback from state updates.
///
/// This can be used to mark predicted non-networked entities in deterministic replication, or to stop a
/// state-replicated entity from being able to trigger rollbacks from state mismatch.
///
/// This entity will still get rolled back to its predicted history when a rollback happens.
pub struct DeterministicPredicted {
    pub skip_despawn: bool,
    pub enable_rollback_after: u8,  // default 20 ticks
}
```
source: `lightyear-src/crates/replication/prediction/src/rollback.rs:246-275`

**What it's designed for — both, explicitly, per its own doc comment**: "predicted non-networked
entities in deterministic replication" (whole-world lockstep, `ReplicationMode::Deterministic` —
see `lightyear_deterministic_replication`) **or** "to stop a state-replicated entity from being
able to trigger rollbacks from state mismatch" (a state-synced entity opted OUT of comparison
while staying rollback-participant). The doc comment itself frames it as a general-purpose
"predicted, no state comparison" marker, not scoped to one replication mode. **UNCERTAIN
resolved**: this is not ambiguous — it's textually both, by design.

**Does it assume replication / a server counterpart / prespawning?** No. `on_add` only touches
`PredictionManager` bookkeeping (registers the entity in `deterministic_despawn` or
`deterministic_skip_despawn`, keyed by spawn tick) — it does not read `Replicate`, `PreSpawned`,
`Predicted`, or any networking component, and does not require `PredictionResource`/
`PredictionManager` to exist server-side (silently returns early via `let Some(...) = ... else
{ return; }` if absent, matching the `local_rollback` no-op-without-`PredictionPlugin` pattern
already documented in §3.2 above):
```rust
fn on_add(mut world: DeferredWorld, context: HookContext) {
    let deterministic_predicted = *world.get::<DeterministicPredicted>(context.entity).unwrap();
    let tick = world.resource::<LocalTimeline>().tick();
    let Some(prediction_manager_entity) = world.get_resource::<PredictionResource>()
        .map(|r| r.link_entity) else { return; };
    let Some(mut manager) = world.get_mut::<PredictionManager>(prediction_manager_entity) else { return; };
    if !deterministic_predicted.skip_despawn {
        manager.deterministic_despawn.push((tick, context.entity));
    } else {
        manager.deterministic_skip_despawn.push((tick + enable_rollback_after as i32, context.entity));
    }
}
```
source: `rollback.rs:277-301`

**Can it be safely inserted on a purely local, client-spawned child entity?** Yes — confirmed by
direct precedent, not inference. `examples/projectiles`' `OnlyInputsReplicated` mode spawns bullets
with `commands.spawn((bullet_bundle, DeterministicPredicted::default()))` **identically on client
and server, no `Replicate`/`PreSpawned`/networking component in the bundle at all**:
source: `lightyear-src/examples/projectiles/src/shared.rs:996, 1078, 1364`. The
`deterministic_replication` example's ball (`shared.rs:156-176`) and its `handle_deterministic_spawn`
player-decoration observer (`client.rs:82-99`, quoted in full below) both confirm the same: a
purely local component with zero replication markers, added either at spawn or later via a
separate observer reacting to an unrelated marker (`On<Add, PlayerMarker>`) — this is direct
precedent for our "net-side observer decorates children after the fact" plan:
```rust
pub(crate) fn handle_deterministic_spawn(
    trigger: On<Add, PlayerMarker>,
    query: Query<(&PlayerId, &GameReplicationMode)>,
    mut commands: Commands,
) {
    if let Ok((player_id, mode)) = query.get(trigger.entity)
        && mode == &GameReplicationMode::OnlyInputsReplicated
    {
        commands.entity(trigger.entity).insert((
            shared::player_bundle(player_id.0, GameReplicationMode::OnlyInputsReplicated),
            DeterministicPredicted { skip_despawn: true, ..default() },
        ));
    }
}
```
source: `lightyear-src/examples/deterministic_replication/src/client.rs:82-99`

**One caveat, not previously flagged**: every real usage found (`projectiles`, `deterministic_replication`)
runs the *entire* room/mode as deterministic lockstep — the player root itself carries no
`Predicted`/`PredictionTarget` under `OnlyInputsReplicated`
(`examples/projectiles/src/server.rs:312-324`, no `PredictionTarget` in that arm, contrast with
the `AllPredicted`/`ClientPredictedNoComp` arms at `:289-308` which all have one). **No example in
the tree mixes a state-synced-and-compared `Predicted` root with `DeterministicPredicted` children
on the same logical object in the same replication mode** — our design (hull is `Predicted` +
state-compared, children are `DeterministicPredicted` + never compared) is architecturally novel
relative to the reference material, though nothing in the source *forbids* it — see §7.4.

**Rollback restoration code path for `DeterministicPredicted` components under
`local_rollback`**: identical mechanism to any other `local_rollback`-registered component —
confirmed by re-reading with this specific question in mind. `add_prediction_history`'s trigger
list includes `DeterministicPredicted` as an alternative "has history" gate
(`predicted_history.rs:238-247`, `Has<DeterministicPredicted>` OR'd with `Has<Predicted>`/
`Has<PreSpawned>` at line 262). Once `PredictionHistory<C>` exists, `prepare_rollback::<C>`
(registered by `local_rollback` per §3.1's `add_non_networked_rollback_systems`, `plugin.rs:75-100`)
restores strictly from `predicted_history.get_state(rollback_tick)` — **not** `ConfirmedHistory`,
because the `is_state_rollback` branch only consults confirmed history `if let Some(history) =
confirmed_history.as_ref()`, and a purely local `local_rollback`-registered component never has a
`ConfirmedHistory<C>` component at all (nothing ever inserts one for non-replicated components).
So for our `ServoState`/`Reload`/`Suspension`, every rollback — whether triggered by state mismatch
on the parent's `Position` or by input mismatch — restores from the child's **own recorded past**
via the same `predicted_history.get_state(rollback_tick).cloned()` call used for input rollbacks.
source: `rollback.rs:911-937` (branch structure), confirmed against `predicted_history.rs:237-264`
(the gate) and `plugin.rs:75-100` (`local_rollback`'s system registration, identical for
`Predicted` and `DeterministicPredicted` entities — the marker only changes whether
`check_rollback` compares the entity's state to a confirmed value, not how restoration works).

### 2. The replayed-parent interaction (stabilization-shaped case)

**(a) Children rewound to tick-N recorded values before replay — confirmed.** `prepare_rollback::<C>`
runs in `PreUpdate`, in `RollbackSystems::Prepare`, gated `run_if(is_in_rollback)`, chained
strictly before `RollbackSystems::Rollback` (which runs `run_rollback`, the system that executes
`world.run_schedule(FixedMain)`):
```rust
app.configure_sets(PreUpdate, (
    RollbackSystems::Check,
    RollbackSystems::RemoveDisable.run_if(is_in_rollback),
    RollbackSystems::Prepare.run_if(is_in_rollback),
    RollbackSystems::Rollback.run_if(is_in_rollback),
    RollbackSystems::EndRollback.run_if(is_in_rollback),
).chain().in_set(PredictionSystems::Rollback));
```
source: `rollback.rs:119-131`. `prepare_rollback::<C>` is registered per-type by `local_rollback`
(`plugin.rs:94` inside `add_non_networked_rollback_systems`) exactly as for any other
`local_rollback`-registered type — it runs once for every registered `C` (including
`ServoState`/`Reload`/`Suspension` once decorated), restoring each to its `get_state(rollback_tick)`
value before the replay loop (`RollbackSystems::Rollback`) begins. Confirmed above at §7.1 and
cross-referenced against the already-verified §3 finding that this is the exact same code path
`DriveState` will use.

**(b) The replayed parent's Position/Rotation are correctly restored+resimulated, visible to
child systems reading `Position` — confirmed.** `run_rollback` (`rollback.rs:1051-1174`) is a
single function: it sets `Time<Fixed>`/`Time`, then loops `for i in 0..num_rollback_ticks { ...
world.run_schedule(FixedMain); ... }` (`:1115-1143`). `FixedMain` is Bevy's stock schedule group,
containing `FixedPreUpdate`/`FixedUpdate`/`FixedPostUpdate`, which is where avian's own
`PhysicsSchedule` (advancing `Position`/`Rotation` from `RigidBody` state) and our own
`FixedUpdate` game systems both live — nothing in `run_rollback` reorders, filters, or splits this
schedule group per-entity. The parent's `Position`/`Rotation` were themselves already restored to
their tick-`rollback_tick` history value by their own `prepare_rollback::<Position>`/
`prepare_rollback::<Rotation>` call (same `RollbackSystems::Prepare` set, same mechanism as any
other replicated-and-predicted component) before the loop starts, and are then re-simulated
tick-by-tick by avian's physics step running inside each `FixedMain` pass — same as normal
gameplay. A child system reading `Query<&Position, With<TankRoot>>` or similar during replay sees
whatever avian just computed for that replay tick, not a stale or interpolated value.
source: `rollback.rs:1112-1143` (the loop + comment "Run the fixed update schedule (which should
contain ALL predicted/rollback components and resources)")

**(c) `is_in_rollback`-ungated child systems run normally during replay ticks — confirmed.**
`is_in_rollback` (`lightyear_core::timeline::is_in_rollback`, `crates/core/core/src/timeline.rs:236-239`)
is a plain run condition — `Query<(), With<Rollback>>.single().is_ok()`. It is opt-in per system
via `.run_if(...)`; nothing in `run_schedule(FixedMain)` filters systems by this condition
automatically. A servo-integration system with no such run condition executes on every replay
tick exactly as it does on a normal tick, reading whatever component values exist in the `World`
at that point in the schedule (including the just-restored-and-resimulated parent `Position` from
(b)). This matches the general rule already established in §3.4 of this doc: only true
externalities need `not(is_in_rollback)` gating; anything computing purely from
rollback-participating state is correct to run unconditionally during replay.
source: `crates/core/core/src/timeline.rs:236-239`; ordering evidence same as (b).

### 3. Edge cases for locally-spawned children under rollback

**The empty-history case is a graceful no-op, not a panic, not a despawn (by itself) — confirmed
at the `HistoryBuffer` level.** `PredictionHistory<C>::get_state(tick)` delegates to
`HistoryBuffer::get_state`:
```rust
pub fn get_state(&self, tick: Tick) -> Option<&HistoryState<R>> {
    let partition = self.buffer.partition_point(|(buffer_tick, _)| *buffer_tick <= tick);
    if partition == 0 { return None; }
    self.buffer.get(partition - 1).map(|(_, state)| state)
}
```
source: `lightyear-src/crates/core/core/src/history_buffer.rs:159-168`. If the child's history has
zero entries at or before `rollback_tick` (rollback target predates the child's spawn/bind), this
returns `None`. `prepare_rollback::<C>` branches explicitly on this:
```rust
match restore_state {
    None => { /* leave current value in place */ }
    Some(HistoryState::Removed) => { entity_mut.try_remove::<C>(); }
    Some(HistoryState::Updated(correct)) => { /* insert/overwrite */ }
}
```
source: `rollback.rs:964-1009`. **`None` is handled as "leave the current live value untouched,"
not a panic and not a despawn.** This is deliberately the fixed behavior — GitHub issue
[#1511](https://github.com/cBournhonesque/lightyear/issues/1511) ("Predicted component wrongly
removed on state rollback when no history sample exists at-or-before the rollback tick") describes
exactly this scenario going wrong in an *earlier* version of this same function, where "no sample"
was conflated with "authoritatively removed" and the live component got stripped. Fixed by PR
[#1512](https://github.com/cBournhonesque/lightyear/pull/1512) ("fix(prediction): only remove
component on explicit `ConfirmedState::Removed`"), closed 2026-06-17 — **9 days before our pinned
commit `28e823d` (2026-06-26)**. Re-reading `rollback.rs:964-1009` in our checkout confirms the
fixed three-way branch (`None` / `Removed` / `Updated`) is present, so **our decoration design does
not hit the bug this issue describes.**

**But this "leave current value in place" behavior is not the whole story for our async-bind
scenario — the entity-existence question is separate from the component-value question.**
`DeterministicPredicted::on_add` registers every such entity in
`PredictionManager.deterministic_despawn: Vec<(Tick, Entity)>` (unless `skip_despawn: true`).
Before every rollback's replay loop starts (in the same `check_rollback` system that computes
`rollback_tick`, upstream of `RollbackSystems::Prepare`), this list is drained and any entity
whose *spawn tick* is `> rollback_tick` is unconditionally despawned:
```rust
prediction_manager.deterministic_despawn.drain(..).for_each(|(t, e)| {
    if t > rollback_tick && let Ok(mut c) = commands.get_entity(e) {
        c.despawn();
    }
});
```
source: `rollback.rs:756-765`, called from the site quoted at `rollback.rs:726-813` (comment:
"Rollback! Despawning all PreSpawned/DeterministicPredicted entities spawned after the rollback
tick... they will get respawned during the rollback"). **So the actual failure-mode order is:
entity-level despawn-and-recreate is the primary mechanism, not "component silently keeps stale
value on an entity that persists."** For a child spawned *inside* the current rollback window
(spawn tick > rollback_tick): the entity itself is despawned before replay starts, and — critically
— **nothing in lightyear re-creates it** unless our own spawn/binder code runs again during the
replay and re-spawns it (same "always call the spawn code unconditionally" pattern already
established for `PreSpawned` shells in §2 of this doc). Our rig binder does not run per-tick inside
`FixedUpdate` (it's an async asset-load completion callback, not a deterministic-from-input
system), so **a child rig-bound during a since-rolled-back window would be despawned by this
mechanism and NOT respawned by replay**, since nothing drives its recreation from replayed input.

**Applied to our actual timing (rig binds seconds after spawn, rollbacks ~7 ticks / ~100 ms
deep)**: this is not a real collision. `deterministic_despawn` only fires for `t > rollback_tick`,
and `rollback_tick` is always within `max_rollback_ticks` (default 100, our practical case ~7,
per §1(c) above) of the *current* tick — i.e., only a few hundred milliseconds in the past. A
child bound seconds after tank spawn has a spawn tick that is, by the time any rollback happens
seconds later, far older than any live `rollback_tick` target. **The despawn-and-respawn edge case
only bites if a rollback's target tick is older than the child's bind tick — impossible once
several seconds (≫ 100 ms max rollback depth) have elapsed since bind.** The only theoretical
window of actual risk is a rollback occurring in the first ~7 ticks immediately after the async
bind completes and inserts `DeterministicPredicted` — a narrow but non-zero window worth a note in
the implementation, not a blocking concern for the design.

**Failure mode, stated plainly: neither panic nor "restore to default."** It is **despawn +
silent non-recreation** if the spawn/decoration itself falls inside the replayed window and
nothing replay-side recreates the entity; **graceful no-op (leave current value)** for
`PredictionHistory<C>::get_state` misses on an entity that survives the despawn check. Both are
confirmed non-panicking. **UNCERTAIN**: no source or test exercises the specific "despawn then
nothing recreates it, what does the rest of the frame look like" tail (e.g., does a system
querying `Query<&ServoState>` elsewhere simply skip the now-missing entity that tick, or is there
a dangling-reference risk via `rig.gun`/`rig.turret` `Entity` handles cached on `RigBound`-style
components per `tank.rs:52-56`?) — worth a forced-rollback-during-bind-window spike test if the
implementation wants to fully close this out, mirroring the empirical-validation pattern already
used for increments 5/6 and flagged as outstanding for `PreSpawned` in §2 Q4.

### 4. Community prior art

**Direct hits, all from lightyear's own repo — no third-party prior art found:**

- `examples/projectiles`, `OnlyInputsReplicated` mode: the only reference-material example that
  spawns a `DeterministicPredicted`-marked entity client-side with zero other networking
  components in the same frame the "shoot" input is processed
  (`examples/projectiles/src/shared.rs:996,1078,1364`) — closest existing precedent to our
  "locally-derived-from-input child entity" pattern, but for a standalone bullet, not a child of a
  `Predicted` root.
- `examples/deterministic_replication`: whole-world lockstep demo; `DeterministicPredicted` used
  both at spawn (`shared.rs:161-164`, the ball) and via a later decorating observer keyed on an
  unrelated marker (`client.rs:82-99`, `handle_deterministic_spawn` — the closest structural match
  to our planned "net-side observer decorates rig children" approach, though it decorates a
  standalone player entity, not a child of a separately-predicted parent).
- GitHub issue [#886](https://github.com/cBournhonesque/lightyear/issues/886) "Add Extrapolation +
  an entity with replicated vehicles" (open, unresolved) — the maintainer's own framing of
  "vehicles" as a distinct, harder case ("make sure that all entities are in the same 'predicted'
  timeline"), but scoped to extrapolation/correction blending for a single replicated vehicle
  entity, not multi-entity vehicle rigs with deterministic children. No mention of
  `DeterministicPredicted` or child hierarchies in this issue.
- GitHub issue [#627](https://github.com/cBournhonesque/lightyear/issues/627) "Predicted entities
  don't sync hierarchy" (closed) and [#485](https://github.com/cBournhonesque/lightyear/issues/485)
  "Client hierarchy not properly replicated" (closed) — both about `ReplicateHierarchy`/
  `ParentSync` propagating a *replicated* hierarchy to `Predicted` descendants, i.e. the opposite
  problem from ours (we deliberately do NOT want the children replicated/state-synced). Confirms
  the maintainers' hierarchy work has focused on the replicated-child case, not the
  locally-spawned-deterministic-child case.
- GitHub issue [#1128](https://github.com/cBournhonesque/lightyear/issues/1128)
  "`lightyear_avian` interferes with child collider sync" (open) — reports that
  `update_child_collider_position` (the same stateless kinematic re-derivation system cited in §3
  of this doc) doesn't fire correctly in some setups, causing child colliders to not follow a
  moving parent `RigidBody`. Not about prediction history, but a live open bug in the exact
  avian-integration file our children's positions will indirectly depend on if any rig child also
  carries an avian `Collider` — worth a smoke-check once our children are wired in, independent of
  the `DeterministicPredicted` decoration question itself.
- GitHub issue [#795](https://github.com/cBournhonesque/lightyear/issues/795) "Rollback issues in
  avian3d" (closed, older lightyear version) — the historical origin story for exactly the
  "non-networked resource lacks history at the rollback tick, restoration removes it and avian
  panics" failure mode that `DeterministicPredicted`/`local_rollback`'s careful `None` handling
  (§7.3) was built to prevent. Useful as design-intent context, not current-version risk.
- GitHub issue [#1511](https://github.com/cBournhonesque/lightyear/issues/1511) / PR
  [#1512](https://github.com/cBournhonesque/lightyear/pull/1512) — see §7.3; the most directly
  relevant hit, confirming the empty-history-at-rollback-tick path was a **live, recently-fixed
  bug** in the exact function our design depends on.

**Searched and found nothing**: no GitHub issue, PR, or discussion mentions `local_rollback`
outside lightyear's own crate/example/test code (`grep`-equivalent web + GitHub code search, zero
hits beyond the source tree already cited in §3). No reddit r/bevy post, blog, or public write-up
discusses multi-entity predicted objects (vehicles, turrets, or otherwise) in lightyear
specifically — the one relevant Reddit hit from search (`r/bevy` "Building a 3d shooting
multiplayer fps game") does not mention lightyear's prediction internals. No bevy_replicon
discussion surfaced specific hierarchy-replication wisdom transferable here (bevy_replicon's
`Component`-level prediction/rollback model differs enough from lightyear's `PredictionHistory<C>`
approach that its hierarchy discussions, where found, address a different problem: replicating
`ChildOf`, not per-entity deterministic rollback participation). **Stated plainly: nobody has
publicly built or documented this specific pattern (state-synced Predicted root + local
DeterministicPredicted children via a decorating observer) outside what this research
reconstructed from source + the two closest example precedents above.** This is a genuinely novel
application of the API, assembled correctly from primitives that are each independently exercised
in the reference material, but never combined this way anywhere in the public record.

### Final verdict

**Safe to build as specified, with three amendments:**

1. **Use `DeterministicPredicted::default()` (`skip_despawn: false`)**, not the `skip_despawn:
   true` variant seen in the `deterministic_replication` examples — that variant exists for
   one-off server-triggered spawns (balls, players joining) that need a grace window before
   participating in rollback despawn-checks. Our rig children are input-derived every tick once
   bound; the plain despawn-and-recreate-on-replay semantics (§7.3) are the correct fit, and
   given the ~7-tick/~100 ms rollback depth vs. seconds-later async bind timing, the collision
   window this variant would protect against is already vanishingly narrow.
2. **Do not treat the empty-history-at-rollback-tick case as needing special handling in our own
   code** — `prepare_rollback`'s `None` branch already does the right thing (leave current value),
   and it's the *entity despawn* path (`deterministic_despawn`), not the component-restore path,
   that would matter if a rollback ever did target a tick before bind — which, per the timing
   analysis in §7.3, essentially cannot happen in practice given our bind-then-play-for-seconds
   pattern.
3. **Budget one forced-rollback spike test** once the rig binder emits the decorating observer,
   specifically forcing a rollback whose target tick predates a freshly-bound child's first
   history entry (artificially, e.g. by triggering rollback in the same tick range as bind) — to
   empirically confirm the "despawn, nothing recreates it, rest of frame is fine" tail from §7.3's
   UNCERTAIN flag, and to confirm no dangling `Entity` handles on `RigBound`/similar cached-handle
   components misbehave when their target briefly doesn't exist. This mirrors the existing
   empirical-validation discipline already applied to increments 5/6 and flagged for `PreSpawned`
   in §2 Q4 — cheap insurance before this becomes load-bearing for `ServoState`/`Reload`/
   `Suspension` correctness.

No amendment changes the core plan from §3's Option A: mark rig children with a marker
(`DeterministicPredicted`, now confirmed as the *semantically correct* choice, not just an
available one, since we explicitly want "predicted, no state comparison" not "predicted +
would-be-compared-if-it-had-replicated-state") plus `local_rollback::<C>()` per component type.
The two-marker combination (`DeterministicPredicted` for the entity, `local_rollback::<C>()` per
component) is exactly what every real call site in the reference tree does — there is no simpler
or more idiomatic path available in 0.28.0.
