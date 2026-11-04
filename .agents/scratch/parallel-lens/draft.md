Draft for owner approval. Nothing is adopted. Read-only inspection at HEAD `fb193e02`; no edits, builds, tests or git state changes.

## Proposed Working-discipline paragraph

- **Server work is parallel by ownership, ordered at its effects.** On the hosted server, each tank, building or independent group computes from its own state and immutable inputs for the same update; cross-owner effects commit in a declared order. Keep shared mutation and exclusive access at that boundary. Serial work needs a named data dependency or atomic-visibility invariant, confined to what it requires, or a measurement showing parallel overhead loses on the hosted allocation. Convenience and shared scratch are not reasons. ADR-0037 sets the determinism boundary: the server simulates; determinism is required only where a client derives state. Order those derivations without serializing unrelated server work.

## Proposed `.agents/lenses/parallel-work.md`

# Lens: parallel-work

Subject: CPU work on the authoritative server’s update path. Client execution is outside this lens.

Principle: `AGENTS.md`, “Server work is parallel by ownership, ordered at its effects.”

Review against the **DERIVED allocation target: two CPU cores**, from the owner’s hosting requirement. Include fixed schedules, their nested physics schedules, recurring frame work, and the commands and observers those paths invoke.

A serial reason names the dependency or atomic visibility it preserves and bounds the work it requires. An overhead reason cites a measurement on the hosted allocation, with its workload and population range. A reason applies to that portion of work, not automatically to its caller or subsystem.

Source notation below:

- `E/`: `/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_ecs-0.19.1/src/`.
- `A/`: `vendor/avian3d-0.7.0-child-pose/src/`.

Engine versions, source coordinates and severity labels are identifiers. Scheduling and independence conclusions below are **DERIVED from the cited source**.

## Finding classes

- **Serial independent work**: a growing collection of owners, contacts or independent groups is evaluated sequentially because its adapter walks them sequentially. Recognise a loop whose iterations read immutable shared inputs and change disjoint state, or could return separate results before shared effects apply. It leaves independent CPU work unavailable to the other allocated core.  
  **Fix:** separate independent evaluation from shared effects; expose disjoint mutable ownership or separate outputs, and batch evaluation on the compute pool. Preserve each owner’s required input order. Retain serial dispatch only under a named serial reason.  
  **Source:** `E/system/query.rs:1251` and `:1271` expose parallel query iteration; `E/query/par_iter.rs:104` selects the compute pool and partitions work. Avian follows the same pattern for body integration at `A/dynamics/integrator/mod.rs:278`.

- **Shared writer across independent evaluation**: a whole-owner loop holds a global `ResMut`, broad mutable query, shared accumulator or lock throughout its calculations, although shared mutation is needed only when collecting or applying results. Include an unnecessarily mutable parameter and mutable queries whose disjoint ownership is invisible to scheduling. Access declared in a parameter still conflicts when a runtime branch does no writing. On the allocated cores, this prevents otherwise compatible systems from overlapping or makes workers queue behind the same writer.  
  **Fix:** narrow access to its actual owner; use immutable inputs during evaluation and separate outputs for the bounded shared commit. Keep genuinely coupled state together. A lock around every result preserves the bottleneck.  
  **Source:** `E/system/system_param.rs:743` registers `ResMut` as write access; `E/schedule/executor/multi_threaded.rs:188` builds access conflicts and `:569` prevents conflicting systems from running together. Avian’s narrow phase uses local status buffers during contact evaluation at `A/collision/narrow_phase/system_param.rs:527`.

- **Unjustified ordering or flush**: `.chain()`, broad set ordering, explicit deferred application or an exclusive system orders more work than a named dependency requires. It makes independent work wait; command application also requires exclusive world access. Read the complete schedule graph before finding a missing or excessive edge.  
  **Fix:** order the actual producer, visibility boundary and consumer. Preserve required command visibility; consolidate boundaries only where the consumer can still see the correct state in the same update. Replacing `.chain()` with an ignore-deferred variant without checking visibility is not a fix.  
  **Source:** `E/schedule/config.rs:326` and `:338` give dependency edges command-visibility semantics; `:464` makes chaining a sequence and adds deferred boundaries where required. `E/schedule/executor/mod.rs:175` marks `ApplyDeferred` exclusive. `E/schedule/executor/multi_threaded.rs:543` requires running work to finish before an exclusive system starts.

- **Computation hidden in exclusive application**: an expensive calculation moves into a `Commands` closure, component hook or observer, or remains inside `&mut World`, although only its final structural changes need that context. Enqueuing work does not execute it in parallel. Observers run in the triggering execution context rather than becoming independently scheduled systems. This lengthens a serial application boundary on the hosted allocation.  
  **Fix:** calculate from explicit inputs before application; carry the prepared result into the smallest required structural transaction. Keep immediate hooks and observers limited to their named invariant, and stage other work for an explicit consumer in the same update.  
  **Source:** command triggers call into the world at `E/system/commands/command.rs:281`; observer dispatch calls runners directly at `E/observer/mod.rs:171`, and the runner invokes the observer at `E/observer/runner.rs:110`. Deferred application obtains exclusive world access at `E/schedule/executor/multi_threaded.rs:700`.

- **Effect order left to execution**: a producer and consumer rely on access contention instead of a dependency; cross-owner effects use worker completion, lock acquisition, parallel command application or unspecified iteration order as their ordering rule. Floating-point accumulation is included. This either makes update semantics depend on execution order or recovers order by serializing the evaluation itself.  
  **Fix:** name the required producer-consumer edge and define the effect order independently of worker assignment. Collect and reduce effects using keys that identify their source and sequence; retain within-owner sequential dependencies. Effects whose actual operations are order-independent may batch. Where clients derive state, the ordering inputs and arithmetic must satisfy that derivation’s contract.  
  **Source:** `E/system/query.rs:1273` gives no parallel iteration or thread-assignment order; `E/system/commands/parallel_scope.rs:26` warns that noncommuting commands can produce different results, and `:64` applies thread queues sequentially. Access exclusion at `E/schedule/executor/multi_threaded.rs:569` prevents overlap; it does not declare which conflicting system must run first.

- **Parallelism assumed or dispatch unpriced**: a change assumes that enabling engine multithreading parallelizes a game loop, disables an existing parallel path without a reason, or adds per-item tasks, a private CPU pool or waits that consume the allocation without shortening the update’s critical path. Extra workers do not create extra hosted CPU capacity.  
  **Fix:** retain the enabled engine paths, use scoped work on the existing compute pool for update-bound calculations, and batch at a granularity supported by measurement. Price the whole server under the actual allocation, including main, compute, async and IO work. Preserve same-update completion unless the owner separately changes gameplay timing.  
  **Source:** `E/query/par_iter.rs:102` selects parallel execution from feature and pool availability. Avian’s helper uses `ComputeTaskPool` and retains a serial path for small work at `A/utils.rs:59`. Its default `parallel` feature enables Bevy multithreading in `vendor/avian3d-0.7.0-child-pose/Cargo.toml:57` and `:82`; the root `Cargo.toml:29` retains those defaults. Server pool sizing is owned by `src/net/server_host.rs:56`.

## Not a finding

- A required read-after-write dependency, sequential state evolution, or structural transaction whose invariant names why it must be atomic.
- An ordered cross-owner commit after independent evaluation. The commit does not justify serializing the evaluation.
- Immutable shared resources, disjoint mutation, or independent commands whose application order cannot change their result.
- A bounded local loop within an owner’s calculation. A loop hit alone is not evidence of lost parallel work.
- Serial dispatch supported by a measurement showing parallel overhead loses over the relevant workload range.
- Startup or loading work outside the recurring update path.
- An exclusive nested schedule runner merely because its signature takes `&mut World`. Avian’s runner at `A/schedule/mod.rs:235` invokes the physics schedule at `:263`; its internals include parallel contact-pair work at `A/collision/narrow_phase/system_param.rs:550` and parallel constraints within each graph colour at `A/dynamics/solver/plugin.rs:557`. Dependencies between colours remain required.
- A phase chain with a real cross-feature dependency. The production chain at `src/ballistics.rs:1948` orders `WallLawSet` before `TreeFallSet`; their local registrations need not repeat that edge.
- Server physics differing across executions or platforms merely because it runs concurrently. Apply ADR-0037’s determinism requirement only where a client derives state; identify the line creating that dependency.

## Severity

- **P1:** execution order changes a reachable current game outcome, or measurements on the hosted allocation show the breach causing current update overruns or starvation. Name the reachable input or measured workload.
- **P2:** scalable independent work is structurally serialized, or a new access conflict, boundary or dispatch choice lacks an accepted reason. A current overload measurement is not required to flag building further on that assumption.
- **P3:** the reviewer establishes that the serial work is justified and correctly bounded, but its reason is unstated.

Each finding names the server registration, owning state, dependency or conflicting access, and the class’s fix. Label cost evidence **MEASURED** and structural conclusions **DERIVED**. If cost is unmeasured, say so; do not invent a speedup.

# Sweep preview at HEAD

These are **DERIVED architectural findings**, not new performance measurements. The production path mounts `SimPlugin` at [server.rs:110](/Users/Yan/github/vikng-dev/overmatch/src/net/server.rs:110); driving runs in `FixedUpdate`, construction work in `FixedPostUpdate`, and contact readings inside the physics schedules.

| Site | Class and source-derived finding | Cost evidence |
|---|---|---|
| [track/sim.rs:895](/Users/Yan/github/vikng-dev/overmatch/src/track/sim.rs:895), `apply_track_forces` | **Serial independent work.** Tanks already read a common tick-start support snapshot, but their belt calculations run sequentially alongside shared contact staging. | **MEASURED historical:** drive span **3,601 μs**, fixed busy time **6,819 μs**; **DERIVED approximately 53%**. This is the earlier trace, not HEAD pricing. [Trace report:20](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-11-01/server-tick.md:20). |
| [buildings/breakage.rs:1329](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:1329), `apply_deposits` | **Serial independent work.** Within a deposit, owner-local law evaluation runs through the ordered owner map sequentially; record writes and departures share that adapter. Deposit sequencing itself remains a dependency. | **MEASURED historical:** the entire adapter accounts for **34.77% of inclusive active CPU samples** in the hunt’s post-fix ram window. That is neither this loop’s isolated cost nor an update wall-time share. [Hunt:16](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/slowdown-hunt/report.md:16). |
| [trees.rs:984](/Users/Yan/github/vikng-dev/overmatch/src/trees.rs:984), `fell_on_loads` | **Serial independent work.** Loads have already been grouped by root, but each root’s construction assessment runs sequentially. | **DERIVED:** independent root assessments share this serial pass. Update share unmeasured. |
| [contact_transfer.rs:447](/Users/Yan/github/vikng-dev/overmatch/src/contact_transfer.rs:447), `keep` | **Shared writer across independent evaluation.** Contact interpretation builds global point, inertia and indexing collections directly while walking the constraint graph. Independent contact calculations are tied to collection mutation. | **DERIVED:** work grows with retained constraints and points. Update share unmeasured. |
| [contact_transfer.rs:592](/Users/Yan/github/vikng-dev/overmatch/src/contact_transfer.rs:592), `add_substep`; [restitution counterpart:614](/Users/Yan/github/vikng-dev/overmatch/src/contact_transfer.rs:614) | **Serial independent work.** Each retained constraint updates its separately assigned output slots, but the arithmetic runs sequentially across the retained collection. | **DERIVED:** the substep pass repeats inside `SubstepSchedule`; the restitution pass follows its solver stage. Their individual update shares are unmeasured. |
| [debris/launch.rs:162](/Users/Yan/github/vikng-dev/overmatch/src/debris/launch.rs:162), `cap_exchanges` | **Serial independent work.** The function explicitly separates connected exchange components, then evaluates disjoint components sequentially. Coupling within a component remains necessary. | **DERIVED:** separate components have disjoint departure membership and source ownership. Update share unmeasured. |
| [tank/servo.rs:377](/Users/Yan/github/vikng-dev/overmatch/src/tank/servo.rs:377), `drive_servos` | **Serial independent work.** The adapter walks all servo nodes sequentially while reaching each tank’s mutable state through its root. Ownership is per tank, rather than per arbitrary servo iteration. | **DERIVED:** independent tanks share this serial traversal. Update share unmeasured. Server registration: [tank.rs:71](/Users/Yan/github/vikng-dev/overmatch/src/tank.rs:71). |

The drive measurement predates later optimizations recorded in the same report. The hunt’s CPU-sample shares use a different denominator and cannot be added to the drive’s fixed-busy share. Neither establishes performance under the **DERIVED target allocation of two cores**; [fidelity-levers.md:197](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/slowdown-hunt/fidelity-levers.md:197) explicitly leaves that allocation measurement outstanding.

Where prose and code disagree, **the code wins**. The historical description that “everything” serializes is not a present engine invariant: the vendored Avian source contains parallel execution. Conversely, enabling Avian’s parallel feature does not parallelize the game’s serial owner loops.

## Ratchet note

After owner approval, this lens joins independent diff review at batch boundaries and before release, plus area sweeps. Edited items and their tests must leave clean under it, as under the existing lenses.

`backlog.md` keeps its shrinking-only contract. Put the full sweep’s evidence in `.agents/scratch/parallel-work-sweep/report.md`; accepted open findings become native GitHub sub-issues under the server-at-scale parent. The report owns evidence; those issues own status. Lanes verify and close findings naming their edited files, alongside existing backlog rows.

The README’s sweep instruction must therefore distinguish the frozen backlog from new findings tracked through sweep reports and issues. Merely appending findings to `backlog.md` would contradict its current rule.

Proposed README table line:

```markdown
| [`parallel-work`](parallel-work.md) | Server work is parallel by ownership, ordered at its effects |
```

## Decisions for the owner

Ownership is the architectural rule; measured dispatch overhead can justify serial execution without preserving serial coupling.
Required same-update ordering stays intact; moving authoritative calculations into later async updates requires a separate timing decision.
The construction queue’s production ordering is already present in the shared phase chain, so it is excluded from the findings.