# Collision geometry: per-part convex proxies authored on the model, decomposed in the DCC

Each part's collision shape is a **simplified convex proxy authored as geometry on the model** — a hidden `*_Collider` mesh child of the part — turned into an Avian collider via `ColliderConstructor::ConvexHullFromMesh`. Concave shapes (e.g. the stepped hull front) are **split into several convex pieces** (`<Part>_Collider_0`, `_1`, …) that form a compound, **in Blender**, not at runtime. This extends [[0005-raycast-roadwheel-locomotion]] (model = geometry) into a broader **per-part layering**: each part (Hull, Turret, …) carries parallel layers — visual mesh, collision proxy, and later armor plates and internal modules — as child geometry/components, each consumed *by type* (see [[0010-per-variant-data-in-ron]]).

Why this shape, and not the obvious alternatives:

- **Convex constraint.** A dynamic body's collider must be convex, or a compound of convex pieces — the solver's contact algorithms (GJK/EPA) require it. A single concave mesh is for *static* geometry only. So a concave part is represented as several convex pieces, never one concave mesh.
- **Proxy, not the visual mesh.** The collider is a coarse hand-authored shape, not the detailed render mesh — which carries the antenna, fine details, and concavities that collision neither needs nor can use. Collision only cares about the convex outer envelope.
- **Decompose in the DCC, not at runtime.** Pre-authored/pre-split proxies are inspectable, deterministic, and free of load cost; runtime convex decomposition produces opaque hulls that "bulge into gaps", is approximate, and pays a (cached) load hit. Runtime decomposition is a prototyping fallback, not the shipped path.

## Considered Options

- **Code-defined primitive** (the seed's placeholder box, `HULL_WIDTH/HEIGHT/LENGTH/BELLY`). Simple, but the dimensions are per-variant geometry hardcoded in code, and it can't capture shape. Retired — geometry belongs on the model.
- **Trimesh of the visual mesh** (`TrimeshFromMesh`). Exact, but concave-trimesh colliders are unstable/unsupported on *dynamic* bodies and expensive; reserved for static terrain.
- **Runtime convex decomposition** (`ConvexDecompositionFromMesh`, VHACD). No DCC tooling, but rougher hulls, opaque results that bulge/gap, and a load cost. Kept as a quick-prototype escape hatch and for genuinely complex / at-scale geometry (destructible props, many variants) where a manual split doesn't scale — preferably run offline (CoACD) rather than at load.

## Consequences

- **Binding contract:** `on_tank_ready` attaches `ConvexHullFromMesh` + `Layer::Vehicle` + `Visibility::Hidden` to any `*_Collider*` node; one piece or several compose into the body identically. Requires Avian's `collider-from-mesh` feature.
- **Co-location:** a part's collider is its *child*, so it inherits the part's transform — the turret's proxy rotates with the turret. (Same reason armor/internals live under their part.)
- **Collision ≠ armor.** Armor needs per-plate *angle + thickness* for the penetration raycast, so the stepped front's three plates live in the armor layer (oriented surfaces), not the convex collision proxy. Same geometry, two layers, two representations.
- **Mass** stays a code constant (`HULL_DENSITY`) for now — a separate per-variant migration into the RON spec sheet ([[0010-per-variant-data-in-ron]]).
- **Deferred fidelity:** while the roadwheels carry the load ([[0005-raycast-roadwheel-locomotion]]), a single hull box suffices; a hand-built 2–3-piece compound is the upgrade when debris/destructible interactions need shells and rubble to rest on the real silhouette. Auto-decomposition (offline CoACD/V-HACD) is the tool for the destructible props themselves, not the tank.
