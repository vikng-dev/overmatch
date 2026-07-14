# Sim/view split: the tank is built from data at spawn, the glb is a view

The tank's sim body — servo frames, wheel stations, collision hulls, armor trimeshes,
`Rig`/`TankSim`/indices, and mass properties — is built synchronously from extracted data
(`bake::TankGeometry`). Authority and analytical routes create the root and complete body together
through `tank::spawn_complete_tank`. The GLB is only a view that may attach later.

One explicit violation remains: the network client receives a replicated root first and calls
`tank::attach_replicated_tank_body` after its wire pose arrives. That path is asset-independent and
assembles the body in one flush, but it is still late rollback-state attachment. The debt and its
required spawn-intent/ack evidence are tracked in the root `ARCHITECTURE.md`.

## The problem: the tank was born twice

lightyear's prediction/rollback model assumes a predicted entity is **born complete** — every sim-relevant component exists the tick it spawns, and history, rollback restore, and replication all key off that. The old tank violated this: the replicated root arrived at tick T, but the actual sim body assembled asynchronously later, whenever the glb scene instantiated and `on_tank_ready` bound it. Everything in that **bind window** was a hazard class, and every netcode casualty to date lived there:

- **`Position::PLACEHOLDER` NaN** — physics ran before the body existed.
- **ghost-child replication** — replication walked a hierarchy that appeared late.
- **history-attach races** — rollback state attached late to late-arriving entities (the retired step-7/8 despawn-grace machinery).
- **`ConfirmedHistory` rest-capture corruption** — lightyear enshrined a mid-life-inserted `TankSim`'s add-time value as permanent server truth; every rollback restored bind defaults, and the lazy servo-rest capture then baked the current lay into the servo zero (aim desync, gun visibly outside travel limits).

That is one architectural mismatch surfacing five ways, not five bugs. Neither replicon's blueprint pattern (marker + **synchronous** local construction) nor any lightyear example covers a predicted entity whose sim state materializes N ticks after spawn — we were improvising in a gap the libraries do not acknowledge.

## The standing rule (tier 2)

*Nothing rollback-registered may be initialized from an asset or inserted late onto a replicated
entity — sim state must be constructible at spawn, synchronously, from data.* Complete construction
makes this structural for authority and analytical routes. The replicated-client exception above
does not weaken the target; it names the remaining work honestly.

## The architecture

**Extract once, as pure data.** `bake::extract_tank_geometry` parses the tank's `.glb` **as data** — the `gltf` crate against the file, no Bevy scene, no asset dependency — into `TankGeometry`: every node's name, parent, local transform, root-relative pose, and (for sim-consumed meshes) raw vertex/index buffers. It runs at startup into a resource. The same function is phase 2's offline-compiler core: one parser, two mounting points.

**Construct the complete sim body synchronously from data.** `tank::spawn_complete_tank` creates
normal roots and queues the private body assembler in the same command batch. The client exception
uses `tank::attach_replicated_tank_body` only after replicated `Position` and `Rotation` exist; view
asset readiness is not an input to either constructor.

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

- **The asset bind window is dead as a sim concept.** A late GLB scene is cosmetic. The separate
  replicated-root timing violation remains open as described above.
- **Two same-named sibling trees under one root** (the sim skeleton and the instantiated scene), and a **per-consumer skeleton-skip** in the view walk — accepted interim costs. `bind_tank_view` must skip sim parts or it self-references `ViewOf` and stamps `Visibility` onto bare skeleton nodes. Phase 2's baked artifact removes the second tree from the sim binary entirely.
- **Contract violations are fatal at spawn.** A spec-declared node with no matching extracted node panics immediately, with a precise message (fail-fast, [[0010-per-variant-data-in-ron]] / [[0011-required-model-contract-fails-fast]]) — a broken rig is a bug, not a degraded runtime state.
- **Lazy captures are gone.** Servo rest, recoil rest, camera pivot, and center of mass come from
  construction data.
- **`strip_confirmed_history` stays load-bearing while the replicated-root exception exists.** The
  client still inserts sim components onto an existing root, so history guards must remain until a
  spawn-intent/ack design removes that lifecycle.

## Addendum (2026-07-06): the split now also carries rollback-correction smoothing

Commit 597ec21 moved the client to **instant sim correction** (`CorrectionPolicy::instant_correction()`
— the sim pose snaps to the corrected present within one frame) with **all visible smoothing in
the render-space error layer** (`net/render_error.rs`), which accumulates each rollback's jump as
an offset on the root's post-writeback `Transform` and decays it — "the sim snaps, the view never
does". This extends the paragraph above one level up: sim transforms stay pure per-tick truth not
only against servo blending but against netcode correction too, and the smoothing lives strictly
render-side. The split is what made that possible — with sim truth and presentation already
separate planes, the correction offset had a place to live that provably cannot feed back into
the simulation. Doctrine context in [[0015-divergence-doctrine]].

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
