# Lens: parallel-work

Subject: CPU work on the authoritative server's update path. Client execution is outside this lens.

Principle: `AGENTS.md`, "Server work is parallel by ownership, ordered at its effects". Review
against the hosted allocation, two CPU cores (the owner's hosting requirement). Include the fixed
schedules, their nested physics schedules, recurring frame work, and the commands and observers
those paths invoke.

A serial reason names the dependency or atomic visibility it preserves and bounds the work it
requires. An overhead reason cites a measurement on the hosted allocation, with its workload and
population range. A reason covers that portion of work, not its caller or subsystem.

Sources: `E/` is `bevy_ecs-0.19.1/src/` (cargo registry); `A/` is
`vendor/avian3d-0.7.0-child-pose/src/`. Both are pinned; recheck the citations on an engine bump.

## Finding classes

- **Serial independent work**: a growing collection of owners, contacts or independent groups is
  evaluated one after another because its adapter walks them that way. Recognise a loop whose
  iterations read immutable shared inputs and change disjoint state, or could return separate
  results before shared effects apply. It leaves the other core idle.
  **Fix:** separate independent evaluation from shared effects; expose disjoint mutable ownership
  or separate outputs, and batch evaluation on the compute pool. Keep each owner's required input
  order. Serial dispatch stays only under a named serial reason.
  **Source:** `E/system/query.rs:1251` and `:1271` expose parallel query iteration;
  `E/query/par_iter.rs:104` selects the compute pool and partitions work. Avian integrates bodies
  the same way at `A/dynamics/integrator/mod.rs:278`.

- **Shared writer across independent evaluation**: a whole-owner loop holds a global `ResMut`, a
  broad mutable query, a shared accumulator or a lock throughout its calculation, although shared
  mutation is needed only when collecting or applying results. Includes a needlessly mutable
  parameter, and mutable queries whose disjoint ownership the scheduler cannot see. Declared access
  conflicts even when a runtime branch writes nothing. Compatible systems can't overlap, or workers
  queue behind one writer.
  **Fix:** narrow access to its owner; immutable inputs during evaluation, separate outputs for the
  bounded shared commit. Keep genuinely coupled state together. A lock around every result keeps
  the bottleneck.
  **Source:** `E/system/system_param.rs:743` registers `ResMut` as write access;
  `E/schedule/executor/multi_threaded.rs:188` builds access conflicts and `:569` keeps conflicting
  systems apart. Avian's narrow phase evaluates contacts into local status buffers at
  `A/collision/narrow_phase/system_param.rs:527`.

- **Unjustified ordering or flush**: `.chain()`, broad set ordering, an explicit deferred
  application or an exclusive system orders more work than a named dependency requires. Independent
  work waits, and command application takes exclusive world access. Read the whole schedule graph
  before calling an edge missing or excessive.
  **Fix:** order the actual producer, visibility boundary and consumer. Keep required command
  visibility; merge boundaries only where the consumer still sees the right state in the same
  update. Swapping `.chain()` for an ignore-deferred variant without checking visibility is not a
  fix.
  **Source:** `E/schedule/config.rs:326` and `:338` give dependency edges command-visibility
  semantics; `:464` makes chaining a sequence with deferred boundaries where required.
  `E/schedule/executor/mod.rs:175` marks `ApplyDeferred` exclusive;
  `E/schedule/executor/multi_threaded.rs:543` drains running work before an exclusive system starts.

- **Computation hidden in exclusive application**: an expensive calculation moves into a `Commands`
  closure, component hook or observer, or stays inside `&mut World`, although only its final
  structural change needs that context. Enqueued work does not run in parallel, and an observer runs
  in its trigger's execution context, not as a scheduled system. The serial boundary grows.
  **Fix:** calculate from explicit inputs beforehand; carry the prepared result into the smallest
  structural transaction. Hooks and observers keep only their named invariant; other work is staged
  for an explicit consumer in the same update.
  **Source:** command triggers call into the world at `E/system/commands/command.rs:281`; observer
  dispatch calls runners directly at `E/observer/mod.rs:171`, and the runner invokes the observer at
  `E/observer/runner.rs:110`. Deferred application takes exclusive world access at
  `E/schedule/executor/multi_threaded.rs:700`.

- **Effect order left to execution**: a producer and consumer rely on access contention instead of
  a dependency, or cross-owner effects take their order from worker completion, lock acquisition,
  parallel command application or unspecified iteration order. Floating-point accumulation counts.
  Update semantics then depend on execution order, or order is recovered by serializing the
  evaluation itself.
  **Fix:** name the producer-consumer edge and define effect order independently of worker
  assignment: collect and reduce effects keyed by source and sequence; keep within-owner sequential
  dependencies. Effects whose operations commute may batch. Where a client derives state, the
  ordering inputs and arithmetic meet that derivation's contract.
  **Source:** `E/system/query.rs:1273` gives parallel iteration no order;
  `E/system/commands/parallel_scope.rs:26` warns that non-commuting commands differ, and `:64`
  applies thread queues one after another. Access exclusion at
  `E/schedule/executor/multi_threaded.rs:569` prevents overlap but chooses no order.

- **Parallelism assumed or dispatch unpriced**: a change assumes engine multithreading parallelizes
  a game loop, disables an existing parallel path without a reason, or adds per-item tasks, a
  private CPU pool or waits that spend the allocation without shortening the update's critical
  path. More workers don't make more hosted CPU.
  **Fix:** keep the engine's enabled paths, use scoped work on the existing compute pool for
  update-bound calculation, and batch at a granularity a measurement supports. Price the whole
  server on the real allocation, main, compute, async and IO work included. Keep same-update
  completion unless the owner changes gameplay timing.
  **Source:** `E/query/par_iter.rs:102` chooses parallel execution from feature and pool. Avian's
  helper uses `ComputeTaskPool` with a serial path for small work at `A/utils.rs:59`; its default
  `parallel` feature turns on Bevy multithreading (Avian's `Cargo.toml`), and the root `Cargo.toml`
  keeps those defaults. `src/net/server_host.rs` sizes the server's task pools.

## Not a finding

- A required read-after-write dependency, sequential state evolution, or a structural transaction
  whose invariant names why it must be atomic.
- An ordered cross-owner commit after independent evaluation. The commit doesn't justify
  serializing the evaluation.
- Immutable shared resources, disjoint mutation, or independent commands whose application order
  can't change the result.
- A bounded local loop inside one owner's calculation. A loop hit alone is no evidence of lost
  parallel work.
- Serial dispatch backed by a measurement showing parallel overhead loses over the relevant
  workload range.
- Startup or loading work off the recurring update path.
- An exclusive nested schedule runner merely because it takes `&mut World`: Avian's runner
  (`A/schedule/mod.rs:235`, invoking the physics schedule at `:263`) runs parallel contact-pair work
  (`A/collision/narrow_phase/system_param.rs:550`) and parallel constraints within each graph colour
  (`A/dynamics/solver/plugin.rs:557`). Dependencies between colours remain required.
- A phase chain with a real cross-feature dependency, such as the shared phase chain in
  `src/ballistics.rs`; local registrations need not repeat its edges.
- Server physics differing between runs or platforms merely because it runs concurrently. ADR-0037
  requires determinism only where a client derives state; a finding names the line that creates
  that dependency.

## Severity

- **P1**: execution order changes a reachable game outcome now, or measurements on the hosted
  allocation show the breach causing update overruns or starvation now. Name the input or the
  measured workload.
- **P2**: scalable independent work is structurally serialized, or a new access conflict, boundary
  or dispatch choice lacks an accepted reason. No current overload measurement is needed to stop
  building further on the serial assumption.
- **P3**: the serial work is justified and bounded, but its reason is unstated.

Each finding names the server registration, the owning state, the dependency or conflicting access,
and the class's fix. Cost evidence is MEASURED, structural conclusions DERIVED; unmeasured cost is
said to be unmeasured, never estimated as a speedup.
