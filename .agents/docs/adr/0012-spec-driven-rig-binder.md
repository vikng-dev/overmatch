# Spec-driven rig binder: iterate the spec, not match-on-name

The rig bind in `on_tank_ready` no longer hardcodes `match name { "Turret" => … }` against a parallel `REQUIRED` singleton list. The per-variant RON ([[0010-per-variant-data-in-ron]]) now declares a tank's parts as node-keyed maps — `servos`, `weapons`, `volumes`, `views` — and the binder builds a **name → entity index** over the loaded glTF scene once, then **iterates the spec**, resolving each declared node and attaching its behaviour. The node name is the sole join key between model, RON, and code, and is **never parsed for behaviour** (role/function/gating are RON data; the `*_Ballistic` / `*_Collider` / `Armor_…` affixes are documentation only). This dissolves the three-way hand-sync the design sketch flagged and makes a tank's parts enumerable, so composability is no longer structurally blocked. Refines [[0002-plugin-per-feature-architecture]] (still "name = the contract," reactive attachment) and [[0010-per-variant-data-in-ron]]; **supersedes** the provisional sketch `design/rig-ron-sot-and-composability.md`.

## Naming contract

- **Fixed-name singletons** — only `Hull` and `Center_Of_Mass` (exactly one per tank, matched by literal name).
- **Suffix / set tags** — `*_Collider` (physics proxy, [[0008-collision-convex-proxies]]); roadwheels `Wheel_{L,R}_<n>` matched by side + numeric index. Set membership, no per-node tuning.
- **Everything variable is RON-declared, keyed by node name** — servos, weapons (muzzle + optional recoil barrel), ballistic volumes, and view anchors. A variant adds parts by writing RON against model nodes, not by editing code.

## Bidirectional contract (fail-fast, two guards)

- **Runtime (ships):** every spec-declared node must resolve, plus the fixed singletons / ≥1 collider / ≥1 roadwheel per side — a miss panics at bind ([[0011-required-model-contract-fails-fast]]). The binder asserts against the *bound* entities, after Bevy's name handling (it skips `GltfMaterialName` render-primitive leaves, which carry the mangled `{mesh}.{material}` names).
- **CI test:** `tiger_1_spec_binds_to_model` reads the `.glb` node names with the `gltf` crate and checks **both** directions — every spec reference resolves, and no `*_Ballistic` node is an orphan (undeclared). Catches name drift at `cargo test`, before a rename reaches a runtime panic.

The design sketch warned against a "second glTF parser" testing a different name-resolution path than ships. We took it deliberately: the two guards are **complementary, not a substitute**. The runtime contract is the one that ships (it validates the real bound scene); the CI test is a cheap pre-flight, and crucially it owns the *reverse* direction (orphan detection), which is why the binder needs no runtime drift-scan.

## Considered options

- **Hardcoded `match name` + `REQUIRED` array (status quo).** Duplicated three ways (match arms, required list, RON field names), singleton-bound ("a tank has *a* turret"), verified only by `cargo run` + log-greps. The thing this replaces.
- **Scan nodes, match each against the spec inline during the descendant walk.** An intermediate state we passed through; it interleaved volume binding into the walk and needed a runtime drift-lint for orphans. Replaced by iterate-the-spec — one pattern for servos / weapons / volumes / views — with the CI test owning orphan detection.
- **Axis-from-node (rejected).** The sketch's highest-leverage idea was to drop the `Axis` enum and read a servo's hinge axis from its pivot node's orientation. Rejected: node orientation as the source of truth for axis is fragile to re-export (a re-rigged pivot silently changes behaviour) and couples a gameplay concern to a modelling accident. Instead a servo's `role` (`Yaw`/`Pitch`) is declared in RON and the **axis is derived from the role** (Yaw → +Y, Pitch → +X) — reviewable in the text RON, stable across re-exports. Compound mounts are still nested 1-DOF servos (the composability win survives); only the axis *source* differs.

## Consequences

- A new tank = new `.glb` + `.tank.ron` with no code change, **plus a case in the CI bind test**.
- `Turret` / `Gun` / `Muzzle` are no longer fixed rig-contract names — they're resolved from the gunner `view` node and the Primary weapon (see [[0013-composable-rig-control]]). The glossary's "Rig contract" entry is updated accordingly.
- Binding is uniform: servos, weapons, and volumes all resolve via the same index after the descendant walk, so the walk only handles the fixed-name / set-tag nodes.
