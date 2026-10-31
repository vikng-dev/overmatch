//! Buildings and rubble under the running gear, in the game's tank driving physics (game state
//! only). The terrain field ([`super::oracle::BlockField`]) is the heightmap and the authored
//! blocks; a building's elements and standing chunks and every loose piece are Avian colliders the
//! field never holds. [`RubbleOracle`] folds those colliders onto a terrain oracle with the field's
//! own probe semantics and names the collider each probe hit, so the belt model's support and grip
//! press on it, and [`piece_impulses`] sizes what a loose piece takes back in the same tick.
//!
//! - **What counts as rubble**: a collider carrying [`BuildingElement`] or [`BuildingChunk`] (on a
//!   static building root), or any collider of a [`DebrisPiece`] body. Terrain colliders, trees and
//!   vehicle colliders are not: the field already holds the ground, and a hull is no ground.
//! - **Probe semantics**: one solid ray over the segment [`TerrainOracle::depth_along`] walks
//!   (origin `reach` behind the station, length `2 × reach`). A buried origin hits at distance 0,
//!   depth `reach`: the field's own buried saturation. The deeper answer wins; a tie goes to the
//!   field.
//! - **Cost gate**: one AABB query per tank per tick over the whole probe reach
//!   ([`RubbleQuery::near`]). With no rubble collider in it every probe is the field's alone and
//!   no ray is cast, so open ground is bit-identical to the field-only oracle.
//! - **Tick truth**: avian refits dynamic bodies' AABBs at the end of its physics step
//!   (`update_solver_body_aabbs`) and each candidate's narrow test reads its current
//!   `Position`/`Rotation`, so a cast in `FixedUpdate` sees the poses the last step wrote.
//!
//! **The two sides of a belt-piece contact are sized differently**, because a piece resting on
//! something passes the tank's load on to what holds it, and only avian's solver sees that.
//! - *The tank's side* is the track law: a piece supports and grips the tank as ground does, with
//!   the slip and the support damping measured against the piece's own velocity at the contact
//!   ([`RubbleBodies::velocity_at`]).
//! - *The piece's side* is the clamped contact impulse of sequential-impulse solvers (Catto, the
//!   family avian's own solver belongs to), one pass over the tick's contacts in emission order,
//!   each sized after the impulses before it: along the normal, the support reaction's impulse
//!   `load × dt`, at most what stops the piece closing on the belt; along the traction reaction's
//!   direction, its impulse, at most what stops the piece sliding against the belt surface's
//!   velocity there and at most μ × that normal impulse (μ the belt's against the piece's
//!   collider, combined as avian combines a pair). Each bound uses the piece's effective mass
//!   along it at the contact, `1 / (1/m + (r×d)·I⁻¹(r×d))`; the two components couple through
//!   the inertia, so both scale by one factor until neither passes its bound. Nothing beyond
//!   that reaches the piece; avian's own contacts resolve what holds it.
//! - **Accepted error**: a piece resting on nothing (a brick falling off a pile edge) still
//!   supports the tank for the ticks it is probed. The refinement, not built, limits the support
//!   by the impulses avian's contacts on the piece carried the tick before.
//! - **Rejected alternative**: kinematic belt colliders on the debris layer with avian's conveyor
//!   contacts (`ContactManifold::tangent_velocity`, set in `CollisionHooks::modify_contacts`)
//!   would let avian push pieces too, but they model the belt twice (probes hold the tank,
//!   colliders push pieces) and push without limit. It is the fallback if piles look wrong in
//!   play.

use avian3d::prelude::{
    ColliderAabb, ColliderOf, ComputedAngularInertia, ComputedCenterOfMass, ComputedMass,
    DefaultFriction, Forces, Friction, ReadRigidBodyForces, RigidBody, SpatialQuery,
    SpatialQueryFilter, WriteRigidBodyForces,
};
use bevy::ecs::query::QueryFilter;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::buildings::BuildingElement;
use crate::buildings::breakage::BuildingChunk;
use crate::debris::DebrisPiece;

use super::forces::BeltContact;
use super::oracle::{Probe, TerrainOracle};

/// The spatial query, the markers that say which colliders are rubble, and their friction.
#[derive(SystemParam)]
pub(crate) struct RubbleQuery<'w, 's> {
    spatial: SpatialQuery<'w, 's>,
    built: Query<'w, 's, (), Or<(With<BuildingElement>, With<BuildingChunk>)>>,
    collider_of: Query<'w, 's, &'static ColliderOf>,
    pieces: Query<'w, 's, (), With<DebrisPiece>>,
    frictions: Query<'w, 's, &'static Friction>,
    default_friction: Res<'w, DefaultFriction>,
}

impl RubbleQuery<'_, '_> {
    /// Whether `collider` is rubble (module doc).
    fn is_rubble(&self, collider: Entity) -> bool {
        self.built.contains(collider)
            || self
                .collider_of
                .get(collider)
                .is_ok_and(|of| self.pieces.contains(of.body))
    }

    /// The rubble colliders whose AABBs meet the sphere of `radius` about `center`, in the order
    /// avian's trees visit them.
    pub(crate) fn near(&self, center: Vec3, radius: f32) -> Vec<Entity> {
        let mut near = Vec::new();
        self.spatial.aabb_intersections_with_aabb_callback(
            ColliderAabb::new(center, Vec3::splat(radius)),
            |collider| {
                if self.is_rubble(collider) {
                    near.push(collider);
                }
                true
            },
        );
        near
    }

    /// The rigid body `collider` belongs to (itself when it has none).
    pub(crate) fn body_of(&self, collider: Entity) -> Entity {
        self.collider_of
            .get(collider)
            .map_or(collider, |of| of.body)
    }

    /// The friction avian's narrow phase gives `collider` in a pair: its own, else its body's,
    /// else [`DefaultFriction`].
    fn friction(&self, collider: Entity) -> Friction {
        self.frictions
            .get(collider)
            .or_else(|_| self.frictions.get(self.body_of(collider)))
            .copied()
            .unwrap_or(self.default_friction.0)
    }
}

/// What a dynamic piece's impulses read and write. Pieces lock no axes, so the effective mass
/// [`PieceBody`] computes is the one avian's impulse applies against.
pub(crate) type PieceData = (
    Forces,
    &'static RigidBody,
    &'static ComputedMass,
    &'static ComputedAngularInertia,
    &'static ComputedCenterOfMass,
);

/// A dynamic piece's rigid state at the tick's start, advanced by each impulse it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PieceBody {
    pub(crate) entity: Entity,
    /// World centre of mass (m).
    pub(crate) com: Vec3,
    /// 1/m (1/kg).
    pub(crate) inv_mass: f32,
    /// World inverse angular inertia.
    pub(crate) inv_inertia: Mat3,
    pub(crate) linear: Vec3,
    pub(crate) angular: Vec3,
}

impl PieceBody {
    /// The world velocity of the body's material at `point`.
    pub(crate) fn velocity_at(&self, point: Vec3) -> Vec3 {
        self.linear + self.angular.cross(point - self.com)
    }

    /// The effective mass along unit `dir` at `point`: `1 / (1/m + (r×d)·I⁻¹(r×d))`.
    fn effective_mass(&self, point: Vec3, dir: Vec3) -> f32 {
        let rd = (point - self.com).cross(dir);
        1.0 / (self.inv_mass + rd.dot(self.inv_inertia * rd))
    }

    /// The velocity change at `point` that `impulse` there makes.
    fn response(&self, impulse: Vec3, point: Vec3) -> Vec3 {
        let r = point - self.com;
        impulse * self.inv_mass + (self.inv_inertia * r.cross(impulse)).cross(r)
    }

    /// Take `impulse` at `point`: avian's `apply_linear_impulse_at_point` arithmetic.
    fn apply(&mut self, impulse: Vec3, point: Vec3) {
        self.linear += impulse * self.inv_mass;
        self.angular += self.inv_inertia * (point - self.com).cross(impulse);
    }
}

/// One rubble collider near the tank: its dynamic body's slot in [`RubbleBodies::bodies`]
/// (`None` for a static or kinematic one, which takes nothing), and μ against the belt.
#[derive(Clone, Copy, Debug)]
struct Surface {
    collider: Entity,
    body: Option<usize>,
    mu: f32,
}

/// The rubble near one tank this tick: each collider's surface and its dynamic bodies' states.
#[derive(Default)]
pub(crate) struct RubbleBodies {
    surfaces: Vec<Surface>,
    pub(crate) bodies: Vec<PieceBody>,
}

impl RubbleBodies {
    /// Read the `near` colliders' bodies out of `bodies`, with `belt` the belt surface's friction.
    pub(crate) fn gather<F: QueryFilter>(
        rubble: &RubbleQuery,
        near: &[Entity],
        belt: Friction,
        bodies: &Query<PieceData, F>,
    ) -> Self {
        let mut out = Self::default();
        for &collider in near {
            let body = rubble.body_of(collider);
            let mut slot = out.bodies.iter().position(|b| b.entity == body);
            if slot.is_none()
                && let Ok((forces, RigidBody::Dynamic, mass, inertia, com)) = bodies.get(body)
            {
                let rotation = forces.rotation().0;
                out.bodies.push(PieceBody {
                    entity: body,
                    com: forces.position().0 + rotation * com.0,
                    inv_mass: mass.inverse(),
                    inv_inertia: inertia.rotated(rotation).inverse().to_mat3(),
                    linear: forces.linear_velocity(),
                    angular: forces.angular_velocity(),
                });
                slot = Some(out.bodies.len() - 1);
            }
            out.surfaces.push(Surface {
                collider,
                body: slot,
                mu: belt.combine(rubble.friction(collider)).dynamic_coefficient,
            });
        }
        out
    }

    fn surface(&self, collider: Entity) -> Option<Surface> {
        self.surfaces
            .iter()
            .find(|s| s.collider == collider)
            .copied()
    }

    /// Whether `collider` belongs to a gathered dynamic piece.
    pub(crate) fn is_dynamic(&self, collider: Entity) -> bool {
        self.surface(collider).is_some_and(|s| s.body.is_some())
    }

    /// The world velocity of `collider`'s surface at `point`: its dynamic body's, zero for a
    /// static one or a collider not gathered.
    pub(crate) fn velocity_at(&self, collider: Entity, point: Vec3) -> Vec3 {
        self.surface(collider)
            .and_then(|s| s.body)
            .map_or(Vec3::ZERO, |slot| self.bodies[slot].velocity_at(point))
    }
}

/// A terrain oracle with the rubble near one tank folded on (module doc).
pub(crate) struct RubbleOracle<'a, 'w, 's, O> {
    pub(crate) ground: &'a O,
    pub(crate) rubble: &'a RubbleQuery<'w, 's>,
    /// [`RubbleQuery::near`] of this tank this tick.
    pub(crate) near: &'a [Entity],
    /// The near pieces' states, for [`TerrainOracle::surface_velocity`]; empty where nothing
    /// reads it.
    pub(crate) bodies: &'a RubbleBodies,
}

impl<O: TerrainOracle> TerrainOracle for RubbleOracle<'_, '_, '_, O> {
    fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
        self.probe(station, out, reach).depth
    }

    fn probe(&self, station: Vec3, out: Vec3, reach: f32) -> Probe {
        let ground = self.ground.probe(station, out, reach);
        if self.near.is_empty() {
            return ground;
        }
        let Ok(dir) = Dir3::new(out) else {
            return ground;
        };
        let Some(hit) = self.rubble.spatial.cast_ray_predicate(
            station - out * reach,
            dir,
            2.0 * reach,
            true,
            &SpatialQueryFilter::default(),
            &|collider| self.near.contains(&collider),
        ) else {
            return ground;
        };
        let depth = reach - hit.distance;
        if depth > ground.depth {
            Probe {
                depth,
                collider: Some(hit.entity),
            }
        } else {
            ground
        }
    }

    fn surface_velocity(&self, collider: Entity, point: Vec3) -> Vec3 {
        self.bodies.velocity_at(collider, point)
    }
}

/// The probe sphere's radius about the hull origin (m), DERIVED from the probe geometry: a ray
/// ends at most `|pin| + face + 2 × travel + reach` from the origin (station = pin + out ×
/// (face + travel), ray end = station + out × (reach + travel)), with `pin_radius` the farthest
/// pin-line point from the hull origin and `column` the widest lateral column offset.
pub(crate) fn probe_radius(
    pin_radius: f32,
    column: f32,
    face: f32,
    max_travel: f32,
    reach: f32,
) -> f32 {
    pin_radius + column + face + 2.0 * max_travel + reach
}

/// One impulse a dynamic piece took: `impulse` (N·s, world) at `point`, through `collider`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Push {
    pub(crate) body: Entity,
    pub(crate) collider: Entity,
    pub(crate) impulse: Vec3,
    pub(crate) point: Vec3,
}

/// The piece's side of every contact in `contacts` on a dynamic piece, in order, each taken
/// into `bodies` before the next is sized (module doc). One push per such contact, zero where
/// the piece is not closing on the belt.
pub(crate) fn piece_impulses(
    bodies: &mut RubbleBodies,
    contacts: &[BeltContact],
    dt: f32,
) -> Vec<Push> {
    let mut pushes = Vec::new();
    for contact in contacts {
        let Some(surface) = contact.collider.and_then(|c| bodies.surface(c)) else {
            continue;
        };
        let Some(slot) = surface.body else {
            continue;
        };
        let body = &mut bodies.bodies[slot];
        let impulse = contact_impulse(body, contact, surface.mu, dt);
        body.apply(impulse, contact.point);
        pushes.push(Push {
            body: body.entity,
            collider: surface.collider,
            impulse,
            point: contact.point,
        });
    }
    pushes
}

/// The impulse one contact puts on `body` (module doc). `contact.normal` points from the piece
/// to the belt and the traction is the belt's force on the hull, so the piece's side runs along
/// `-normal` and `-traction`.
fn contact_impulse(body: &PieceBody, contact: &BeltContact, mu: f32, dt: f32) -> Vec3 {
    let (point, normal) = (contact.point, contact.normal);
    // The piece's velocity against the belt surface at the contact.
    let rel = body.velocity_at(point) - contact.belt_velocity;
    let closing = rel.dot(normal);
    let normal_j = (contact.load * dt).min(body.effective_mass(point, normal) * closing.max(0.0));
    if normal_j <= 0.0 {
        return Vec3::ZERO;
    }
    let mut impulse = -normal * normal_j;
    let mut along = None;
    if let Ok(dir) = Dir3::new(-contact.traction) {
        let lag = -rel.dot(*dir);
        let tangent_j = (contact.traction.length() * dt)
            .min(body.effective_mass(point, *dir) * lag.max(0.0))
            .min(mu * normal_j);
        if tangent_j > 0.0 {
            impulse += *dir * tangent_j;
            along = Some((*dir, lag));
        }
    }
    let response = body.response(impulse, point);
    let mut scale = 1.0_f32;
    let normal_dv = response.dot(normal);
    if normal_dv < 0.0 {
        scale = scale.min(closing / -normal_dv);
    }
    if let Some((dir, lag)) = along {
        let tangent_dv = response.dot(dir);
        if tangent_dv > 0.0 {
            scale = scale.min(lag / tangent_dv);
        }
    }
    impulse * scale
}

/// Put every push on its body through avian, which wakes a sleeping body for a non-zero impulse.
pub(crate) fn apply_pushes<F: QueryFilter>(pushes: &[Push], bodies: &mut Query<PieceData, F>) {
    for push in pushes {
        if let Ok((mut forces, ..)) = bodies.get_mut(push.body) {
            forces.apply_linear_impulse_at_point(push.impulse, push.point);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use avian3d::prelude::{
        Collider, ColliderDensity, Gravity, LinearVelocity, PhysicsPlugins, Position, Rotation,
    };
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::{Affine3A, Quat, Vec2};
    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::track::forces::{
        ForceParams, GripElements, SideInput, SideReport, SideState, TravelField, contact_side,
        grip_stiffness,
    };
    use crate::track::oracle::{BlockField, TerrainBlock};

    /// One fixed tick (s).
    const DT: f32 = 1.0 / 64.0;

    /// Avian at 64 Hz, one fixed tick per update, under `gravity`.
    fn physics_with(gravity: Vec3) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity(gravity))
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

    /// No gravity: a body moves only by what the test applies.
    fn physics() -> App {
        physics_with(Vec3::ZERO)
    }

    /// A loose box of `size` centred at `at`, at a brick's density (fixture data).
    fn piece_box(app: &mut App, at: Vec3, size: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                DebrisPiece,
                Position(at),
                Rotation::default(),
                RigidBody::Dynamic,
                Collider::cuboid(size.x, size.y, size.z),
                ColliderDensity(1800.0),
            ))
            .id()
    }

    /// A 2 × 0.4 × 2 m loose piece whose top is at y = 0.4, centred at `(x, z)` (fixture data).
    fn piece(app: &mut App, x: f32, z: f32) -> Entity {
        piece_box(app, Vec3::new(x, 0.2, z), Vec3::new(2.0, 0.4, 2.0))
    }

    /// A static building root holding one 2 × 0.6 × 2 m standing chunk whose top is at y = 0.6,
    /// centred at `(x, z)` (fixture data); returns the root and the chunk.
    fn standing_chunk(app: &mut App, x: f32, z: f32) -> (Entity, Entity) {
        let root = app
            .world_mut()
            .spawn((
                Transform::from_xyz(x, 0.3, z),
                Position(Vec3::new(x, 0.3, z)),
                RigidBody::Static,
            ))
            .id();
        let chunk = app
            .world_mut()
            .spawn((
                ChildOf(root),
                Transform::IDENTITY,
                Collider::cuboid(2.0, 0.6, 2.0),
                BuildingChunk(0),
            ))
            .id();
        (root, chunk)
    }

    /// Flat terrain field, top face at y = 0.
    fn ground() -> BlockField {
        BlockField::new(vec![TerrainBlock::new(
            Vec3::new(0.0, -0.5, 0.0),
            Quat::IDENTITY,
            Vec3::new(200.0, 1.0, 200.0),
        )])
    }

    /// Probe `station` straight down at reach 0.5 m through the rubble near it.
    fn probe_down(app: &mut App, station: Vec3) -> Probe {
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery| {
                let field = ground();
                let near = rubble.near(station, 1.0);
                let oracle = RubbleOracle {
                    ground: &field,
                    rubble: &rubble,
                    near: &near,
                    bodies: &RubbleBodies::default(),
                };
                oracle.probe(station, Vec3::NEG_Y, 0.5)
            })
            .expect("the probe system runs")
    }

    /// A PROBE OVER A PIECE REPORTS THE PIECE'S SURFACE, AND ONLY RUBBLE COUNTS: the track
    /// model's support datum (`forces::contact_side`) reads this depth, so a probe blind to
    /// pieces leaves a tank's belts sinking through rubble its hull then slides on, and a probe
    /// that took any collider would make terrain colliders and a hull's own proxies its ground.
    /// Over a piece (top 0.4 m) a station at 0.3 m is 0.1 m deep and names the piece; over a
    /// standing chunk (top 0.6 m) 0.3 m deep and names the chunk; over an unmarked collider of
    /// the piece's size, and over open ground, it reads the field's own bits and no collider.
    #[test]
    fn a_probe_over_a_piece_reports_the_piece_surface() {
        let mut app = physics();
        let loose = piece(&mut app, 0.0, 0.0);
        let (_, chunk) = standing_chunk(&mut app, 10.0, 0.0);
        app.world_mut().spawn((
            Position(Vec3::new(20.0, 0.2, 0.0)),
            RigidBody::Static,
            Collider::cuboid(2.0, 0.4, 2.0),
        ));
        app.update();

        let on_piece = probe_down(&mut app, Vec3::new(0.0, 0.3, 0.0));
        assert!((on_piece.depth - 0.1).abs() < 1e-5, "{on_piece:?}");
        assert_eq!(on_piece.collider, Some(loose));

        let on_chunk = probe_down(&mut app, Vec3::new(10.0, 0.3, 0.0));
        assert!((on_chunk.depth - 0.3).abs() < 1e-5, "{on_chunk:?}");
        assert_eq!(on_chunk.collider, Some(chunk));

        let field = ground().depth_along(Vec3::new(0.0, 0.3, 0.0), Vec3::NEG_Y, 0.5);
        for open in [Vec3::new(20.0, 0.3, 0.0), Vec3::new(-10.0, 0.3, 0.0)] {
            let probe = probe_down(&mut app, open);
            assert_eq!(probe.depth.to_bits(), field.to_bits(), "{open}: {probe:?}");
            assert_eq!(probe.collider, None, "{open}");
        }
    }

    /// A contact pressing `load` (N) along +Y on `collider` at `point`, belt at rest, no traction.
    fn press(point: Vec3, collider: Entity, load: f32) -> BeltContact {
        BeltContact {
            point,
            load,
            load_elastic: load,
            slip: 0.0,
            slip_lat: 0.0,
            f_long: 0.0,
            f_lat: 0.0,
            normal: Vec3::Y,
            traction: Vec3::ZERO,
            belt_velocity: Vec3::ZERO,
            collider: Some(collider),
        }
    }

    /// A PIECE TAKES THE SUPPORT REACTION ONLY UP TO WHAT STOPS IT CLOSING ON THE BELT, AND A
    /// STANDING CHUNK TAKES NONE: the piece's side of a belt contact is a clamped impulse, so a
    /// 16 kg brick closing on a resting belt at 1 m/s under ten 10 kN columns ends the tick at
    /// the belt's velocity (the raw reaction would launch it at ~96 m/s, and ten impulses each
    /// sized against the tick-start velocity at ~9 m/s), while a 2.9 t piece, which the reaction
    /// cannot stop in one tick, takes the whole reaction, `−F·dt/m`. The static chunk's root
    /// never takes a push. Avian's own step then runs on the velocities these leave.
    #[test]
    fn a_piece_takes_the_reaction_only_up_to_what_stops_it_closing() {
        let mut app = physics();
        let light = piece_box(&mut app, Vec3::ZERO, Vec3::new(0.3, 0.1, 0.3));
        let heavy = piece(&mut app, 10.0, 0.0);
        let (root, chunk) = standing_chunk(&mut app, 20.0, 0.0);
        app.update();
        for body in [light, heavy] {
            app.world_mut().get_mut::<LinearVelocity>(body).unwrap().0 = Vec3::Y;
        }

        let load = 10_000.0;
        let mut contacts: Vec<BeltContact> = (0..10)
            .map(|_| press(Vec3::new(0.0, 0.05, 0.0), light, load))
            .collect();
        contacts.push(press(Vec3::new(10.0, 0.4, 0.0), heavy, load));
        contacts.push(press(Vec3::new(20.0, 0.6, 0.0), chunk, load));
        let pushes = app
            .world_mut()
            .run_system_once(move |rubble: RubbleQuery, mut bodies: Query<PieceData>| {
                let near = rubble.near(Vec3::new(10.0, 0.0, 0.0), 15.0);
                let mut pieces = RubbleBodies::gather(&rubble, &near, Friction::new(0.9), &bodies);
                let pushes = piece_impulses(&mut pieces, &contacts, DT);
                apply_pushes(&pushes, &mut bodies);
                pushes
            })
            .expect("the impulse system runs");
        app.update();

        assert_eq!(pushes.len(), 11, "{pushes:?}");
        assert!(pushes.iter().all(|p| p.body != root));
        let light_v = app.world().get::<LinearVelocity>(light).unwrap().0;
        assert!(light_v.length() < 1e-4, "the light piece left at {light_v}");
        let heavy_v = app.world().get::<LinearVelocity>(heavy).unwrap().0;
        let expected = 1.0 - load * DT / (2.0 * 0.4 * 2.0 * 1800.0);
        assert!(
            (heavy_v - Vec3::Y * expected).length() < 1e-4,
            "the heavy piece took {heavy_v}, expected {expected}"
        );
    }

    /// A deterministic stream of fixture values in `[0, 1)`.
    struct Lcg(u64);

    impl Lcg {
        fn unit(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.unit()
        }
        fn vec(&mut self, half: f32) -> Vec3 {
            Vec3::new(
                self.range(-half, half),
                self.range(-half, half),
                self.range(-half, half),
            )
        }
    }

    /// THE IMPULSE ON A PIECE NEVER DRIVES IT PAST THE BELT, AND ITS TANGENTIAL PART NEVER
    /// EXCEEDS μ × ITS NORMAL PART: the clamp is what keeps a light piece from being flung, so
    /// over 4000 fixture contacts (pieces of 5 kg to 3 t with skewed inertia, off-centre contacts,
    /// any normal, traction and belt velocity) the normal impulse lies in `[0, load·dt]`, the
    /// tangential one within `|traction|·dt` and μ × the normal one, and after the impulse the
    /// piece is still closing on the belt or level with it along the normal, and still behind
    /// the belt surface or level with it along the tangential impulse's direction. A clamp
    /// sized without the inertia coupling, a cap on the wrong side, or a friction bound off the
    /// support reaction instead of the clamped impulse fails here; every clamp is seen to bind.
    #[test]
    fn the_impulse_never_drives_a_piece_past_the_belt() {
        let mut rng = Lcg(0x5eed);
        let (mut by_closing, mut by_load, mut by_cone, mut by_lag) = (0, 0, 0, 0);
        for case in 0..4000 {
            let mass = rng.range(5.0, 3000.0);
            let axes = Quat::from_euler(
                EulerRot::XYZ,
                rng.range(-3.0, 3.0),
                rng.range(-3.0, 3.0),
                rng.range(-3.0, 3.0),
            );
            let principal = Vec3::new(
                rng.range(0.02, 1.0),
                rng.range(0.02, 1.0),
                rng.range(0.02, 1.0),
            ) * mass
                * 0.1;
            let frame = Mat3::from_quat(axes);
            let body = PieceBody {
                entity: Entity::PLACEHOLDER,
                com: rng.vec(1.0),
                inv_mass: 1.0 / mass,
                inv_inertia: frame * Mat3::from_diagonal(principal.recip()) * frame.transpose(),
                linear: rng.vec(3.0),
                angular: rng.vec(5.0),
            };
            let normal = rng.vec(1.0).normalize();
            let tangent = rng.vec(1.0).reject_from(normal) * rng.range(0.0, 20_000.0);
            let contact = BeltContact {
                point: body.com + rng.vec(0.5),
                load: rng.range(0.0, 20_000.0),
                load_elastic: 0.0,
                slip: 0.0,
                slip_lat: 0.0,
                f_long: 0.0,
                f_lat: 0.0,
                normal,
                traction: tangent,
                belt_velocity: rng.vec(3.0),
                collider: None,
            };
            let mu = rng.range(0.0, 1.2);
            let impulse = contact_impulse(&body, &contact, mu, DT);

            let normal_j = -impulse.dot(normal);
            let tangent_j = (impulse + normal * normal_j).length();
            let tol = 1e-4 * (1.0 + impulse.length());
            assert!(normal_j >= -tol, "case {case}: pulled at {normal_j}");
            assert!(
                normal_j <= contact.load * DT + tol,
                "case {case}: {normal_j}"
            );
            assert!(
                tangent_j <= mu * normal_j + tol,
                "case {case}: outside the cone"
            );
            assert!(
                tangent_j <= contact.traction.length() * DT + tol,
                "case {case}"
            );

            let before = body.velocity_at(contact.point) - contact.belt_velocity;
            let mut after_body = body;
            after_body.apply(impulse, contact.point);
            let after = after_body.velocity_at(contact.point) - contact.belt_velocity;
            let vtol = 1e-4 * (1.0 + before.length());
            if normal_j > 0.0 {
                assert!(
                    after.dot(normal) >= -vtol,
                    "case {case}: driven past the belt along the normal, {} → {}",
                    before.dot(normal),
                    after.dot(normal),
                );
            }
            if let Ok(dir) = Dir3::new(-contact.traction)
                && impulse.dot(*dir) > tol
            {
                assert!(
                    -after.dot(*dir) >= -vtol,
                    "case {case}: driven past the belt surface, {} → {}",
                    -before.dot(*dir),
                    -after.dot(*dir),
                );
            }

            if normal_j > tol {
                if normal_j < contact.load * DT * 0.999 {
                    by_closing += 1;
                } else {
                    by_load += 1;
                }
                if tangent_j > tol && tangent_j > mu * normal_j * 0.999 {
                    by_cone += 1;
                } else if tangent_j > tol {
                    by_lag += 1;
                }
            }
        }
        for (name, count) in [
            ("closing", by_closing),
            ("load", by_load),
            ("cone", by_cone),
            ("lag", by_lag),
        ] {
            assert!(count > 20, "the {name} bound bound only {count} times");
        }
    }

    /// One contact pass of a 2 m pad (pins at local y = 0.02, the face 25 mm out) under a hull at
    /// `affine`, belt at `state`, moving at `vel` everywhere (fixture data).
    fn pad_pass<O: TerrainOracle>(
        oracle: &O,
        affine: Affine3A,
        state: SideState,
        command: f32,
        vel: Vec3,
        elements: &mut GripElements,
    ) -> SideReport {
        let loop_pts = [
            Vec2::new(-1.0, 0.02),
            Vec2::new(1.0, 0.02),
            Vec2::new(1.0, 0.52),
            Vec2::new(-1.0, 0.52),
            Vec2::new(-1.0, 0.02),
        ];
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: [(-0.1, 0.25), (0.0, 0.5), (0.1, 0.25)],
            command,
            travel: TravelField { knots: &[] },
        };
        let params = ForceParams {
            face_offset: 0.025,
            free_travel: 0.0,
            support_stiffness_per_m: 2.0e6,
            support_damping_per_m: 1.0e5,
            engage_depth: 0.002,
            probe_reach: 0.5,
            mu: 0.9,
            slip_saturation: 0.5,
            max_speed: 10.0,
            engine_power: 1.0e5,
            engine_force: 1.0e5,
            governor_gain: 1.0e4,
            inertia: 500.0,
            grip_stiffness: grip_stiffness(0.9, 30_000.0 * 9.81),
        };
        let (report, live) = contact_side(
            &input,
            state,
            affine,
            DT,
            &params,
            oracle,
            |_| vel,
            elements,
        );
        assert!(live);
        report
    }

    /// The fixture pad 5 mm into flat ground at y = 0, driving and sliding.
    fn flat_pass<O: TerrainOracle>(oracle: &O) -> SideReport {
        pad_pass(
            oracle,
            Affine3A::IDENTITY,
            SideState {
                speed: 0.4,
                phase: 0.0,
                grip: Vec2::ZERO,
            },
            0.3,
            Vec3::new(0.0, -0.01, 0.3),
            &mut GripElements::for_links(25),
        )
    }

    /// Park a 6 t hull (fixture data) on the fixture pad for `ticks`, dropped at height `h` and
    /// integrated vertically alone, the world stepped once per tick with the pieces' impulses
    /// applied; returns the hull's final height.
    fn park(app: &mut App, mut h: f32, ticks: usize) -> f32 {
        const MASS: f32 = 6000.0;
        let mut vy = 0.0;
        let mut elements = GripElements::for_links(25);
        for _ in 0..ticks {
            let (lift, kept) = app
                .world_mut()
                .run_system_once(move |rubble: RubbleQuery, mut bodies: Query<PieceData>| {
                    let mut elements = std::mem::take(&mut elements);
                    let field = ground();
                    let near = rubble.near(Vec3::new(0.0, h, 0.0), 3.0);
                    let mut pieces =
                        RubbleBodies::gather(&rubble, &near, Friction::new(0.9), &bodies);
                    let report = pad_pass(
                        &RubbleOracle {
                            ground: &field,
                            rubble: &rubble,
                            near: &near,
                            bodies: &pieces,
                        },
                        Affine3A::from_translation(Vec3::new(0.0, h, 0.0)),
                        SideState::default(),
                        0.0,
                        Vec3::new(0.0, vy, 0.0),
                        &mut elements,
                    );
                    let pushes = piece_impulses(&mut pieces, &report.contacts, DT);
                    apply_pushes(&pushes, &mut bodies);
                    let lift: f32 = report.apps.iter().map(|app| app.force.y).sum();
                    (lift, elements)
                })
                .expect("the parked tick runs");
            elements = kept;
            vy += (lift / MASS - 9.81) * DT;
            h += vy * DT;
            app.update();
        }
        h
    }

    /// A TANK PARKED ON A BRICK LYING ON FLAT GROUND KEEPS ITS RIDE HEIGHT, AND THE BRICK STAYS
    /// PUT: a piece supports the tank as ground does, and what the piece takes back is only what
    /// stops it closing on the belt, so a 173 kg brick under a 6 t hull's 59 kN neither sinks
    /// into the ground under it nor shifts. The hull settles exactly as high above the brick's
    /// top as above bare ground (3 s, from a drop at first touch). A brick that took the raw
    /// reaction instead shifts 5 cm in those 3 s (MEASURED, that mutant); a support blind to the
    /// brick lets the hull sink to the ground.
    #[test]
    fn a_tank_parked_on_a_brick_keeps_its_ride_height() {
        let gravity = Vec3::NEG_Y * 9.81;
        let mut bare = physics_with(gravity);
        bare.update();
        let on_bare = park(&mut bare, 0.0, 192);

        let mut app = physics_with(gravity);
        app.world_mut().spawn((
            Position(Vec3::new(0.0, -0.5, 0.0)),
            RigidBody::Static,
            Collider::cuboid(20.0, 1.0, 20.0),
        ));
        let brick = piece_box(
            &mut app,
            Vec3::new(0.0, 0.05, 0.0),
            Vec3::new(0.4, 0.1, 2.4),
        );
        for _ in 0..64 {
            app.update();
        }
        let rest = app.world().get::<Position>(brick).unwrap().0;
        let on_brick = park(&mut app, rest.y + 0.05, 192);
        let after = app.world().get::<Position>(brick).unwrap().0;

        assert!(
            (after - rest).length() < 1e-3,
            "the brick moved {rest} → {after}"
        );
        let top = after.y + 0.05;
        assert!(
            ((on_brick - top) - on_bare).abs() < 5e-4,
            "ride height {} above the brick, {on_bare} above bare ground",
            on_brick - top,
        );
    }

    /// THE TANK'S SLIP ON A PIECE IS MEASURED AGAINST THE PIECE: a pad over a piece that moves
    /// with the hull sees the slip the belt speed alone makes, as on ground that moves with it,
    /// so a piece carried along under a track is not ground streaming past. Over the static
    /// field the same pad slips at belt speed less the hull's ground speed.
    #[test]
    fn the_slip_on_a_piece_is_measured_against_the_piece() {
        let mut app = physics();
        let carried = piece(&mut app, 0.0, 0.0);
        app.update();
        // The fixture pad drives along −Z.
        let hull_vel = Vec3::new(0.0, 0.0, -0.3);
        app.world_mut()
            .get_mut::<LinearVelocity>(carried)
            .unwrap()
            .0 = hull_vel;
        let (on_piece, on_field) = app
            .world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let field = ground();
                let near = rubble.near(Vec3::ZERO, 3.0);
                let pieces = RubbleBodies::gather(&rubble, &near, Friction::new(0.9), &bodies);
                let pass = |oracle: &dyn Fn(&mut GripElements) -> SideReport| {
                    oracle(&mut GripElements::for_links(25))
                };
                let state = SideState {
                    speed: 0.4,
                    phase: 0.0,
                    grip: Vec2::ZERO,
                };
                let lifted = Affine3A::from_translation(Vec3::new(0.0, 0.4, 0.0));
                let on_piece = pass(&|elements| {
                    pad_pass(
                        &RubbleOracle {
                            ground: &field,
                            rubble: &rubble,
                            near: &near,
                            bodies: &pieces,
                        },
                        lifted,
                        state,
                        0.3,
                        hull_vel,
                        elements,
                    )
                });
                let on_field = pass(&|elements| {
                    pad_pass(&field, Affine3A::IDENTITY, state, 0.3, hull_vel, elements)
                });
                (
                    on_piece.contacts.iter().map(|c| c.slip).collect::<Vec<_>>(),
                    on_field.contacts.iter().map(|c| c.slip).collect::<Vec<_>>(),
                )
            })
            .expect("the slip system runs");
        assert!(!on_piece.is_empty() && !on_field.is_empty());
        for slip in on_piece {
            assert!(
                (slip - 0.4).abs() < 1e-5,
                "slip {slip} on the carried piece"
            );
        }
        for slip in on_field {
            assert!((slip - 0.1).abs() < 1e-5, "slip {slip} on the field");
        }
    }

    /// Every bit of a report's force applications, contacts and belt reaction.
    fn bits(report: &SideReport) -> Vec<u32> {
        let mut out = Vec::new();
        for app in &report.apps {
            out.extend(app.force.to_array().map(f32::to_bits));
            out.extend(app.point.to_array().map(f32::to_bits));
            assert_eq!(app.collider, None);
        }
        for c in &report.contacts {
            out.extend(
                [
                    c.load,
                    c.load_elastic,
                    c.slip,
                    c.slip_lat,
                    c.f_long,
                    c.f_lat,
                ]
                .map(f32::to_bits),
            );
        }
        out.push(report.belt_reaction.to_bits());
        out
    }

    /// FLAT GROUND WITH NO PIECES GIVES BIT-IDENTICAL TRACK FORCES: the rubble fold is free where
    /// there is no rubble, so a tank on open ground keeps the exact arithmetic of the field-only
    /// oracle (`track::sim` runs `BlockField` alone). A fold that re-derived the ground, took a
    /// collider hit of equal depth over the field's, or measured the slip against a surface
    /// velocity where none was hit, would move bits. Checked with nothing near (no ray cast) and
    /// with a piece 50 m away forced into the near list and gathered (rays cast, every one
    /// losing to the field).
    #[test]
    fn flat_ground_with_no_pieces_gives_bit_identical_track_forces() {
        let reference = bits(&flat_pass(&ground()));
        assert!(reference.len() > 1, "the fixture pad is in contact");

        let mut app = physics();
        let far = piece(&mut app, 50.0, 0.0);
        app.update();
        app.world_mut().get_mut::<LinearVelocity>(far).unwrap().0 = Vec3::new(1.0, 2.0, 3.0);
        let (nothing_near, far_near) = app
            .world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let field = ground();
                let empty = RubbleOracle {
                    ground: &field,
                    rubble: &rubble,
                    near: &[],
                    bodies: &RubbleBodies::default(),
                };
                let near = [far];
                let pieces = RubbleBodies::gather(&rubble, &near, Friction::new(0.9), &bodies);
                let forced = RubbleOracle {
                    ground: &field,
                    rubble: &rubble,
                    near: &near,
                    bodies: &pieces,
                };
                (bits(&flat_pass(&empty)), bits(&flat_pass(&forced)))
            })
            .expect("the contact system runs");
        assert_eq!(nothing_near, reference);
        assert_eq!(far_near, reference);
    }
}
