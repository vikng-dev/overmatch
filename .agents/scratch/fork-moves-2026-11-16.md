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

The seven moves and their review corrections are reflected in these final dispositions.

1. Avian's original child writer and registration are restored, with public access. The game
   removes it and the published Lightyear writer in plugin cleanup, after builds and finishes,
   and requires exactly one removal at each boundary. The Lightyear integration vendor directory
   and patch entry are gone; Cargo.lock carries the published checksum. Metadata, the build and
   the CI cargo-deny command accepted that resolution.
2. The rest pass is upstream's serial timer/vote loop with the RestTest call. Its parallel output
   buffer, refusal and dispatch-only witness are gone. The island vote and split-candidate update
   remain in that loop. The removed pass used the same 4,096-entity floor as pristine 0.8.0
   (`4778f6ce^` sleeping.rs:268-269 and utils.rs:19; pristine utils.rs:19), so the recorded
   body counts ran serially before and after.
3. Collider-transform propagation dispatches disjoint root hierarchies directly. The per-query
   floor counts roots rather than descendants walked. The restored patch record preserves the
   MEASURED 436.8 µs serial / 176.6 µs dispatched containing-phase comparison and its workload,
   plus the historical two-worker evidence; the new local draft proposes this as a bug fix.
4. Contact visits come from public graph lengths at each actual pass boundary, with overflow
   serial and the pinned worker-count/floor rule. The engine counter fields, recording calls and
   altered dispatch return value are gone. Before and after, the unchanged census assertions
   cover pass multiplicity, per-step reset, serial/parallel classification and overflow. The
   source-contract witness checks the pinned dispatch rule; no timing run was used.
5. One First-stage game gate refuses incomplete posed, enabled participants before child
   placement. Negative archetypal queries avoid a scan of healthy entities. Only attachments
   initially missing their owner enter a pending queue: admission requires a rigid body and its
   reciprocal collider list, while valid late construction, disabled entities and unposed replicas
   remain named states. Avian can create a newly enabled node before its first solver index;
   the game requires SleepTimer with BodyIslandNode so it is complete at node creation. The narrow-phase and metric fetches
   use upstream's optional forms. The wake walk expects every sleeping-island link to resolve,
   including direct commands outside admission; the moved-proxy engine-internal refusal is gone.
   The solver-membership tripwire stays with the
   sleeping-increments fix. Two added attachment witnesses cover the incomplete owner and a late
   owner outside the hierarchy whose reciprocal list was never built.
6. The engine carries the combined unknown-feature identity and nearest-contact fix. The
   ContactMatcher resource and game matcher module are deleted. Known identities and unknown
   positions have distinct admissibility rules, and all old manifolds participate in one nearest
   search. Warm-start assertions are unchanged; the two direct unit calls now use the engine's
   slice API. As drafted, NaN distance comparison uses `<` rather than the deleted adapter's
   `total_cmp`; finite contact data retains the same ranking. No non-finite input policy is added.
7. The upstream index has 16 proposals, all marked **not sent**, with no project name
   in any draft. The child proposals expose Avian's system and fix Lightyear's unconditional
   writes respectively. The sleeping-increments draft includes its membership tripwire; the size
   metrics draft mirrors the parallel once-per-body computation without test instrumentation.
   Every embedded patch applies to its pristine pinned crate with `patch --dry-run` (MEASURED).
   Retired investigations that source comments still cite are preserved outside the proposal
   queue, with short pointers; obsolete unreferenced proposals are dropped. Their historical
   dependency on the rollback-starvation investigation is preserved in that archive too.

## Assertion changes

- No warm-start, contact-visit or physical-law assertion changed.
- Narrow-phase and metric missing-state expectations name the game gate. Missing-SleepTimer
  witnesses cover both scheduled admission and direct WakeIslands; a fresh enabled node gains
  its required timer at creation before its first solver index.
- The existing Lightyear schedule-absence assertion reads live systems rather than Bevy's retained
  system-set IDs, as explained above. Its asserted invariant is unchanged.
- The parallel rest implementation's dispatch-only witness was deleted with that implementation.

## Final fork measurements

MEASURED added + removed source lines against the pristine release, using `diff -ru` and counting
fork-only source files in full. The baseline was independently reproduced from `21a66be7`; each
existing move's committed source delta was rechecked against pristine as part of continuation.

| Crate | Before | After |
|---|---:|---:|
| avian3d 0.8.0 | 915 | 729 |
| bevy_pbr 0.20.1 | 357 | 357 |
| lightyear_netcode 0.31.0 | 159 | 159 |
| lightyear_avian3d 0.31.0 | 9 | 0 |
| Total | 1440 | 1245 |

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

## Review corrections

The child-writer removals all use `RemoveSystemsOnly`. Cleanup supplies an Avian completion
marker and clears a Lightyear pending marker; admission refuses updates without completion.
Both schedule guards inspect live systems for the game writer's presence and the engine writer's
absence through one local helper. Production and fixtures mount the same Lightyear installer,
including its spawn-transform override. Stale mutant descriptions now name admission invariants;
single-manifold matcher fixtures hold manifolds directly. No wire registration or net-22 behavior
changed, and the backlog row remains open.

Deleted investigations cited by ADR-0028 and ADR-0032 are preserved in the history directory;
those ADRs and ADR-0022 now cite the archived paths. Prepared-mass prose identifies its pointer
as historical. The nearest-match draft distinguishes identity correctness, nearest ranking, the
iterator API proposal and the change from total NaN ordering to strict comparison.

MEASURED source deltas against SHA-256-verified cached published archives: Avian 729 (651 in
upstream files + 78 fork-only), Bevy PBR 357, netcode 159, Lightyear Avian 0; total 1245.
Section counts and the draft index were rechecked. No timing measurement or external action.

Review-correction gates: `cargo fmt --all --check` and locked workspace/all-target CI clippy with
`dev_ui,bitprobe` passed. Targeted `scripts/serial-cargo test --lib --features dev_ui,bitprobe`
results (MEASURED): narrow phase 11, body-size metrics 4, child poses 15, island sleep 21,
warm start 3, net::protocol 26, net::physics 2 — **82 passed, 0 failed**. All 16 draft patches
apply to the cached pristine releases with `patch --dry-run`; checked Markdown links resolve.
The earlier full-library UI fixture mismatch remains outside this correction; no full suite or
timing run was repeated.
