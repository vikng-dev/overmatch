# Fork moves at 21a66be7

Scope: the video game's rigid-body physics and diagnostics. No timing runs or external postings.

MEASURED baseline, added + removed source lines from `diff -ruN` against pristine release crates:
avian3d 0.8.0 **915**; bevy_pbr 0.20.1 **357**; lightyear_netcode 0.31.0 **159**;
lightyear_avian3d 0.31.0 **9**. The earlier audit's bevy_pbr count was 359; this lane starts after
main removed its two alignment lines.

## Source checks

- Bevy 0.20.1 `bevy_app/src/app.rs::remove_systems_in_set` returns the removed count.
  `bevy_ecs/src/schedule/schedule.rs::Schedule::remove_systems_in_set` initializes a changed graph
  before removal. Replacements therefore remove the upstream writer before adding the game writer,
  in plugin cleanup after all builds and finishes. Each requires exactly one removal.
- Published lightyear_avian3d 0.31.0 archive SHA-256:
  `d0ea43838ce698bd3b21b0298b74a6838b3f1942052f4ccea2489413d766c739`.
  Its only source delta was the Position-mode child writer's registration.
- Avian 0.8.0 `utils.rs::ParallelQueryForEach` checks `MIN_PAR_ITER_ENTITIES` (4096) and the
  compute-pool size. Upstream `update_sleeping_states` instead computes timers and commits island
  votes in one serial loop. The restored form retains that dependency and the RestTest call.
- Avian 0.8.0 `utils.rs::par_for_each`: serial exactly when the pool has one worker or the slice
  is shorter than its supplied floor. The three contact pass sites supply 64; overflow is serial.
  Public `ConstraintGraph::colors`, `GraphColor::contact_constraints`, `COLOR_OVERFLOW_INDEX` and
  `ComputeTaskPool::thread_num` expose every input to that decision. The root dependency retains
  Avian's default parallel feature.
- Bevy 0.20.1 `Query::is_empty` uses a no-data fetch for archetypal query data. With only With,
  Without and Or filters, a missing-state query skips valid archetypes rather than inspecting
  every valid entity each tick.

## Verification

Move 1: cargo-deny: `advisories ok, bans ok, licenses ok, sources ok`.

Move 1 test adjustment: Bevy 0.20.1 `schedule/node.rs::SystemSets::remove` removes the set arena
entry but retains its `ids` entry, which `SystemSets::contains` tests. A removed system therefore
still appeared registered to the old assertion. The guards inspect live `graph.systems` and each
system's typed default sets instead. The invariant remains absence of the engine writer.
The launched-replica fixture now installs the published Lightyear plugin before the replacement;
its gameplay assertions are unchanged. Lightyear removal lives in `net::physics`, preserving the
simulation's independence of networking.

Pre-existing finding met by a test-only edit in `src/net/protocol.rs`: backlog `net-22`, remote
servo presentation timing (also issue 187). This lane changes only the test fixture's plugin
composition and leaves that behavior unchanged; the finding remains with its owning work.

## Commits and gates

| Move | Commit | Avian source delta, MEASURED | Targeted library gate |
|---|---|---:|---|
| 1: child placement | `0f2b5a70` | 880 | 1288 passed, 0 failed, 21 ignored |
| 2: serial rest loop | `4778f6ce` | 821 | 1287 passed, 0 failed, 21 ignored |
| 3: propagation floor | `00bb0a04` | 814 | 1287 passed, 0 failed, 21 ignored |
| 4: solver visits | `b49f068d` | 749 | 1288 passed, 0 failed, 21 ignored |
| 5: required-state gate | `dcc9ba94` | 660 | 1290 passed, 0 failed, 21 ignored |
| 6: engine matcher | `f451a854` | 717 | 1290 passed, 0 failed, 21 ignored |

Each row also passed `cargo fmt --all --check` and CI clippy with `dev_ui,bitprobe`.
Only move 1, which updated Cargo.lock, omitted clippy's `--locked`; later rows restore it.
Targeted filters are passed after libtest's `--`, since Cargo itself accepts one positional filter:
`tests::avian_ net:: track construction debris cost`.

Move 2 restores upstream's split-candidate comparison and query-order tie behavior as well as the
serial loop. The retired implementation used an island-ID tie break. No law assertion was changed.
Its fork delta shrank by 59 lines (MEASURED), rather than treating the earlier sort's partitioned
line estimate as a target.

## Dispositions

All seven moves are done; none was skipped.

1. Avian's original child writer and registration are restored, with public access. The game
   removes it and the published Lightyear writer in plugin cleanup, after builds and finishes,
   and requires exactly one removal at each boundary. The Lightyear integration vendor directory
   and patch entry are gone; Cargo.lock carries the published checksum. Metadata, the build and
   the CI cargo-deny command accepted that resolution.
2. The rest pass is upstream's serial timer/vote loop with the RestTest call. Its parallel output
   buffer, refusal and dispatch-only witness are gone. The island vote and split-candidate update
   remain in that loop; this lane makes no performance claim.
3. Collider-transform propagation uses upstream's per-query dispatch floor.
4. Contact visits come from public graph lengths at each actual pass boundary, with overflow
   serial and the pinned worker-count/floor rule. The engine counter fields, recording calls and
   altered dispatch return value are gone. Before and after, the unchanged census assertions
   cover pass multiplicity, per-step reset, serial/parallel classification and overflow. The
   source-contract witness checks the pinned dispatch rule; no timing run was used.
5. One First-stage game gate refuses incomplete posed, enabled participants before child
   placement. Negative archetypal queries avoid a scan of healthy entities. Only attachments
   initially missing their owner enter a pending queue: admission requires a rigid body and its
   reciprocal collider list, while valid late construction, disabled entities and unposed replicas
   remain named states. The narrow-phase, metric and island fetches use upstream's optional forms;
   the moved-proxy engine-internal refusal is gone. The solver-membership tripwire stays with the
   sleeping-increments fix. Two added attachment witnesses cover the incomplete owner and a late
   owner outside the hierarchy whose reciprocal list was never built.
6. The engine carries the combined unknown-feature identity and nearest-contact fix. The
   ContactMatcher resource and game matcher module are deleted. Known identities and unknown
   positions have distinct admissibility rules, and all old manifolds participate in one nearest
   search. Warm-start assertions are unchanged; the two direct unit calls now use the engine's
   slice API. As drafted, NaN distance comparison uses `<` rather than the deleted adapter's
   `total_cmp`; finite contact data retains the same ranking. No non-finite input policy is added.
7. The upstream index has the 15 rebased proposals, all marked **not sent**, with no project name
   in any draft. The child proposals expose Avian's system and fix Lightyear's unconditional
   writes respectively. The sleeping-increments draft includes its membership tripwire; the size
   metrics draft mirrors the parallel once-per-body computation without test instrumentation.
   Every embedded patch applies to its pristine pinned crate with `patch --dry-run` (MEASURED).
   Retired investigations that source comments still cite are preserved outside the proposal
   queue, with short pointers; obsolete unreferenced proposals are dropped. Their historical
   dependency on the rollback-starvation investigation is preserved in that archive too.

## Assertion changes

- No warm-start, contact-visit or physical-law assertion changed.
- The four missing-state panic expectations name the game gate instead of the former fork sites.
  The missing-SleepTimer witness advances an ordinary physics update, where admission now lives,
  instead of directly invoking an engine wake command.
- The existing Lightyear schedule-absence assertion reads live systems rather than Bevy's retained
  system-set IDs, as explained above. Its asserted invariant is unchanged.
- The parallel rest implementation's dispatch-only witness was deleted with that implementation.

## Final fork measurements

MEASURED added + removed source lines against the pristine release, using `diff -ru` and counting
fork-only source files in full. The baseline was independently reproduced from `21a66be7`; each
existing move's committed source delta was rechecked against pristine as part of continuation.

| Crate | Before | After |
|---|---:|---:|
| avian3d 0.8.0 | 915 | 717 |
| bevy_pbr 0.20.1 | 357 | 357 |
| lightyear_netcode 0.31.0 | 159 | 159 |
| lightyear_avian3d 0.31.0 | 9 | 0 |
| Total | 1440 | 1233 |

The combined engine matcher increases Avian's delta by 57 lines (MEASURED), replacing a hook
with a bug fix and deleting the game module. Fork size is recorded, not used as a target.
The patch records' section totals reproduce their source deltas. No wire type changed.

## Final verification

Move 7 passed `cargo fmt --all --check`, locked CI clippy over the workspace/all targets with
`dev_ui,bitprobe`, and the required targeted library gate: **1290 passed, 0 failed, 21 ignored**.
All 15 embedded draft patches apply to pristine, and all local Markdown links resolve.

The whole library ran once, as requested:

```text
test result: FAILED. 2796 passed; 1 failed; 49 ignored; 0 measured; 0 filtered out; finished in 406.41s
```

The game's damage-model camera UI fixture failed at
`src/damage_cam/pen_cam/game/killcam/tests.rs:878`: it expected throttle and steering both equal
to the throttle notch, `(0.33333334, 0.33333334)`, and received `(0.33333334, 1.0)`.
These are MEASURED assertion values, not new expectations. Its isolated rerun reproduced it:

```text
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2845 filtered out; finished in 0.29s
```

Baseline evidence: `21a66be7` already contains both that assertion and
`src/command.rs:791-792`, which maps held steering directly to the difference of the two pressed
keys (one positive key gives 1.0). The input owner's existing
`steering_follows_held_keys_without_cruise` guard pins that contract. Both files are unchanged by
this lane. The failing UI composition (`src/damage_cam/pen_cam/game.rs:625-631`) mounts task pools,
assets, transforms and world serialization, with empty collider trees; it does not install the
physics plugins changed here. This pre-existing fixture mismatch remains for its owner; no
assertion was weakened to hide it.
