# Overmatch architecture and debt map

This file is the root structural authority for the repository. It records the accepted target,
the dependency direction, and the debt-repayment sequence. Product truth lives in
[`.agents/PRODUCT.md`](.agents/PRODUCT.md), domain terms in
[`.agents/GLOSSARY.md`](.agents/GLOSSARY.md), settled decisions in
[`.agents/docs/adr/`](.agents/docs/adr/), and provisional feel decisions in
[`.agents/scratch/playtest-forks/`](.agents/scratch/playtest-forks/). Those sources provide detail;
this file is the map a contributor should be able to start from.

Code is the final witness of current behaviour. A comment, design document, or ADR that disagrees
with executable code is debt to correct, not evidence that the code behaves as described. Every
quantitative claim added here must be labelled **MEASURED** or **DERIVED**.

## Product and runtime topology

Overmatch is an official-server-hosted online PvP tank game. A dedicated server owns Battle truth;
the normal client submits `TankCommand` intent, predicts immediate causes, and reconciles
authoritative consequences. The client must not calculate privileged damage truth independently.
One authoritative world owns one complete Battle; distributed gameplay authority and world
sharding are not target architecture.

Player-facing worlds reuse that topology:

- An Online Battle uses a remote dedicated authority.
- A Shooting range launches the same authority runtime locally and connects the normal client to
  it. It is not a second gameplay implementation.
- Armor inspection is an analytical adapter over the same tank and ballistic rules. It may expose
  privileged diagnostic truth and may drive simulation directly.
- Replay and spectating are future adapters, not reasons to create frameworks before their real
  interfaces are known.

Garage, Progression, production AI, join-in-progress, community hosting, detailed X-ray feedback,
and building destruction remain deferred as described in the product target. Do not create empty
modules for them.

## Non-negotiable simulation rules

### Authority and prediction

- The client predicts causes; the server confirms consequences.
- Penetration, ricochet, damage, crew state, module state, knockout, Battle results, and
  Progression consequences are authoritative.
- Presentation may react immediately to local intent, but it may not invent an authoritative
  outcome.
- Damage confirmation is an authoritative fact. Its visual disclosure remains a playtest fork.

### Spawn completeness

All rollback-registered simulation state must be constructed synchronously, from versioned data,
in the entity's spawn transaction. A GLB is a view and authoring input, never a simulation
constructor. Loaded assets may delay admission or view attachment; they may not cause simulation
state to appear late on an already-replicated entity.

The desired content seam is a validated, spawn-ready `TankBlueprint`. The simulation consumes that
plain data. Runtime GLB extraction is transitional; an offline bake and content fingerprint are the
target described by ADR-0014.

### Determinism and schedules

- `SimPlugin` owns the shared fixed-step rules run by the authority and predicted client.
- Simulation consumes `TankCommand`, never devices or transport messages.
- Cross-feature ordering is owned centrally by named schedule sets and explicit edges.
- Stable indices, entity creation order where relevant, random seeds, and every other derived
  ordering input must not depend on hash-map iteration or incidental system order.
- Canonical state digests, replay rejoin tests, and cross-platform comparison are the proof of
  determinism. Documentation is not proof.

## Target module direction

The repository remains a single Cargo package during this migration. Directories establish
internal modules and private seams; they do not justify broad Rust visibility.

```text
runtime
  |-- client process --> client --> sim
  |                         |        ^
  |                         +--> net-+
  |-- server process ------------> net --> sim
  +-- analytical tools -----------------> sim

sim -------------------------------------> content schema
content loaders/bake --------------------> content schema
telemetry -------------------------------> sim / net   (observation only)
```

Rules implied by that direction:

- `sim` never imports `net`, `client`, device input, UI, rendering, or process configuration.
- `net` adapts wire facts and commands to simulation vocabulary. Simulation never adapts itself to
  Lightyear.
- `client` owns device translation and presentation. Named online-feedback modules may consume a
  deliberately exposed authoritative client fact; arbitrary presentation modules may not reach
  through the networking implementation.
- `telemetry` may observe simulation or networking state but must not write authoritative state.
- `runtime` owns environment variables, sockets, window/headless selection, plugin composition,
  and process lifetime. It contains no game rules.

## Target source contour

This is a migration target, not an instruction to create empty directories. A directory appears
when code moves behind its interface.

```text
src/
  lib.rs                         # narrow facade for executable entry points

  runtime/
    client.rs                    # client process composition
    server.rs                    # authority process composition

  sim/
    mod.rs                       # SimPlugin and simulation interface
    lifecycle.rs                 # simulation state and GameplaySet gate
    schedule.rs                  # shared ordering contract
    command.rs                   # net-neutral TankCommand vocabulary
    tank/                        # tank model and complete spawn
    mobility/                    # tracked movement and contact laws
    gunnery/                     # aim, range, weapons, recoil
    combat/                      # projectile and damage lifecycle
    battlefield/                 # simulation-relevant world state

  content/
    schema.rs                    # versioned plain authored/baked data
    tank_spec.rs                 # RON authoring input
    tank_geometry.rs             # GLB extraction and validation
    loader.rs                    # runtime/development loading adapter

  client/
    input.rs                     # devices to TankCommand
    session.rs                   # cursor, overlays, and client-only state
    tank_view.rs                 # view attachment only
    camera.rs
    sight.rs
    hud.rs
    overlay.rs
    combat_feedback.rs
    vfx/

  net/
    mod.rs                       # private networking adapter facade
    protocol/                    # wire schema, fingerprint, registration, bridges
    client/                      # connection, prediction, receipt, ownership
    server/                      # admission, spawn, replication, publication
    physics.rs
    rig.rs
    watchdog.rs
    diagnostics.rs
    harness/

  telemetry/
    trace.rs
    cost.rs
    shot_trace.rs

  tools/
    armor_sandbox/
    track_sandbox/

  bin/                           # thin executable callers
```

## Module and file rules

A module is deep when callers receive substantial behaviour through a small interface. Directory
nesting is useful only when it hides implementation and improves locality.

- Split by owned invariant, state machine, or reason to change. Never split solely to satisfy a
  line-count target.
- The facade owns plugin installation, schedule and ordering requirements, failure behaviour, and
  deliberately exposed vocabulary.
- Child modules default to private. Prefer `pub(super)` for cooperation inside an implementation,
  then earned `pub(crate)` for a real cross-module caller. Plain `pub` exists only for a binary or
  genuine external consumer.
- Use explicit re-exports. Do not glob-export an implementation tree.
- Do not create `common`, `utils`, `helpers`, `manager`, or package-per-source-file dumping grounds.
- Keep one correctness state machine locally readable. Penetration, replica outcome consumption,
  protocol fingerprint registration, and overlay reconciliation must not be fragmented merely to
  shorten a file.
- A test crosses the same interface as a caller. Unit tests for pure mechanics stay beside their
  implementation; ECS behaviour tests stay with the owning module; cross-module and runtime
  contracts live under `tests/`.
- Retain and deepen the static network-direction guard in `tests/net_boundary.rs`. Add equivalent
  guards when `sim` and `client` directories materialize.

Bevy function plugins and plugin tuples are valid internal implementation tools. Introduce a
custom `PluginGroup` only when a caller genuinely needs to select or reorder a stable collection;
folder organization alone is not a reason.

## Comment and documentation policy

Source comments may state:

- a non-obvious invariant and the code that owns it;
- a surprising reason for the current implementation;
- an interface's ordering, configuration, failure, or performance contract;
- a sourced engine or protocol constraint; or
- the removal condition for a temporary adapter or workaround.

Comments should not narrate nearby code, preserve a chronological incident log, duplicate an ADR,
or claim a test result without naming its evidence. State an invariant once at its owning seam.
Move investigations, measurements, upstream research, and superseded approaches into `.agents/`
evidence documents and link them briefly when the implementation still depends on the finding.

## Debt ledger

| State | Debt | Required evidence for repayment |
|---|---|---|
| **OPEN — correctness** | `net::rig::attach_replicated_rig` waits for both replicated `Position`/`Rotation` and `PendingTankAssets::loaded`, then inserts `net_tank_rig`, the body role, and `DisableRollback` and calls `spawn_tank_sim` on an existing `Remote` root. This violates spawn completeness even though the local insert is synchronous within its later command flush. | A source-verified Lightyear lifecycle design plus an automated client/server test showing rollback state exists in the replicated entity's initial usable state. If that cannot be controlled reliably in the production harness, propose the dedicated multiplayer laboratory before building it. |
| **OPEN — content seam** | Simulation spawn still depends on runtime GLB extraction and Bevy asset readiness rather than a versioned baked `TankBlueprint`. | Server boots and simulates with the GLB absent; content validation happens before Battle admission; client and server compare a content fingerprint. |
| **OPEN — dependency closure** | The dedicated server is headless at runtime but still compiles Bevy rendering/window dependencies through the shared package. | A targeted server dependency report excludes render, WGPU, and Winit, and a headless Battle test runs the actual server composition. |
| **REPAID — guarded** | The executables previously reached through `overmatch::net::{client,server}` and `net` declared its client, server, protocol, diagnostic, and harness children public. | Executables now call crate-root `run_client`/`run_server`; networking children are private or crate-private; `tests/net_boundary.rs` rejects `overmatch::net` reach-through and compile-checks the root interface. |
| **OPEN — locality** | Ballistics, protocol, client networking, tank lifecycle, and server networking mix several independent reasons to change in large files. | Each moves behind a facade with private children, while lifecycle and loss/rollback contract tests remain green. |
| **OPEN — schedule ownership** | Cross-feature ordering is distributed among feature plugin implementations and long comments. | Named simulation sets and edges are registered at the owning simulation seam and pinned by behaviour tests. |
| **OPEN — prose drift** | Source and design prose still contains retired features, historical mechanisms, and claims contradicted by current code. | Comments satisfy the policy above; current docs link to evidence rather than embedding incident chronology; stale claims are removed or corrected. |

Debt is not repaid by moving a file or renaming a type. The required evidence must exist.

## Migration sequence

Structural and behavioural changes must remain independently reviewable. Do not combine a module
move with a netcode, physics, ballistics, or game-rule change.

- Preserve and land the active shooting-correctness work before moving the files it touches.
- Establish this root map and link it from the README.
- Narrow the crate and networking interfaces without changing runtime behaviour.
- Resolve the late replicated-rig spawn violation as a dedicated correctness change.
- Introduce a validated `TankBlueprint` seam and prove complete synchronous spawn from it.
- Deepen combat/ballistics, then protocol, client networking, tank, server networking, and mobility.
- Move client presentation, telemetry, tools, and runtime composition behind their target facades as
  real seams emerge.
- Remove compatibility re-exports and temporary forwarding modules after their callers migrate.
- Re-measure build and dependency behaviour before considering Cargo package extraction.

Each structural slice must pass focused owning-module tests while it is developed, followed by the
repository gates before commit:

```text
cargo fmt --all --check
cargo clippy --profile ci --locked --all-targets -- -D warnings
cargo test --profile ci --locked
```

## Cargo workspace extraction gates

Do not create a workspace because the source tree feels large. Extract a package only when its
interface has independent build, dependency, ownership, or release value.

The first plausible package seam is content schema plus simulation, after all of these are true:

- `TankBlueprint` is versioned plain data rather than Bevy scene state.
- The complete server simulation runs with view assets absent.
- A targeted server build has a render-free dependency closure.
- Another real consumer, such as the offline tank compiler or replay tooling, needs the interface.
- **MEASURED:** targeted build and CI data demonstrates that extraction improves the workflow or
  release artifact rather than merely moving compilation between packages.

An eventual workspace may separate content schema, simulation, client/server networking adapters,
presentation, and executable applications. That contour is not a commitment to package count.
Workspace-wide feature unification also means only targeted server builds can prove the server's
minimal dependency closure.

## Evidence and related decisions

- [Product target](.agents/PRODUCT.md)
- [Glossary](.agents/GLOSSARY.md)
- [ADR-0002: plugin composition and thin binaries](.agents/docs/adr/0002-plugin-per-feature-architecture.md)
- [ADR-0009: repository content roles](.agents/docs/adr/0009-release-artifacts-and-repo-layout.md)
- [ADR-0010: per-variant RON data](.agents/docs/adr/0010-per-variant-data-in-ron.md)
- [ADR-0011: fail-fast model contract](.agents/docs/adr/0011-required-model-contract-fails-fast.md)
- [ADR-0012: spec-driven rig binder](.agents/docs/adr/0012-spec-driven-rig-binder.md)
- [ADR-0014: sim/view split](.agents/docs/adr/0014-sim-view-split.md)
- [ADR-0015: divergence doctrine](.agents/docs/adr/0015-divergence-doctrine.md)
- [ADR-0016: replicate causes, derive consequences](.agents/docs/adr/0016-replicate-causes-derive-consequences.md)
- [ADR-0018: wire surface fingerprint](.agents/docs/adr/0018-wire-surface-fingerprinted-and-refused.md)
- [ADR-0019: overlay reconciliation](.agents/docs/adr/0019-overlays-declare-presence-consequences-derive.md)
- [ADR-0021: fire replication](.agents/docs/adr/0021-fire-replication-architecture.md)
- [ADR-0022: input attestation](.agents/docs/adr/0022-input-attestation-not-detection.md)
- [ADR-0024: player-facing authority runtime](.agents/docs/adr/0024-one-authoritative-runtime-for-player-facing-worlds.md)
- [Playtest forks](.agents/scratch/playtest-forks/README.md)
