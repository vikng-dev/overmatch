# Spec-driven rig binder: iterate the spec, not match-on-name

> **Status: runtime-join mechanics superseded by [[0014-sim-view-split]] (2026-07-05).** The *declaration* model below stands unchanged and is inherited: the RON declares a variant's parts as node-keyed maps, the spec is the source of truth, the node name is the sole join key (never parsed for behaviour), and the bidirectional fail-fast contract holds. What moved is *when and against what the join runs*: it no longer walks the instantiated glb scene in `on_tank_ready`. It happens at **spawn**, against extracted geometry (`bake::TankGeometry`), for the sim body, and at **view-attach**, against the sim part table, for presentation. Read the "runtime name→entity index / descendant walk" mechanics below as historical; the naming contract and the two-guard fail-fast are current.

The rig bind in `on_tank_ready` no longer hardcodes `match name { "Turret" => … }` against a parallel `REQUIRED` singleton list. The per-variant RON ([[0010-per-variant-data-in-ron]]) now declares a tank's parts as node-keyed maps — `servos`, `weapons`, `volumes`, `views` — and the binder builds a **name → entity index** over the loaded glTF scene once, then **iterates the spec**, resolving each declared node and attaching its behaviour. The node name is the sole join key between model, RON, and code, and is **never parsed for behaviour** (role/function/gating are RON data; the `*_Ballistic` / `*_Collider` / `Armor_…` affixes are documentation only). This dissolves the three-way hand-sync the design sketch flagged and makes a tank's parts enumerable, so composability is no longer structurally blocked. Refines [[0002-plugin-per-feature-architecture]] (still "name = the contract," reactive attachment) and [[0010-per-variant-data-in-ron]]; **supersedes** the provisional sketch `design/rig-ron-sot-and-composability.md`.

## Naming contract

- **Fixed-name singletons** — only `Hull` and `Center_Of_Mass` (exactly one per tank, matched by literal name).
- **Suffix / set tags** — `*_Collider` (physics proxy, [[0008-collision-convex-proxies]]); roadwheels `Wheel_{L,R}_<n>` matched by side + numeric index. Set membership, no per-node tuning.
- **Everything variable is RON-declared, keyed by node name** — servos, weapons (muzzle + optional recoil barrel), ballistic volumes, and view anchors. A variant adds parts by writing RON against model nodes, not by editing code.

## Bidirectional contract (fail-fast, two guards)

The two-guard contract stands; its runtime seam moved (per [[0014-sim-view-split]]). The private
tank body assembler now resolves each spec-declared node against extracted `TankGeometry` during
complete construction. A miss panics before gameplay, not after a scene-walk bind. The CI guard is
unchanged.

- **Runtime (ships):** every spec-declared node must resolve, together with the fixed singletons,
  collision geometry, and roadwheels on both tracks. A miss panics during complete construction
  ([[0011-required-model-contract-fails-fast]]). The separate view walk still skips render-primitive
  leaves when joining presentation by name.
- **CI test:** `tiger_1_spec_binds_to_model` reads the `.glb` node names with the `gltf` crate and checks **both** directions — every spec reference resolves, and no `*_Ballistic` node is an orphan (undeclared). Catches name drift at `cargo test`, before a rename reaches a runtime panic.

The guards are complementary: runtime validates extracted construction data in the shipping path;
the CI test adds the reverse orphan check before launch.

## Considered options

- **Hardcoded `match name` + `REQUIRED` array (status quo).** Duplicated three ways (match arms, required list, RON field names), singleton-bound ("a tank has *a* turret"), verified only by `cargo run` + log-greps. The thing this replaces.
- **Scan nodes, match each against the spec inline during the descendant walk.** An intermediate state we passed through; it interleaved volume binding into the walk and needed a runtime drift-lint for orphans. Replaced by iterate-the-spec — one pattern for servos / weapons / volumes / views — with the CI test owning orphan detection.
- **Axis-from-node (rejected).** The sketch's highest-leverage idea was to drop the `Axis` enum and read a servo's hinge axis from its pivot node's orientation. Rejected: node orientation as the source of truth for axis is fragile to re-export (a re-rigged pivot silently changes behaviour) and couples a gameplay concern to a modelling accident. Instead a servo's `role` (`Yaw`/`Pitch`) is declared in RON and the **axis is derived from the role** (Yaw → +Y, Pitch → +X) — reviewable in the text RON, stable across re-exports. Compound mounts are still nested 1-DOF servos (the composability win survives); only the axis *source* differs.

## Consequences

- A new tank = new `.glb` + `.tank.ron` with no code change, **plus a case in the CI bind test**.
- `Turret` / `Gun` / `Muzzle` are no longer fixed rig-contract names — they're resolved from the gunner `view` node and the Primary weapon (see [[0013-composable-rig-control]]). The glossary's "Rig contract" entry is updated accordingly.
- Binding is uniform: servos, weapons, and volumes all resolve via the same index after the descendant walk, so the walk only handles the fixed-name / set-tag nodes. *(The descendant-walk index is superseded by [[0014-sim-view-split]]: the index is now `TankGeometry::by_name`, built by the extractor, not by walking the scene at bind; resolution is otherwise identical.)*
