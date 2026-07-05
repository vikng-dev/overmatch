# Sim/view split: the tank is built from data at spawn, the glb is a view

The tank's sim body — servo frames, wheel stations, collision hulls, armor trimeshes, `Rig`/`TankSim`/indices, mass properties — is now built **synchronously at spawn, from extracted data** (`bake::TankGeometry`), on every spawn path. The glb scene is no longer the sim's constructor; it is a **view** that attaches whenever it loads and only renders (`tank::bind_tank_view`). This dissolves the tank's two-phase birth — the single architectural mismatch behind a run of netcode bugs — by making the whole sim state exist the tick the entity spawns. It graduates phase 1 of `design/sim-view-split-and-tank-bake.md`; phase 2 (the offline bake) remains that sketch's to own (see *Deferred*). Inherits the spec-driven *declaration* model of [[0012-spec-driven-rig-binder]] and supersedes only its runtime-join *mechanics*; extends the fail-fast lineage of [[0010-per-variant-data-in-ron]] / [[0011-required-model-contract-fails-fast]].

## The problem: the tank was born twice

lightyear's prediction/rollback model assumes a predicted entity is **born complete** — every sim-relevant component exists the tick it spawns, and history, rollback restore, and replication all key off that. The old tank violated this: the replicated root arrived at tick T, but the actual sim body assembled asynchronously later, whenever the glb scene instantiated and `on_tank_ready` bound it. Everything in that **bind window** was a hazard class, and every netcode casualty to date lived there:

- **`Position::PLACEHOLDER` NaN** — physics ran before the body existed.
- **ghost-child replication** — replication walked a hierarchy that appeared late.
- **history-attach races** — rollback state attached late to late-arriving entities (the retired step-7/8 despawn-grace machinery).
- **`ConfirmedHistory` rest-capture corruption** — lightyear enshrined a mid-life-inserted `TankSim`'s add-time value as permanent server truth; every rollback restored bind defaults, and the lazy servo-rest capture then baked the current lay into the servo zero (aim desync, gun visibly outside travel limits).

That is one architectural mismatch surfacing five ways, not five bugs. Neither replicon's blueprint pattern (marker + **synchronous** local construction) nor any lightyear example covers a predicted entity whose sim state materializes N ticks after spawn — we were improvising in a gap the libraries do not acknowledge.

## The standing rule (tier 2)

*Nothing rollback-registered may be initialized from an asset or inserted late onto a replicated entity — sim state must be constructible at spawn, synchronously, from data.* This ADR is the architecture that makes the rule **structural** rather than disciplinary: with the sim body built from `TankGeometry` at spawn, the rule holds by construction for all existing state. It is also recorded in AGENTS.md as standing working discipline.

## The architecture

**Extract once, as pure data.** `bake::extract_tank_geometry` parses the tank's `.glb` **as data** — the `gltf` crate against the file, no Bevy scene, no asset dependency — into `TankGeometry`: every node's name, parent, local transform, root-relative pose, and (for sim-consumed meshes) raw vertex/index buffers. It runs at startup into a resource. The same function is phase 2's offline-compiler core: one parser, two mounting points.

**Spawn the complete sim body synchronously.** `tank::spawn_tank_sim` builds the entire sim from `TankGeometry` in the root's spawn flush, on every path (SP at spawn, server at connect-spawn, client the tick the replicated root's **pose** lands — `attach_replicated_rig` gates on replicated `Position`/`Rotation`, which can trail the root's markers by a few frames on a cold join; that pose gate is the one gate that remains, and it is lightyear's own replicated-state arrival, not an asset):

- servo frames carrying `ServoRest` (the rest quaternion is spawned from data — it used to be lazily captured on the first tick, the field the `ConfirmedHistory` bug corrupted), wheel stations, and collider nodes;
- **collision hulls** via `Collider::convex_hull` (≡ the old `ConvexHullFromMesh` at the parry call level — it ignores indices) and **armor trimeshes** via `trimesh_with_config` with `MERGE_DUPLICATE_VERTICES` (≡ the old `TrimeshFromMesh`), so raw-data construction reproduces today's shapes byte-for-byte;
- `Rig` handles, `TankSim`, and the servo/weapon indices;
- **mass properties**: authored mass and inertia extents (`NoAutoMass`, [[0011-required-model-contract-fails-fast]]) with a data-derived `CenterOfMass`.

Missing structure is fatal here — every spec-declared node must resolve against the extracted geometry, and an absence panics at spawn with the list of what is missing (the rig contract, now enforced against data instead of the bound scene).

**The glb is a view that attaches whenever it loads.** `tank::bind_tank_view` observes the scene instantiating and joins its named nodes against the sim skeleton's part table: it tags each glb node that has a same-named sim part `ViewOf` that part (plus `ViewServo` where the part is a servo frame), back-links the sim part with `ViewNode`, hides the authored physics geometry (the sim colliders are built from data; the glb copies are just meshes), and re-parents an already-launched turret subtree onto its free body if cook-off fired during the load. Nothing here constructs sim state.

**Render smoothing lives on the view tree.** Sim transforms are pure per-tick truth. `interpolate_servos` writes the render-blended pose to **view** nodes (its write set is `ViewServo`), never to sim nodes. Every render reader resolves its render-side node through the single fallback rule `ViewNode::resolve` — the attached view node, or the sim part itself before the scene attaches (cosmetic: nothing slews during the spawn pop-in).

**Determinism device.** Index assignment (`ServoIndex`/`WeaponIndex` — values both wire ends derive) is **sorted-by-name at spawn**, so a HashMap's iteration order can never decide them; both ends construct identical shapes and indices from identical data.

**Equivalence guard retained.** The step-0 shadow harness stays on: the extractor runs at startup and a shadow observer compares its output against every instantiated scene (names, hierarchy, bit-exact local transforms and composed poses, collider/ballistic mesh bytes). Post-split its meaning reverses — it now proves the *view the player sees* matches the *data the sim runs on* (and still catches a bevy_gltf coordinate-conversion flip). Mismatches panic in debug, log in release.

## Consequences

- **The bind window is dead as a sim concept.** A late glb scene is now cosmetic pop-in (~100ms), which no netcode has to care about. Every remaining mention of "bind window" / "bound rig" in comments must describe the *view* attach or be deleted.
- **Two same-named sibling trees under one root** (the sim skeleton and the instantiated scene), and a **per-consumer skeleton-skip** in the view walk — accepted interim costs. `bind_tank_view` must skip sim parts or it self-references `ViewOf` and stamps `Visibility` onto bare skeleton nodes. Phase 2's baked artifact removes the second tree from the sim binary entirely.
- **Contract violations are fatal at spawn.** A spec-declared node with no matching extracted node panics immediately, with a precise message (fail-fast, [[0010-per-variant-data-in-ron]] / [[0011-required-model-contract-fails-fast]]) — a broken rig is a bug, not a degraded runtime state.
- **Lazy captures are gone.** Servo rest, recoil rest, the camera's turret-pivot capture, the COM `GlobalTransform` read — all spawned from data now. `ServoState` shrinks to true per-tick state.
- **`strip_confirmed_history` stays load-bearing.** On the client the sim components are still, in lightyear's eyes, inserted *mid-life* onto an already-replicated root (the pose gate means the entity exists a few frames before the sim body attaches), so lightyear still seeds `ConfirmedHistory` with their add-time values — the split fixed what the corrupted restore *did* (the lazy rest capture), not the seeding itself. Deleting the strip guard would resurrect the aim-desync class with correct-looking spawn code.

## Deferred — phase 2 (governed by the design sketch)

Out of scope here; still owned by `design/sim-view-split-and-tank-bake.md`:

- **The offline tank compiler** (`tools/tankc`) — the same extractor plus the RON, run at build time, emitting a serialized artifact; today's spawn-time contract panics become build errors.
- **The baked artifact + connect-handshake hash** — client and server prove they run the same bake by hashing the versioned artifact into the connect handshake.
- **The server shedding the glb entirely** — the runtime loads the artifact only; the ~65 MB startup glb re-read (phase-1 scaffolding) and the server's `bevy_gltf` dependency go away. The honesty test: the server binary simulates a full duel with the glb deleted from its disk.

## Related

- **Supersedes the runtime-join mechanics of [[0012-spec-driven-rig-binder]].** The three-way join no longer happens by walking the instantiated scene at bind: it happens at **spawn**, against extracted geometry (sim), and at **view-attach**, against the sim part table (presentation).
- **Inherits ADR-0012's spec-driven declaration model, unchanged.** The RON still declares a variant's servos / weapons / volumes / views keyed by node name; the spec is the source of truth; the node name is the sole join key and is never parsed for behaviour; the bidirectional fail-fast contract still stands (only the seam it fires at moved). A new tank is still a `.glb` + `.tank.ron` with no code change.
- Extends [[0010-per-variant-data-in-ron]] and [[0011-required-model-contract-fails-fast]] (fail-fast, authored mass properties). The collision-proxy geometry it constructs is still [[0008-collision-convex-proxies]].
</content>
</invoke>
