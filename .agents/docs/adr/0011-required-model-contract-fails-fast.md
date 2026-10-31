# Required model contract fails fast — no silent defaults

Everything the model is *required* to provide — its per-variant spec sheet ([[0010-per-variant-data-in-ron]]) and its structural rig nodes ([[0002-plugin-per-feature-architecture]]) — has **no code default and no silent fallback**. Absent or unloadable required data is fatal: we panic (in every build), not degrade. This is a deliberate consequence of the genre — a realistic, competitive PvP sim where a tank silently running on *guessed* stats, or with a silently broken rig, is a fairness/correctness bug strictly worse than a crash.

Enforced at three seams:

- **Spec data** — a `LoadState::Failed` `.tank.ron` panics (at spawn for the initial load; via `report_failed_spec` for a bad hot-reload); there is no `Drivetrain` / `SuspensionParams` / `ServoSpec` default. A schema-drift test deserializes the shipped file at `cargo test` time, catching the same class pre-ship.
- **Map data** — every build loads a map: `map::read_manifest` panics on an absent `maps/<id>/level.json`, naming the path and the fix (ship `maps/` in the assets directory, or point `BEVY_ASSET_ROOT` at one that does), and on any manifest that does not hold. There is no flat-world fallback; the flat slab is a fixture world that only the explicit `terrain_grid::ForceFlatWorld` marker selects.
- **Rig structure** — `on_tank_ready` asserts the **rig contract** after binding (required singletons `Hull` / `Turret` / `Gun` / `Gun_Barrel` / `Muzzle` / `Center_Of_Mass`, plus ≥1 `*_Collider` and ≥1 roadwheel per side) and panics naming what's absent. This finally *enforces* ADR-0002's "name = the contract".

The spec is a **load dependency**, not a thing applied after the fact: the tank is spawned only once its `.tank.ron` has loaded (gated by `AppState::Loading`), so `on_tank_ready` binds the spec onto the rig in a single pass with the data already in hand — there is no spec-less window where the sim could run on absent stats. (A dynamic body still has no collider for the few frames between spawn and the glTF mesh finishing load; that is inherent to async scene loading, not a missing-data fallback.)

## Considered Options

- **Graceful degrade to code defaults (rejected).** A tank that loads on default tuning is playable but *wrong*, and silently so — exactly the unfair, hard-to-diagnose state a competitive sim must not ship. "Default stats" has no honest per-variant meaning.
- **Debug-only panic, release degrade (rejected for required data).** Tempting for crash-averse UX, but for a *required* invariant it just ships a broken (again, unfair) experience with no signal. Reserved only for genuinely optional/recoverable assets — of which there are none yet.
- **Fail fast, all builds (chosen).** A panic with a precise message is debuggable from a player report; a silently-wrong tank is not. This is the idiomatic Rust stance: a violated invariant is a bug, not a recoverable runtime condition (`expect`-style), and a required in-repo asset that fails to load *is* a violated invariant.

## Mass properties are authored, not derived from the collision proxy

A corollary the same principle forces: the hull's **mass, centre of mass, and angular inertia are authored** (RON `mass` + `inertia_extents`, and the `Center_Of_Mass` empty), and the hull carries `NoAutoMass` / `NoAutoAngularInertia` / `NoAutoCenterOfMass`. The `*_Collider` proxies — and the future turret ramming collider — are therefore **collision-only** and contribute *zero* mass. The proxy is an abstract envelope; its volume and centroid must not silently set the tank's weight or balance.

This surfaced a real latent bug: previously `CenterOfMass` was set on the (mass-less) hull root *without* `NoAutoCenterOfMass`. Avian's `ComputedCenterOfMass` is a strictly mass-weighted average, so the authored point had weight 0 and was ignored — the body's COM was the proxy *centroid* (measured ~17 cm above the authored point). Adding `NoAutoCenterOfMass` makes the authored COM authoritative (verified: computed COM now equals the empty exactly). Inertia is approximated as a box of the authored extents at the authored mass — distribution from the box, magnitude from the real mass — never from the proxy.

## Consequences

- The "the model owns it" claims — COM ([[0005-raycast-roadwheel-locomotion]]), geometry ([[0008-collision-convex-proxies]]), the spec sheet ([[0010-per-variant-data-in-ron]]) — are now *enforced*, not merely intended.
- A missing/renamed node or a malformed spec crashes immediately at load, caught in dev/CI rather than shipped silently.
- Genuinely optional future data must opt *out* of this policy explicitly (and say why), rather than defaulting to silence.
