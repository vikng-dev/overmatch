# Per-variant data in RON spec-sheet assets; skein deferred

Per-tank-variant tuning data — drivetrain characteristics, servo speeds/travel, and later masses, caliber, reload — lives in a **per-variant RON data asset** (`<variant>.tank.ron`), deserialized via `serde` straight into the same components the sim reads (`Drivetrain`, `ServoSpec`) and applied to the rig once both the asset and the name-bound markers are ready (`apply_tank_spec`). The Blender model keeps **geometry and spatial anchors** (the `Center_Of_Mass` Empty, the `*_Collider` proxies, future hardpoints — all native glTF, no addon); **structure** stays name-bound in code ([[0002-plugin-per-feature-architecture]]); only the flat per-variant *numbers* move out to RON. This **supersedes [[0007-model-authored-data-via-skein]]**.

Why RON, and why not the model itself:

- **The data isn't node-bound.** Skein's one real capability is binding *typed structs to specific model nodes* (so the datum and the node travel together). We have none of that: the drivetrain is one blob, and the two servos are non-spatial and already enumerated by name in code. Flat per-variant scalars gain nothing from living on a node — and lose a lot.
- **git-diffable / reviewable.** A `.glb` is binary; a thrust change embedded in it vanishes from PR review. A `.ron` file is plain text.
- **No DCC round-trip.** Editable without Blender open, hot-reloadable, no recompile. The live-tuning loop targets the *game* and can persist back to RON — a cleaner "game is the consumer" story than hand-editing the `.blend`.
- **No fast-moving dependency.** Skein is a Bevy plugin **plus** a Blender addon **plus** a BRP schema bridge, all version-coupled to a bleeding-edge Bevy/Avian stack. RON is `serde` + a ~25-line in-crate `AssetLoader`.

## Considered Options

- **skein (the superseded decision).** Typed components authored in Blender, carried in glTF extras via the registry/BRP. The right tool when **many node-bound typed structs** need spatial/visual authoring, or a non-coder content pipeline. **Deferred, not rejected forever**: re-adoptable when Phase-4 multi-variant work brings genuinely node-attached structs (per-plate armor angle+thickness, per-wheel suspension params, hardpoint modules). It was adopted in ADR-0007 but *no data ever flowed through it* — the dependency, plugin, and dev BRP server carried zero payload.
- **Native `GltfExtras` (hand-parsed).** Same "data embedded in the binary" downside as skein, without the typed-authoring upside. No.
- **`bevy_common_assets` `RonAssetPlugin`.** The standard RON-asset loader, but it has no crates.io release for Bevy 0.19 (git `main` only). A git dependency on a fast-moving crate is the very version-lag fragility we're avoiding; a tiny in-crate loader is more robust.
- **All-code consts (status quo).** Can't vary per variant, can't be tuned/reviewed separately from code, forces a recompile per change.

## Consequences

- **Dependencies:** `serde` + `ron` added (small, stable, non-engine); `bevy_skein` removed.
- **Components:** `Drivetrain`, `ServoSpec`, `Axis`, `Travel` derive `serde::Deserialize` (+ `Clone`); they are no longer `Reflect` / `register_type`'d. Re-adding reflection later (if skein returns) is cheap.
- **Spatial data stays on the model**, unchanged and skein-free: COM is an Empty's transform, colliders are mesh-derived ([[0008-collision-convex-proxies]]). This is the data class that genuinely belongs on the model — and native glTF already covers it.
- **Load timing:** `apply_tank_spec` copies the spec onto the rig once the asset has loaded *and* the servos are bound (robust to either order). Until then `Drivetrain` uses its code `Default`; the servos simply don't slew for those first frames.
- **Units:** servo angles are authored in **degrees** in RON (the human unit); `drive_servos` converts to radians (unchanged from the servo split).
