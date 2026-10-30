//! The debris stress lever: a dev-only instrument that prices server-simulated wall fragments,
//! `OVERMATCH_DEBRIS_STRESS=<pieces>:<period_s>[:<live_cap>[:<frozen_cap>]]` on the dedicated
//! server. Unset, empty, `off`, `false` or zero pieces is off, and an off server mounts none of its
//! releases.
//!
//! Every `period_s` of fixed time the server releases one breach of `pieces` dynamic fragments
//! beside one tank, round-robin over every [`Tank`] root: a wall section [`WALL_OFFSET_M`] off the
//! hull's flank, its pieces 0–3 m above the ground and thrown toward the hull, so they land on and
//! against hulls as well as on the terrain. A piece collides with `Layer::Terrain` (the ground,
//! houses, trunks), `Layer::Vehicle` (the tanks' collision proxies) and the other pieces.
//!
//! SETTLING is the game's own rule ([`crate::debris`]), which a building's fallen chunks follow
//! too: the lever's `live_cap` ([`crate::debris::DEFAULT_LIVE_CAP`]; `0` is none) is the rule's cap
//! while it is armed. `frozen_cap` (`0` or absent: none) despawns the oldest frozen pieces past it.
//!
//! REPLICATION: each server piece, the lever's and a building's ([`WallPiece`]) alike, is a
//! server-only body plus one replicated carrier entity holding its [`DebrisPose`] ([`Carrier`]),
//! written only while the piece is live and only when it moved (`set_if_neq`), so a frozen piece's
//! carrier stops changing and replicon sends it nothing more. A separate carrier, because avian's
//! `Position`/`Rotation`/velocities replicate from any replicated entity that holds them
//! (`protocol::plugin`): replicating the body itself would send four components per piece per
//! tick. Clients draw a lever carrier as a plain mesh at its pose ([`install_client`]) and a
//! building's from its chunks (`buildings::view`); they run no physics on either. A building's
//! carrier is interpolated, so its piece draws at the cursor the hulls draw at (ADR-0037); the
//! lever's apply on arrival, as the instrument has always priced them.
//!
//! The lever's piece shape and density are fixture numbers, not a measured stone:
//! [`FRAGMENT_HULL`], [`DENSITY_KG_M3`].

use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Layer;
use crate::buildings::breakage::WallPiece;
use crate::debris::{DEFAULT_LIVE_CAP, DebrisPiece, DebrisSettle, DebrisSettleSet, Frozen};
use crate::tank::Tank;
use crate::terrain_grid::HeightGrid;

/// The lever.
const LEVER: &str = "OVERMATCH_DEBRIS_STRESS";

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
    pub(crate) fn transform(self) -> Transform {
        Transform::from_translation(self.position).with_rotation(self.rotation)
    }
}

/// A carrier's pose `t` of the way from `start` to `end`: the position along the line, the
/// rotation along the shorter arc.
pub(crate) fn debris_pose_lerp(start: DebrisPose, end: DebrisPose, t: f32) -> DebrisPose {
    DebrisPose {
        position: start.position.lerp(end.position, t),
        rotation: start.rotation.slerp(end.rotation, t),
    }
}

/// A server piece's replicated carrier entity.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct Carrier(pub(crate) Entity);

/// The armed lever and its running counts.
#[derive(Resource)]
pub(super) struct DebrisStress {
    lever: Lever,
    collider: Collider,
    next_release_s: f64,
    breaches: u64,
    released: u64,
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
            despawned: 0,
        }
    }
}

/// The stats line's view of the pieces: present only while the lever is armed.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct DebrisCensus<'w, 's> {
    stress: Option<Res<'w, DebrisStress>>,
    settle: Option<Res<'w, DebrisSettle>>,
    live: Query<'w, 's, (), (With<DebrisPiece>, Without<Frozen>)>,
    frozen: Query<'w, 's, (), (With<DebrisPiece>, With<Frozen>)>,
}

impl DebrisCensus<'_, '_> {
    /// `live` and `frozen` pieces now (the lever's and buildings'); the cumulative counts since the
    /// server started: breaches, lever pieces released, frozen by the rest rule and by the cap,
    /// woken, despawned by `frozen_cap`.
    pub(super) fn json(&self) -> Option<Value> {
        let stress = self.stress.as_ref()?;
        let settle = self.settle.as_deref().copied().unwrap_or_default();
        Some(json!({
            "live": self.live.iter().count(),
            "frozen": self.frozen.iter().count(),
            "breaches": stress.breaches,
            "released": stress.released,
            "rest_frozen": settle.rest_frozen,
            "cap_frozen": settle.cap_frozen,
            "thawed": settle.thawed,
            "despawned": stress.despawned,
        }))
    }
}

fn fragment_collider() -> Collider {
    Collider::convex_hull(FRAGMENT_HULL.to_vec()).expect("the fixture hull is not degenerate")
}

/// Every server: a carrier for each building piece and every live piece's pose onto its carrier.
/// Then the lever, when armed; a malformed value is refused.
pub(super) fn install_server(app: &mut App) -> Result<(), String> {
    app.add_observer(carry_wall_piece).add_systems(
        FixedPostUpdate,
        publish_poses.after(DebrisSettleSet::Settle),
    );
    let Some(lever) = Lever::parse(std::env::var(LEVER).ok().as_deref())? else {
        return Ok(());
    };
    info!(
        "server: debris stress — a breach of {} pieces every {} s, live cap {:?}, frozen cap {:?} \
         ({LEVER})",
        lever.pieces, lever.period_s, lever.live_cap, lever.frozen_cap
    );
    app.insert_resource(DebrisSettle {
        live_cap: lever.live_cap,
        ..default()
    });
    app.insert_resource(DebrisStress::new(lever));
    app.add_systems(FixedUpdate, release_breaches);
    app.add_systems(
        FixedPostUpdate,
        despawn_oldest_frozen.after(DebrisSettleSet::Settle),
    );
    Ok(())
}

/// Draw every arriving lever carrier as a plain mesh at its pose, and follow every carrier's pose.
/// Mounted by every client; a client without mesh or material assets draws nothing.
pub(super) fn install_client(app: &mut App) {
    app.add_systems(Startup, build_view)
        .add_systems(Update, (attach_view, follow_pose).chain());
}

/// A building piece's carrier: its pose and its group, replicated to every client.
fn carry_wall_piece(
    added: On<Add, WallPiece>,
    pieces: Query<(&WallPiece, &Position, &Rotation)>,
    mut commands: Commands,
) {
    let Ok((piece, position, rotation)) = pieces.get(added.entity) else {
        return;
    };
    let carrier = commands
        .spawn((
            Name::new("WallPieceCarrier"),
            DebrisPose {
                position: position.0,
                rotation: rotation.0,
            },
            piece.clone(),
            Replicate::to_clients(NetworkTarget::All),
            // ONE TIMELINE: a piece draws at the interpolation cursor, with the hulls and the
            // record's presentation (`buildings::view`), never at its pose's arrival.
            InterpolationTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    commands.entity(added.entity).insert(Carrier(carrier));
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
            DebrisPiece::default(),
            Carrier(carrier),
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

/// `FixedPostUpdate`, after the step: every live piece's pose onto its carrier, written only when
/// it moved. A frozen piece is skipped, so its carrier's last write is the pose it froze at.
fn publish_poses(
    pieces: Query<(&Carrier, &Position, &Rotation), Without<Frozen>>,
    mut carriers: Query<&mut DebrisPose>,
) {
    for (carrier, position, rotation) in &pieces {
        if let Ok(mut pose) = carriers.get_mut(carrier.0) {
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
    frozen: Query<(Entity, &DebrisPiece, Option<&Carrier>), With<Frozen>>,
    mut commands: Commands,
) {
    let Some(cap) = stress.lever.frozen_cap else {
        return;
    };
    let count = frozen.iter().count();
    let Some(excess) = count.checked_sub(cap as usize).filter(|excess| *excess > 0) else {
        return;
    };
    let mut oldest: Vec<(u64, Entity, Option<Entity>)> = frozen
        .iter()
        .map(|(piece, body, carrier)| (body.seq, piece, carrier.map(|c| c.0)))
        .collect();
    oldest.sort_unstable_by_key(|(seq, ..)| *seq);
    for (_, piece, carrier) in oldest.into_iter().take(excess) {
        commands.entity(piece).despawn();
        if let Some(carrier) = carrier {
            commands.entity(carrier).despawn();
        }
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

/// A carrier's pose as its transform, from arrival; a lever carrier also takes the fixture mesh.
fn attach_view(
    view: Option<Res<DebrisView>>,
    arrived: Query<(Entity, &DebrisPose, Has<WallPiece>), Added<DebrisPose>>,
    mut commands: Commands,
) {
    for (carrier, pose, wall) in &arrived {
        let mut entity = commands.entity(carrier);
        // A building piece's view may already hold it hidden until its fall is presented.
        entity
            .insert(pose.transform())
            .insert_if_new(Visibility::default());
        if let (false, Some(view)) = (wall, view.as_deref()) {
            entity.insert((
                Mesh3d(view.mesh.clone()),
                MeshMaterial3d(view.material.clone()),
            ));
        }
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

    fn physics_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            // Avian's collider cache reads `AssetEvent<Mesh>`.
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            crate::debris::sim_plugin,
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
        app.add_systems(
            FixedPostUpdate,
            publish_poses.after(DebrisSettleSet::Settle),
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
                DebrisPiece::default(),
                Carrier(carrier),
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
        assert_eq!(app.world().resource::<DebrisSettle>().rest_frozen, 1);
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
