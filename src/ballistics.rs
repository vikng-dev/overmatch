//! Ballistics: the shared shell mechanic. Spawn a kinematic shell, integrate gravity, raycast the
//! terrain along each step, and emit an `Impact`. This is the library seam both the player's gun
//! (`shooting`) and the armor sandbox (`bin/armor_sandbox`) drive: they raise a `FireShell` event;
//! ballistics owns the trajectory and the impact query. Hand-integrated on purpose — we own the
//! trajectory (muzzle velocity, gravity, later drag/penetration as data + rules); Avian only answers
//! the impact query: what the segment hit, where, and the surface normal.
//!
//! The armor penetration march, ballistic volumes, and spall (design doc
//! `.agents/docs/design/armor-penetration-and-damage.md`) grow off the `Impact` seam here.

use avian3d::prelude::{Forces, LayerMask, SpatialQuery, SpatialQueryFilter, WriteRigidBodyForces};
use bevy::prelude::*;

use crate::damage::VolumeOf;
use crate::state::GameplaySet;
use crate::{ClientReplica, Layer};

/// Gravity applied to shells each fixed tick (m/s²).
const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

/// World-floor height (m): a shell that descends past this has cleared the map edge into the void
/// below the terrain and is culled. Gravity guarantees every shell reaches it within seconds unless
/// it hits terrain first — and an in-play arc always does, impacting the ground well above this — so
/// this only removes the escapees that would otherwise integrate forever (the never-despawn leak),
/// with zero effect on any legitimate shot (including straight-up or lobbed shells, which come back
/// down onto terrain). Far below the lowest terrain (the ~0 m slab). A shell can't reach it via
/// f32 overflow instead: max reach is ~10^5 m (gravity bounds the apex), ~33 orders below `f32::MAX`.
const KILL_FLOOR: f32 = -100.0;

/// Lumped drag-form constant for the quadratic air-drag model `dv/dt = −k·v²`. The per-shell
/// coefficient is `k = DRAG_FORM · caliber²/mass` (1/m): `caliber²/mass` is the shell's (inverse)
/// sectional density, so a heavy-for-bore round (the 88) holds velocity while a light-for-bore one
/// (the 7.9 mm coax) bleeds it. Calibrated so the 88 (0.088 m, 10.2 kg) keeps its hand-tuned
/// k ≈ 2e-4 — which, from sectional density alone, makes the coax bleed ~7× faster with no per-weapon
/// field. A per-shell form factor (shape: pointed AP vs APCR vs ball) joins the shell data later.
/// Sandbox-tunable.
const DRAG_FORM: f32 = 0.263;

/// A shell's quadratic-drag coefficient `k` (1/m), from its (inverse) sectional density. Shared by
/// the live shell and the fire-control range table so the aim solution and the actual flight bleed
/// speed identically — penetration `capability` (∝ vⁿ) then falls with range for both.
pub fn drag_k(caliber: f32, mass: f32) -> f32 {
    DRAG_FORM * caliber * caliber / mass
}

/// One free-flight integration step: apply gravity, then quadratic drag, returning the new velocity.
/// Drag is integrated analytically (`v ← v/(1 + k·v·dt)`, unconditionally stable, unlike explicit
/// Euler at high `v·dt`). This is the shared flight kernel — the live shell march
/// ([`integrate_projectiles`]) and the fire-control range table both step it, so a shell lands where
/// the superelevation solution said it would. In-plate cost dwarfs drag, so this is free-flight only.
pub fn freeflight_step(velocity: Vec3, drag_k: f32, dt: f32) -> Vec3 {
    let v = velocity + GRAVITY * dt;
    let speed = v.length();
    if speed == 0.0 {
        return v;
    }
    (v / speed) * (speed / (1.0 + drag_k * speed * dt))
}

/// Penetration capability: `pen = K · mass^Mₑ · speed^N` (reference-mm — the DeMarre shape, design
/// doc §3). **Mass is the primary driver** (sectional density / kinetic energy), speed the secondary;
/// caliber is deliberately *not* here — it drives overmatch and spall hole-size, not raw penetration.
/// Calibrated so the 88 (≈10.2 kg PzGr at ~773 m/s) ≈ 250 mm — *identical to the old speed-only curve
/// at that mass*, so the existing 88 behaviour is unchanged; the mass term only separates other
/// rounds (a ~13 g rifle/MG round lands ~10 mm → can't defeat real armour, only chips exposed parts).
/// Per-shell constants become shell data later.
const PEN_K: f32 = 0.005_8;
const PEN_N: f32 = 1.43;
/// Exponent on projectile mass (kg). ~0.5 ≈ sectional-density-like — the lever that separates a heavy
/// tank shell (deep) from light small arms (shallow). Sandbox-tunable.
const MASS_EXP: f32 = 0.5;

/// Reference-mm a projectile of `mass` kg can defeat at `speed` m/s.
fn capability(mass: f32, speed: f32) -> f32 {
    PEN_K * mass.powf(MASS_EXP) * speed.powf(PEN_N)
}

/// Inverse of [`capability`] for a fixed projectile: the speed carrying `capability` reference-mm at
/// this `mass`. Spending cost then inverting is the Lambert–Jonas residual-velocity shape —
/// barely-penetrate exits slow, big overmatch barely slows (design doc §3).
fn speed_for(mass: f32, capability: f32) -> f32 {
    (capability / (PEN_K * mass.powf(MASS_EXP))).powf(1.0 / PEN_N)
}

/// Fragment directions for a spall cone, each paired with its normalized polar position `t` ∈ [0,1]
/// (0 = on-axis): `n` rays inside a cone of half-angle `half_angle` about `axis`, spread by the
/// golden angle and packed denser toward the axis (design §5). `t` lets the caller make on-axis
/// fragments stronger — the continuous form of War Thunder's "more power ↔ narrower cone" groups.
/// Deterministic: the same shot throws the same cone (A/B in the sandbox).
fn spall_directions(axis: Dir3, half_angle: f32, n: usize) -> Vec<(Dir3, f32)> {
    let z = Vec3::from(axis);
    let up = if z.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
    let x = z.cross(up).normalize();
    let y = z.cross(x);
    const GOLDEN: f32 = 2.399_963_2;
    (0..n)
        .filter_map(|k| {
            let t = (k as f32 + 0.5) / n as f32;
            let polar = half_angle * t;
            let az = k as f32 * GOLDEN;
            let local = z * polar.cos() + (x * az.cos() + y * az.sin()) * polar.sin();
            Dir3::new(local).ok().map(|d| (d, t))
        })
        .collect()
}

/// Max RHA-mm an on-axis fragment can defeat at full shot energy (WT puts secondary fragments at
/// 3–30 mm RHA). Scaled down by off-axis angle and residual energy at birth.
const FRAG_PEN_MAX: f32 = 30.0;
/// Fragment air drag (1/m): a fragment's penetration bleeds with distance — low mass + tumbling, so
/// steep. Lethal point-blank behind the plate, nearly spent a few metres on (the BAD short range).
const FRAG_DRAG: f32 = 0.6;
/// HP a fragment deposits per RHA-mm of its current penetration at the moment of impact.
const FRAG_DMG_PER_MM: f32 = 0.12;

/// March one spall fragment as a mini-penetrator: it flies to the first ballistic volume, deposits
/// damage scaled by its current penetration (an energy packet), and either punches through a thin
/// volume (losing the cost it spent) or stops in a thick one — so the engine block still shadows the
/// crew, but a thin bulkhead no longer fully protects them and a strong fragment can exit the tank
/// to reach another (design §5). `pen` bleeds with distance (drag). Returns the visual trace.
fn cast_spall_fragment(
    origin: Vec3,
    dir: Dir3,
    mut pen: f32,
    mut range: f32,
    spatial: &SpatialQuery,
    volumes: &Query<&BallisticVolume>,
    parents: &Query<&ChildOf>,
    health: &mut Query<&mut ComponentHealth>,
    filter: &SpatialQueryFilter,
    // Authority-only HP deposition: `false` on the net client (a replica), which still traces the
    // fragment (for FX / `deposited`) but leaves the actual HP write to the server.
    deposit: bool,
) -> SpallFragment {
    const EPS: f32 = 1.0e-3;
    const PROBE: f32 = 50.0;
    let mut pos = origin;
    let mut deposited = false;
    while range > EPS {
        let Some(hit) = spatial.cast_ray(pos, dir, range, true, filter) else {
            pos += Vec3::from(dir) * range; // flew the rest, hit nothing
            break;
        };
        let at = pos + Vec3::from(dir) * hit.distance;
        pen = (pen / (1.0 + FRAG_DRAG * hit.distance)).max(0.0); // drag over the gap
        // Resolve the struck volume's node + material factor (walk up from the mesh primitive).
        let mut probe = hit.entity;
        let mut node = None;
        loop {
            if let Ok(v) = volumes.get(probe) {
                node = Some((probe, v.material_factor));
                break;
            }
            match parents.get(probe) {
                Ok(parent) => probe = parent.parent(),
                Err(_) => break,
            }
        }
        let Some((node_entity, factor)) = node else {
            pos = at;
            break;
        };
        // Deposit damage scaled by current penetration (energy), if it's a damageable component.
        // `deposited` still records the hit (the visual trace) on a replica; only the HP write is
        // authority-gated.
        if let Ok(mut hp) = health.get_mut(node_entity) {
            if deposit {
                hp.current = (hp.current - pen * FRAG_DMG_PER_MM).max(0.0);
            }
            deposited = true;
        }
        // Cost to cross this volume = its thickness along the fragment path × material factor.
        let span = spatial
            .cast_ray_predicate(
                at + Vec3::from(dir) * EPS,
                dir,
                PROBE,
                false,
                filter,
                &|e| e == hit.entity,
            )
            .map(|exit| EPS + exit.distance)
            .unwrap_or(0.0);
        let cost = span * factor;
        if pen > cost {
            // Punch through: spend the crossing cost and continue from the far face.
            pen -= cost;
            pos = at + Vec3::from(dir) * (span + EPS);
            range -= hit.distance + span + EPS;
        } else {
            // Stops inside this volume (depth scaled by the fraction it could pay).
            pos = at + Vec3::from(dir) * span * (pen / cost.max(EPS));
            break;
        }
    }
    SpallFragment {
        end: pos,
        deposited,
    }
}

/// Mirror a travel direction about a surface normal — the specular deflection of a ricochet.
fn reflect(dir: Dir3, normal: Dir3) -> Dir3 {
    let d = Vec3::from(dir);
    let n = Vec3::from(normal);
    Dir3::new(d - 2.0 * d.dot(n) * n).unwrap_or(dir)
}

/// Rotate `dir` toward `target` by `angle` radians (clamped to the angle between them). Used to bend
/// the penetrator toward the inward normal on entry — normalization.
fn bend_toward(dir: Dir3, target: Dir3, angle: f32) -> Dir3 {
    let d = Vec3::from(dir);
    let t = Vec3::from(target);
    let between = d.angle_between(t);
    if between < 1.0e-5 || angle <= 0.0 {
        return dir;
    }
    let Ok(axis) = Dir3::new(d.cross(t)) else {
        return dir;
    };
    Dir3::new(Quat::from_axis_angle(Vec3::from(axis), angle.min(between)) * d).unwrap_or(dir)
}

/// Whether a spent shell freezes in place — keeping its stuck mesh, tracer, and penetration marks
/// for inspection — instead of despawning. The game despawns (default); the sandbox opts in.
#[derive(Resource, Default)]
pub struct RetainSpentShells(pub bool);

/// How the shell march is integrated. The game uses `Real`; the sandbox can toggle to `Demo`.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarchMode {
    /// On the fixed server-rate step (`Time<Fixed>`) — the true simulation cadence, so slow-motion
    /// shows the actual discrete hops.
    #[default]
    Real,
    /// Per-frame on virtual time (`Time<Virtual>`) — smooth continuous motion for demoing
    /// (frame-rate dependent; the velocity, hence penetration, is unchanged).
    Demo,
}

fn march_real(mode: Res<MarchMode>) -> bool {
    *mode == MarchMode::Real
}

fn march_demo(mode: Res<MarchMode>) -> bool {
    *mode == MarchMode::Demo
}

/// Fire a shell — the trigger-agnostic seam. The player's gun and the sandbox camera both raise
/// this; ballistics spawns and integrates the shell. Geometry only — origin, bore direction, muzzle
/// speed — so it carries no assumption about *what* fired it.
#[derive(Event)]
pub struct FireShell {
    pub origin: Vec3,
    pub direction: Dir3,
    pub speed: f32,
    /// Shell calibre (m) — drives overmatch (a round whose calibre dwarfs a plate can't be
    /// deflected by it) and spall hole-size, *not* raw penetration.
    pub caliber: f32,
    /// Projectile mass (kg) — the primary driver of penetration capability (design §3).
    pub mass: f32,
    /// The tank root that fired this shell, or `None` for trigger sources with no tank (the
    /// sandbox's free-fly camera). Ballistics ignores it — `on_fire_shell` just spawns the shell —
    /// but the net server's `FireShell` observer reads it to broadcast the cosmetic tracer to the
    /// OTHER clients (`net::server`, the "FireEvent" seam): a shot whose shooter is known can be
    /// attributed to the right replicated tank; `None` shots (sandbox) simply never broadcast. Only
    /// `net::server` reads this, so it is dead code in a build without the `net` feature (SP /
    /// sandbox) — allow it there rather than drop a field the wire seam depends on.
    #[cfg_attr(not(feature = "net"), allow(dead_code))]
    pub shooter: Option<Entity>,
}

/// A shell in flight. Kinematic — integrated by hand, no physics engine.
#[derive(Component)]
struct Projectile {
    velocity: Vec3,
    caliber: f32,
    mass: f32,
    /// Quadratic-drag coefficient (1/m), from the shell's sectional density at spawn (see [`drag_k`]).
    drag_k: f32,
}

/// The shell's flight path, accumulated one point per step — the data the sandbox's tracer gizmo
/// draws. Public so inspection tooling can read it; the game simply doesn't draw it. The growing
/// `Vec` is freed when the shell despawns on impact.
#[derive(Component, Default)]
pub struct ShellPath {
    pub points: Vec<Vec3>,
}

/// A ballistic volume: a solid the penetrator marches *through*, taxing it over the geometric
/// line-of-sight distance (the unified primitive — armor plates and modules alike, design doc §2).
/// On the `Armor` layer. `material_factor` (density/hardness → reference-mm per metre) is authored;
/// the march doesn't spend it yet — that is the next increment.
#[derive(Component)]
pub struct BallisticVolume {
    pub material_factor: f32,
}

/// Role tags layered on a ballistic volume for the sandbox's visibility passes: armor plates vs
/// internal components (modules / crew / ammo). Attached at bind alongside `BallisticVolume`; the
/// game ignores them.
#[derive(Component)]
pub struct ArmorVolume;

#[derive(Component)]
pub struct ComponentVolume;

/// A component's HP pool (crew/module/ammo). A spall fragment deposits 1; the main penetrator
/// transiting deposits many (scaled by the cost it paid crossing — design §6). `current` clamps at
/// 0; the *consequences* of reaching 0 (cookoff, crew death, knock-out) are later increments (§§7–8).
#[derive(Component)]
pub struct ComponentHealth {
    pub current: f32,
    pub max: f32,
}

/// One crossing of a ballistic volume by the penetrator: where it entered and exited the solid.
/// `(exit - entry).length()` is the geometric line-of-sight thickness — slope captured by geometry,
/// no cosine term (design doc §2).
pub struct PenetrationEvent {
    pub entry: Vec3,
    pub exit: Vec3,
    /// Whether this crossing was an overmatch (calibre ≫ plate thickness): ricochet suppressed,
    /// slope largely cancelled.
    pub overmatched: bool,
}

/// The volume crossings a shell has made this flight — what the sandbox draws to inspect the march.
/// Public, like `ShellPath`; freed when the shell despawns.
#[derive(Component, Default)]
pub struct PenetrationMarks {
    pub events: Vec<PenetrationEvent>,
    /// Points where the shell ricocheted off a too-oblique face (deflected, did not enter).
    pub ricochets: Vec<Vec3>,
}

/// A single spall fragment's trace: where it stopped, and whether it deposited HP (hit a component)
/// or merely shadowed / flew on (hit armor or air). Carries 1 HP; no penetration of its own (§5).
pub struct SpallFragment {
    pub end: Vec3,
    pub deposited: bool,
}

/// One spall event — the cone thrown from a perforation exit. Origin + axis + half-angle describe
/// the fixed-shape cone; `fragments` are the resolved rays the sandbox draws.
pub struct SpallBurst {
    pub origin: Vec3,
    pub axis: Dir3,
    pub half_angle: f32,
    pub fragments: Vec<SpallFragment>,
}

/// The spall a shell has thrown this flight — one burst per perforation exit. Public like
/// `PenetrationMarks`; freed when the shell despawns.
#[derive(Component, Default)]
pub struct SpallMarks {
    pub bursts: Vec<SpallBurst>,
}

/// Live per-shell readout for the sandbox's info layer — current speed (m/s) and remaining
/// penetration capability (reference-mm). Public; refreshed each step.
#[derive(Component, Default)]
pub struct ShellReadout {
    pub speed: f32,
    pub capability: f32,
}

/// Preloaded shell scene, cloned per shot rather than loaded each time.
#[derive(Resource)]
struct ProjectileAssets {
    scene: Handle<WorldAsset>,
}

/// A shell hit something — the seam the armor penetration march/spall and impact VFX hang off. The
/// hit's normal and struck entity are available from the raycast; add them here when a feature needs
/// them. Global event (the shell despawns), handled by the `on_impact` observer.
#[derive(Event)]
struct Impact {
    position: Vec3,
}

/// One crossing's share of a shell's momentum, handed to the struck volume's owning body:
/// `impulse = m·(v_in − v_out)`, applied at the crossing's entry `point`. The `on_hit_impulse`
/// observer applies it — so a hit *rocks* the tank in proportion to the momentum it actually
/// absorbed (a shell that stops shoves it most; a clean overpenetration barely nudges it).
#[derive(Event)]
struct HitImpulse {
    body: Entity,
    impulse: Vec3,
    point: Vec3,
}

/// Preloaded mesh+material for the debug impact marker, cloned per hit by `on_impact`.
#[derive(Resource)]
struct ImpactDebug {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

/// Tags the debug impact marker, so the sandbox's clear command can find and remove it.
#[derive(Component)]
pub struct ImpactMarker;

pub fn plugin(app: &mut App) {
    app.init_resource::<RetainSpentShells>()
        .init_resource::<MarchMode>()
        .add_observer(on_fire_shell)
        .add_observer(on_impact)
        .add_observer(on_hit_impulse)
        .add_systems(Startup, setup_assets)
        // The same march, integrated on whichever clock the mode selects: `Real` on the fixed
        // server step (`Res<Time>` is `Time<Fixed>` here), `Demo` per-frame on virtual time
        // (`Res<Time>` is `Time<Virtual>` here). One reads as the true sim, the other as smooth.
        .add_systems(
            FixedUpdate,
            integrate_projectiles.in_set(GameplaySet).run_if(march_real),
        )
        .add_systems(
            Update,
            integrate_projectiles.in_set(GameplaySet).run_if(march_demo),
        );
}

fn setup_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Preload once; firing clones the handle rather than hitting the asset server per shot.
    commands.insert_resource(ProjectileAssets {
        scene: asset_server.load(GltfAssetLabel::Scene(0).from_asset("shell/shell.glb")),
    });
    // Small red sphere reused for every impact marker.
    commands.insert_resource(ImpactDebug {
        mesh: meshes.add(Sphere::new(0.2)),
        material: materials.add(Color::srgb(1.0, 0.3, 0.1)),
    });
}

/// Spawn a shell from a `FireShell`: at the origin, oriented down the bore, with velocity along the
/// bore at the muzzle speed.
fn on_fire_shell(fire: On<FireShell>, assets: Res<ProjectileAssets>, mut commands: Commands) {
    commands.spawn((
        Projectile {
            velocity: fire.direction * fire.speed,
            caliber: fire.caliber,
            mass: fire.mass,
            drag_k: drag_k(fire.caliber, fire.mass),
        },
        ShellPath {
            points: vec![fire.origin],
        },
        PenetrationMarks::default(),
        SpallMarks::default(),
        ShellReadout {
            speed: fire.speed,
            capability: capability(fire.mass, fire.speed),
        },
        WorldAssetRoot(assets.scene.clone()),
        Transform::from_translation(fire.origin).looking_to(fire.direction, Vec3::Y),
    ));
}

fn integrate_projectiles(
    mut projectiles: Query<(
        Entity,
        &mut Transform,
        &mut Projectile,
        &mut ShellPath,
        &mut PenetrationMarks,
        &mut ShellReadout,
        &mut SpallMarks,
    )>,
    volumes: Query<&BallisticVolume>,
    owners: Query<&VolumeOf>,
    mut health: Query<&mut ComponentHealth>,
    parents: Query<&ChildOf>,
    retain: Res<RetainSpentShells>,
    // Present only on the net client (a replica): shells still fly, raycast, spark, and despawn, but
    // HP deposition and hit impulse are the server's authority. Absent in SP / sandbox / server.
    replica: Option<Res<ClientReplica>>,
    spatial: SpatialQuery,
    time: Res<Time>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    // Authority = not a replica: only then does a hit actually mutate health here.
    let deposit = replica.is_none();
    // The march casts against terrain (which stops the shell) and ballistic volumes (which it
    // crosses); the struck entity being a `BallisticVolume` is what tells the two apart.
    let world = SpatialQueryFilter::from_mask(
        LayerMask::from(Layer::Terrain) | LayerMask::from(Layer::Armor),
    );
    let armor = SpatialQueryFilter::from_mask(Layer::Armor);
    // Nudge past each boundary we resolve so we don't immediately re-hit it.
    const EPS: f32 = 1.0e-3;
    // How far ahead to search for a volume's far face — its full geometric thickness, even past the
    // end of this step (thin plates resolve well within it).
    const PROBE: f32 = 50.0;
    // Steeper than this from the surface normal, an un-overmatched round ricochets (rad, ~70°).
    const RICOCHET_ANGLE: f32 = 1.221;
    // Speed retained through a ricochet.
    const RICOCHET_BLEED: f32 = 0.6;
    // Shock a glancing bounce jars into an *exposed component* (not armor): scaled by impact energy
    // (capability) × squareness (cos incidence). A graze chips structural integrity without one-
    // shotting; a faint graze barely registers; small arms barely scratch. Armor has no HP → shrugs.
    const SHOCK_K: f32 = 0.045;
    // Share of the impact angle the round straightens toward the normal on entry (normalization).
    const NORMALIZATION: f32 = 0.2;
    // Overmatch when calibre ≥ this × the plate's thickness: ricochet suppressed, slope cancelled.
    const OVERMATCH_RATIO: f32 = 3.0;
    // Spall (design §5). Budget = (material chewed / ref) × (residual energy / ref) × (caliber /
    // ref), capped — both a fragment supply (cost) and a push (v_res²) are needed, so a thin/soft
    // body or a barely-through round throws little. The cone's shape is fixed; only density scales.
    const SPALL_MAX_FRAGMENTS: usize = 24;
    const SPALL_COST_REF: f32 = 100.0; // ref-mm (≈ a 100 mm steel plate)
    const SPALL_VRES_REF: f32 = 500.0; // m/s
    const SPALL_CALIBER_REF: f32 = 0.088; // m (the 88)
    const SPALL_HALF_ANGLE: f32 = 0.35; // rad (~20°)
    const SPALL_RANGE: f32 = 6.0; // m — fragments are short-range
    // Main-penetrator transit damage = cost paid crossing the component × this (design §6).
    const TRANSIT_K: f32 = 1.0;

    for (entity, mut transform, mut projectile, mut path, mut marks, mut readout, mut spall) in
        &mut projectiles
    {
        // Advance free-flight velocity (gravity + drag) through the shared flight kernel, so the live
        // shell bleeds speed identically to the fire-control table that aimed it. The march below may
        // *bend* the direction (normalization / ricochet), so we carry direction + speed and rebuild
        // the velocity at the end rather than assuming a straight segment.
        let stepped = freeflight_step(projectile.velocity, projectile.drag_k, dt);
        let Ok(mut dir) = Dir3::new(stepped) else {
            continue;
        };
        let mut speed = stepped.length();
        let mut pos = transform.translation;
        let mut remaining = speed * dt;
        let mut stopped = false;

        // Ray-march the step: free flight until a surface, then resolve it — terrain stops the
        // shell; a ballistic volume ricochets (too oblique) or is crossed (normalize → spend cost →
        // perforate or embed) — and keep marching the leftover budget along the new direction.
        while remaining > EPS {
            let origin = pos + dir * EPS;
            let Some(hit) = spatial.cast_ray(origin, dir, remaining, true, &world) else {
                pos += dir * remaining; // open air — fly out the rest of the step
                break;
            };
            let entry = origin + dir * hit.distance;
            let travelled = EPS + hit.distance;

            // The hit lands on the collider's mesh-primitive entity; the `BallisticVolume` sits on its
            // named parent node — walk up to find it, keeping the node entity so transit damage and
            // spall can address the component. No volume in the ancestry ⇒ terrain.
            let mut probe = hit.entity;
            let mut resolved = None;
            loop {
                if let Ok(found) = volumes.get(probe) {
                    resolved = Some((probe, found.material_factor));
                    break;
                }
                match parents.get(probe) {
                    Ok(parent) => probe = parent.parent(),
                    Err(_) => break,
                }
            }
            let Some((node_entity, factor)) = resolved else {
                // Terrain: stop here.
                commands.trigger(Impact { position: entry });
                pos = entry;
                stopped = true;
                break;
            };

            // Momentum bookkeeping for this crossing: the incoming velocity (before any bend/bleed)
            // and the body that owns the struck volume. Each resolution branch below hands the body
            // its share of the shell's momentum, `m·(v_in − v_out)` — a shell that stops dumps it all,
            // a perforation less (it carries momentum out), a ricochet a partial normal-ward kick.
            let v_in = Vec3::from(dir) * speed;
            let body = owners.get(node_entity).ok().map(|owner| owner.tank());

            // Outward surface normal; angle of incidence is measured from it (0 = head-on).
            let normal = Dir3::new(hit.normal).unwrap_or(-dir);
            let incidence = Vec3::from(dir).angle_between(-Vec3::from(normal));

            // Plate thickness *along its normal* (perpendicular, face to face) — the overmatch test:
            // a round whose calibre dwarfs the plate cannot be deflected by it.
            let thickness = spatial
                .cast_ray_predicate(
                    entry - Vec3::from(normal) * EPS,
                    -normal,
                    PROBE,
                    false,
                    &armor,
                    &|e| e == hit.entity,
                )
                .map(|back| EPS + back.distance)
                .unwrap_or(0.0);
            let overmatched = thickness > 0.0 && projectile.caliber >= OVERMATCH_RATIO * thickness;

            // Ricochet: too oblique → deflect off the face (no entry, no spall) — unless overmatch
            // suppresses it (design §4).
            if !overmatched && incidence > RICOCHET_ANGLE {
                // Shock: even a deflected hit jars an exposed component (barrel, optic) — scaled by
                // impact energy (capability) and how square the graze was. Armor has no HP, so it
                // shrugs the bounce off; a fragile module loses integrity without being one-shot.
                if deposit && let Ok(mut hp) = health.get_mut(node_entity) {
                    let shock = SHOCK_K * capability(projectile.mass, speed) * incidence.cos();
                    hp.current = (hp.current - shock).max(0.0);
                }
                dir = reflect(dir, normal);
                speed *= RICOCHET_BLEED;
                if let Some(body) = body {
                    commands.trigger(HitImpulse {
                        body,
                        impulse: projectile.mass * (v_in - Vec3::from(dir) * speed),
                        point: entry,
                    });
                }
                marks.ricochets.push(entry);
                path.points.push(entry);
                pos = entry;
                remaining -= travelled;
                continue;
            }

            // Normalize: a modest bend toward the inward normal as the round bites in (shortens the
            // path it cuts and nudges the exit). Overmatch does NOT bend it further — the round drives
            // through in roughly the same direction; overmatch instead cancels the *slope cost* below.
            dir = bend_toward(dir, -normal, NORMALIZATION * incidence);
            let span = spatial
                .cast_ray_predicate(entry + dir * EPS, dir, PROBE, false, &armor, &|e| {
                    e == hit.entity
                })
                .map(|exit| EPS + exit.distance)
                .unwrap_or(0.0);

            // Cost = effective metres × the material's reference-mm-per-metre. An overmatched plate
            // can't present its oblique line-of-sight to a round that dwarfs it, so it charges only
            // the perpendicular thickness; otherwise the full slope span.
            let cap = capability(projectile.mass, speed);
            let effective = if overmatched { thickness } else { span };
            let cost = effective * factor;
            if cap <= cost {
                // Defeated: embed partway through (depth scaled by the capability it could pay).
                let embed = entry + dir * span * (cap / cost);
                marks.events.push(PenetrationEvent {
                    entry,
                    exit: embed,
                    overmatched,
                });
                path.points.push(embed);
                // It buried itself here, spending all it had (`cap`) — deposit that as transit damage
                // if the volume is a damageable component (design §6). No exit, so no spall.
                if deposit && let Ok(mut hp) = health.get_mut(node_entity) {
                    hp.current = (hp.current - cap * TRANSIT_K).max(0.0);
                }
                commands.trigger(Impact { position: embed });
                // Stopped: the body absorbs the full remaining momentum (v_out = 0).
                if let Some(body) = body {
                    commands.trigger(HitImpulse {
                        body,
                        impulse: projectile.mass * v_in,
                        point: entry,
                    });
                }
                pos = embed;
                stopped = true;
                break;
            }

            // Perforate: spend the cost (residual speed) and continue along the bent direction.
            speed = speed_for(projectile.mass, cap - cost);
            // The body keeps the momentum the shell lost crossing it; the shell carries the rest on.
            if let Some(body) = body {
                commands.trigger(HitImpulse {
                    body,
                    impulse: projectile.mass * (v_in - Vec3::from(dir) * speed),
                    point: entry,
                });
            }
            let exit = entry + dir * span;
            marks.events.push(PenetrationEvent {
                entry,
                exit,
                overmatched,
            });
            path.points.push(exit);

            // Transit damage: the main penetrator drove through this volume — if it's a damageable
            // component, deposit the cost it paid crossing (design §6). Armor has no HP, so no-op.
            if deposit && let Ok(mut hp) = health.get_mut(node_entity) {
                hp.current = (hp.current - cost * TRANSIT_K).max(0.0);
            }

            // Spall: the exit face throws a cone of fragments. The *count* comes from the material
            // chewed (cost) and the hole size (caliber) — the fragment supply; each fragment's
            // *energy* comes from the shot's residual (v_res²) and its position in the cone (on-axis
            // strongest). So a thin/soft body throws few fragments and a barely-through round throws
            // weak ones — both extremes low (design §5). Each fragment then penetrates per its energy.
            let count_f = SPALL_MAX_FRAGMENTS as f32
                * (cost / SPALL_COST_REF)
                * (projectile.caliber / SPALL_CALIBER_REF);
            let count = (count_f.round() as i32).clamp(0, SPALL_MAX_FRAGMENTS as i32) as usize;
            if count > 0 {
                // Residual energy sets how hard each fragment is thrown (full at the reference exit
                // speed); the on-axis fragments (`t→0`) keep the most of it.
                let shot_energy = (speed / SPALL_VRES_REF).powi(2).clamp(0.0, 1.0);
                let mut burst = SpallBurst {
                    origin: exit,
                    axis: dir,
                    half_angle: SPALL_HALF_ANGLE,
                    fragments: Vec::with_capacity(count),
                };
                for (fdir, t) in spall_directions(dir, SPALL_HALF_ANGLE, count) {
                    let birth_pen = FRAG_PEN_MAX * shot_energy * (1.0 - t);
                    burst.fragments.push(cast_spall_fragment(
                        exit + Vec3::from(fdir) * EPS,
                        fdir,
                        birth_pen,
                        SPALL_RANGE,
                        &spatial,
                        &volumes,
                        &parents,
                        &mut health,
                        &armor,
                        deposit,
                    ));
                }
                spall.bursts.push(burst);
            }

            pos = exit;
            remaining -= travelled + span;
        }

        // Reorient the shell to its travel direction so the mesh follows the (gravity-curved,
        // ricochet-bent) path instead of holding its launch heading.
        transform.translation = pos;
        transform.look_to(dir, Vec3::Y);
        path.points.push(pos);

        if stopped {
            if retain.0 {
                // Sandbox: freeze where it stopped — drop the live components so it is no longer
                // integrated or labelled, but keep the stuck shell, its path, and its penetration
                // marks on screen for study (the sandbox's `c` command clears them).
                commands
                    .entity(entity)
                    .remove::<(Projectile, ShellReadout)>();
            } else {
                // Game: the spent shell is done.
                commands.entity(entity).despawn();
            }
        } else if pos.y < KILL_FLOOR {
            // Left the world: cleared the map edge and fell into the void below the terrain. Despawn
            // outright — there is no impact to inspect, so this ignores the sandbox's retain (unlike a
            // real impact). This is what bounds a shell that never hits terrain; see `KILL_FLOOR`.
            commands.entity(entity).despawn();
        } else {
            projectile.velocity = Vec3::from(dir) * speed;
            readout.speed = speed;
            readout.capability = capability(projectile.mass, speed);
        }
    }
}

/// Apply a crossing's momentum share to the struck body (immediate velocity change; the off-CoM
/// entry point also imparts the angular rock). A static or non-rigid owner simply won't match.
fn on_hit_impulse(
    hit: On<HitImpulse>,
    // Authority-only: on the net client (a replica) the struck body's motion is server-owned and
    // arrives by replication — applying a local impulse here would fight it (a divergent shove).
    replica: Option<Res<ClientReplica>>,
    mut bodies: Query<Forces>,
) {
    if replica.is_some() {
        return;
    }
    if let Ok(mut forces) = bodies.get_mut(hit.body) {
        forces.apply_linear_impulse_at_point(hit.impulse, hit.point);
    }
}

fn on_impact(impact: On<Impact>, debug: Res<ImpactDebug>, mut commands: Commands) {
    info!("shell impact at {:?}", impact.position);
    // Debug marker for now; the armor penetration march/spall and impact VFX hook in here.
    commands.spawn((
        ImpactMarker,
        Mesh3d(debug.mesh.clone()),
        MeshMaterial3d(debug.material.clone()),
        Transform::from_translation(impact.position),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `drag_k` is calibrated so the 88 keeps its hand-tuned coefficient, and a light-for-bore round
    /// (the 7.9 mm coax) bleeds far faster from sectional density alone — the reason a coax drops more
    /// than the main gun at the same range, with no per-weapon drag field.
    #[test]
    fn drag_k_calibration() {
        let main = drag_k(0.088, 10.2); // 88 mm, 10.2 kg
        let coax = drag_k(0.0079, 0.0118); // 7.9 mm, 11.8 g
        assert!(
            (main - 2.0e-4).abs() < 1.0e-5,
            "88 drag k should be ≈ 2e-4, got {main}"
        );
        assert!(
            coax > 6.0 * main,
            "coax should bleed far faster than the 88 (got {coax} vs {main})"
        );
    }

    /// Drag only slows a shell — never speeds it up or reverses it — and gravity always pulls the
    /// vertical component down. Guards the analytic drag step against a sign or stability slip.
    #[test]
    fn freeflight_step_bleeds_speed_and_falls() {
        let v0 = Vec3::new(700.0, 0.0, 0.0);
        let v1 = freeflight_step(v0, drag_k(0.088, 10.2), 0.01);
        assert!(v1.length() < v0.length(), "drag must reduce speed");
        assert!(v1.y < 0.0, "gravity must pull the shell down");
    }
}
