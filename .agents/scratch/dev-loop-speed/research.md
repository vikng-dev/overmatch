No, it does not have to feel this slow. The biggest lever is to keep the game running while presentation data changes, and preserve useful compiler caches between agent tasks. Ordinary Rust edits still need compilation, but our longer waits also contain queueing, repeated validation and rebuilding discarded work. Bevy offers asset reload and experimental code hot-patching today. I would build that faster preview loop before changing compilers or splitting the whole crate.

This report reflects sources checked as of November 4, 2026. I made no edits, builds, test runs or game launches. **MEASURED** identifies recorded observations; **DERIVED** identifies calculations or explicitly conditional estimates. Software versions and dates are identifiers.

**The older audit and today’s queue tell different parts of the story.** Historically, compiling dominated. Today, waiting for the shared build slot is also substantial.

The controlled experiments used the same M-series machine and pinned Rust compiler, but an earlier revision of Overmatch. They establish what was achievable, rather than a fresh benchmark of the current larger crate. These results are **MEASURED** in [measurements.md](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/build-throughput-2026-10-24/measurements.md).

| Work | MEASURED local result | What it means |
|---|---:|---|
| Warm, small Rust body edit, original development settings | 49.5 s median | An ordinary edit already took less than a minute without a queue. |
| Same edit with local LTO disabled | 18.2 s median | Compiler configuration made a large difference. |
| Incremental compiler frontend | 1.7–4.3 s | Parsing and type checking were a relatively small part of this rebuild. |
| LLVM thin-local LTO | 33–39 s | This was the dominant avoidable compiler pass. |
| Actual linker invocation | 0.75–1.16 s | A different linker has little time to recover here. |
| Warm CI-style Clippy after an edit | 5.5 s | A separate validation pass, even with matching features. |
| Formatting check | 0.8 s | Negligible beside compilation and queueing. |
| Representative targeted test execution with LTO disabled | 3.1–5.5 s | The test body was much cheaper than producing its executable. |
| Library test executable | 295.6 MB | Filtering tests still builds a large executable. |
| First launch of a newly linked test executable | Additional 2.5–4.1 s | There is launch overhead beyond linking and reported test execution. |

Disabling local LTO saved **DERIVED 31.3 s, approximately 63%**, from that body-edit rebuild: `49.5 − 18.2`. This is why the existing `edit` profile matters.

A subsequent fixed-path bay experiment measured a small body edit at **MEASURED 22.8 s** with the required feature set and the `edit` profile. That is useful corroboration of the faster loop. [Bay measurements](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/build-throughput-2026-10-24/bays.md:18)

The original agent audit recorded **MEASURED 205 agent-minutes inside Cargo and 13 agent-minutes waiting outside it**. Together, that was **DERIVED approximately 41% of audited agent time**. These are accumulated minutes across agents, not elapsed time during which the owner necessarily sat waiting. [Audit summary](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/build-throughput-2026-10-24/audit-summary.md)

The more detailed reconstruction attributed:

- **DERIVED 113.6 call-minutes** to ordinary edit compilation and mutation recompilation.
- **DERIVED 40.6 call-minutes** to cold starts and dependency/build-variant rebuilds.
- **DERIVED 15.7 call-minutes** to builds using the expensive shipping profile.
- **DERIVED 15.1 call-minutes** to Clippy.
- **DERIVED 6.7 call-minutes** to executing tests.

Those buckets use slightly different call boundaries from the headline audit and should not be combined with its total. They nevertheless show that **the historical problem was predominantly producing executables, rather than executing tests**. [Detailed decomposition](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/build-throughput-2026-10-24/measurements.md:242)

I also read the existing [Cargo queue log](/Users/Yan/.cache/overmatch/cargo-queue.tsv). The snapshot ending at November 4, 07:49:35 UTC contained **DERIVED 1,647 valid completed records** beginning November 2. The following statistics are **DERIVED from those MEASURED timestamps**; all durations are seconds.

| Successful commands | Count | Median pre-start wait | Median Cargo duration | Median total duration | Total duration, 90th percentile |
|---|---:|---:|---:|---:|---:|
| Bay library tests using `edit` | 530 | 25 | 42 | 117 | 405 |
| Clippy | 384 | 55.5 | 12 | 74.5 | 356 |

The total medians are calculated directly; medians of the separate components do not add.

Across all valid records, pre-start time totalled **DERIVED 2,483.1 agent-minutes**, versus **DERIVED 1,989.7 minutes running Cargo**. Pre-start time was therefore **DERIVED 55.5% of recorded command turnaround**.

There are important limits to that observation:

- These commands mix actual edits, unchanged reruns, experiments and different test filters. They do not measure a one-line rebuild.
- The wrapper records “started” after target warming and waiting for foreign compiler processes. Its pre-start interval includes those activities as well as waiting for the lock.
- Cargo duration includes test execution and, for `run`, application execution. It is not pure compiler time.
- Only completed commands appear.

**The owner’s many-minute experience is real.** Historically, repeated compilation, cold variants and shipping-profile experiments accumulated into long waits. In the current log, queueing adds substantial delay to otherwise shorter commands.

The process around those commands adds further time, but the evidence does not isolate it cleanly. The earlier audit measured **MEASURED 226 agent-minutes thinking or choosing calls, 39 emitting edits, and 31 reading, using Git or doing other work**. Those categories include useful implementation work. Reviews, bay availability and orchestrator merge delays were not separately timed, so I cannot honestly assign them a percentage. Its CI runs took **MEASURED 5–16 minutes per push**, but were asynchronous during that audit.

**Several current configuration details change the recommendations.**

The [manifest](/Users/Yan/github/vikng-dev/overmatch/Cargo.toml:191) and scripts establish the current behavior:

- Targeted tests already use `edit`, which sets `lto = "off"`. Recommending that change again would count an existing improvement twice.
- Ordinary play builds still use `dev`, which leaves local LTO enabled through Cargo’s default.
- The `perf` profile uses `lto = false`. Despite its “no LTO” comment, that permits **thin-local LTO**. It disables cross-crate LTO; it does not mean `"off"`. [Cargo profile definitions](https://doc.rust-lang.org/cargo/reference/profiles.html#lto)
- Feature harmonization changes described in the earlier research have landed.
- Matching features does not make Clippy, the library test harness and a game executable the same compilation unit. Different profiles also keep different artifacts.
- Bay release now explicitly removes incremental state. [Release implementation](/Users/Yan/github/vikng-dev/overmatch/scripts/bays/release:64)

That last point is consequential. The earlier bay proposal retained incremental caches across lanes. Today, bays retain dependency artifacts but discard incremental caches on release. The recorded reason is a real disk problem: **MEASURED, as documented in the pool implementation, incremental state occupied 33 GB of a bay’s 57 GB target directory**. [Pool documentation](/Users/Yan/github/vikng-dev/overmatch/scripts/bays/lib.sh:14)

Deleting incremental state does not invalidate an otherwise fresh executable. Its cost arrives when a subsequent source edit needs that missing state.

The earlier research also overstates two findings. Its frontend estimate was superseded by the controlled phase measurements above. Its assertion that security scanning explains every fresh-executable delay remains a hypothesis: the launch penalty was measured, but the cause and benefit of changing macOS Developer Tools settings were not established.

**The available tools address different parts of this problem.**

| Option | What it saves | Fit for macOS arm64, Bevy and Avian | Cost and maturity |
|---|---|---|---|
| Bevy `hotpatching` with Dioxus Subsecond | Replaces running system code while preserving the application and its world. Patches still compile Rust. | Bevy’s versioned example supports systems and observers; Subsecond supports macOS arm64. The exact Overmatch workspace and patched dependency stack need a pilot. | Experimental. Initial builds, state-layout limits and queue integration remain. [Bevy example](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/examples/ecs/hotpatching_systems.rs) |
| `bevy/dynamic_linking` | Avoids repeatedly statically linking the engine and may affect downstream code generation. | Supported development technique on this platform. Avian and game-specific generic code do not automatically become a replaceable game module. | Established. Requires another artifact configuration and runtime library; unsuitable as the shipping default. [Bevy setup](https://bevy.org/learn/quick-start/getting-started/setup/#dynamic-linking) |
| Cranelift | Reduces backend optimization work. | macOS arm64 is supported by the backend, but that does not establish compatibility with this complete application or test harness. | Nightly preview; generated code has different performance and there are important unwind limitations. |
| Parallel rustc frontend | Parallelizes some compiler frontend work. | Architecture-compatible, but unavailable as a normal option on the pinned stable toolchain. | Nightly experimentation. Additional memory and limited opportunity in the historically short frontend phase. [Rust stabilization plan](https://github.com/rust-lang/compiler-team/issues/1005) |
| Alternative macOS linker | Shortens final linking. | Apple’s modern default linker already targets fast static linking; LLVM’s Mach-O LLD is another supported implementation. | Mature tools, small local opportunity. Changing flags also invalidates artifacts. [Apple release notes](https://developer.apple.com/documentation/xcode-release-notes/xcode-15-release-notes/), [LLD documentation](https://lld.llvm.org/MachO/index.html) |
| `cargo check` and rust-analyzer | Gives type and diagnostic feedback without producing the game executable. | Works with this stack and the pinned stable compiler. | Established. Does not execute code or report every code-generation error. Background checks must respect the queue. [Cargo documentation](https://doc.rust-lang.org/cargo/commands/cargo-check.html) |
| sccache and shared caches | Reuses exact compiled artifacts, particularly dependencies on cold machines or checkouts. | Applicable to cacheable dependencies on macOS. | sccache is mature, but refuses incremental compilations and crates invoking the linker. It does not solve editing this incremental game crate. [sccache limitations](https://github.com/mozilla/sccache#rust) |
| Further workspace splitting | Gives independent modules smaller compilation and test units. | Fully compatible with the engine versions and stable Rust. | Architectural work. Gains depend on dependency direction and where edits occur. |
| Asset and typed-data reload | Avoids Rust compilation altogether for eligible edits. | Bevy supports desktop file watching and glTF asset reload. Custom RON data needs a loader and application-side update behavior. | Established asset machinery; Overmatch-specific refresh work is required. [Versioned asset example](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/examples/asset/hot_asset_reloading.rs) |
| Scripting | Moves eligible behavior changes outside Rust compilation. | Bevy Scripting has a compatible release family; exact patched-engine and native-runtime compatibility need verification. | Community tooling with continuing API changes, plus bindings and runtime costs. [Author’s compatibility table](https://raw.githubusercontent.com/makspll/bevy_mod_scripting/main/readme.md) |

A few distinctions deserve explanation.

**Hot-patching is the closest available equivalent to web HMR.** Bevy’s integration automatically makes ordinary function systems and observers patchable. Other execution paths can require explicit patch boundaries. Code that runs only during startup still needs to run again before its changed behavior becomes visible. Changing an initializer does not reset an existing resource or static.

Subsecond does not provide general migration of existing component layouts. System signatures, component/resource layout changes and initialization changes should trigger a restart in the proposed workflow. Its documentation also describes limitations around globals and thread-local state. [Subsecond documentation](https://docs.rs/subsecond/latest/subsecond/)

There is an upstream documentation trap here. Subsecond’s rustdoc still describes a top-crate-only limitation, while Dioxus subsequently merged workspace hot-patching. Overmatch’s thin executable calling a library should therefore be tested directly, rather than rejected or restructured based on that paragraph. Later workspace regressions and fixes make the CLI version significant. [Workspace implementation](https://github.com/DioxusLabs/dioxus/pull/5291), [subsequent filtering fix](https://github.com/DioxusLabs/dioxus/pull/5903)

The CLI itself still calls hot-patching experimental and warns of development crashes. This is a candidate preview tool, not evidence that every arbitrary Rust edit can safely enter a running session. [CLI source](https://raw.githubusercontent.com/DioxusLabs/dioxus/v0.7.10/packages/cli/src/cli/serve.rs)

**Cranelift is worth watching rather than adopting as the default here.** Its current README supports macOS arm64 but still documents unavailable default panic unwinding on macOS. Experimental unwind work exists; a maintainer’s arm64 assessment was explicitly untested. That makes the earlier blanket rejection too absolute, but does not establish a usable replacement for our test harness. Bevy’s own setup documentation also warns about macOS application crashes. [Backend README](https://github.com/rust-lang/rustc_codegen_cranelift), [unwind tracking](https://github.com/rust-lang/rustc_codegen_cranelift/issues/1567), [Bevy guidance](https://bevy.org/learn/quick-start/getting-started/setup/#cranelift)

Keeping optimized dependencies on LLVM while experimenting with Cranelift for application code is possible. It still creates a separate compiler/artifact configuration and needs both runtime and compile-time measurements.

**Dynamic linking and linker replacement should receive modest expectations.** The historical linker-only opportunity is **DERIVED at most approximately 1.2 seconds per rebuild**, even if linking became free. Dynamic linking can change more than the linker invocation, so that is not a strict bound on its total benefit. It nevertheless leaves the main game crate’s work and the queue in place.

**Quick diagnostics are valuable when used deliberately.** rust-analyzer provides ongoing editor feedback, while its save-triggered Cargo checks are actual compiler work. It supports overriding both diagnostic and build-script commands. Those paths should use the same feature set and scheduling policy. Running a check before every successful test build would add work; running one to catch an uncertain type error before an expensive executable build can save time. [rust-analyzer configuration](https://rust-analyzer.github.io/book/configuration)

**Shared caches do not replace a warm incremental cache.** sccache can help dependencies without requiring incremental compilation to be disabled everywhere. Disabling incrementality on the edited game crate merely to make it cacheable would sacrifice partial reuse whenever its contents change. A common `CARGO_TARGET_DIR` also does not remove rustc’s path-sensitive cache keys. Cargo’s native cross-workspace cache remained under design in its latest available progress report. [Cargo cache progress](https://github.com/rust-lang/goals/issues/626)

**A crate split needs a measured dependency boundary.** It can help beyond frontend time: smaller independent code-generation units, smaller test executables and fewer unrelated modules rebuilt. But a frequently edited shared-types crate can invalidate everything above it, and generic code can still be instantiated downstream.

The earlier dependency analysis found broad cycles; that analysis predates the current tree. I would pilot an independently owned leaf module and compare its ordinary edits before considering a broad decomposition. The existing tools package depends on the game library, so its existence alone does not isolate shared schemas from the engine.

**The most promising live-reload material already exists in this repository.**

Appearance is already authored in RON for world lighting, UI settings, the sight, graticule and overlay. However, several of those files are embedded into Rust and parsed through `LazyLock`. Saving them therefore still recompiles the crate, and an already initialized value cannot simply reread itself. See [lighting loading](/Users/Yan/github/vikng-dev/overmatch/src/world/look.rs:5), [settings loading](/Users/Yan/github/vikng-dev/overmatch/src/settings/look.rs:11) and [sight loading](/Users/Yan/github/vikng-dev/overmatch/src/sight/look.rs:10).

The currently built Bevy feature fingerprints contain neither file watching nor hot-patching. The [asset plugin setup](/Users/Yan/github/vikng-dev/overmatch/src/lib.rs:278) does not override that.

A live data path would parse and validate changed files into typed resources, then update the consumers. Updating a resource alone does not redraw UI properties copied into entities during construction; those consumers need an explicit refresh.

Textures, shader files, sounds and glTF presentation assets offer further opportunities. Bevy supplies file watching, but application-owned derived caches still need invalidation. Overmatch has custom material bindings and geometry tables, and some view-reinstantiation handling already exists. Texture descriptor changes and glTF topology changes deserve particular attention. [Surface implementation](/Users/Yan/github/vikng-dev/overmatch/src/tank_surface.rs:35), [view rebinding](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:350)

Driving and vehicle parameters are also data, but their application boundary is different. Under the repository’s spawn rules, changed simulation parameters should be validated and supplied to a fresh scenario or spawn. They should not asynchronously reconstruct simulation components on existing replicated entities. This can still avoid a Rust rebuild.

Scripting could later serve UI behavior or scenario orchestration. It introduces another language, bindings and failure handling, so I would begin with the existing typed RON data. A scripting integration must preserve server ownership and the current state-construction rules.

**A practical web-style loop would have several response paths.**

| Change | Proposed response |
|---|---|
| Lighting, UI appearance or another reloadable presentation value | Save → validate → update the running view. **DERIVED target: approximately 1–2 s** for small files, assuming inexpensive refresh work. |
| Texture, sound or shader content | Save → asset reload → renderer/audio update. GPU compilation or asset conversion can add latency. |
| Supported system-body edit | Save → compile patch → apply it to the running application. Keep the current world and camera position. |
| Embedded RON under today’s implementation | Rebuild until that data owner gains a runtime loading path. |
| Component layouts, system signatures, startup wiring, dependencies or features | Rebuild and restart under the proposed conservative policy. |
| Simulation construction data | Validate → reset scenario or respawn from the new data. |

For an ordinary LLVM rebuild, the best supported nearby baseline is **MEASURED approximately 18–23 seconds** for a small body edit with warm incremental state and local LTO disabled. Thus **DERIVED approximately 20 seconds is a demonstrated practical target**, conditional on those circumstances. It is not a measured floor for today’s enlarged crate or its full game executable.

The current mixed bay-test median of **DERIVED 42 seconds running Cargo** suggests budgeting tens of seconds for ordinary compiled feedback. There is no controlled evidence here supporting instantaneous arbitrary Rust edits on the current crate.

The scheduling rules also need an explicit adaptation. [`serial-cargo`](/Users/Yan/github/vikng-dev/overmatch/scripts/serial-cargo:38) holds its lock for the entire Cargo command. A persistent `serial-cargo run` therefore holds the slot while the game runs. A preview should build under the lock, then launch the resulting executable outside it.

Likewise, wrapping the lifetime of `dx serve` in the global lock would monopolize the machine. Its compilation and patch requests need integration with the queue. Keep the machine’s compiler concurrency limit; adapt the wrapper to the tool’s build boundaries. The earlier concurrency experiment supported that limit on the measured memory-constrained machine.

The preview configuration should retain `dev_ui,bitprobe`. Additional watcher or hot-patching features should belong to a named, tracked preview launcher with a current user. That is a justified extension of the common-feature policy, rather than arbitrary feature switching between calls.

**I would try these changes in this order, ranked by expected return relative to effort.**

1. **Retain a bounded useful incremental cache in each bay.**  
   **DERIVED expected gain:** approximately **55 seconds on a first small edit** that would otherwise lose all incremental reuse, using the historical compiler-phase comparison of **MEASURED 69.9 seconds from scratch versus 14.4–14.8 seconds incrementally**. Actual benefit depends on changes since the bay last built. Cost is moderate cache-lifecycle work; disk exhaustion is the main risk. Retain current useful units and evict obsolete variants only while idle. Measure the first real edit after release/release-with-retention, subsequent edits, and peak disk usage. An unchanged Cargo invocation is not a sufficient test.

2. **Give presentation RON and assets a live preview path.**  
   **DERIVED expected gain:** remove the entire Rust compilation and queue interval from eligible tuning edits; target **1–2 seconds** from save to visible result under the small-file assumption above. Cost is moderate work on loading, validation and refreshing consumers. Risks are stale derived state and incomplete material or view invalidation. Start with lighting and UI appearance, then measure save-to-visible latency and verify malformed input produces a visible deliberate failure.

3. **Try the existing `edit` profile for ordinary play previews.**  
   **DERIVED expected gain:** the historical body-edit comparison suggests approximately **31 seconds saved per comparable rebuild**, assuming the game executable behaves similarly to the measured test artifact. That assumption needs checking. Cost is low; runtime performance is the main risk. Compare equivalent game builds, frame times and startup behavior. The measured test slowdown cannot be treated as a measured frame-rate penalty. This experiment does not require inventing another profile.

4. **Pilot hot-patching on a presentation system in the actual library.**  
   **DERIVED trial scenario:** if a patch becomes visible within **5 seconds**, against the historical **22.8-second** body rebuild, it saves approximately **18 seconds**, plus avoided restart and navigation time. That is an acceptance scenario, not an Overmatch benchmark. Cost is moderate integration work; risks include workspace replay bugs and retained state incompatible with changed code. Pin the CLI, prove edits in `lib.rs` reach the running game, exercise repeated patches, and establish clear restart triggers before expanding its use.

5. **Make interactive feedback and commit validation distinct scheduled work.**  
   Keep required gates at the commit boundary and avoid repeating them for intermediate states without a reason. Use quick diagnostic feedback when it can prevent an expensive failed executable build.  
   **DERIVED expected gain:** each genuinely unnecessary warm Clippy request avoided would recover approximately **12 seconds of slot service**, using the current successful-call median, plus an unquantified queue benefit. If every request is necessary, this saving is absent. Interactive priority can improve the owner’s experience while delaying background work; it does not create throughput. Cost is low to moderate workflow work. Measure requests per completed change, queue tails, and whether gates detect problems later.

6. **Benchmark additional build capacity if the queue remains saturated.**  
   **DERIVED opportunity bound:** the current recorded pre-start share is **55.5%**; only the avoidable queue portion of that is recoverable, and remote execution adds its own cost. Cost includes paid capacity, source synchronization and maintaining warm caches. Linux can serve headless validation; native Metal previews remain a macOS workload. Preserve lane ownership and explicitly define remote-worker rules rather than creating overflow bays. Compare complete edit-to-result latency and concurrent lane completion before purchasing a permanent setup.

For those trials, record queue entry, lock acquisition, compiler start, compiler finish, first executable launch and visible result separately. Keep the toolchain, features and workload fixed, and use fresh equivalent edits so a previously cached toggle cannot masquerade as a faster build. Judge the interactive loop by save-to-visible latency, and the agent workflow by completed changes and queue delay.