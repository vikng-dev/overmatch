//! The debris stress lever: a dev-only instrument that prices server-simulated wall fragments,
//! `OVERMATCH_DEBRIS_STRESS=<pieces>:<period_s>[:<live_cap>[:<frozen_cap>]]` on the dedicated
//! server. Unset, empty, `off`, `false` or zero pieces is off, and an off server mounts none of it.
//!
//! Every `period_s` of fixed time the server releases one breach of `pieces` dynamic fragments
//! beside one tank, round-robin over every [`Tank`] root: a wall section [`WALL_OFFSET_M`] off the
//! hull's flank, its pieces 0–3 m above the ground and thrown toward the hull, so they land on and
//! against hulls as well as on the terrain. A piece collides with `Layer::Terrain` (the ground,
//! houses, trunks), `Layer::Vehicle` (the tanks' collision proxies) and the other pieces.
//!
//! THE FREEZE RULE ([`freeze_rested`]): a live piece whose speed stays under [`REST_SPEED_M_S`] and
//! spin under [`REST_SPIN_RAD_S`] for [`REST_S`], touching no dynamic body but other pieces,
//! becomes `RigidBody::Static`. Avian 0.7 then takes it out of the solver and the islands, moves its
//! proxy to the static tree, and drops every contact pair it held (`remove_body_on::<Insert,
//! RigidBody>` in its narrow phase), and its broad phase never pairs two static bodies: a frozen
//! piece on the ground costs the physics a static proxy and nothing per tick (its carrier is still
//! in replicon's per-send change scan). Avian's sleeping was rejected:
//! it sleeps whole islands, so neither the rule nor the cap could freeze one piece, and a piece
//! touching a tank shares the tank's island.
//! THE CAP: past `live_cap` live pieces ([`DEFAULT_LIVE_CAP`]; `0` is none), the oldest live pieces
//! freeze by the same switch, mid-air if need be, skipping a piece that touches a dynamic body.
//! THAW ([`thaw_touched`]): a frozen piece that a dynamic body other than a piece starts touching
//! turns dynamic again, so a tank pushes settled rubble rather than meeting a fixed block.
//! `frozen_cap` (`0` or absent: none) despawns the oldest frozen pieces past it.
//!
//! REPLICATION: each piece is a server-only body plus one replicated carrier entity holding its
//! [`DebrisPose`], written only while the piece is live and only when it moved (`set_if_neq`), so
//! a frozen piece's carrier stops changing and replicon sends it nothing more. A separate carrier,
//! because avian's `Position`/`Rotation`/velocities replicate from any replicated entity that holds
//! them (`protocol::plugin`): replicating the body itself would send four components per piece
//! per tick. Clients draw a carrier as a plain mesh at its pose ([`install_client`]); they run no
//! physics on it.
//!
//! The piece's shape and density are fixture numbers, not a measured stone: [`FRAGMENT_HULL`],
//! [`DENSITY_KG_M3`].

use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Layer;
use crate::tank::Tank;
use crate::terrain_grid::HeightGrid;

/// The lever.
const LEVER: &str = "OVERMATCH_DEBRIS_STRESS";

/// The live-piece cap when the lever names none. ASSUMED: one 200-piece breach and the tail of the
/// one before it.
const DEFAULT_LIVE_CAP: u32 = 256;

/// A pre-cut masonry fragment, the fixture: two irregular hexagons 0.4 m apart, the top one turned
/// 15°, about 0.53 × 0.49 × 0.40 m. Every one of the 12 points is a vertex of the hull.
const FRAGMENT_HULL: [Vec3; 12] = [
    Vec3::new(0.2700, -0.2, 0.0000),
    Vec3::new(0.1250, -0.2, 0.2165),
    Vec3::new(-0.1400, -0.2, 0.2425),
    Vec3::new(-0.2600, -0.2, 0.0000),
    Vec3::new(-0.1350, -0.2, -0.2338),
    Vec3::new(0.1200, -0.2, -0.2078),
    Vec3::new(0.2318, 0.2, 0.0621),
    Vec3::new(0.0673, 0.2, 0.2511),
    Vec3::new(-0.1626, 0.2, 0.1626),
    Vec3::new(-0.2415, 0.2, -0.0647),
    Vec3::new(-0.0569, 0.2, -0.2125),
    Vec3::new(0.1768, 0.2, -0.1768),
];

/// ASSUMED masonry density, kg/m³ (fixture).
const DENSITY_KG_M3: f32 = 1_800.0;

/// At rest: speed under this, m/s. The rest constants are avian 0.7's sleep defaults
/// (`SleepThreshold`, `TimeToSleep`), ASSUMED to suit a fragment.
const REST_SPEED_M_S: f32 = 0.15;
/// At rest: spin under this, rad/s.
const REST_SPIN_RAD_S: f32 = 0.15;
/// How long a piece stays at rest before it freezes, s.
const REST_S: f32 = 0.5;

/// Horizontal distance from the tank root to the breach's near face, m. Fixture: outside the hull's
/// flank for a tank up to ~6 m wide.
const WALL_OFFSET_M: f32 = 3.5;
/// The breach lattice's pitch, m: wider than the fragment's widest footprint at any release turn
/// (0.62 m, DERIVED: its 0.28 m outer radius plus the ±0.15 rad tilt's lean), so no two pieces start
/// overlapping.
const LATTICE_M: f32 = 0.65;
/// Rows of the breach: piece bottoms from 0.05 m to 2.65 m above the ground.
const ROWS: u32 = 5;
/// Columns along the wall at most; more pieces stack further layers outward.
const COLUMNS: u32 = 10;
/// A piece's lowest point above the ground at release, m.
const GROUND_CLEARANCE_M: f32 = 0.05;
/// The fragment's half height, m ([`FRAGMENT_HULL`]).
const HALF_HEIGHT_M: f32 = 0.2;

/// The lever's reading.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Lever {
    pieces: u32,
    period_s: f64,
    live_cap: Option<u32>,
    frozen_cap: Option<u32>,
}

impl Lever {
    /// `None` is off; a malformed value is an error, so a stress run never measures a server that
    /// silently ignored its lever.
    fn parse(value: Option<&str>) -> Result<Option<Self>, String> {
        let Some(value) = value.map(str::trim) else {
            return Ok(None);
        };
        if value.is_empty()
            || value.eq_ignore_ascii_case("off")
            || value.eq_ignore_ascii_case("false")
        {
            return Ok(None);
        }
        let malformed = || {
            format!("{LEVER}={value}: want <pieces>:<period_s>[:<live_cap>[:<frozen_cap>]], or off")
        };
        let fields: Vec<&str> = value.split(':').map(str::trim).collect();
        if !(2..=4).contains(&fields.len()) {
            return Err(malformed());
        }
        let count = |field: &str| field.parse::<u32>().map_err(|_| malformed());
        let pieces = count(fields[0])?;
        let period_s: f64 = fields[1].parse().map_err(|_| malformed())?;
        if !(period_s.is_finite() && period_s > 0.0) {
            return Err(malformed());
        }
        let live_cap = match fields.get(2) {
            Some(field) => Some(count(field)?).filter(|cap| *cap > 0),
            None => Some(DEFAULT_LIVE_CAP),
        };
        let frozen_cap = match fields.get(3) {
            Some(field) => Some(count(field)?).filter(|cap| *cap > 0),
            None => None,
        };
        Ok((pieces > 0).then_some(Self {
            pieces,
            period_s,
            live_cap,
            frozen_cap,
        }))
    }
}

/// A piece's authoritative world pose, on its replicated carrier.
#[derive(Component, Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct DebrisPose {
    pub(crate) position: Vec3,
    pub(crate) rotation: Quat,
}

impl DebrisPose {
    fn transform(self) -> Transform {
        Transform::from_translation(self.position).with_rotation(self.rotation)
    }
}

/// A server-side piece body: its release order and its replicated carrier.
#[derive(Component, Clone, Copy, Debug)]
struct DebrisPiece {
    seq: u64,
    carrier: Entity,
}

/// Seconds a live piece has spent at rest, reset by any motion over the rest thresholds.
#[derive(Component, Clone, Copy, Debug, Default)]
struct DebrisRest(f32);

/// A piece frozen by the freeze rule or the cap: `RigidBody::Static`, its pose final until a thaw.
#[derive(Component, Clone, Copy, Debug)]
struct Frozen;

/// The armed lever and its running counts.
#[derive(Resource)]
pub(super) struct DebrisStress {
    lever: Lever,
    collider: Collider,
    next_release_s: f64,
    breaches: u64,
    released: u64,
    rest_frozen: u64,
    cap_frozen: u64,
    thawed: u64,
    despawned: u64,
}

impl DebrisStress {
    /// The first breach goes out on the first fixed tick a tank stands.
    fn new(lever: Lever) -> Self {
        Self {
            lever,
            collider: fragment_collider(),
            next_release_s: 0.0,
            breaches: 0,
            released: 0,
            rest_frozen: 0,
            cap_frozen: 0,
            thawed: 0,
            despawned: 0,
        }
    }
}

/// The stats line's view of the pieces: present only while the lever is armed.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct DebrisCensus<'w, 's> {
    stress: Option<Res<'w, DebrisStress>>,
    live: Query<'w, 's, (), (With<DebrisPiece>, Without<Frozen>)>,
    frozen: Query<'w, 's, (), (With<DebrisPiece>, With<Frozen>)>,
}

impl DebrisCensus<'_, '_> {
    /// `live` and `frozen` pieces now; the cumulative counts since the server started: breaches,
    /// pieces released, frozen by the rest rule and by the cap, thawed, despawned by `frozen_cap`.
    pub(super) fn json(&self) -> Option<Value> {
        let stress = self.stress.as_ref()?;
        Some(json!({
            "live": self.live.iter().count(),
            "frozen": self.frozen.iter().count(),
            "breaches": stress.breaches,
            "released": stress.released,
            "rest_frozen": stress.rest_frozen,
            "cap_frozen": stress.cap_frozen,
            "thawed": stress.thawed,
            "despawned": stress.despawned,
        }))
    }
}

fn fragment_collider() -> Collider {
    Collider::convex_hull(FRAGMENT_HULL.to_vec()).expect("the fixture hull is not degenerate")
}

/// Arm the lever on the dedicated server. A malformed value is refused.
pub(super) fn install_server(app: &mut App) -> Result<(), String> {
    let Some(lever) = Lever::parse(std::env::var(LEVER).ok().as_deref())? else {
        return Ok(());
    };
    info!(
        "server: debris stress — a breach of {} pieces every {} s, live cap {:?}, frozen cap {:?} \
         ({LEVER})",
        lever.pieces, lever.period_s, lever.live_cap, lever.frozen_cap
    );
    app.insert_resource(DebrisStress::new(lever));
    app.add_systems(FixedUpdate, release_breaches);
    app.add_systems(
        FixedPostUpdate,
        (
            thaw_touched,
            publish_poses,
            freeze_rested,
            despawn_oldest_frozen,
        )
            .chain()
            .after(PhysicsSystems::StepSimulation),
    );
    Ok(())
}

/// Draw every arriving carrier as a plain mesh at its pose. Mounted by every client; a client
/// without mesh or material assets draws nothing.
pub(super) fn install_client(app: &mut App) {
    app.add_systems(Startup, build_view)
        .add_systems(Update, (attach_view, follow_pose).chain());
}

/// One piece of a breach, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Release {
    position: Vec3,
    rotation: Quat,
    velocity: Vec3,
    spin: Vec3,
}

/// A deterministic unit value in [0, 1) for piece `seq`'s draw `k` (splitmix64).
fn unit(seq: u64, k: u64) -> f32 {
    let mut z = seq
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(k.wrapping_mul(0xbf58_476d_1ce4_e5b9));
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// One breach's pieces beside a tank at `anchor` heading `heading`, on its `side` (+1 right, −1
/// left). `ground` is the surface height under an XZ, `None` off the map (that piece is skipped).
/// Pieces fill rows, then columns centred on the tank, then layers outward. The throw is fixture
/// data (ASSUMED, a wall section giving way): 1–3.5 m/s toward the hull, up to 0.8 m/s up, ±0.4 m/s
/// along the wall, up to 2 rad/s of spin per axis, any yaw and a ±0.15 rad tilt.
fn breach(
    anchor: Vec3,
    heading: Quat,
    side: f32,
    pieces: u32,
    first_seq: u64,
    ground: impl Fn(Vec2) -> Option<f32>,
) -> Vec<Release> {
    let right = heading * Vec3::X;
    let outward = Vec3::new(right.x, 0.0, right.z).normalize_or(Vec3::X) * side;
    let along = Vec3::Y.cross(outward);
    let columns = pieces.div_ceil(ROWS).clamp(1, COLUMNS);
    let centre = (columns - 1) as f32 * 0.5;
    (0..pieces)
        .filter_map(|i| {
            let seq = first_seq + u64::from(i);
            let (row, column, layer) = (i % ROWS, (i / ROWS) % columns, i / (ROWS * columns));
            let xz = anchor
                + outward * (WALL_OFFSET_M + layer as f32 * LATTICE_M)
                + along * ((column as f32 - centre) * LATTICE_M);
            let ground = ground(Vec2::new(xz.x, xz.z))?;
            let y = ground + GROUND_CLEARANCE_M + HALF_HEIGHT_M + row as f32 * LATTICE_M;
            let jitter = |k| unit(seq, k);
            Some(Release {
                position: Vec3::new(xz.x, y, xz.z),
                rotation: Quat::from_rotation_y(jitter(0) * std::f32::consts::TAU)
                    * Quat::from_rotation_x((jitter(1) - 0.5) * 0.3),
                velocity: -outward * (1.0 + 2.5 * jitter(2))
                    + Vec3::Y * (0.8 * jitter(3))
                    + along * (0.8 * jitter(4) - 0.4),
                spin: (Vec3::new(jitter(5), jitter(6), jitter(7)) * 2.0 - Vec3::ONE) * 2.0,
            })
        })
        .collect()
}

/// `FixedUpdate`: once the period has passed and a tank stands, release the next breach.
fn release_breaches(
    time: Res<Time>,
    mut stress: ResMut<DebrisStress>,
    tanks: Query<(&Position, &Rotation), With<Tank>>,
    grid: Option<Res<HeightGrid>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    if now < stress.next_release_s {
        return;
    }
    let count = tanks.iter().count() as u64;
    if count == 0 {
        return;
    }
    let Some((anchor, heading)) = tanks.iter().nth((stress.breaches % count) as usize) else {
        return;
    };
    // Each tank's flank alternates from one round of the tanks to the next.
    let side = if (stress.breaches / count).is_multiple_of(2) {
        1.0
    } else {
        -1.0
    };
    let ground = |xz: Vec2| match grid.as_deref() {
        Some(grid) => grid
            .contains_xz(xz.x, xz.y)
            .then(|| grid.max_height_in_square(xz.x, xz.y, HALF_HEIGHT_M + LATTICE_M * 0.5)),
        None => Some(0.0),
    };
    let releases = breach(
        anchor.0,
        heading.0,
        side,
        stress.lever.pieces,
        stress.released,
        ground,
    );
    stress.next_release_s = now + stress.lever.period_s;
    stress.breaches += 1;
    let layers = CollisionLayers::new(
        [Layer::Debris],
        [Layer::Terrain, Layer::Vehicle, Layer::Debris],
    );
    for release in releases {
        let seq = stress.released;
        stress.released += 1;
        let carrier = commands
            .spawn((
                Name::new("DebrisCarrier"),
                DebrisPose {
                    position: release.position,
                    rotation: release.rotation,
                },
                Replicate::to_clients(NetworkTarget::All),
            ))
            .id();
        // `Position`/`Rotation` before `RigidBody`: the body must never flush with avian's
        // placeholder pose.
        commands.spawn((
            Name::new("Debris"),
            DebrisPiece { seq, carrier },
            DebrisRest::default(),
            Position(release.position),
            Rotation(release.rotation),
            RigidBody::Dynamic,
            stress.collider.clone(),
            ColliderDensity(DENSITY_KG_M3),
            layers,
            LinearVelocity(release.velocity),
            AngularVelocity(release.spin),
        ));
    }
}

/// Whether `piece` touches a dynamic body that is not a piece: it rides or leans on a vehicle,
/// so freezing it would leave a fixed block in that vehicle's way.
fn touches_vehicle(
    piece: Entity,
    collisions: &Collisions,
    vehicles: &Query<&RigidBody, Without<DebrisPiece>>,
) -> bool {
    collisions.collisions_with(piece).any(|pair| {
        let other = if pair.body1 == Some(piece) {
            pair.body2
        } else {
            pair.body1
        };
        other
            .and_then(|body| vehicles.get(body).ok())
            .is_some_and(RigidBody::is_dynamic)
    })
}

/// The switch both the freeze rule and the cap make. `CollisionEventsEnabled` rides the same insert
/// so the static proxy avian rebuilds for the new body type carries contact events ([`thaw_touched`]
/// reads them); the velocities are zeroed so a thaw starts the piece from rest.
fn freeze(commands: &mut Commands, piece: Entity) {
    commands.entity(piece).insert((
        Frozen,
        CollisionEventsEnabled,
        LinearVelocity::ZERO,
        AngularVelocity::ZERO,
        RigidBody::Static,
    ));
}

/// The live pieces the cap freezes: oldest release first, past `cap`, skipping any for which
/// `riding` holds. `live` is `(seq, piece)`, sorted here.
fn past_cap(live: &mut [(u64, Entity)], cap: u32, riding: impl Fn(Entity) -> bool) -> Vec<Entity> {
    let excess = live.len().saturating_sub(cap as usize);
    if excess == 0 {
        return Vec::new();
    }
    live.sort_unstable_by_key(|(seq, _)| *seq);
    live.iter()
        .map(|(_, piece)| *piece)
        .filter(|piece| !riding(*piece))
        .take(excess)
        .collect()
}

/// `FixedPostUpdate`, after the step: THE FREEZE RULE, then THE CAP (module doc).
fn freeze_rested(
    time: Res<Time>,
    mut stress: ResMut<DebrisStress>,
    collisions: Collisions,
    vehicles: Query<&RigidBody, Without<DebrisPiece>>,
    mut live: Query<
        (
            Entity,
            &DebrisPiece,
            &mut DebrisRest,
            &LinearVelocity,
            &AngularVelocity,
        ),
        Without<Frozen>,
    >,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let mut remaining = Vec::new();
    for (piece, body, mut rest, velocity, spin) in &mut live {
        let at_rest = velocity.0.length() < REST_SPEED_M_S && spin.0.length() < REST_SPIN_RAD_S;
        rest.0 = if at_rest { rest.0 + dt } else { 0.0 };
        if rest.0 >= REST_S && !touches_vehicle(piece, &collisions, &vehicles) {
            freeze(&mut commands, piece);
            stress.rest_frozen += 1;
        } else {
            remaining.push((body.seq, piece));
        }
    }
    let Some(cap) = stress.lever.live_cap else {
        return;
    };
    for piece in past_cap(&mut remaining, cap, |piece| {
        touches_vehicle(piece, &collisions, &vehicles)
    }) {
        freeze(&mut commands, piece);
        stress.cap_frozen += 1;
    }
}

/// `FixedPostUpdate`, after the step: THAW (module doc). A frozen piece carries
/// `CollisionEventsEnabled`, so its new pairs with a moving body report their start here.
fn thaw_touched(
    mut started: MessageReader<CollisionStart>,
    frozen: Query<(), With<Frozen>>,
    vehicles: Query<&RigidBody, Without<DebrisPiece>>,
    mut stress: ResMut<DebrisStress>,
    mut thawed: Local<Vec<Entity>>,
    mut commands: Commands,
) {
    thawed.clear();
    for event in started.read() {
        for (piece, other) in [(event.body1, event.body2), (event.body2, event.body1)] {
            let (Some(piece), Some(other)) = (piece, other) else {
                continue;
            };
            let pushed = vehicles.get(other).is_ok_and(RigidBody::is_dynamic);
            if frozen.contains(piece) && pushed && !thawed.contains(&piece) {
                thawed.push(piece);
                commands
                    .entity(piece)
                    .remove::<(Frozen, CollisionEventsEnabled)>()
                    .insert((DebrisRest::default(), RigidBody::Dynamic));
            }
        }
    }
    stress.thawed += thawed.len() as u64;
}

/// `FixedPostUpdate`, after the step: every live piece's pose onto its carrier, written only when
/// it moved. A frozen piece is skipped, so its carrier's last write is the pose it froze at.
fn publish_poses(
    pieces: Query<(&DebrisPiece, &Position, &Rotation), Without<Frozen>>,
    mut carriers: Query<&mut DebrisPose>,
) {
    for (piece, position, rotation) in &pieces {
        if let Ok(mut pose) = carriers.get_mut(piece.carrier) {
            pose.set_if_neq(DebrisPose {
                position: position.0,
                rotation: rotation.0,
            });
        }
    }
}

/// `FixedPostUpdate`: past `frozen_cap`, despawn the oldest frozen pieces and their carriers.
fn despawn_oldest_frozen(
    mut stress: ResMut<DebrisStress>,
    frozen: Query<(Entity, &DebrisPiece), With<Frozen>>,
    mut commands: Commands,
) {
    let Some(cap) = stress.lever.frozen_cap else {
        return;
    };
    let count = frozen.iter().count();
    let Some(excess) = count.checked_sub(cap as usize).filter(|excess| *excess > 0) else {
        return;
    };
    let mut oldest: Vec<(u64, Entity, Entity)> = frozen
        .iter()
        .map(|(piece, body)| (body.seq, piece, body.carrier))
        .collect();
    oldest.sort_unstable_by_key(|(seq, ..)| *seq);
    for (_, piece, carrier) in oldest.into_iter().take(excess) {
        commands.entity(piece).despawn();
        commands.entity(carrier).despawn();
    }
    stress.despawned += excess as u64;
}

/// The client's shared fragment mesh and material.
#[derive(Resource)]
struct DebrisView {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

/// The fixture hull as a flat-shaded triangle list, every face wound outward.
fn fragment_mesh() -> Mesh {
    let collider = fragment_collider();
    let hull = collider
        .shape()
        .as_convex_polyhedron()
        .expect("a convex-hull collider is a convex polyhedron");
    let (points, triangles) = hull.to_trimesh();
    let centroid = points.iter().copied().sum::<Vec3>() / points.len() as f32;
    let mut positions = Vec::with_capacity(triangles.len() * 3);
    let mut normals = Vec::with_capacity(triangles.len() * 3);
    for [a, b, c] in triangles {
        let (a, mut b, mut c) = (points[a as usize], points[b as usize], points[c as usize]);
        let mut normal = (b - a).cross(c - a).normalize_or_zero();
        if normal.dot((a + b + c) / 3.0 - centroid) < 0.0 {
            std::mem::swap(&mut b, &mut c);
            normal = -normal;
        }
        positions.extend([a, b, c].map(|point| point.to_array()));
        normals.extend([normal.to_array(); 3]);
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
}

fn build_view(
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut commands: Commands,
) {
    let (Some(mut meshes), Some(mut materials)) = (meshes, materials) else {
        return;
    };
    commands.insert_resource(DebrisView {
        mesh: meshes.add(fragment_mesh()),
        material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.56, 0.52, 0.47),
            perceptual_roughness: 0.95,
            ..default()
        }),
    });
}

fn attach_view(
    view: Option<Res<DebrisView>>,
    arrived: Query<(Entity, &DebrisPose), Added<DebrisPose>>,
    mut commands: Commands,
) {
    let Some(view) = view else {
        return;
    };
    for (carrier, pose) in &arrived {
        commands.entity(carrier).insert((
            Mesh3d(view.mesh.clone()),
            MeshMaterial3d(view.material.clone()),
            pose.transform(),
        ));
    }
}

fn follow_pose(mut moved: Query<(&DebrisPose, &mut Transform), Changed<DebrisPose>>) {
    for (pose, mut transform) in &mut moved {
        *transform = pose.transform();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;

    use super::*;

    /// The lever's readings. A mutant that arms an unset or `off` lever spawns fragments on every
    /// production server; one that drops a field silently measures the wrong condition tonight; one
    /// that accepts a malformed value runs a stress condition with no stress in it.
    #[test]
    fn the_lever_is_off_unless_it_names_pieces_and_a_period() {
        for off in [None, Some(""), Some("off"), Some("false"), Some("0:5")] {
            assert_eq!(Lever::parse(off), Ok(None), "{off:?}");
        }
        assert_eq!(
            Lever::parse(Some("35:5")),
            Ok(Some(Lever {
                pieces: 35,
                period_s: 5.0,
                live_cap: Some(DEFAULT_LIVE_CAP),
                frozen_cap: None,
            }))
        );
        assert_eq!(
            Lever::parse(Some(" 200 : 2.5 : 0 : 1000 ")),
            Ok(Some(Lever {
                pieces: 200,
                period_s: 2.5,
                live_cap: None,
                frozen_cap: Some(1000),
            }))
        );
        for bad in [
            "35",
            "35:",
            "35:0",
            "35:-1",
            "x:5",
            "35:5:x",
            "1:2:3:4:5",
            "on",
        ] {
            assert!(Lever::parse(Some(bad)).is_err(), "{bad}");
        }
    }

    /// The cap takes the oldest live pieces first and never one riding a vehicle; a mutant that
    /// takes the newest leaves the stalest bodies costing the solver, and one that freezes a riding
    /// piece leaves a fixed block in a tank's path.
    #[test]
    fn the_cap_freezes_the_oldest_pieces_that_ride_nothing() {
        let piece = |n: u32| Entity::from_raw_u32(n).expect("a valid index");
        let mut live: Vec<(u64, Entity)> = [3, 1, 4, 0, 2]
            .into_iter()
            .map(|seq| (seq, piece(seq as u32 + 1)))
            .collect();
        let riding = piece(1);
        let frozen = past_cap(&mut live, 3, |p| p == riding);
        assert_eq!(frozen, vec![piece(2), piece(3)]);
        assert!(past_cap(&mut live, 5, |_| false).is_empty());
    }

    fn physics_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            // Avian's collider cache reads `AssetEvent<Mesh>`.
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        // One fixed tick per update at the fixed clock's default 64 Hz.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app
    }

    /// THE FREEZE RULE's price, the claim the design rests on: a piece dropped onto the ground
    /// comes to rest, freezes to `Static`, and from then on holds no contact pair (avian dropped its
    /// pair with the ground at the switch and never pairs two static bodies again) while its
    /// carrier's pose stops changing, so replicon sends it nothing. A mutant that never freezes, or
    /// that freezes by avian sleep (the pair survives, asleep), reds this; so does any change that
    /// writes a frozen piece's carrier.
    #[test]
    fn a_resting_piece_freezes_holds_no_contact_and_stops_publishing() {
        let mut app = physics_app();
        // No breach system: the test places its own piece.
        app.insert_resource(DebrisStress::new(Lever {
            pieces: 1,
            period_s: 1.0,
            live_cap: None,
            frozen_cap: None,
        }))
        .add_systems(
            FixedPostUpdate,
            (thaw_touched, publish_poses, freeze_rested)
                .chain()
                .after(PhysicsSystems::StepSimulation),
        );
        app.world_mut().spawn((
            RigidBody::Static,
            Position(Vec3::new(0.0, -0.5, 0.0)),
            Collider::cuboid(20.0, 1.0, 20.0),
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
        ));
        let pose = DebrisPose {
            position: Vec3::Y * 0.6,
            rotation: Quat::IDENTITY,
        };
        let carrier = app.world_mut().spawn(pose).id();
        let piece = app
            .world_mut()
            .spawn((
                DebrisPiece { seq: 0, carrier },
                DebrisRest::default(),
                Position(pose.position),
                Rotation(pose.rotation),
                RigidBody::Dynamic,
                fragment_collider(),
                ColliderDensity(DENSITY_KG_M3),
                CollisionLayers::new(
                    [Layer::Debris],
                    [Layer::Terrain, Layer::Vehicle, Layer::Debris],
                ),
            ))
            .id();
        let mut froze_at = None;
        for tick in 0..640 {
            app.update();
            if app.world().get::<Frozen>(piece).is_some() {
                froze_at = Some(tick);
                break;
            }
        }
        let froze_at = froze_at.expect("the piece came to rest and froze within 10 s");
        let landed = app
            .world()
            .get::<DebrisPose>(carrier)
            .copied()
            .expect("carrier");
        assert!(
            landed.position.y < 0.5,
            "the published pose followed the fall: {landed:?}"
        );
        app.update();
        assert_eq!(
            app.world().get::<RigidBody>(piece),
            Some(&RigidBody::Static),
            "frozen at tick {froze_at}: the piece is static"
        );
        let last_change = app
            .world()
            .entity(carrier)
            .get_ref::<DebrisPose>()
            .expect("carrier")
            .last_changed();
        for _ in 0..64 {
            app.update();
        }
        let graph = app.world().resource::<ContactGraph>();
        assert_eq!(
            graph.contact_pairs_with(piece).count(),
            0,
            "a frozen piece on the ground holds no contact pair"
        );
        let carrier_ref = app
            .world()
            .entity(carrier)
            .get_ref::<DebrisPose>()
            .expect("carrier");
        assert_eq!(
            carrier_ref.last_changed(),
            last_change,
            "a frozen piece's carrier no longer changes, so it sends nothing"
        );
        assert_eq!(app.world().resource::<DebrisStress>().rest_frozen, 1);
    }

    /// The breach places every piece 0–3 m above the ground beside the tank's flank and throws it
    /// toward the hull: a mutant that drops the clearance buries pieces in the terrain, one that
    /// flips the throw sends them away from the tanks they exist to strike.
    #[test]
    fn a_breach_stands_beside_the_flank_and_is_thrown_at_the_hull() {
        let anchor = Vec3::new(10.0, 5.0, -4.0);
        let ground = 4.0;
        let pieces = breach(anchor, Quat::IDENTITY, 1.0, 200, 0, |_| Some(ground));
        assert_eq!(pieces.len(), 200);
        for piece in &pieces {
            let height = piece.position.y - HALF_HEIGHT_M - ground;
            assert!((0.0..=3.0).contains(&height), "{piece:?}");
            assert!(piece.position.x - anchor.x >= WALL_OFFSET_M, "{piece:?}");
            assert!(piece.velocity.x < 0.0, "thrown toward the hull: {piece:?}");
        }
        for (i, a) in pieces.iter().enumerate() {
            for b in &pieces[i + 1..] {
                assert!(
                    a.position.distance(b.position) >= LATTICE_M - 1.0e-4,
                    "no two pieces start overlapping"
                );
            }
        }
        assert!(breach(anchor, Quat::IDENTITY, 1.0, 10, 0, |_| None).is_empty());
    }
}
