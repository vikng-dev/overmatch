//! The game's environment destruction model, game state only: shared debris carriers and the
//! operator stress workload.
//!
//! Each server body has one replicated carrier holding its [`DebrisPose`] ([`Carrier`]). Only
//! moved poses are written, so sleeping pieces stop publishing. Separate carriers keep the body's
//! registered physics components off this stream. Received poses are checked before interpolation;
//! malformed records go through the client's visible match-close path.
//!
//! Construction carriers draw at the shared interpolation cursor (ADR-0037); stress carriers apply on
//! arrival and use the scenario mesh. Neither view runs physics. [`install_server`] owns carriers
//! and publication; [`install_client`] owns presentation.
//!
//! The dedicated-server operator sets `OVERMATCH_DEBRIS_STRESS=<pieces>:<period_s>`. Unset, empty,
//! `off`, `false` or zero pieces is off. An armed server releases breaches beside tanks in turn,
//! alternating flanks. Pieces collide with terrain, vehicle proxies and each other. Their rest
//! rule is [`crate::debris`]'s. Sleeping pieces have no cap: removing a body wakes its contact
//! island, and removing pieces from the bottom of a pile would create the wakes being observed.
//!
//! Required construction and appearance live in `assets/dev/debris-stress.ron`, with every authored
//! value's job and provenance. Authority and presentation admit it before play.

use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::ecs::entity::EntityHashMap;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, ParallelSlice};
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod pose;
mod scenario;

use scenario::Scenario;

use pose::WirePose;
pub(super) use pose::replication as pose_replication;

use super::harness::{LeverError, env_text};
use super::world_reset::AuthorityWorld;
use crate::Layer;
use crate::construction::breakage::{Piece, RetirePiece};
use crate::debris::DebrisPiece;
use crate::tank::Tank;
use crate::terrain_grid::HeightGrid;

/// Operator workloads in `scripts/perf/run-debris-stress.sh` and `scripts/edgegap/session.sh`.
const LEVER: &str = "OVERMATCH_DEBRIS_STRESS";

/// The lever's reading.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Lever {
    pieces: u32,
    period_s: f64,
}

impl Lever {
    /// `None` is off; a malformed value is an error, so a stress run never measures a server that
    /// silently ignored its lever.
    fn parse(value: Option<&str>) -> Result<Option<Self>, LeverError> {
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
            LeverError::invalid(
                LEVER,
                value,
                "<u32 pieces>:<finite positive period_s>, or off",
            )
        };
        let fields: Vec<&str> = value.split(':').map(str::trim).collect();
        let [pieces, period_s] = fields[..] else {
            return Err(malformed());
        };
        let pieces: u32 = pieces.parse().map_err(|_| malformed())?;
        let period_s: f64 = period_s.parse().map_err(|_| malformed())?;
        if !(period_s.is_finite() && period_s > 0.0) {
            return Err(malformed());
        }
        Ok((pieces > 0).then_some(Self { pieces, period_s }))
    }
}

/// A piece's authoritative world pose, on its replicated carrier.
#[derive(Component, Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(try_from = "WirePose")]
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
    scenario: Scenario,
    next_release_s: f64,
    breaches: u64,
    released: u64,
}

impl DebrisStress {
    /// The first breach goes out on the first fixed tick a tank stands.
    fn new(lever: Lever, scenario: Scenario) -> Self {
        Self {
            lever,
            scenario,
            next_release_s: 0.0,
            breaches: 0,
            released: 0,
        }
    }
}

/// The stats line's view of the pieces, the lever's and constructions'.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct DebrisCensus<'w, 's> {
    stress: Option<Res<'w, DebrisStress>>,
    live: Query<'w, 's, (), (With<DebrisPiece>, Without<Sleeping>)>,
    sleeping: Query<'w, 's, (), (With<DebrisPiece>, With<Sleeping>)>,
}

impl DebrisCensus<'_, '_> {
    /// `live` (awake) and `sleeping` pieces now, always; while the lever is armed also its
    /// cumulative counts since the server started: `breaches` and lever pieces `released`.
    pub(super) fn json(&self) -> Value {
        let mut census = json!({
            "live": self.live.iter().count(),
            "sleeping": self.sleeping.iter().count(),
        });
        if let Some(stress) = &self.stress {
            census["breaches"] = json!(stress.breaches);
            census["released"] = json!(stress.released);
        }
        census
    }
}

#[derive(Debug)]
pub(super) enum StartupError {
    Lever(LeverError),
    Scenario(scenario::ScenarioError),
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lever(error) => error.fmt(f),
            Self::Scenario(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StartupError {}

/// Every server: a carrier for each construction piece and every moved piece's pose onto its carrier.
/// Then the lever, when armed; a malformed value is refused.
pub(super) fn install_server(app: &mut App) -> Result<(), StartupError> {
    let value = env_text(LEVER).map_err(StartupError::Lever)?;
    install_server_from(app, value.as_deref(), scenario::RON)
}

fn install_server_from(
    app: &mut App,
    value: Option<&str>,
    scenario_text: &str,
) -> Result<(), StartupError> {
    let scenario = Scenario::parse(scenario_text).map_err(StartupError::Scenario)?;
    let lever = Lever::parse(value).map_err(StartupError::Lever)?;
    app.add_observer(carry_construction_piece)
        .add_observer(retire_construction_piece)
        .add_systems(
            FixedPostUpdate,
            publish_poses.in_set(crate::state::SimPhase::Publish),
        );
    let Some(lever) = lever else {
        return Ok(());
    };
    info!(
        "server: debris stress — a breach of {} pieces every {} s ({LEVER})",
        lever.pieces, lever.period_s
    );
    app.insert_resource(DebrisStress::new(lever, scenario));
    app.add_systems(FixedUpdate, release_breaches);
    Ok(())
}

/// Draw every arriving lever carrier as a plain mesh at its pose, and follow every carrier's pose.
/// Mounted by every client; a client without mesh or material assets draws nothing.
pub(super) fn install_client(app: &mut App) {
    app.init_resource::<bevy_replicon::prelude::ReplicationStorage>()
        .add_message::<super::matchmaking::ServerGone>()
        .add_systems(
            PreUpdate,
            refuse_peer_pose.after(bevy_replicon::prelude::ClientSystems::Receive),
        )
        .add_systems(Startup, build_view)
        .add_systems(Update, (attach_view, follow_pose).chain());
}

/// The ordinary match-close path shows the cause in the lobby and stops reconnecting this link.
fn refuse_peer_pose(
    mut storage: ResMut<bevy_replicon::prelude::ReplicationStorage>,
    mut gone: MessageWriter<super::matchmaking::ServerGone>,
) {
    // No refusal is the normal state of a valid stream.
    if let Some(refusal) = storage.global.remove::<pose::PoseRefusal>() {
        warn!("client: refusing server debris pose: {refusal}");
        gone.write(super::matchmaking::ServerGone(refusal.to_string()));
    }
}

/// A construction piece's carrier: its pose and its group, replicated to every client. The carrier
/// holds the group's `Piece` too, so its own spawn reaches this observer: it is the carrier
/// (it has a [`DebrisPose`]), not a piece, and gets none.
fn carry_construction_piece(
    added: On<Add, Piece>,
    pieces: Query<(&Piece, Option<(&Position, &Rotation)>, Has<DebrisPose>)>,
    generation: Res<AuthorityWorld>,
    mut commands: Commands,
) {
    let (piece, pose, carrier) = pieces
        .get(added.entity)
        .expect("an added Piece is on its entity in its own Add observer");
    if carrier {
        return;
    }
    let (position, rotation) = pose.expect(
        "`construction::breakage` spawns every piece with its Position and Rotation in the Piece \
         bundle",
    );
    let carrier = commands
        .spawn((
            Name::new("ConstructionPieceCarrier"),
            DebrisPose {
                position: position.0,
                rotation: rotation.0,
            },
            piece.clone(),
            generation.0,
            Replicate::to_clients(NetworkTarget::All),
            // ONE TIMELINE: a piece draws at the interpolation cursor, with the hulls and the
            // record's presentation (`construction::view`), never at its pose's arrival.
            InterpolationTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    commands.entity(added.entity).insert(Carrier(carrier));
}

/// Keep the parent's final pose and retirement tick on the wire until the shared view
/// cursor reaches the split. Its children carry their own birth tick and occupied chunks.
fn retire_construction_piece(
    retired: On<RetirePiece>,
    bodies: Query<(&Carrier, &Position, &Rotation)>,
    mut carriers: Query<(&mut Piece, &mut DebrisPose)>,
) {
    let (carrier, position, rotation) = bodies.get(retired.entity).expect(
        "only the server installs this observer, and `carry_construction_piece` gives every server chunk \
         group its carrier in the flush that spawns it, before a deposit can stage its split",
    );
    let (mut piece, mut pose) = carriers
        .get_mut(carrier.0)
        .expect("a server chunk group retains its carrier through its split tick");
    piece.retired = Some(retired.tick);
    pose.set_if_neq(DebrisPose {
        position: position.0,
        rotation: rotation.0,
    });
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
/// Pieces fill rows, then columns centred on the tank, then layers outward; the admitted scenario
/// supplies the release layout and kinetics.
fn breach(
    scenario: &Scenario,
    (anchor, heading): (Vec3, Quat),
    side: f32,
    radius: f32,
    pieces: u32,
    first_seq: u64,
    ground: impl Fn(Vec2) -> Option<f32> + Sync,
) -> Vec<Release> {
    // The operator's release frame uses heading yaw even when the body has rolled onto its side.
    let yaw = heading.to_euler(EulerRot::YXZ).0;
    let outward = Quat::from_rotation_y(yaw) * Vec3::X * side;
    let along = Vec3::Y.cross(outward);
    let parameters = &scenario.parameters;
    let (rows, pitch) = (parameters.rows.0, parameters.lattice_m.0);
    let columns = pieces.div_ceil(rows).clamp(1, parameters.columns.0);
    let centre = (columns - 1) as f32 * 0.5;
    // Evaluate releases independently; ordered spawn commands commit this breach afterwards.
    let indices: Vec<_> = (0..pieces).collect();
    indices
        .par_splat_map(ComputeTaskPool::get(), None, |_, indices| {
            indices
                .iter()
                .filter_map(|&i| {
                    let seq = first_seq + u64::from(i);
                    let (row, column, layer) =
                        (i % rows, (i / rows) % columns, (i / rows) / columns);
                    let xz = anchor
                        + outward * (radius + pitch + layer as f32 * pitch)
                        + along * ((column as f32 - centre) * pitch);
                    let ground = ground(Vec2::new(xz.x, xz.z))?;
                    let y = ground
                        + parameters.ground_clearance_m.0
                        + scenario.half_height_m
                        + row as f32 * pitch;
                    let jitter = |k| unit(seq, k);
                    let [inward_min, inward_max] = parameters.inward_speed_m_s.0;
                    Some(Release {
                        position: Vec3::new(xz.x, y, xz.z),
                        rotation: Quat::from_rotation_y(jitter(0) * std::f32::consts::TAU)
                            * Quat::from_rotation_x(
                                (jitter(1) - 0.5) * (2.0 * parameters.tilt_rad.0),
                            ),
                        velocity: -outward * (inward_min + (inward_max - inward_min) * jitter(2))
                            + Vec3::Y * (parameters.upward_speed_m_s.0 * jitter(3))
                            + along
                                * (2.0 * parameters.along_speed_m_s.0 * jitter(4)
                                    - parameters.along_speed_m_s.0),
                        spin: (Vec3::new(jitter(5), jitter(6), jitter(7)) * 2.0 - Vec3::ONE)
                            * parameters.spin_rad_s.0,
                    })
                })
                .collect::<Vec<_>>()
        })
        .into_iter()
        .flatten()
        .collect()
}

/// `FixedUpdate`: once the period has passed and a tank stands, release the next breach.
fn release_breaches(
    time: Res<Time>,
    mut stress: ResMut<DebrisStress>,
    tanks: Query<(&Position, &Rotation), With<Tank>>,
    grid: Option<Res<HeightGrid>>,
    source: crate::tank::TankSimSource,
    generation: Res<AuthorityWorld>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    if now < stress.next_release_s {
        return;
    }
    let count = tanks.iter().count() as u64;
    // Before the first tank spawns there is no breach anchor.
    if count == 0 {
        return;
    }
    let (anchor, heading) = tanks
        .iter()
        .nth((stress.breaches % count) as usize)
        .expect("a round-robin index below the unchanged tank query count resolves");
    // Each tank's flank alternates from one round of the tanks to the next.
    let side = if (stress.breaches / count).is_multiple_of(2) {
        1.0
    } else {
        -1.0
    };
    let ground = |xz: Vec2| match grid.as_deref() {
        Some(grid) => grid.contains_xz(xz.x, xz.y).then(|| {
            grid.max_height_in_square(
                xz.x,
                xz.y,
                stress.scenario.half_height_m + stress.scenario.parameters.lattice_m.0 * 0.5,
            )
        }),
        // The authored flat world (no heightfield) has its ground slab's top at y=0.
        None => Some(0.0),
    };
    let releases = breach(
        &stress.scenario,
        (anchor.0, heading.0),
        side,
        source.get().contact_radius(),
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
    // This structural transaction pairs each body with its carrier before publishing the batch.
    for release in releases {
        stress.released += 1;
        let carrier = commands
            .spawn((
                Name::new("DebrisCarrier"),
                generation.0,
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
            DebrisPiece,
            Carrier(carrier),
            Position(release.position),
            Rotation(release.rotation),
            RigidBody::Dynamic,
            stress.scenario.collider.clone(),
            ColliderDensity(stress.scenario.parameters.density_kg_m3.0),
            layers,
            LinearVelocity(release.velocity),
            AngularVelocity(release.spin),
        ));
    }
}

/// `FixedPostUpdate`, after the step: every piece's pose onto its carrier, written only when it
/// moved. A sleeping piece's pose is not written, so its carrier's last write is the pose it fell
/// asleep at.
fn publish_poses(
    pieces: Query<(&Carrier, &Position, &Rotation), Or<(Changed<Position>, Changed<Rotation>)>>,
    mut carriers: Query<(Entity, &mut DebrisPose)>,
    mut updates: Local<EntityHashMap<DebrisPose>>,
) {
    updates.clear();
    for (carrier, position, rotation) in &pieces {
        assert!(
            carriers.contains(carrier.0),
            "every live body's Carrier retains its DebrisPose until the body is retired"
        );
        assert!(
            updates
                .insert(
                    carrier.0,
                    DebrisPose {
                        position: position.0,
                        rotation: rotation.0,
                    }
                )
                .is_none(),
            "each moved body owns a distinct carrier"
        );
    }
    // Validate ownership before dispatch: distinct targets and fixed replacements make writes
    // commute, independent of iteration order or worker assignment.
    let updates = &*updates;
    carriers
        .par_iter_many_unique_mut(updates.keys())
        .for_each(|(entity, mut pose)| {
            pose.set_if_neq(updates[&entity]);
        });
}

/// The client's shared fragment mesh and material.
#[derive(Resource)]
struct DebrisView {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

/// The admitted hull has finite, nonzero face normals; every triangle is wound outward.
fn fragment_mesh(scenario: &Scenario) -> Mesh {
    let hull = scenario
        .collider
        .shape()
        .as_convex_polyhedron()
        .expect("a convex-hull collider is a convex polyhedron");
    let (points, triangles) = hull.to_trimesh();
    let centroid = points.iter().copied().sum::<Vec3>() / points.len() as f32;
    let mut positions = Vec::with_capacity(triangles.len() * 3);
    let mut normals = Vec::with_capacity(triangles.len() * 3);
    for [a, b, c] in triangles {
        let (a, mut b, mut c) = (points[a as usize], points[b as usize], points[c as usize]);
        let mut normal = (b - a).cross(c - a).normalize();
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
    let scenario = Scenario::shipped();
    // A renderer-free client receives poses but has no fragment mesh/material collections.
    let (Some(mut meshes), Some(mut materials)) = (meshes, materials) else {
        return;
    };
    let [red, green, blue] = scenario.parameters.albedo_srgb.0;
    commands.insert_resource(DebrisView {
        mesh: meshes.add(fragment_mesh(&scenario)),
        material: materials.add(StandardMaterial {
            base_color: Color::srgb(red, green, blue),
            perceptual_roughness: scenario.parameters.roughness.0,
            ..default()
        }),
    });
}

/// A carrier's pose as its transform, from arrival; a lever carrier also takes the fixture mesh.
fn attach_view(
    view: Option<Res<DebrisView>>,
    arrived: Query<(Entity, &DebrisPose, Has<Piece>), Added<DebrisPose>>,
    mut commands: Commands,
) {
    for (carrier, pose, construction) in &arrived {
        let mut entity = commands.entity(carrier);
        // A construction piece's view may already hold it hidden until its fall is presented.
        entity
            .insert(pose.transform())
            .insert_if_new(Visibility::default());
        if let (false, Some(view)) = (construction, view.as_deref()) {
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

    /// FIXTURE DATA: the generation every carrier of these worlds is tagged with.
    fn fixture_world() -> AuthorityWorld {
        AuthorityWorld(crate::world::WorldGeneration(1))
    }

    /// Every construction piece gets exactly one replicated carrier at its spawn pose, and the carrier
    /// (which holds the group's `Piece` too) gets none. A mutant that skips a pose-less entity
    /// silently, or that carries the carrier, breaks one of the two counts; clients then draw no
    /// fallen group, or a phantom at the origin.
    #[test]
    fn a_piece_gets_one_carrier_and_the_carrier_none() {
        let mut world = World::new();
        world.insert_resource(fixture_world());
        world.add_observer(carry_construction_piece);
        let at = Vec3::new(3.0, 1.5, -2.0);
        let piece = world
            .spawn((
                Piece {
                    construction: 2,
                    chunks: vec![4, 5],
                    born: 0,
                    retired: None,
                },
                Position(at),
                Rotation(Quat::IDENTITY),
            ))
            .id();
        world.flush();
        let carrier = world.get::<Carrier>(piece).expect("the piece is carried").0;
        assert_eq!(world.get::<DebrisPose>(carrier).unwrap().position, at);
        assert!(world.get::<Carrier>(carrier).is_none());
        assert_eq!(world.query::<&DebrisPose>().iter(&world).count(), 1);
    }

    /// Birth and retirement are wire facts, including in a late joiner's full snapshot. A
    /// split preserves the parent's final pose rather than despawning it ahead of the cursor.
    #[test]
    fn a_retired_group_keeps_its_final_pose_and_wire_interval() {
        let mut world = World::new();
        world.insert_resource(fixture_world());
        world.add_observer(carry_construction_piece);
        world.add_observer(retire_construction_piece);
        let body = world
            .spawn((
                Piece {
                    construction: 3,
                    chunks: vec![1, 2],
                    born: 4,
                    retired: None,
                },
                Position(Vec3::ZERO),
                Rotation(Quat::IDENTITY),
            ))
            .id();
        world.flush();
        let carrier = world.get::<Carrier>(body).unwrap().0;
        world.get_mut::<Position>(body).unwrap().0 = Vec3::new(1.0, 2.0, 3.0);
        world.trigger(RetirePiece {
            entity: body,
            tick: 12,
        });
        world.flush();
        let piece = world.get::<Piece>(carrier).unwrap();
        assert_eq!((piece.born, piece.retired), (4, Some(12)));
        assert_eq!(
            world.get::<DebrisPose>(carrier).unwrap().position,
            Vec3::new(1.0, 2.0, 3.0)
        );
        let bytes = postcard::to_allocvec(piece).unwrap();
        assert_eq!(&postcard::from_bytes::<Piece>(&bytes).unwrap(), piece);
    }

    /// A piece spawned without its body pose is a construction bug, never a group that silently
    /// draws nowhere.
    #[test]
    #[should_panic(expected = "spawns every piece with its Position and Rotation")]
    fn a_piece_without_its_pose_is_a_bug() {
        let mut world = World::new();
        world.insert_resource(fixture_world());
        world.add_observer(carry_construction_piece);
        world.spawn(Piece {
            construction: 2,
            chunks: vec![4],
            born: 0,
            retired: None,
        });
        world.flush();
    }

    /// Authority must publish every moved body. Losing the carrier or its pose is a lifetime bug,
    /// never a body whose movement silently disappears from the client stream.
    #[test]
    #[should_panic(expected = "every live body's Carrier retains its DebrisPose")]
    fn a_live_body_without_its_carrier_pose_is_a_bug() {
        let mut app = App::new();
        app.add_systems(Update, publish_poses);
        let carrier = app.world_mut().spawn_empty().id();
        app.world_mut()
            .spawn((Carrier(carrier), Position(Vec3::ZERO), Rotation::default()));
        app.update();
    }

    /// Publication cannot resolve conflicting owners by worker order: sharing a carrier is a bug.
    #[test]
    #[should_panic(expected = "each moved body owns a distinct carrier")]
    fn two_moved_bodies_cannot_share_a_carrier() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_systems(Update, publish_poses);
        let carrier = app
            .world_mut()
            .spawn(DebrisPose {
                position: Vec3::ZERO,
                rotation: Quat::IDENTITY,
            })
            .id();
        for _ in 0..2 {
            app.world_mut()
                .spawn((Carrier(carrier), Position(Vec3::ZERO), Rotation::default()));
        }
        app.update();
    }

    /// Clients rely on each carrier receiving its owner's pose. Fixture permutations vary body
    /// traversal and batch membership; cross-owner reads or writes fail the same identity contract.
    #[test]
    fn publication_preserves_ownership_across_body_orders() {
        // Fixture: a prime population and coprime strides visit every owner in different orders.
        const COUNT: usize = 257;
        for stride in [1, 17, COUNT - 1] {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .add_systems(Update, publish_poses);
            let expected: Vec<_> = (0..COUNT)
                .map(|i| DebrisPose {
                    position: Vec3::new(i as f32, -(i as f32), (i * i) as f32),
                    rotation: Quat::from_rotation_y(i as f32),
                })
                .collect();
            let carriers: Vec<_> = expected
                .iter()
                .map(|_| {
                    app.world_mut()
                        .spawn(DebrisPose {
                            position: Vec3::ZERO,
                            rotation: Quat::IDENTITY,
                        })
                        .id()
                })
                .collect();
            for i in 0..COUNT {
                let owner = i * stride % COUNT;
                let pose = expected[owner];
                app.world_mut().spawn((
                    Carrier(carriers[owner]),
                    Position(pose.position),
                    Rotation(pose.rotation),
                ));
            }
            app.update();
            for (&carrier, pose) in carriers.iter().zip(&expected) {
                assert_eq!(app.world().get::<DebrisPose>(carrier), Some(pose));
            }
        }
    }

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
            }))
        );
        assert_eq!(
            Lever::parse(Some(" 200 : 2.5 ")),
            Ok(Some(Lever {
                pieces: 200,
                period_s: 2.5,
            }))
        );
        // A third field is refused, so a run scripted for the lever's old cap form fails at launch
        // instead of measuring a condition it did not ask for.
        for bad in [
            "35",
            "35:",
            "35:0",
            "35:-1",
            "x:5",
            "35:5:x",
            "200:2.5:1000",
            "12:2:8192:512",
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
        // The network fixture clock advances the publication boundary once per update.
        .insert_resource(TimeUpdateStrategy::ManualDuration(
            crate::net::test_harness::TICK,
        ));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app
    }

    /// A RESTING PIECE STOPS PUBLISHING: a piece dropped onto the ground falls asleep, its
    /// published pose follows the fall, and from then on its carrier's pose stops changing, so
    /// replicon sends it nothing. A mutant that writes every piece's carrier every tick, or that
    /// keeps a resting piece awake, reds this.
    #[test]
    fn a_sleeping_piece_stops_publishing() {
        let mut app = physics_app();
        app.add_systems(
            FixedPostUpdate,
            publish_poses.after(PhysicsSystems::StepSimulation),
        );
        let scenario = Scenario::shipped();
        let radius = scenario
            .parameters
            .fragment_hull_m
            .0
            .iter()
            .map(|point| Vec3::from(*point).xz().length())
            .fold(0.0, f32::max);
        // Fixture floor thickness and drop height; floor width follows the admitted hull.
        app.world_mut().spawn((
            RigidBody::Static,
            Position(Vec3::new(0.0, -0.5, 0.0)),
            Collider::cuboid(4.0 * radius, 1.0, 4.0 * radius),
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
        ));
        let pose = DebrisPose {
            position: Vec3::Y * (scenario.half_height_m + 0.4),
            rotation: Quat::IDENTITY,
        };
        let carrier = app.world_mut().spawn(pose).id();
        let piece = app
            .world_mut()
            .spawn((
                DebrisPiece,
                Carrier(carrier),
                Position(pose.position),
                Rotation(pose.rotation),
                RigidBody::Dynamic,
                scenario.collider.clone(),
                ColliderDensity(scenario.parameters.density_kg_m3.0),
                CollisionLayers::new(
                    [Layer::Debris],
                    [Layer::Terrain, Layer::Vehicle, Layer::Debris],
                ),
            ))
            .id();
        let slept_at = (0..640)
            .find(|_| {
                app.update();
                app.world().get::<Sleeping>(piece).is_some()
            })
            .expect("the piece came to rest and slept within 10 s");
        let landed = app
            .world()
            .get::<DebrisPose>(carrier)
            .copied()
            .expect("carrier");
        assert!(
            landed.position.y < pose.position.y,
            "the published pose followed the fall: {landed:?}"
        );
        assert_eq!(
            landed.position,
            app.world().get::<Position>(piece).unwrap().0,
            "asleep at tick {slept_at}: the carrier holds the pose it fell asleep at"
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
        assert!(app.world().get::<Sleeping>(piece).is_some(), "still asleep");
        let carrier_ref = app
            .world()
            .entity(carrier)
            .get_ref::<DebrisPose>()
            .expect("carrier");
        assert_eq!(
            carrier_ref.last_changed(),
            last_change,
            "a sleeping piece's carrier no longer changes, so it sends nothing"
        );
    }

    /// The breach places every piece above the ground beside the tank's flank and throws it
    /// toward the hull: a mutant that drops the clearance buries pieces in the terrain, one that
    /// flips the throw sends them away from the tanks they exist to strike.
    #[test]
    fn a_breach_stands_beside_the_flank_and_is_thrown_at_the_hull() {
        // Fixture coordinates and count exercise rows, columns and outward layers.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let anchor = Vec3::new(10.0, 5.0, -4.0);
        let ground = 4.0;
        let radius = 7.0; // Fixture body radius, deliberately wider than the fragment lattice.
        let scenario = Scenario::shipped();
        let pitch = scenario.parameters.lattice_m.0;
        let pieces = breach(
            &scenario,
            (anchor, Quat::IDENTITY),
            1.0,
            radius,
            200,
            0,
            |_| Some(ground),
        );
        assert_eq!(pieces.len(), 200);
        for piece in &pieces {
            let height = piece.position.y - scenario.half_height_m - ground;
            let top = scenario.parameters.ground_clearance_m.0
                + (scenario.parameters.rows.0 - 1) as f32 * pitch;
            // Resolution: allow accumulated f32 translation rounding at the fixture anchor.
            let tolerance = piece.position.abs().max_element() * f32::EPSILON * 4.0;
            assert!(
                height >= scenario.parameters.ground_clearance_m.0 - tolerance
                    && height <= top + tolerance,
                "{piece:?}"
            );
            for vertex in &scenario.parameters.fragment_hull_m.0 {
                assert!(
                    (piece.position + piece.rotation * Vec3::from(*vertex)).y >= ground,
                    "admitted tilted hull starts above terrain: {piece:?}"
                );
            }
            assert!(
                piece.position.x - anchor.x + tolerance >= radius + pitch,
                "{piece:?}"
            );
            assert!(
                scenario.parameters.inward_speed_m_s.0[0] <= -piece.velocity.x
                    && -piece.velocity.x <= scenario.parameters.inward_speed_m_s.0[1],
                "inward release speed follows the scenario: {piece:?}"
            );
        }
        for (i, a) in pieces.iter().enumerate() {
            for b in &pieces[i + 1..] {
                assert!(
                    a.position.distance(b.position) >= pitch - 1.0e-4,
                    "no two pieces start overlapping"
                );
            }
        }
        assert!(
            breach(
                &scenario,
                (anchor, Quat::IDENTITY),
                1.0,
                radius,
                10,
                0,
                |_| None
            )
            .is_empty()
        );
    }
}
