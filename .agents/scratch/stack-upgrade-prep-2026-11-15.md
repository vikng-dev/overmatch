# Stack upgrade: read-only preparation (2026-11-15)

Subject: the game's engine dependencies and its server physics step. Game state only.
Read-only: nothing built, no bay, no state-changing git, no tracked file edited, nothing filed.
`main` at `23bbde63` (three commits past the plan's `252939a0`).

This file holds the results of section A1 of `.agents/scratch/stack-upgrade-plan-2026-11-15.md`,
the fork-line check of its B2 table, and the baseline commands of A2. Each section ends in a
decision or a list the cutover lane acts on.

Labels: MEASURED = read from a file, a grep or a published crate's source on this date.
DERIVED = my arithmetic or judgement; inputs named.

Sources read:

- `0.8:` = avian3d 0.8.0, `~/.cargo/registry/src/index.crates.io-*/avian3d-0.8.0/src/`.
- `0.7:` = pristine avian3d 0.7.0 in the same registry; "fork" = `vendor/avian3d-0.7.0-child-pose`.
- Bevy 0.20.1 and 0.19.1: `bevy_internal`, `bevy_ecs`, `bevy`, `bevy_log` in the registry;
  `bevy_ui_widgets`, `bevy_picking`, `bevy_dev_tools` 0.20.1 were not there and were read from
  the published `.crate` files (crates.io), unpacked outside the repo.
- lightyear 0.31.0 is not in the registry. `lightyear_avian3d` 0.31.0 was read from its published
  `.crate`, unpacked outside the repo. No other lightyear 0.31.0 crate was read.
- The prototype: branch `lane/stack-proto`, `proto/stack-seams/`.

## What moved under the plan since it was written

| Fact | Source | Consequence |
|---|---|---|
| `main` is `23bbde63`: `lane/sleeping-pairs` is merged (0 ahead), the query-gap panic and the resting-miss witnesses are on `main` | `git log main`, `git rev-list --count` (MEASURED) | The "lane patches not on `main`" list shrinks to `lane/soft-run` |
| The sleeping-pairs hold is recorded on `main` as forgone: "Resting misses stay in the narrow phase (no source change; a rule for the 0.8 rebase)" | fork `OVERMATCH_PATCH.md:266-291` (MEASURED) | The plan's row "#1032 transfer refused", ≈100 fork lines, assumed the hold's flag and transfer methods. On `main` they do not exist. The row is ≈10 (section 7) |
| `lane/sleeping-pairs-hold` is 2 ahead, 11 behind | `git rev-list --count` (MEASURED) | Not a base for the port. Plan A3 and question F7 named it; they should name `lane/soft-run` only |
| `src/tests/avian_*.rs` is 11 files, 61 tests; `src/net/ram/avian_tests.rs` has 13, six of them `stationary_miss_*` | `grep -c '#\[test\]'` (MEASURED) | The plan's "57 in 10 files" is stale: `avian_body_query_gap.rs` (1) and `avian_compliant_contact.rs` (2) are on `main`, and `avian_island_sleep.rs` grew |
| Bays: bay-1 `lane/soft-run` (9 ahead, dirty), bay-2 `lane/ramp-test` (dirty), bay-3 free | `scripts/bays/status` (MEASURED) | The queue has two lanes to drain |
| `scripts/capture/surroundings/` holds nine plans | `ls` (MEASURED) | The plan said eight |
| The game never inserts a `NarrowPhaseConfig` | `grep -rn NarrowPhaseConfig src` finds readers only (MEASURED) | "Recycling off by config" needs a new insert site, not an edit to an existing one |

## 1. The fork's export list, frozen (A1.1)

Method: every `pub` item the fork adds over 0.7 (`diff -ruN`, MEASURED), grepped over
`src tools build.rs`. "Sites" are lines, comments included, so they are upper bounds.

Slices B2 and B4 code against the right-hand column. `avian_ext` is the game module the plan
proposes (`src/avian_ext/`, one file per seam). An item marked *fork* stays exported by the 0.8
fork under the path given.

| Fork-only item today (path under `avian3d::`) | Game sites (MEASURED) | Name after the cutover | Owner |
|---|---|---|---|
| `collision::collider::collider_transform::ChildColliderPoses` (a `SystemParam`; `.update()`) | `src/ballistics/motion.rs:185`; the vendored `lightyear_avian3d` `plugin.rs` | `crate::avian_ext::child_poses::ChildColliderPoses`, same `update()` | game |
| `…::collider_transform::update_child_collider_position` (made `pub` by the fork) | `src/tests/avian_child_pose_churn.rs:384,454` | `crate::avian_ext::child_poses::place_child_colliders`, a system the game adds in `PhysicsStepSystems::First` | game |
| `LightyearAvianPlugin::update_child_collider_position` (lightyear's, patched to delegate) | `src/tests/avian_child_pose_churn.rs:111,139,177`; `src/ballistics/recording_tests.rs:92` | the same `place_child_colliders`, added by the game in `RunFixedMainLoopSystems::AfterFixedMainLoop`; lightyear's own registration is removed by its patch (section 2) | game |
| `…::collider_transform::Reenabled` (a `Message`) | `src/ballistics/motion.rs:8,96,100,121,233` | `crate::avian_ext::child_poses::Reenabled` | game |
| `…::collider_transform::ReenabledPlugin` | `src/ballistics/motion.rs:552,744,1148,1190,1254,1326`; `src/tests/avian_child_pose_churn.rs:429` | `crate::avian_ext::child_poses::ReenabledPlugin`, added where the game builds its physics plugins | game |
| `…::collider_transform::WatchedDisabling` (`assert_covers`) | 0 in `src/`; used inside `ChildColliderPoses` | `crate::avian_ext::child_poses::WatchedDisabling` | game |
| `prelude::ReducibleContacts` (marker component) | `src/construction/breakage.rs:671`; `src/tests/avian_contact_reduction.rs` (3) | `crate::avian_ext::contact_reduction::ReducibleContacts` | game |
| `collision::contact_types::reduction::{reduce_body_pair, PairManifolds, ReductionStep, ReductionCounts, MAX_NORMALS, HULL_TOLERANCE, BEARING_GAP}` | `src/tests/avian_contact_reduction.rs` (28 lines) | `crate::avian_ext::contact_reduction::{…}`, same names; `Scalar`/`Vector` become `f32`/`Vec3`/`Vec2` (0.8 no longer exports them, `0.8:math/mod.rs:69`) | game |
| `ContactPairFlags::REDUCIBLE_CONTACTS` | 0 | gone: the pass queries the marker | — |
| `Collider::with_mass_properties_cache(density)` | `src/construction/breakage.rs:2739`; `breakage/tests.rs:6251,6269` | unchanged name and path | fork |
| `avian3d::AncestorMarker` (crate-root re-export) | `src/construction/breakage.rs:3109` | unchanged: `avian3d::AncestorMarker`. 0.8 keeps `ancestor_marker` `pub(crate)` (`0.8:lib.rs:557`), so the one-line re-export stays | fork |
| `prelude::ContactCompliance { stiffness, damping }` (plus `rest`, `bears()` on `lane/soft-run`) | `src/track/belt_segment.rs:33,212` | `crate::avian_ext::compliance::ContactCompliance`, same fields | game |
| `ContactManifold::compliance: Option<ContactCompliance>` | `src/track/belt_segment.rs:217,291,345`; `src/headless_test/track_scorecard.rs:2630`; `track_scorecard/rubble_climb.rs:1389,1390` | gone from the manifold. Writers call `crate::avian_ext::compliance::Laws::set(contact_id, manifold_index, law)`; readers call `Laws::get(..)`. `Laws` is a resource cleared each step before the narrow phase | game |
| `ContactNormalPart::{compliant, spring}` | 0 in `src/` | unchanged names; written by `avian_ext::compliance` after `SolverSystems::PrepareContactConstraints` | fork |
| `SolverBody::kinetic_speed_squared(mass, inertia, rotation)` | `src/net/contact_census.rs:747`; `src/construction/breakage/tests.rs:1030`; `src/tests/avian_island_sleep.rs:570,572,575` | `crate::avian_ext::rest_test::kinetic_speed_squared(&SolverBody, ComputedMass, ComputedAngularInertia, Rotation)`, a free function | game |
| `ContactManifold::match_contacts` taking an iterator of all prior points | `src/tests/avian_warm_start.rs:20` | `crate::avian_ext::contact_match::match_pair(new: &mut [ContactManifold], old: &[ContactManifold], distance_threshold: f32)`; the fork calls it through a resource (section 7, rows 8 and 9). Upstream's `match_contacts(&[ContactPoint], f32)` returns to its 0.8 signature | game |
| `ColliderTree::{moved_proxies(), mark_moved, clear_moved, set_moved, take_moved}` | `src/tests/avian_moved_lists.rs` (6); the vendored `lightyear_avian3d` `rollback.rs` | gone: upstream's `ColliderTree::moved_proxies: MovedProxyList` (`push`, `remove`, `clear`). The test file is rewritten against it or deleted (plan F4) | upstream |
| `ColliderTreeDiagnostics::moved_aabb_candidates` | `src/tests/avian_moved_aabb_prune.rs:62` | unchanged | fork |
| `CollisionDiagnostics::broad_phase_candidates` | `src/cost.rs`, `src/net/ram.rs:379`, `src/tests/avian_broad_phase_layers.rs:60` | unchanged | fork |
| `CollisionDiagnostics::{stationary_non_touching_pairs, stationary_non_touching_queries}` | `src/net/ram.rs:393-394`; `src/net/ram/avian_tests.rs:174-175` | unchanged | fork |
| `CollisionDiagnostics::reduction_{groups, landing_whole, ceiling_whole, overlap_whole, warm_points_removed, cold_points_kept}` | `src/net/ram.rs:395-400` | `Res<crate::avian_ext::contact_reduction::ReductionCounts>`, fields `groups, landing_whole, ceiling_whole, overlap_whole, warm_removed, cold_kept` (the struct's existing names). `src/cost.rs` keeps its own field names | game |
| `SolverDiagnostics::{serial_contact_visits, parallel_contact_visits}` | `src/net/ram.rs:389-390`; `src/net/ram/avian_tests.rs` (7) | unchanged | fork |
| (new) upstream `CollisionDiagnostics::recycled_count` | 0 | read by `src/net/ram.rs` into the cost account; must be 0 while recycling is off | upstream |

Not on the list because the game does not name them: `StationaryMiss`, `serial_through`,
`ParallelQueryForEach` (all `pub(crate)` in the fork).

**Decision.** This table is the contract. Two things in it differ from the plan's B2 table and
are settled here:

1. `ChildColliderPoses` moves to the game, and **lightyear's vendored crate stops depending on a
   fork-only Avian item**: its patch becomes "do not register `update_child_collider_position`"
   and the game registers its own system in the same slot. A vendored crate cannot call the game
   crate, so the plan's "delegates to `ChildColliderPoses`" was not buildable with the logic in
   the game. Consequence: B4's `lightyear_avian3d` re-port no longer waits for B2.
2. The compliance law leaves `ContactManifold` for a game store keyed by `(ContactId, manifold
   index)`. Six game lines read or write the manifold field today; they move to the store.

## 2. `lightyear_avian3d` 0.31.0 against the planned fork (A1.2)

Read: `lightyear_avian3d-0.31.0/src/{lib.rs,plugin.rs,rollback.rs}` (57, 1,790 and 1,291 lines,
MEASURED) and its `Cargo.toml`.

| Question | Finding (MEASURED from source) |
|---|---|
| Is `rollback.rs` compiled for us? | Yes, always: `mod rollback` is gated only on the `2d`/`3d` feature (`lib.rs:42-43`) and `lightyear_prediction` is a non-optional dependency (`Cargo.toml:122`) |
| Does it run for us? | No. `register_rollback` and `register_island_rollback` are called only when `rollback_resources` is true (`plugin.rs:439-447`); the default is false (`plugin.rs:213`) and the game builds the plugin with `..default()` (`src/net/protocol.rs:976-982`) |
| How does it snapshot? | `ContactGraph`, `ConstraintGraph`, `JointGraph`, `PhysicsIslands` by whole-value `Clone` (`local_rollback`, `rollback.rs:188-190,352`). `ContactStatusChangeQueue.0` by `clone_from` (`:236-249`). The collider trees **field by field**: `ColliderTreeSnapshot { bvh, proxies, moved_proxies }` (`:68-90`), over `ColliderTrees::{dynamic_tree, kinematic_tree, static_tree, standalone_tree}` and a cloned `MovedProxies` (`:100-126`) |
| What must the fork keep for it to compile? | `ColliderTree::{bvh: Bvh2, proxies: StableVec<ColliderTreeProxy>, moved_proxies: MovedProxyList}` public with those types; the four tree field names; `MovedProxies: Clone + Default` with `clear()` and `contains()`; `Clone` on the four graph resources; `ContactStatusChangeQueue(pub Vec<ContactStatusChange>)` with `ContactStatusChange: Clone`; `ColliderAabb`, `EnlargedAabb`, `ColliderTreeProxyKey`, `BodyIslandNode`, `Sleeping`, `SleepTimer` clonable components |

Fork-added fields the planned fork puts inside those types, and whether a snapshot would miss
them:

| Type | Planned fork field | In a snapshot? |
|---|---|---|
| `ColliderTree`, `ColliderTrees`, `MovedProxies` | none. Patch 16 (the 0.7 fork's moved-list index inside `ColliderTree`) is dropped for upstream's `MovedProxyList` | nothing to miss. A field added to `ColliderTree` **would** be missed, because that snapshot is field by field |
| `ContactGraph` (holds `ContactPair`) | `ContactPair::stationary_miss: Option<Box<StationaryMiss>>` (patch 13) | carried by the whole-value clone, provided `StationaryMiss: Clone`. It needs `Clone`, `Debug`, `PartialEq` for `ContactPair`'s derives and `#[serde(skip)]` as today |
| `ConstraintGraph` (holds `ContactConstraint`) | `ContactNormalPart::{compliant: bool, spring: SoftnessCoefficients}` (patch 19) | carried by the whole-value clone; both types already derive what `ContactNormalPart` derives |
| `PhysicsIslands` | none: the wake rule and the rest test are separate resources or game passes | — |
| `ContactManifold` | none: `compliance` leaves it (section 1) | — |

**Decisions.**

1. **`rollback.rs` is taken unpatched.** With patch 16 dropped, the 0.30.1 patch to this file
   (`moved_proxies()`, `set_moved`, `mark_moved`, `clear_moved`) has nothing to adapt.
2. **Rule for B2: no fork field on `ColliderTree`, `ColliderTrees` or `MovedProxies`.** Fork
   state that must sit beside a tree goes in its own resource.
3. **`plugin.rs` keeps one patch**: delete the registration of
   `LightyearAvianPlugin::update_child_collider_position` in the Position-mode arm
   (`plugin.rs`, the `app.add_systems(RunFixedMainLoop, …AfterFixedMainLoop)` block, 5 lines).
   Stock 0.31 still assigns every child pose unconditionally (`plugin.rs:910-937`), which is the
   churn the patch exists to stop. The game adds `avian_ext::child_poses::place_child_colliders`
   in `RunFixedMainLoopSystems::AfterFixedMainLoop`. The vendored directory is then
   `lightyear_avian3d-0.31.0-child-pose` with a patch of about 5 deleted lines (DERIVED), and it
   names no fork-only Avian item.
4. New fields of `LightyearAvianPlugin` in 0.31 are `register_physics_components` (already set
   by the game) and `rollback_resources`; `..default()` covers both. No call-site change.

Not verified: that 0.31's other crates compile against the game (not read). First boot decides
the B0001 item the Bevy scout raised.

## 3. The six `par_iter_many` sites (A1.3)

Verified against `bevy_ecs-0.20.1/src/query/par_iter.rs:377-378,543-544`: both
`QueryParManyIter::for_each` and `QueryParManyUniqueIter::for_each` take
`Fn(Result<QueryItem, QueryEntityError>)`. `QueryEntityError` has three variants
(`query/error.rs:11-25`): `QueryDoesNotMatch(Entity, ArchetypeId)`, `NotSpawned(..)`,
`AliasedMutability(Entity)`. `.matched()` exists only on the serial many-iterators
(`query/iter.rs:1620,2500`), so each parallel site handles the `Result` in its closure.

| # | Site | Where the keys come from (MEASURED by reading) | `Err` is | Write |
|---|---|---|---|---|
| 1 | `src/track/world_contact/admission.rs:120` `par_iter_many(sensors.read().chain(fields.read()))` | `RemovedComponents<Sensor>` and `RemovedComponents<FieldSurface>`: entities that lost a marker, some by despawn, some by losing their collider too | **a named legitimate absence** for both `NotSpawned` and `QueryDoesNotMatch`: the entity is no longer geometry, so it has no world-contact obligation (the comment at `:118` already says so for despawn) | `match candidate { Ok(c) => refuse_unsupported(c, &sources), Err(QueryEntityError::NotSpawned(_) \| QueryEntityError::QueryDoesNotMatch(..)) => {} /* no longer a collider: nothing to admit */, Err(e @ QueryEntityError::AliasedMutability(_)) => unreachable!("a read-only many-query cannot alias: {e}") }` |
| 2 | `src/net/debris_stress.rs:542` `par_iter_many_unique_mut(updates.keys())` | built in the same system from `pieces`; each key is asserted `carriers.contains(carrier.0)` at `:521-524` | **a bug** | `.expect("every live body's Carrier retains its DebrisPose until the body is retired")` in the closure, and delete the `carriers.contains` assert at `:521-524`: it is the same invariant checked twice, and the `expect` is its one owner. The distinct-carrier assert stays |
| 3 | `src/construction/breakage.rs:1952` `par_iter_many_unique_mut(batch.keys())` | `taken`: roots from `p.constructions.iter()` (`:1914`), from `owner_geometry` over the same query, from line crossings whose owner is already `expect`ed live with `frames[&root]` (`:1473-1476`), from contact deposits filtered by `constructions.get(root).is_ok_and(..)` when queued (`:904-908`) in `read_contacts`, which is chained directly before `apply_deposits` (`:3619`), and from `RawDeposit::changed` | **a bug** | `.expect("a deposit names a construction that is live when its deposits apply")`. One residual check for the lane before it writes this: no writer of `RawDeposit::changed` was found by grep (`changed: Vec<Entity>` at `:805`, read at `:1913`); if a writer exists and can name a root despawned before the apply, that root's absence gets a name (`NotSpawned` only) and `QueryDoesNotMatch` still panics. If no writer exists, delete the field (a switch with no user) |
| 4 | `src/debris/crumble.rs:670` `par_iter_many_unique_mut(landings.keys())` | `landings(..)` admits an entity only when `groups.get(entity).is_ok_and(\|item\| !item.5)` on the same query in the same system (`:628-629`) | **a bug** | `.expect("a landing's group passed this system's own groups.get")` |
| 5 | `src/construction/breakage/sweep_intake.rs:877` `par_iter_many_unique_mut(&roots)` | `prepared.roots`, each admitted by `constructions.contains(other.body)` (`:763`) in the same system | **a bug** | `.expect("a prepared sweep root passed constructions.contains in this intake")` |
| 6 | `src/construction/breakage/sweep_intake.rs:1225` `par_iter_many_unique_mut(departing.keys())` | roots of this round's cuts (`:421-425`), which come from `geometry` (site 5) over the same query; commands are deferred, so nothing despawns between | **a bug** | `.expect("a cut's owner was resolved from constructions in this round")` |

A panic inside a parallel batch propagates through Bevy's task scope, so the `expect`s fail the
tick as ADR-0011 requires (DERIVED from `bevy_tasks` scope semantics; not run).

**Decision.** One named absence (site 1), five bugs. No site takes the migration guide's
`let Ok(..) else { return }`.

## 4. Hand-kept plugin lists against Bevy 0.20.1 and Avian 0.8 (A1.4)

The 43 `.disable::<` lines (MEASURED) are in 8 files: `src/lib.rs` 31, `tests/bevy_mesh_removal_lifecycle.rs` 4,
`net/physics.rs` 2, `tests/avian_body_query_gap.rs` 2, and one each in `bitprobe.rs`,
`headless_test.rs`, `net/client/hosted_e2e.rs`, `world/boundary.rs`.

**`DefaultPlugins`.** `diff` of the `plugin_group!` block of `bevy_internal` 0.19.1 against
0.20.1 is one line (MEASURED): `RenderDebugOverlayPlugin`'s gate changed from
`all(feature = "bevy_dev_tools", feature = "bevy_pbr")` to `feature = "render_dev_tools"`.
Membership is otherwise identical. `render_dev_tools` is in Bevy's `dev` feature collection, not
in `default` (`["2d","3d","ui","audio"]` in both versions, MEASURED), and the game enables
`debug`, `jpeg`, `basis-universal` only (`Cargo.toml:28`). So the overlay plugin is **not** in
our build on either version, and the Bevy scout's "added to default plugins" does not apply to
our feature set. No sprite-mesh plugin appears in the 0.20.1 group.

**`UiWidgetsPlugins`** (a nested group of `DefaultPlugins`) is where membership changed
(`bevy_ui_widgets` 0.19.1 `lib.rs:66-75` against 0.20.1 `lib.rs:93-107`, MEASURED):

| 0.19.1 | 0.20.1 |
|---|---|
| `PopoverPlugin`, `ButtonPlugin`, `CheckboxPlugin`, `ListBoxPlugin`, `MenuPlugin`, `RadioGroupPlugin`, `ScrollAreaPlugin`, `ScrollbarPlugin`, `SliderPlugin`, `EditableTextInputPlugin` | the same nine, `TextInputPlugin` (rename of `EditableTextInputPlugin`), and four new: `DialogPlugin`, `ModalDialogPlugin`, `TabPlugin`, `bevy_input_focus::pointer_focus::PointerFocusPlugin` |

`DefaultPickingPlugins` is unchanged (`PointerInputPlugin`, `PickingPlugin`,
`InteractionPlugin`; MEASURED in both).

**Avian `PhysicsPlugins`.** One addition 0.7 to 0.8 (MEASURED, `lib.rs` `.add(` lines):
`BodySizeMetricsPlugin::<Collider>`. It feeds `sweep_radius` to contact recycling and CCD
(`0.8:collision/narrow_phase/system_param.rs` `'recycle` block). It stays enabled.

**Exact edits for `server_plugin_floor` (`src/lib.rs:348-377`).**

1. `:359` `ui_widgets::EditableTextInputPlugin` becomes `ui_widgets::TextInputPlugin`.
2. Add four disables beside it:
   `ui_widgets::DialogPlugin`, `ui_widgets::ModalDialogPlugin`, `ui_widgets::TabPlugin`,
   `input_focus::pointer_focus::PointerFocusPlugin`.
   The first three are re-exported at the crate root (`pub use dialog::*`, `modal::*`,
   `tabs::*`, `bevy_ui_widgets` 0.20.1 `lib.rs:68-77`).
3. Nothing else in the list changes: every other disabled plugin is still a member under the
   same path.

**Other lists.** `gpu_less_default_plugins` (`lib.rs:327`, `WinitPlugin`),
`bevy_mesh_removal_lifecycle.rs:87-90`, `hosted_e2e.rs:88`: members unchanged. The Avian lists
(`net/physics.rs:33-34`, `bitprobe.rs:446`, `headless_test.rs:138`, `world/boundary.rs:56`,
`avian_body_query_gap.rs:50-51`): `PhysicsTransformPlugin`, `PhysicsInterpolationPlugin`,
`IslandPlugin`, `IslandSleepingPlugin` are all still members of 0.8's groups. No Avian plugin is
newly disabled: section 7 keeps `ColliderTransformPlugin` enabled.

**Decision.** One rename and four added disables, all in `src/lib.rs`. Not verified by a build:
whether `PointerFocusPlugin` is reachable as `bevy::input_focus::pointer_focus::PointerFocusPlugin`
through the `bevy` facade (read from `bevy_ui_widgets`' own import, not from `bevy_input_focus`).
Disabling a plugin that is not in the group panics at build of the group, so the first server
boot proves the list.

## 5. Draft: the missing `ContactCompliance` section of the fork's patch doc (A1.5)

For `vendor/avian3d-0.7.0-child-pose/OVERMATCH_PATCH.md` on `main`, placed after "Nearest
warm-start contact". It describes `main`'s code (MEASURED: `diff` of 0.7 against the fork's
`contact_types/mod.rs`, `solver/contact/mod.rs`, `solver/contact/normal_part.rs`,
`solver/plugin.rs:374,445`, `narrow_phase/system_param.rs:1045-1046`). `lane/soft-run` carries
its own longer section (rest length, inert points); when that lane lands, its section replaces
this one whole, so this draft is needed only if the lane parks.

```markdown
## Compliant contact (`src/collision/contact_types/mod.rs`, `src/collision/narrow_phase/system_param.rs`, `src/dynamics/solver/contact/`, `src/dynamics/solver/plugin.rs`)

Subject: the game's tank driving physics (the track belt against loose pieces), game state only.

`ContactManifold::compliance: Option<ContactCompliance>` is an absolute per-point spring-damper
law along the manifold's normal, `F = k·overlap + c·closing`, in N/m and N·s/m, in place of the
global contact softness. `None` keeps Avian's rigid contact. The narrow phase resets it to `None`
on every manifold each step; a writer sets it after the narrow phase and before the solver, so a
law never outlives the step that set it.

Constraint preparation (`ContactConstraint::generate`, which gains the substep length as an
argument) turns the law into soft-constraint coefficients at each point's own effective mass:
`ContactCompliance::coefficients` is `SoftnessParameters::compute_coefficients` rewritten with
`ω² = k/m` and `2ζω = c/m`. They are stored on the point as `ContactNormalPart::spring`, with
`compliant` set. A point of zero effective mass has no soft response.

`ContactNormalPart::solve_impulse` gives a compliant point two arms of its own. At positive
separation it removes its accumulated impulse, so it pushes only while it overlaps and never acts
as a speculative stop in front of the spring. In overlap it solves the spring in the biased and
the relaxing pass alike, uncapped by the overlap solve speed, so the compression under a steady
load is the spring's static deflection (`impulse = k·overlap·h`) and the relaxing pass does not
push it out.

The game's belt segments (`src/track/belt_segment.rs`) are the writer.

Guard: `src/tests/avian_compliant_contact.rs`. Local upstream draft:
`upstream/avian-compliant-contact.md` (DRAFT, not filed).
```

**Decision.** If `lane/soft-run` merges before the cutover (plan A3), nothing is committed from
this section. If it parks, the orchestrator commits this block as a doc commit.

## 6. Draft issues (A1.6)

The plan's section G is corrected in place for T2, T3, T6, T12 and the #353 note (see the plan).
Nothing was created or filed. T4's text also changes: the sleeping-pairs lanes are no longer
part of "settle the base".

## 7. The fork line target, patch by patch

"Fork lines after" counts lines inside upstream's files. Every figure is DERIVED unless marked.
"Reading" names what in 0.8.0 I read for the row; "Rests on" names what is still unproven and
what would prove it.

| # | Patch | Target form | Fork lines after | Reading of 0.8.0 | Rests on |
|---|---|---|---|---|---|
| 1 | Child-collider pose writes | hook, changed from the plan: `ColliderTransformPlugin` **stays**; its one registration of `update_child_collider_position` is deleted and the game adds `place_child_colliders` in `PhysicsStepSystems::First` | ≈4 | Registration at `0.8:collision/collider/collider_transform/plugin.rs:61-63`; the system is 20 lines over public components (`:66-91`). Disabling the plugin, as the plan had it, would also need `AncestorMarkerPlugin` public (`ancestor_marker` is `pub(crate)`, `0.8:lib.rs:557`) and the game to re-add the propagation system | first compile. Low risk |
| 2 | Attachment refusal | game-side observer on `Insert<ColliderOf>` | 0 | Both silent returns at `0.8:collision/collider/collider_hierarchy/mod.rs:96-101` | the observer's cost on a spawn batch of thousands: measured in B2 with `avian_spawn_batch`'s fixture; fallback is the 18-line patch |
| 3 | Moved-collider bounds prune | kept | ≈40 | Upstream's TODO and the unfiltered loop at `0.8:collider_tree/update.rs:913-934`; the system already holds a `ParamSet` (`colliders.p0()`) | the port itself (upstream rewrote the system around it) |
| 4 | Standalone key write-back | dropped | 0 | not re-read (scout: `0.8:collider_tree/update.rs:143-199`) | **unverified by me.** `avian_collider_of_removal.rs` (2 tests) on unpatched code verifies it |
| 5a | Island wake rule | hook, **larger than a predicate** (see below), or a game pass | ≈30 to 45, or 0 | `apply_contact_status_change` wakes any sleeping island a new contact links (`0.8:dynamics/solver/plugin.rs:303-313`) and has already pushed the pair's manifolds (`:298-300`); `apply_contact_status_changes` holds `Res<ContactGraph>`, not `ResMut` (`:191`) | **unverified, the weakest row.** See "Row 5" below |
| 5b | Rest history in `WakeIslands` | kept | ≈15 | 0.8 `WakeIslands` skips an island that is not sleeping and zeroes every body's timer in one that is (`0.8:dynamics/solver/islands/sleeping.rs:500-545`); the `Sleeping` removal hook does not reset the remover's timer (`:108-125`) | whether a merged island of awake and sleeping bodies still reaches the loop. `waking_a_merged_island_preserves_already_awake_rest_time` and `removing_sleeping_restarts_the_woken_bodys_rest_interval` on stock 0.8 say whether the patch is needed at all |
| 6 | Prepared collider mass | kept | ≈26 | not read (`collider/parry/mod.rs` changed 208/163 upstream) | **unverified.** The port; guard `prepared_contact_mass_is_valid_at_insertion_and_invalidates_with_its_inputs` |
| 7 | Minimum speculative margin | dropped | 0 | `speculative_distance = speculative_distance.max(contact_tolerance)` feeds both `max_contact_distance` and the pruning test (`0.8:collision/narrow_phase/system_param.rs`, "Ensure that the speculative distance…" and the `retain_points_mut` closure) | the witness on unpatched code |
| 8, 9 | Unknown features; nearest warm start | hook: a matcher **resource** read by the narrow phase, called where upstream matches. Not a field of `NarrowPhaseConfig`: that struct derives `Reflect`, `PartialEq` and serde (`0.8:collision/narrow_phase/mod.rs:176-180`), which a function pointer field fights | ≈15 | The match site, with `old_manifolds` a local clone dropped at the end of the closure, so no later game pass can see it (`system_param.rs`, "Save the old manifolds" to the `match_contacts` loop). Upstream matches only when `manifolds.len() <= 4` | whether the fork's matcher keeps or lifts upstream's four-manifold gate: decide in B2, name it in the patch doc |
| 10 | Physics work counters | kept, shrunk | ≈70 | `recycled_count` exists (`0.8:collision/diagnostics.rs:21`). The six reduction counters leave the fork (section 1) | **the estimate is unverified.** Counted when the port is done |
| 11 | Collider batches, explicit mass ownership | kept | ≈55 | `AncestorMarker` still needs its one-line re-export (`0.8:lib.rs:557`) | the port (`mass_properties/mod.rs` changed 69/78 upstream) |
| 12 | Pairless-layer broad-phase prune | kept | ≈35 | not re-read; the scout's merge was clean (MEASURED there) | first compile |
| 13 | Stationary non-touching pair cache | module: fork file `collision/narrow_phase/stationary_miss.rs` (≈120 lines, not an upstream file) plus the `ContactPair` field and two call sites | ≈20 | `update_contacts` is private and the per-pair closure owns every input (`system_param.rs:393` on); the two sites are before `contact_manifolds_with_context` and after pruning. `ContactPair` derives `Clone, Debug, PartialEq` and serde (`0.8:collision/contact_types/mod.rs:128-130`) | the six `stationary_miss_*` tests. Plus 5 over the plan for the field and its derives |
| 14 | Body-pair contact reduction | game-side pass in `NarrowPhaseSystems::Last` | **0 (MEASURED by prototype 1)** | — | the 18 guards not run on 0.8, and the items in the plan's B2 acceptance list |
| 15 | Rest energy | hook: a rest-test function resource called per body in `update_sleeping_states` | ≈20 | The loop is serial and reads `SolverBodies` by index (`0.8:dynamics/solver/islands/sleeping.rs:186-246`); `AwakeIslandBitVec` is `pub(crate)` (`:164`), so a game system cannot make the decision itself. The fork's split-candidate tie-break by island id existed for its parallel loop and goes with it | the five rest tests |
| 16 | Moved-list removal by index | dropped | 0 | `MovedProxyList::push` returns `false` on a second entry (`0.8:collider_tree/tree.rs:57-61`) | `avian_moved_lists.rs` rewritten to upstream's API |
| 17 | Sleeping bodies' velocity increments | game-side system in `IntegrationSystems::ClearVelocityIncrements` | 0 | `linear_increment`, `angular_increment` are `pub` (`0.8:dynamics/integrator/mod.rs:217-233`); the set exists (`:97-114`); the mismatch stands: `pre_process_velocity_increments` runs over every body, `clear_velocity_increments` only `With<SolverBodyIndex>` (`:261-327`) | `a_woken_body_takes_only_its_first_steps_gravity` |
| 18 | Serial small loops | dropped | 0 | `MIN_PAR_ITER_ENTITIES = 4096` (`0.8:utils.rs:19`) | measured in D2 |
| 19 | Compliant contact, rest length, inert points | hook: `compliant` and `spring` on `ContactNormalPart`, two arms in `solve_impulse` | ≈25 | Every field the game writes is `pub`: `initial_separation`, `normal_part`, `tangent_part` (`0.8:dynamics/solver/contact/mod.rs:26-47`), `impulse`, `effective_mass`, `softness` (`normal_part.rs:14-31`). `solve_impulse::<USE_BIAS>` is one `if` chain (`:114-160`). No public field can make the relaxing arm soft, so the two arms cannot be game-side | `avian_compliant_contact.rs`. The game finds constraints by scanning on `contact_id` and `manifold_index`, not through handles (prototype 2, workaround 3) |
| — | #1032 transfer refused for non-touching pairs | hook: `sleep_entity_with`'s `should_sleep` widened to see the edge; `SleepIslands` passes `edge.is_touching() && is_other_body_asleep(other)` | **≈10, down from ≈100** | `S: Fn(Option<Entity>) -> bool` receives only the other body (`0.8:collision/contact_types/contact_graph.rs:785-820`); the edge is in hand at the call (`:806-817`) and has `is_touching()` (`contact_types/mod.rs:90`); the caller is `sleeping.rs:418-447`. The plan's ≈100 counted the hold's `RESTING_MISS` flag and pair-transfer methods, which `main` records as forgone (fork `OVERMATCH_PATCH.md:266-291`) | the three witnesses named there, which are on `main` in `src/tests/avian_island_sleep.rs` |
| — | Narrow-phase query-gap panic | kept | ≈24 | on `main` now ("Unreadable pair state") | `a_body_query_gap_still_panics_the_solver`, `src/net/ram/avian_tests.rs` |
| — | Upstream #1101 | cherry-pick | ≈1 | `solve_continuous`'s `colliders` query has no `Without<ColliderDisabled>` in 0.8.0 (`0.8:dynamics/ccd/mod.rs:596`) | — |
| — | Contact recycling off | config value | 0 | `recycle_distance <= 0.0` breaks out of `'recycle` first (`system_param.rs`, `'recycle` block) | a new `insert_resource(NarrowPhaseConfig { recycle_distance: 0.0, ..default() })` where the game composes physics (`src/net/physics.rs`); none exists today |
| — | Belt friction rows | game-side (#353) | **0 (MEASURED by prototype 2)** | — | the items in the plan's D1 acceptance list |

**Sum (DERIVED).** 4 + 0 + 40 + (30 to 45) + 15 + 26 + 15 + 70 + 55 + 35 + 20 + 0 + 20 + 0 + 25
+ 10 + 24 + 1 = **about 390 to 405 lines inside upstream files**, plus one fork-only file of
about 120 lines. The plan's 476 falls by about 90 for the forgone hold and by 1 for row 1, and
rises by 5 for row 13 and by up to 15 for row 5. If row 5a becomes a game pass the sum is about
360.

**Row 5, why a predicate is not enough.** The fork's rule on 0.7 does three things when a new
contact links a sleeping island and neither participant is an awake non-static body (fork
`narrow_phase/system_param.rs:440-541`): it links the contact to the island, it does not wake,
and it moves the participants' pairs to the sleeping set and pops their constraints. On 0.8 a
hook inside `apply_contact_status_change` that only says "do not wake" leaves a constraint in
the graph between bodies that have no solver body. So the hook must also undo the
`push_manifold` and transfer the pair, which needs `ResMut<ContactGraph>` where upstream holds
`Res`. Two candidates:

- (b) the hook as planned, widened: predicate resource, plus the transfer and pop in the fork.
  About 30 to 45 lines in `solver/plugin.rs` (DERIVED).
- (c) a game pass in `NarrowPhaseSystems::Last`, the same slot and method as the reduction pass:
  for each queued `StartedGeneratingConstraints(contact)` whose two bodies are both sleeping or
  static, remove the queue entry, call `PhysicsIslands::add_contact` itself (public,
  `0.8:dynamics/solver/islands/mod.rs:571`), and move the pair with
  `ContactGraph::sleep_entity_with` (public). `WakeIslands` later pushes the manifolds of every
  touching pair it wakes (`sleeping.rs:520-533`). 0 fork lines. `PhysicsIsland::is_sleeping` is
  `pub(crate)` (`islands/mod.rs:228`), so the pass reads `Has<Sleeping>` on the bodies instead.

What verifies it: port the four wake witnesses
(`stationary_contact_does_not_wake_a_resting_island`,
`an_impact_wakes_an_island_that_merges_again_before_the_solver`,
`waking_a_merged_island_preserves_already_awake_rest_time`,
`removing_sleeping_restarts_the_woken_bodys_rest_interval`) into `proto/stack-seams/` and run
them three ways on stock 0.8.0: with nothing (what does upstream do today), with pass (c), and
record which fail. That is the first task of B2's solver-and-islands step, before row 5 is
written. **Decision: B2 tries (c) first and falls back to (b).**

**Rows still resting on an unverified reading**, in order of risk: 5a (above), 10 (the size),
6 (not read), 4 and 12 (the scout's reading, not mine), 5b (may not be needed). None blocks the
start: each is settled inside B2 by a named guard.

## 8. Baseline commands (plan A2), each verified to exist

Run at the alpha.16 tag. All paths are relative to the repo root. "Verified" = the file exists
and takes the flags shown (MEASURED by reading the script or its argument parser).

| Baseline | Command | Verified against |
|---|---|---|
| Build the binaries the runners need | `scripts/serial-cargo build --locked --features dev_ui,bitprobe --bin overmatch --bin overmatch-server` (debug, for `scripts/cost/run.sh`, the captures and the shader gate); `scripts/serial-cargo build --locked --release --features dev_ui,bitprobe --bin overmatch --bin overmatch-server` (for `pile_park.py`, whose `--bin` defaults to `target/release`) | `scripts/serial-cargo:1-45`; `scripts/cost/run.sh:33`; `scripts/perf/pile_park.py:503` |
| Bitprobe, macOS leg | `scripts/serial-cargo run --release --locked --features bitprobe --bin bitprobe -- bitprobe-macos-aarch64.obp.gz` | `scripts/bitprobe/README.md:13-14`; `Cargo.toml` `[[bin]] bitprobe`, `required-features = ["bitprobe"]` |
| Bitprobe, Linux leg | `gh workflow run bitprobe.yml --ref <pushed ref>`; `gh run watch <run-id> --exit-status`; `gh run download <run-id> --name bitprobe-linux-x86_64 --dir <dir>` | `README.md:21-24`; `.github/workflows/bitprobe.yml:4` (`workflow_dispatch`) |
| Bitprobe, compare | `python3 scripts/bitprobe/diff.py bitprobe-macos-aarch64.obp.gz <dir>/bitprobe-linux-x86_64.obp.gz` | `README.md:30-32`; `scripts/bitprobe/diff.py` |
| Track scorecard | `scripts/track-scorecard` (no arguments; writes `target/track-scorecard.{md,json}`) | the script, 8 lines: it runs `headless_test::track_scorecard::track_scorecard -- --ignored --nocapture` through the queue |
| Track probes | `OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 scripts/serial-cargo test --lib --features dev_ui,bitprobe <name> -- --ignored --nocapture` for each of `belt_friction_probe`, `creep_stall_probe`, `rubble_climb_probe`, `turn_probe`. Also present and not in the plan: `pivot_probe`, `rubble_wedge_probe`, `rubble_park_probe`, `soft_run_probe` | `src/headless_test/track_scorecard/{belt_friction.rs:622, creep_stall.rs:187, rubble_climb.rs:899,1430,1446, turn_probe.rs:711,879, soft_run_probe.rs:862}`, each `#[ignore = "… run on demand"]`. The environment is the scorecard script's own |
| Parked on a pile | `/usr/bin/lockf -k -t 2400 /tmp/overmatch-cargo.lock python3 scripts/perf/pile_park.py --out target/pile-park` (optional `--bin`, positional conditions default `pile flat rubble rubble flat pile`) | `pile_park.py:5-6,502-513` |
| Scheduled collapse | `scripts/perf/collapse.sh --target local --schedule scripts/perf/match-collapse.json --rehearsal` (optional `--out DIR`, `--bin`, `--tanks`, `--clients`). It writes `tick-attribution.json` into the run directory: the game's own stage account, with Avian's counters nested | `collapse.sh:3-4`; `collapse.py:1059-1076,474-483`; `scripts/perf/match-collapse.json` |
| Damage-model cost harness (game state only) | `OUT=<dir> scripts/cost/run.sh idle1 fire1`, then `python3 scripts/cost/analyze.py LABEL=<dir>/<file>.jsonl … [--baseline LABEL]` | `run.sh:3-8,36-41` (scenarios `idle1 fire1 fire2 armor1 loft1`; `OUT` defaults to `/tmp/mg-cost`, so set it); `analyze.py:166-168` |
| Shader pipelines compile | `scripts/render/verify-shader-compile.sh 30` (`BIN` selects the binary directory, default `target/debug`) | the script's runbook header and `:29-33` |
| Shader captures | `OVERMATCH_CAPTURE=<plan> target/debug/overmatch --offline` for `scripts/capture/{example,repair,wood-preview,lod-showcase,tab-board-large,tab-board-small}.ron` and the nine plans in `scripts/capture/surroundings/` (`hull-closeup`, `p1-normal`, `p2-sight`, `p3-hill`, `p4-hill-sight`, `p5-outside`, `p6-corner`, `p7-farplane`, `w1d-wall`) | `src/dev_capture/plan.rs:1-16`; `src/main.rs:2` (`--offline`); `scripts/capture/example.ron:1-8` |
| Tick attribution per system | no tracked wrapper. Proposal below | `.agents/scratch/perf-2026-11-01/server-tick.md:50-66` (the method); `.agents/scratch/perf-2026-10-31/tools/{profile-run.sh,trace_profile.py}` (untracked precedents) |

Two cautions for the lane that runs these:

- `pile_park.py`, `collapse.sh` and `cost/run.sh` take the machine lock themselves
  (`/tmp/overmatch-cargo.lock`). Do not wrap them in `scripts/serial-cargo` or `scripts/build-lock`:
  the lock does not nest.
- The collapse run's `tick-attribution.json` is a coarser account than the per-system trace and
  costs nothing extra. Record both; the per-system trace is what the plan's D3 ("vendored prunes
  made redundant") needs.

### Proposed wrapper: `scripts/perf/tick-trace.sh` (not created)

What exists today for per-system attribution is a method in a scratch note and two untracked
tools. `profile-run.sh` drove the client with `SPIKE_SIMULATE_INPUT`, `SPIKE_SIM_TICKS` and
`SPIKE_SIM_IDLE`, none of which exists in `src/` any more (0 matches, MEASURED); the client is
now scripted by `OVERMATCH_HARNESS` (`src/net/harness.rs:63`). `OVERMATCH_SERVER_THREADS` is
also gone (0 matches). The analyser, `trace_profile.py <trace.json> <window.json> <out.json>`,
reads `w0_us` and `w1_us` (µs from the server's launch) from the window file.

The proposal tracks both: the script below, and `trace_profile.py` moved to
`scripts/perf/trace_profile.py` unchanged. Levers verified in `src/`: `OVERMATCH_BOT`
(`net/server.rs:67`), `OVERMATCH_SERVER_STATS_S` (`net/server_stats.rs:146`),
`OVERMATCH_SERVER_BIND` and `OVERMATCH_MUTE` (`net/server.rs`), `OVERMATCH_SERVER`
(`net/client.rs:199`), `OVERMATCH_HARNESS`, `TRACE_CHROME` (`bevy_log` 0.19.1 `lib.rs:335`,
0.20.1 `lib.rs:325`), the server's "listening" line (`net/server.rs:127`), the client's
"harness: input slot" line (`net/harness.rs:385`), port 5888 (`net/server.rs:44`).

Not verified: that the script runs (nothing was executed). The window lengths are
`Resolution`-free session configuration copied from the 2026-11-01 method (empty map: 30 s from
5 s after listening; combat: 120 s from 15 s after the client's input slot), ASSUMED to still
suit the shipped Battle's fill-up length. A trace of a 29-bot run was 21 to 25 GB on 2026-11-01
(MEASURED there): check free disk first.

```zsh
#!/bin/zsh
# tick-trace.sh — per-system cost per server tick, from Bevy's chrome trace (game state only).
#
#   usage: scripts/perf/tick-trace.sh <out-dir> [label]
#   build: CARGO_PROFILE_PERF_STRIP=false scripts/serial-cargo build --locked --profile perf \
#            --features dev_ui,bitprobe,bevy/trace_chrome --bin overmatch --bin overmatch-server
#   env:   BIN (default target/perf), BOTS (default 29), EMPTY_WARMUP_S (5), EMPTY_S (30),
#          COMBAT_WARMUP_S (15), COMBAT_S (120)
#
# One run, two windows: the server alone on the empty map, then one parked scripted client joins
# and the combat window opens after its input slot. Each window is profiled into
# <out>/<label>.<phase>.profile.json by scripts/perf/trace_profile.py; the trace itself is deleted
# after profiling unless KEEP_TRACE=1 (a 29-bot trace is tens of GB).
#
# A `perf` time compares with other `perf` times only. Before/after pairs run back to back in one
# lock hold, A B B A: the machine mixes core types and one run's tick p50 spans a factor of two.
set -eu
zmodload zsh/datetime

if [[ "${1:-}" != --holding-lock ]]; then
  exec /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$0" --holding-lock "$@"
fi
shift
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:?usage: tick-trace.sh <out-dir> [label]}"; LABEL="${2:-$(git -C "$REPO" rev-parse --short HEAD)}"
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
BIN="${BIN:-target/perf}"
SERVER="$REPO/$BIN/overmatch-server"; CLIENT="$REPO/$BIN/overmatch"
BOTS="${BOTS:-29}"
EMPTY_WARMUP_S="${EMPTY_WARMUP_S:-5}"; EMPTY_S="${EMPTY_S:-30}"
COMBAT_WARMUP_S="${COMBAT_WARMUP_S:-15}"; COMBAT_S="${COMBAT_S:-120}"
[[ -x "$SERVER" && -x "$CLIENT" ]] || { echo "build first (see the header)" >&2; exit 1; }
mkdir -p "$OUT"
TRACE="$OUT/$LABEL.trace.json"; SLOG="$OUT/$LABEL.server.log"; CLOG="$OUT/$LABEL.client.log"
SCRIPT="$OUT/$LABEL.harness.ron"
# The client parks for both warm-ups and the combat window, plus slack; 64 Hz fixed ticks.
print "(drive: Idle, ticks: $(( (COMBAT_WARMUP_S + COMBAT_S + 60) * 64 )))" >"$SCRIPT"

SPID= CPID=
cleanup() {
  [[ -z "$CPID" ]] || { kill "$CPID" 2>/dev/null || true; wait "$CPID" 2>/dev/null || true; }
  if [[ -n "$SPID" ]]; then
    kill -INT "$SPID" 2>/dev/null || true
    for _ in {1..40}; do kill -0 "$SPID" 2>/dev/null || break; sleep 0.25; done
    kill -KILL "$SPID" 2>/dev/null || true; wait "$SPID" 2>/dev/null || true
  fi
  SPID= CPID=
}
trap cleanup EXIT; trap 'exit 130' INT; trap 'exit 143' TERM

if lsof -nP -iUDP:5888 >/dev/null 2>&1; then echo "UDP 5888 is busy" >&2; exit 1; fi
window() { # phase w0 w1
  printf '{"label":"%s","phase":"%s","w0_us":%.0f,"w1_us":%.0f,"bots":%s}\n' \
    "$LABEL" "$1" "$(( ($2 - launch) * 1e6 ))" "$(( ($3 - launch) * 1e6 ))" "$BOTS" \
    >"$OUT/$LABEL.$1.window.json"
}

launch=$EPOCHREALTIME
(cd "$REPO" && exec env OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1 OVERMATCH_SERVER_BIND=127.0.0.1 \
   OVERMATCH_SERVER_STATS_S=1 OVERMATCH_BOT="$BOTS" TRACE_CHROME="$TRACE" \
   "$SERVER" >"$SLOG" 2>&1) &
SPID=$!
for _ in {1..300}; do grep -q 'listening on' "$SLOG" 2>/dev/null && break; sleep 0.2; done
grep -q 'listening on' "$SLOG" || { echo "the server never listened; see $SLOG" >&2; exit 1; }

sleep "$EMPTY_WARMUP_S"; w0=$EPOCHREALTIME; sleep "$EMPTY_S"; w1=$EPOCHREALTIME
kill -0 "$SPID" || { echo "the server died in the empty-map window" >&2; exit 1; }
window empty "$w0" "$w1"

# The client is the same build with its spans filtered off at their callsites; its own trace file
# is the near-empty proof.
(cd "$REPO" && exec env OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1 OVERMATCH_SERVER=127.0.0.1:5888 \
   OVERMATCH_HARNESS="$SCRIPT" TRACE_CHROME="$OUT/$LABEL.client.trace-off.json" \
   RUST_LOG="bevy_ecs=warn,bevy_app=warn" "$CLIENT" >"$CLOG" 2>&1) &
CPID=$!
start=$EPOCHREALTIME
until grep -q 'harness: input slot' "$CLOG" 2>/dev/null; do
  kill -0 "$CPID" 2>/dev/null || { echo "the client exited before its input slot; see $CLOG" >&2; exit 1; }
  (( EPOCHREALTIME - start < 1200 )) || { echo "the client took no input slot" >&2; exit 1; }
  sleep 0.5
done
sleep "$COMBAT_WARMUP_S"; w0=$EPOCHREALTIME; sleep "$COMBAT_S"; w1=$EPOCHREALTIME
kill -0 "$SPID" || { echo "the server died in the combat window" >&2; exit 1; }
kill -0 "$CPID" || { echo "the client died in the combat window" >&2; exit 1; }
window combat "$w0" "$w1"
cleanup; trap - EXIT

for phase in empty combat; do
  python3 "$REPO/scripts/perf/trace_profile.py" "$TRACE" "$OUT/$LABEL.$phase.window.json" \
    "$OUT/$LABEL.$phase.profile.json"
done
[[ -n "${KEEP_TRACE:-}" ]] || rm -f "$TRACE"
{ print "commit: $(git -C "$REPO" rev-parse HEAD)"; print "command: $0 $OUT $LABEL"
  print "BIN=$BIN BOTS=$BOTS EMPTY=$EMPTY_WARMUP_S+$EMPTY_S COMBAT=$COMBAT_WARMUP_S+$COMBAT_S"
} >"$OUT/$LABEL.run.txt"
echo "$LABEL: $OUT/$LABEL.{empty,combat}.profile.json"
```

Open points the lane that lands it settles (none blocks the baseline: the fallback is the
2026-11-01 method by hand, recording its command line):

1. The 1,200 s wait for the input slot assumes the scripted client is admitted during the
   shipped Battle's fill-up; the 2026-11-01 run joined that way. If admission waits for the next
   Battle, the combat window needs the measurement build's shortened mode data described in
   `server-tick.md:60-63`.
2. Whether the unattended client needs `OVERMATCH_WINDOW` unset or `headless` for a GPU-less
   run: `scripts/harness/idle.ron` says the harness "runs GPU-less (no `OVERMATCH_WINDOW`)".
3. `trace_profile.py` was written for tracing-chrome 0.7 on Bevy 0.19; on Bevy 0.20 the "after"
   run confirms the span names (`system: name=…`, `schedule: name=FixedMain`) before its
   numbers are compared.

## 9. Does anything block the cutover from starting when the queue drains?

No. What must be true first, all from the plan's A3 and already in motion:

- `lane/soft-run` (bay-1, 9 ahead, dirty) merged or parked; `lane/ramp-test` (bay-2) merged.
- alpha.16 tagged, and the baselines of section 8 recorded at that tag (one bay).
- The owner's answers to plan questions F1 (three bays) and F2 (push the trunk branch). The
  others have defaults the plan can start on.

Decided here, so the brief can be written now: the export list (section 1); the lightyear patch
shape (section 2); the six sites (section 3); the plugin edits (section 4); B2 tries the wake
rule as a game pass first (section 7).
