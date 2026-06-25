# Model-authored per-variant data via skein; tuning stays a live in-game loop

> **Status: superseded by [[0010-per-variant-data-in-ron]].** We adopted skein here but never flowed any data through it (the dependency + plugin + dev BRP server carried zero payload), then reversed course: per-variant data now lives in RON spec-sheet assets, and skein is deferred until node-bound typed structs actually justify it. Kept as the decision trail — the analysis below of *what skein is for* and *when it earns its keep* remains accurate; only the timing bet ("adopt now") was wrong.

Per-tank-variant data — drivetrain and suspension parameters, gun caliber, masses, COM — is authored **on the Blender model** and arrives in Bevy as **typed components**, via **skein** (`bevy_skein`). Reflection-registered components are attached to objects in Blender (the addon reads our live `TypeRegistry` over the Bevy Remote Protocol) and instantiated on scene spawn from the glTF extras — no manual parsing or name-matching for data. The model becomes the single source of truth for everything model-specific, extending [[0005-raycast-roadwheel-locomotion]] (model = geometry only) and the name-based structural binding of [[0002-plugin-per-feature-architecture]].

What lives where — the deciding axis is *does this differ per tank*, not how often it currently changes:

- **Universal rules / laws → code, always.** The friction-circle and brush-anchor algorithms, the servo motion profile, gravity, and pure sim/feel thresholds (`STICK_SPEED`, `INPUT_RAMP`, `COMMAND_DEADBAND`). The engine.
- **Per-variant characteristics → the model.** Thrust, suspension stiffness/damping, lateral grip, rolling resistance, servo speeds, muzzle velocity, reload, recoil, hull dims/mass, COM, caliber. The spec sheet. These are code consts today only as scaffolding — there is one variant and the feel is unsettled.
- **Per-environment properties → terrain.** `MU` (track-vs-ground friction) is a property of the surface pair, not the tank; it migrates to the ground-type mechanic, not the model.

The enabling refactor is **`const → component`**: systems read parameters from a component (`Drivetrain`, `SuspensionParams`, …) rather than module consts. This decouples *where values come from* (a code `Default` now, glTF/skein later), *how they are tuned* (a live in-game panel), and *where they ultimately live* (the model) into three independent axes. It is mechanism-agnostic — native parsing and skein land the identical components — so it can (and should) precede full skein adoption.

Fast iteration survives model-as-data. An in-game (egui) **live-tuning panel** mutates the parameter components while driving; settled values are then **hand-entered into the `.blend`** at decision points and re-exported. The slow Blender round-trip happens at the frequency of *decisions*, not *iterations* — making this loop faster than today's edit-const-and-recompile. The game stays strictly a **consumer** of the model; write-back is deliberately manual — no game→Blender sync, which is the Godot-shaped bidirectional editor coupling we avoid.

## Considered Options

- **Native `GltfExtras` (zero-dep).** Bevy's loader exposes raw extras JSON we parse by hand into the same components. No dependency, but hand-rolled parsing and untyped Blender authoring — and it does not deliver the "no manual parsing/tree-walking" goal. Retained as the fallback: skein rides standard glTF extras, so if it were abandoned we replace the *loader* (a few hundred lines), not the *data*. Low lock-in.
- **Blenvy / `bevy_gltf_components` (kaosat-dev lineage).** The former front-runner; rejected as effectively abandoned — Blenvy stalled at `0.1.0-alpha.1` (Aug 2024, self-described broken alpha, only ever tracked Bevy `main`), and `bevy_gltf_components` was deprecated in its favour. Not a candidate on Bevy 0.19.
- **All-code consts (status quo).** Correct while single-variant, but a `const` cannot vary per tank, be sourced from the model, or be live-tuned. The forcing function is Phase 4 (multiple variants), where one const cannot serve two tanks.

## Consequences

- New dependency: `bevy_skein` (0.6, tracks Bevy 0.19 within days of release — the version-lag risk that disqualified Blenvy does not apply). Parameter components must be `Reflect` + `#[reflect(Component)]` + registered.
- Authoring uses the Bevy Remote Protocol at *authoring time only*: the app exposes its registry to the Blender addon (cached to `skein-registry.json`). The shipped game loads a static glTF with no BRP and no editor coupling.
- **Avian colliders stay mesh-derived** (`ColliderConstructor`), not authored via reflection UI; skein authors our *spec* components, not collision geometry. Simple Avian scalars (`ColliderDensity`, `Friction`) could be authored later but are out of initial scope.
- Structural binding by node name can shrink over time as markers (`Turret`, `Roadwheel`) become Blender-attached components, but that migration is incremental and not required here.
- Migration is incremental: COM is already model-authored (the proof); parameters move out of code as their feel settles or Phase 4 forces it. Adopting the mechanism now does not mean migrating all data at once.
