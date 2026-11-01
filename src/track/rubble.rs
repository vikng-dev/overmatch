//! Buildings and rubble under the running gear, in the game's tank driving physics (game state
//! only). The terrain field ([`super::oracle::BlockField`]) is the heightmap and the authored
//! blocks; a building's elements and standing chunks and every loose piece are Avian colliders the
//! field never holds. [`RubbleBodies`] snapshots those colliders near one tank, the belt's support
//! reads each through a convex shape check over every link's footprint ([`RubbleBodies::in_slab`])
//! at the ground's own per-metre spring. In the same tick [`piece_impulses`] sizes what a loose
//! piece takes back, and [`RubbleBodies::record_static_contacts`] stages the whole opposite
//! reaction on fixed structural construction.
//!
//! - **What the belt bears on** ([`RubbleQuery::supports`]): solid, layer-compatible colliders,
//!   except [`super::terrain::FieldSurface`] geometry already represented by the field. Callers
//!   exclude self-support by identity ([`RubbleBodies::exclude_source`]). Every admitted dynamic
//!   body carries Avian force state; static construction transmits the full opposite belt
//!   reaction to [`crate::contact_transfer::ContactTransfers`] when it has structural laws.
//! - **The footprint check**: a link's slab ([`Slab`]) is the volume its field probes sweep, made
//!   solid across the true shoe width: along the link, between its measured faces, and from the
//!   probes' origin out to the contact envelope. A piece's extent in it is six support points of
//!   its own convex shape (along, across and through the link), so a piece narrower than the gap
//!   between the probe columns is read. The convex boundary is clipped to the footprint before
//!   its depth is read: a remote vertex and an empty convex corner cannot become support.
//! - **One support law**: `forces::contact_side` partitions overlapping footprints; the deepest
//!   surface in each cell enters the field's own support law at its covered share of the shoe.
//!   Stacked pieces cannot multiply the hull's support.
//! - **Cost gate**: one AABB query per tank per tick over the footprint's bounding sphere
//!   ([`RubbleQuery::near`], [`footprint_radius`]). With no rubble collider in it no shape is
//!   read, so open ground is bit-identical to the field alone.
//! - **Tick truth**: avian refits dynamic bodies' AABBs at the end of its physics step
//!   (`update_solver_body_aabbs`) and writes each collider's `Position`/`Rotation`, so a snapshot
//!   in `FixedUpdate` reads the poses the last step wrote.
//!
//! **What holds the recipient decides its side of a belt contact.** A body whose own support is
//! a belt law this tick (another driving tank's hull) carries a load through that law, which
//! responds to force: it takes the whole opposite reaction, `−(normal × load + traction) × dt`,
//! as fixed construction does, resting or not. Every other dynamic body is a *piece*: one resting
//! on something passes the tank's load on to what holds it, and only avian's solver sees that, so
//! the two sides of a belt-piece contact are sized differently.
//! - *The tank's side* is the track law: a piece supports and grips the tank as ground does, with
//!   the slip and the support damping measured against the piece's own velocity at the contact
//!   ([`RubbleBodies::velocity_at`]).
//! - *The piece's side* is the clamped contact impulse of sequential-impulse solvers (Catto, the
//!   family avian's own solver belongs to), one pass over the tick's contacts in emission order,
//!   each sized after the impulses before it. The push, along the normal: the support reaction's
//!   impulse `load × dt`, at most what stops the piece closing on the belt. Then the drag, along
//!   the traction reaction's direction: its impulse, at most Coulomb's `μ × load × dt` (the
//!   normal force the belt presses with over the tick, whether or not the piece is closing, so a
//!   parked or pivoting hull still drags what it stands on; μ the contact's own pair, the track's
//!   friction on the piece's ([`super::forces::BeltContact::mu`]), the same coefficient that
//!   caps the hull's grip there), and at most what brings the piece, already pushed, to the belt
//!   surface's velocity there. Each bound uses the piece's effective mass along it at the
//!   contact, `1 / (1/m + (r×d)·I⁻¹(r×d))`, so neither part drives the piece past the belt along
//!   its own direction. The drag's turn of the piece may open the contact along the normal (a
//!   brick tipping under the belt), which no bound stops: avian's own contacts resolve what holds
//!   the piece. Nothing beyond that reaches it.
//! - **Accepted error**: a piece resting on nothing (a brick falling off a pile edge) still
//!   supports the tank for the ticks it is read. Avian alone sees what holds the piece; that
//!   support is not limited by its previous contacts. Dynamic-piece velocities are advanced by
//!   each clamped impulse before the next is sized, over every tank's contacts at once in one
//!   canonical order of their geometry ([`piece_impulses`]), so no tank's entity order sizes
//!   another's reaction.
//! - **Rejected alternatives**: probe rays folded onto the colliders (each probe also a ray
//!   against the rubble, the deepest hit wins) see only the three column lines, so a brick between
//!   them sits under the belt unseen. Kinematic belt colliders with avian's conveyor contacts
//!   (`ContactManifold::tangent_velocity`, set in `CollisionHooks::modify_contacts`) model the
//!   belt twice (the field holds the tank, colliders push pieces) and push without limit.

use avian3d::dynamics::solver::solver_body::SolverBodyInertia;
use avian3d::prelude::{
    Collider, ColliderAabb, ColliderOf, CollisionLayers, ComputedAngularInertia,
    ComputedCenterOfMass, ComputedMass, DefaultFriction, Forces, Friction, LayerMask, LockedAxes,
    Position, ReadRigidBodyForces, RigidBody, Rotation, Sensor, SpatialQuery, WriteRigidBodyForces,
};
use bevy::ecs::query::QueryFilter;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::terrain::FieldSurface;
use crate::Layer;
use crate::contact_transfer::{AppliedContact, ContactTransfers, StructureBody};

use super::forces::BeltContact;

/// The contact class the belt bears with: the hull's own collision proxies'. A collider whose
/// filters exclude the hull (a tank's armour and crew volumes) is no surface for the belt either.
pub(crate) fn belt_layers() -> CollisionLayers {
    CollisionLayers::new([Layer::Vehicle], LayerMask::ALL)
}

/// The spatial query, what makes a collider a surface the belt bears on, its shape and friction.
#[derive(SystemParam)]
pub(crate) struct RubbleQuery<'w, 's> {
    spatial: SpatialQuery<'w, 's>,
    field_surfaces: Query<'w, 's, (), With<FieldSurface>>,
    structures: Query<'w, 's, (), With<StructureBody>>,
    collider_of: Query<'w, 's, &'static ColliderOf>,
    sensors: Query<'w, 's, (), With<Sensor>>,
    layers: Query<'w, 's, &'static CollisionLayers>,
    rigid_bodies: Query<'w, 's, &'static RigidBody>,
    shapes: Query<
        'w,
        's,
        (
            &'static Collider,
            &'static Position,
            &'static Rotation,
            &'static ColliderAabb,
        ),
    >,
    frictions: Query<'w, 's, &'static Friction>,
    default_friction: Res<'w, DefaultFriction>,
}

impl RubbleQuery<'_, '_> {
    /// Whether `collider` is a surface the belt bears on (module doc). A collider with no rigid
    /// body is static to the solver, and is read as one here.
    pub(crate) fn supports(&self, collider: Entity) -> bool {
        let layers = self
            .layers
            .get(collider)
            .expect("avian requires CollisionLayers on every Collider");
        !self.sensors.contains(collider)
            && !self.field_surfaces.contains(collider)
            && layers.interacts_with(belt_layers())
    }

    /// The support colliders meeting the AABB of half-extent `radius` about `center`, ascending
    /// by entity, so their contacts and the force sums over them keep one order.
    pub(crate) fn near(&self, center: Vec3, radius: f32) -> Vec<Entity> {
        let mut near = Vec::new();
        self.spatial.aabb_intersections_with_aabb_callback(
            ColliderAabb::new(center, Vec3::splat(radius)),
            |collider| {
                if self.supports(collider) {
                    near.push(collider);
                }
                true
            },
        );
        near.sort_unstable();
        near.dedup();
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

/// What a dynamic support's impulses read and write, including any locked motion axes.
pub(crate) type PieceData = (
    Forces,
    &'static RigidBody,
    &'static ComputedMass,
    &'static ComputedAngularInertia,
    &'static ComputedCenterOfMass,
    Option<&'static LockedAxes>,
);

/// A dynamic piece's rigid state at the tick's start, advanced by each impulse it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PieceBody {
    pub(crate) entity: Entity,
    /// World centre of mass (m).
    pub(crate) com: Vec3,
    /// World inverse mass per permitted translation axis (1/kg).
    pub(crate) inv_mass: Vec3,
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
        1.0 / (dir.dot(self.inv_mass * dir) + rd.dot(self.inv_inertia * rd))
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

/// One rubble collider near the tank, as the footprint check reads it: its convex shape at its
/// tick-start world pose and bounds, its owning rigid body and its dynamic body's slot in [`RubbleBodies::bodies`] (`None`
/// for a static or kinematic one, which takes no impulse), the structure whose load reading takes
/// a fixed one's reaction, and the friction its surface brings to a pair.
#[derive(Clone, Debug)]
struct Surface {
    collider: Entity,
    owner: Entity,
    shape: Collider,
    position: Vec3,
    rotation: Quat,
    aabb: ColliderAabb,
    /// The convex boundary at the same tick-start pose, for clipping support to a footprint.
    triangles: Vec<[Vec3; 3]>,
    body: Option<usize>,
    friction: Friction,
    structure: Option<Entity>,
    /// A kinematic surface moves without taking impulses; static surfaces have no motion.
    motion: Option<SurfaceMotion>,
}

#[derive(Clone, Debug)]
struct SurfaceMotion {
    com: Vec3,
    linear: Vec3,
    angular: Vec3,
}

impl Surface {
    fn triangles(shape: &Collider, position: Vec3, rotation: Quat) -> Vec<[Vec3; 3]> {
        let shape = shape.shape_scaled();
        let (vertices, indices) = if let Some(cuboid) = shape.as_cuboid() {
            cuboid.to_trimesh()
        } else if let Some(hull) = shape.as_convex_polyhedron() {
            hull.to_trimesh()
        } else {
            panic!(
                "belt support shapes are boxes or convex polyhedra; fix the collider constructor"
            );
        };
        let world: Vec<_> = vertices
            .into_iter()
            .map(|v| position + rotation * v)
            .collect();
        indices
            .into_iter()
            .map(|tri| tri.map(|i| world[i as usize]))
            .collect()
    }

    /// The shape's farthest world point along unit `dir`.
    fn support(&self, dir: Vec3) -> Vec3 {
        let map = self
            .shape
            .shape_scaled()
            .as_support_map()
            .unwrap_or_else(|| {
                panic!(
                    "rubble collider {:?} is not convex: support surfaces \
                 are convex, the shapes the belt's footprint check reads",
                    self.collider
                )
            });
        self.position + self.rotation * map.local_support_point(self.rotation.inverse() * dir)
    }

    /// Extrema of the convex shape *inside* the footprint's four side planes. Projected support
    /// intervals alone can overlap at an empty corner, or pick the remote end of a long piece
    /// outside an angled link. Clipping the actual boundary supplies the missing constraints.
    fn clipped_extent(&self, slab: &Slab) -> Option<(Vec3, Vec3)> {
        let mut lo = Vec3::INFINITY;
        let mut hi = Vec3::NEG_INFINITY;
        let mut area = false;
        for triangle in &self.triangles {
            let mut polygon: Vec<_> = triangle
                .iter()
                .map(|p| {
                    let r = *p - slab.origin;
                    Vec3::new(r.dot(slab.axis), r.dot(slab.lat), r.dot(slab.out))
                })
                .collect();
            for (axis, sign, bound) in [
                (0, 1.0, 0.0),
                (0, -1.0, -slab.len),
                (1, 1.0, slab.lateral.0),
                (1, -1.0, -slab.lateral.1),
            ] {
                polygon = clip_polygon(&polygon, axis, sign, bound);
            }
            if !polygon.is_empty() {
                let mut projected_area = 0.0;
                for i in 0..polygon.len() {
                    let a = polygon[i];
                    let b = polygon[(i + 1) % polygon.len()];
                    projected_area += a.x * b.y - a.y * b.x;
                    lo = lo.min(a);
                    hi = hi.max(a);
                }
                area |= projected_area.abs() > 0.0;
            }
        }
        area.then_some((lo, hi))
    }
}

/// Sutherland-Hodgman clipping to one footprint half-plane, in slab coordinates. Boundary
/// points stay in the polygon; a mere touching edge has no projected support area.
fn clip_polygon(points: &[Vec3], axis: usize, sign: f32, bound: f32) -> Vec<Vec3> {
    let mut out = Vec::new();
    if points.is_empty() {
        return out;
    }
    let mut a = points[points.len() - 1];
    let mut da = sign * a[axis] - bound;
    for &b in points {
        let db = sign * b[axis] - bound;
        if (da < 0.0 && db >= 0.0) || (da >= 0.0 && db < 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
        if db >= 0.0 {
            out.push(b);
        }
        a = b;
        da = db;
    }
    out
}

/// One link's slab: the volume its field probes sweep, solid across the shoe (module doc). World
/// axes: `axis` along the link from `origin` (its first pin), `lat` the hull's lateral axis,
/// `out` the link's outward normal.
pub(crate) struct Slab {
    pub(crate) origin: Vec3,
    pub(crate) axis: Vec3,
    pub(crate) lat: Vec3,
    pub(crate) out: Vec3,
    /// Link length (m).
    pub(crate) len: f32,
    /// The shoe's lateral faces as offsets along `lat` from the pin plane (m), low then high.
    pub(crate) lateral: (f32, f32),
    /// Pin line → rest outer face (m).
    pub(crate) face: f32,
    /// How far behind the rest face the probes start, and the depth they saturate at (m).
    pub(crate) reach: f32,
    /// The link's largest free travel past the rest face (m).
    pub(crate) travel: f32,
}

impl Slab {
    /// The slab's world bounds.
    fn aabb(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::INFINITY;
        let mut hi = Vec3::NEG_INFINITY;
        for u in [0.0, self.len] {
            for l in [self.lateral.0, self.lateral.1] {
                for t in [self.face + self.travel, self.face - self.reach] {
                    let p = self.origin + self.axis * u + self.lat * l + self.out * t;
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
        }
        (lo, hi)
    }
}

/// One piece's extent in a [`Slab`]: along the link (`u`, m from the first pin, clipped to the
/// link), across the shoe (`l`, offsets from the pin plane, clipped to the shoe), and `depth`,
/// how far its innermost point lies past the rest face toward the pins (m, saturated at the
/// probes' reach). The local free travel adds to `depth` exactly as it adds to a probe's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SlabOverlap {
    pub(crate) piece: usize,
    pub(crate) u: (f32, f32),
    pub(crate) l: (f32, f32),
    pub(crate) depth: f32,
}

/// The colliders near every driving tank this tick, read once at tick start: each collider's
/// surface and its dynamic bodies' states. Empty where no tank has any near it, and for every
/// pass that reads the field alone (the envelope calibration).
#[derive(Clone, Default)]
pub struct RubbleBodies {
    source: Option<Entity>,
    surfaces: Vec<Surface>,
    pub(crate) bodies: Vec<PieceBody>,
}

impl RubbleBodies {
    /// Read the `near` colliders' shapes, frictions and bodies. Every dynamic body must carry the
    /// force state Avian constructs for it, readable through `bodies`.
    pub(crate) fn gather<F: QueryFilter>(
        rubble: &RubbleQuery,
        near: &[Entity],
        bodies: &Query<PieceData, F>,
    ) -> Self {
        let mut out = Self::default();
        for &collider in near {
            let body = rubble.body_of(collider);
            // A collider without a rigid body is fixed construction in Avian.
            let kind = rubble
                .rigid_bodies
                .get(body)
                .copied()
                .unwrap_or(RigidBody::Static);
            let dynamic = kind.is_dynamic();
            let mut slot = out.bodies.iter().position(|b| b.entity == body);
            if slot.is_none() && dynamic {
                let (forces, _, mass, inertia, com, locked) = bodies.get(body).expect(
                    "an admitted dynamic support body carries Avian force state its caller can read",
                );
                let rotation = forces.rotation().0;
                // Avian uses this constructor to prepare solver inertia; it applies the same
                // axis locks as Forces::apply_linear_impulse_at_point.
                let inertia = SolverBodyInertia::new(
                    mass.inverse(),
                    inertia.rotated(rotation).inverse(),
                    locked.copied().unwrap_or_default(),
                    0,
                    true,
                );
                out.bodies.push(PieceBody {
                    entity: body,
                    com: forces.position().0 + rotation * com.0,
                    inv_mass: inertia.effective_inv_mass(),
                    inv_inertia: inertia.effective_inv_angular_inertia().to_mat3(),
                    linear: forces.linear_velocity(),
                    angular: forces.angular_velocity(),
                });
                slot = Some(out.bodies.len() - 1);
            }
            let motion = if kind.is_kinematic() {
                let (forces, _, _, _, com, _) = bodies.get(body).expect(
                    "an admitted kinematic support body carries Avian pose and velocity state",
                );
                Some(SurfaceMotion {
                    com: forces.position().0 + forces.rotation().0 * com.0,
                    linear: forces.linear_velocity(),
                    angular: forces.angular_velocity(),
                })
            } else {
                None
            };
            let (shape, position, rotation, aabb) = rubble
                .shapes
                .get(collider)
                .expect("a collider the spatial query returned carries its shape and pose");
            out.surfaces.push(Surface {
                collider,
                owner: body,
                shape: shape.clone(),
                position: position.0,
                rotation: rotation.0,
                aabb: *aabb,
                triangles: Surface::triangles(shape, position.0, rotation.0),
                body: slot,
                friction: rubble.friction(collider),
                structure: rubble.structures.contains(body).then_some(body),
                motion,
            });
        }
        out
    }

    /// The one self-support rule: a pass never reads its own source body's surfaces. A shared
    /// tick-start snapshot serves every source, so the rule is identity, set per pass.
    pub(crate) fn exclude_source(&mut self, source: Entity) {
        self.source = Some(source);
    }

    fn surface(&self, collider: Entity) -> &Surface {
        self.surfaces
            .iter()
            .find(|s| s.collider == collider)
            .expect("a belt collider contact belongs to the gathered footprint surfaces")
    }

    /// Whether `collider` belongs to a gathered dynamic piece.
    pub(crate) fn is_dynamic(&self, collider: Entity) -> bool {
        self.surface(collider).body.is_some()
    }

    /// The friction `collider`'s surface brings to a pair, as avian's narrow phase resolves it.
    /// Only a collider from this snapshot may be read.
    pub(crate) fn friction(&self, collider: Entity) -> Friction {
        self.surface(collider).friction
    }

    /// The collider of a [`SlabOverlap::piece`].
    pub(crate) fn collider(&self, piece: usize) -> Entity {
        self.surfaces[piece].collider
    }

    /// The surface's world velocity at `point`: dynamic or kinematic motion, zero on fixed
    /// construction. Only a collider from this snapshot may be read.
    pub(crate) fn velocity_at(&self, collider: Entity, point: Vec3) -> Vec3 {
        let surface = self.surface(collider);
        if let Some(slot) = surface.body {
            self.bodies[slot].velocity_at(point)
        } else {
            surface.motion.as_ref().map_or(Vec3::ZERO, |motion| {
                motion.linear + motion.angular.cross(point - motion.com)
            })
        }
    }

    /// Stage the equal-and-opposite reaction on fixed structural construction. The field has no
    /// collider contact, loose bodies take clamped impulses, and permanent surfaces have no
    /// structural law. Every remaining contact joins the physics step's shared load reading.
    pub(crate) fn record_static_contacts(
        &self,
        body: Entity,
        contacts: &[BeltContact],
        transfers: &mut ContactTransfers,
    ) {
        for contact in contacts {
            let Some(collider) = contact.collider else {
                continue;
            }; // Field-only support.
            let surface = self.surface(collider);
            if surface.body.is_some() {
                continue;
            } // Dynamic recipients take piece_impulses.
            let Some(structure) = surface.structure else {
                continue;
            }; // No structural law.
            assert!(
                surface.motion.is_none(),
                "standing structural construction is fixed, never kinematic"
            );
            transfers.stage(AppliedContact {
                structure,
                structure_collider: collider,
                body,
                point: contact.point,
                normal: contact.normal,
                force: source_force(contact),
                closing_speed: -contact.belt_velocity.dot(contact.normal),
            });
        }
    }

    /// Every gathered piece that reaches into `slab`, with its extent there, in gather order.
    pub(crate) fn in_slab(&self, slab: &Slab) -> Vec<SlabOverlap> {
        let mut overlaps = Vec::new();
        if self.surfaces.is_empty() {
            return overlaps;
        }
        let (lo, hi) = slab.aabb();
        let radial = (slab.face - slab.reach, slab.face + slab.travel);
        for (piece, s) in self.surfaces.iter().enumerate() {
            if self.source == Some(s.owner) {
                continue;
            } // A hull cannot support its own belt.
            if (s.aabb.min.cmpgt(hi) | s.aabb.max.cmplt(lo)).any() {
                continue;
            }
            let extent = |dir: Vec3| {
                (
                    (s.support(-dir) - slab.origin).dot(dir),
                    (s.support(dir) - slab.origin).dot(dir),
                )
            };
            let (u0, u1) = extent(slab.axis);
            let (l0, l1) = extent(slab.lat);
            let (inner, outer) = extent(slab.out);
            let u = (u0.max(0.0), u1.min(slab.len));
            let l = (l0.max(slab.lateral.0), l1.min(slab.lateral.1));
            // The piece must reach out past the probes' origin, `reach` behind the rest face at
            // every travel.
            if u.1 <= u.0 || l.1 <= l.0 || outer <= radial.0 || inner >= radial.1 {
                continue;
            }
            // Six support calls reject separated intervals cheaply. The retained boundary is
            // clipped before its depth is read, so a vertex outside the footprint cannot lift
            // the hull and an empty convex corner cannot become a contact.
            let Some((lo, hi)) = s.clipped_extent(slab) else {
                continue;
            };
            if hi.z <= radial.0 || lo.z >= radial.1 {
                continue;
            }
            overlaps.push(SlabOverlap {
                piece,
                u: (lo.x.max(0.0), hi.x.min(slab.len)),
                l: (lo.y.max(slab.lateral.0), hi.y.min(slab.lateral.1)),
                depth: (slab.face - lo.z).min(slab.reach),
            });
        }
        overlaps
    }
}

/// The footprint's bounding sphere about the hull origin (m), DERIVED by the triangle
/// inequality: the farthest pin, the widest shoe offset, and the larger of the slab's outward
/// travel and inward probe reach. Both radial faces must fit, including a short rig with a long
/// probe reach.
pub(crate) fn footprint_radius(
    pin_radius: f32,
    column: f32,
    face: f32,
    max_travel: f32,
    reach: f32,
) -> f32 {
    pin_radius + column + (face + max_travel).abs().max((face - reach).abs())
}

/// One impulse a dynamic piece took: `impulse` (N·s, world) at `point`, through `collider`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Push {
    pub(crate) body: Entity,
    pub(crate) collider: Entity,
    pub(crate) impulse: Vec3,
    pub(crate) point: Vec3,
}

/// The force the belt law applied to its source through `contact` (N): the support along the
/// normal and the traction. A recipient held by its own laws takes its opposite.
fn source_force(contact: &BeltContact) -> Vec3 {
    contact.normal * contact.load + contact.traction
}

/// Every dynamic recipient's side of `contacts` (module doc): the whole opposite reaction on a
/// body in `belt_held` (one whose support is a belt law this tick), the clamped impulse on any
/// other. Pieces are sized in one canonical order of the contacts' geometry, each impulse taken
/// into `bodies` before the next is sized, so the order the contacts were gathered in, and which
/// tank produced them, decide nothing. One push per contact on a dynamic body, zero where a
/// piece is not closing on the belt.
pub(crate) fn piece_impulses<'c>(
    bodies: &mut RubbleBodies,
    contacts: impl IntoIterator<Item = &'c BeltContact>,
    belt_held: &[Entity],
    dt: f32,
) -> Vec<Push> {
    let mut contacts: Vec<&BeltContact> = contacts.into_iter().collect();
    let key = |c: &BeltContact| {
        [c.point, c.normal, c.traction, c.belt_velocity]
            .map(|v| v.to_array().map(f32::to_bits))
            .concat()
            .into_iter()
            .chain([c.load.to_bits(), c.mu.to_bits()])
            .collect::<Vec<u32>>()
    };
    contacts.sort_by_cached_key(|c| key(c));
    let mut pushes = Vec::new();
    for contact in contacts {
        // The field has no body to push; every collider contact came from this snapshot.
        let Some(collider) = contact.collider else {
            continue;
        };
        let surface = bodies.surface(collider);
        // Static and kinematic construction carries support without taking dynamic impulses.
        let Some(slot) = surface.body else {
            continue;
        };
        let collider = surface.collider;
        let body = &mut bodies.bodies[slot];
        let impulse = if belt_held.contains(&body.entity) {
            -source_force(contact) * dt
        } else {
            contact_impulse(body, contact, dt)
        };
        body.apply(impulse, contact.point);
        pushes.push(Push {
            body: body.entity,
            collider,
            impulse,
            point: contact.point,
        });
    }
    pushes
}

/// The impulse one contact puts on `body` (module doc): the push, then the drag sized against the
/// pushed piece. `contact.normal` points from the piece to the belt and the traction is the
/// belt's force on the hull, so the piece's side runs along `-normal` and `-traction`. The drag's
/// Coulomb bound is the contact's own pair ([`BeltContact::mu`]).
fn contact_impulse(body: &PieceBody, contact: &BeltContact, dt: f32) -> Vec3 {
    let (point, normal) = (contact.point, contact.normal);
    let load_j = contact.load.max(0.0) * dt;
    // The piece's velocity against the belt surface at the contact.
    let rel = body.velocity_at(point) - contact.belt_velocity;
    let closing = rel.dot(normal);
    // An entirely locked direction has infinite effective mass. Test the speed before
    // multiplying so a non-closing contact still takes zero rather than infinity * zero.
    let stop = if closing > 0.0 {
        body.effective_mass(point, normal) * closing
    } else {
        0.0
    };
    let push = -normal * load_j.min(stop);
    let Ok(dir) = Dir3::new(-contact.traction) else {
        return push;
    };
    // How far the pushed piece lags the belt surface along the drag.
    let lag = -(rel + body.response(push, point)).dot(*dir);
    let catch_up = if lag > 0.0 {
        body.effective_mass(point, *dir) * lag
    } else {
        0.0
    };
    let drag = (contact.traction.length() * dt)
        .min(contact.mu * load_j)
        .min(catch_up);
    push + *dir * drag
}

/// Put every push on its body through avian, which wakes a sleeping body for a non-zero impulse.
pub(crate) fn apply_pushes<F: QueryFilter>(pushes: &[Push], bodies: &mut Query<PieceData, F>) {
    for push in pushes {
        let (mut forces, ..) = bodies
            .get_mut(push.body)
            .expect("a gathered dynamic piece remains alive during the synchronous force pass");
        forces.apply_linear_impulse_at_point(push.impulse, push.point);
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
    use crate::buildings::breakage::BuildingChunk;
    use crate::substances::contact_friction;
    use crate::track::forces::{
        ForceParams, GripElements, GripParams, SideInput, SideReport, SideState, TravelField,
        contact_side,
    };
    use crate::track::oracle::{BlockField, TerrainBlock, TerrainOracle};

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
            crate::contact_transfer::sim_plugin,
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

    /// A DYNAMIC BODY SUPPORTS THE BELT BY ITS PHYSICS, NEVER BY WHAT IT IS. An unmarked loose
    /// body is a surface; a sensor and a collider whose filters exclude the hull (an armour
    /// volume's) are not. A mutant that restores a marker gate or drops the layer or sensor test
    /// fails here; self-support is the pass-time identity rule, which the driving adapter's
    /// stacked-hull and lone-hull tests pin. FIXTURE DATA: three 1 m boxes about the origin.
    #[test]
    fn a_dynamic_body_supports_by_its_physics() {
        let mut app = physics();
        let size = Vec3::ONE;
        let plain = piece_box(&mut app, Vec3::new(-1.5, 0.5, 0.0), size);
        let sensor = piece_box(&mut app, Vec3::new(1.5, 0.5, 0.0), size);
        app.world_mut().entity_mut(sensor).insert(Sensor);
        let volume = piece_box(&mut app, Vec3::new(0.0, 0.5, 1.5), size);
        app.world_mut()
            .entity_mut(volume)
            .insert(CollisionLayers::new([Layer::Armor], LayerMask::NONE));
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let near = rubble.near(Vec3::ZERO, 4.0);
                assert!(near.contains(&plain));
                assert!(!near.contains(&sensor) && !near.contains(&volume));
                let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                let gathered: Vec<_> = pieces.surfaces.iter().map(|s| s.collider).collect();
                assert_eq!(gathered, vec![plain]);
                assert!(pieces.is_dynamic(plain));
            })
            .unwrap();
    }

    /// Support admission must not turn a malformed dynamic body into empty ground. The driving
    /// adapter relies on every non-excluded recipient carrying its spawn-established force state.
    #[test]
    #[should_panic(expected = "an admitted dynamic support body carries Avian force state")]
    fn an_admitted_dynamic_body_with_missing_force_state_is_a_bug() {
        let mut app = physics();
        let body = piece(&mut app, 0.0, 0.0);
        app.update();
        app.world_mut().entity_mut(body).remove::<ComputedMass>();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                RubbleBodies::gather(&rubble, &[body], &bodies);
            })
            .unwrap();
    }

    /// A support's staged response must equal Avian's actual impulse response with axis locks.
    /// Subsequent belt contacts rely on that snapshot when clamping their own reactions.
    #[test]
    fn a_locked_support_advances_exactly_as_its_avian_impulse_does() {
        let mut app = physics();
        let body = piece(&mut app, 0.0, 0.0);
        app.world_mut()
            .entity_mut(body)
            .insert(LockedAxes::new().lock_translation_y().lock_rotation_z());
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, mut bodies: Query<PieceData>| {
                let mut snapshot = RubbleBodies::gather(&rubble, &[body], &bodies);
                let state = &mut snapshot.bodies[0];
                // FIXTURE DATA: off-centre impulse exercises both translation and rotation.
                let at = state.com + Vec3::new(0.2, 0.1, 0.3);
                let impulse = Vec3::new(10.0, -30.0, 20.0);
                state.apply(impulse, at);
                let (mut forces, ..) = bodies.get_mut(body).unwrap();
                forces.apply_linear_impulse_at_point(impulse, at);
                assert_eq!(state.linear, forces.linear_velocity());
                assert_eq!(state.angular, forces.angular_velocity());
                assert_eq!(state.linear.y, 0.0);
                assert_eq!(state.angular.z, 0.0);
                let mut contact = press(state.com, body, 100.0);
                contact.point = state.com;
                contact.normal = Vec3::Y;
                contact.traction = Vec3::ZERO;
                contact.belt_velocity = Vec3::ZERO;
                assert_eq!(contact_impulse(state, &contact, DT), Vec3::ZERO);
            })
            .unwrap();
    }

    /// Fixed support delivers exactly the opposite of the applied belt force into the shared
    /// structure reading, with no solver contact standing in for the belt. Walls and trunks rely
    /// on this reading to judge the load; subdividing the contact must not multiply its blow.
    /// FIXTURE DATA: a surface topped at 0.6 m and a small source body clear above it.
    fn fixed_surface_reaction(shape: Collider, closing: f32) {
        let mut app = physics();
        let root = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.3, 0.0),
                RigidBody::Static,
                StructureBody,
            ))
            .id();
        let collider = app
            .world_mut()
            .spawn((ChildOf(root), Transform::IDENTITY, shape))
            .id();
        let source = piece_box(&mut app, Vec3::Y * 1.5, Vec3::splat(0.2));
        app.world_mut()
            .entity_mut(source)
            .insert(LinearVelocity(Vec3::NEG_Y * closing));
        app.update();
        app.update();
        let (expected, before, mass) = app
            .world_mut()
            .run_system_once(
                move |rubble: RubbleQuery,
                      mut bodies: Query<PieceData>,
                      mut transfers: ResMut<ContactTransfers>| {
                    let near = rubble.near(Vec3::ZERO, 4.0);
                    assert!(
                        near.contains(&collider),
                        "unmarked fixed geometry must carry the belt"
                    );
                    let mut pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                    pieces.exclude_source(source);
                    let (mut forces, _, mass, ..) = bodies.get_mut(source).unwrap();
                    let before = forces.linear_velocity();
                    let report = pad_pass(
                        &ground(),
                        &pieces,
                        Affine3A::from_translation(Vec3::Y * 0.6),
                        SideState::default(),
                        before,
                        &mut GripElements::for_links(30),
                    );
                    let expected: Vec3 = report.apps.iter().map(|app| app.force).sum();
                    assert!(expected.y > 0.0, "the surface must support the belt");
                    pieces.record_static_contacts(source, &report.contacts, &mut transfers);
                    for app in report.apps {
                        forces.apply_force_at_point(app.force, app.point);
                    }
                    (expected, before, mass.value())
                },
            )
            .unwrap();
        app.update();
        let transfers = app.world().resource::<ContactTransfers>();
        let mut reaction = Vec3::ZERO;
        let mut blow = Vec3::ZERO;
        let mut seen = 0;
        for (point, struck, press, _) in
            transfers.split().filter(|(point, ..)| point.body == source)
        {
            assert_eq!(point.structure, root);
            assert_eq!(point.structure_collider, collider);
            assert_eq!(point.body_collider, None);
            reaction += struck + press * transfers.dt();
            blow += struck;
            seen += 1;
        }
        assert!(seen > 1, "the reading must include the footprint's patches");
        let impulse = expected * transfers.dt();
        assert!((reaction + impulse).length() < 1e-4 * impulse.length());
        let after = app.world().get::<LinearVelocity>(source).unwrap().0;
        assert!(((after - before) * mass + reaction).length() < 1e-4 * impulse.length());
        if closing > 0.0 {
            assert!(
                blow.y < 0.0 && -blow.y <= mass * closing * (1.0 + 1e-4),
                "the horizontal surface cannot take more than the source's closing momentum: {blow}"
            );
        } else {
            assert_eq!(blow, Vec3::ZERO, "a resting press must not invent a blow");
        }
        app.update();
        assert!(
            !app.world()
                .resource::<ContactTransfers>()
                .split()
                .any(|(point, ..)| point.body == source),
            "an implicit contact must be refreshed each tick, never retained as a sleeping pair"
        );
    }

    #[test]
    fn a_belt_pressing_a_wall_delivers_its_reaction_to_the_walls_reading() {
        for closing in [0.0, 1.0] {
            fixed_surface_reaction(Collider::cuboid(2.0, 0.6, 2.0), closing);
        }
    }

    /// FIXTURE DATA: a standing trunk segment as `trees::segment_collider` builds one, the convex
    /// hull of two twelve-point rings, radius 0.4 m at its foot and 0.35 m at its top, 0.6 m tall.
    #[test]
    fn a_trunk_segment_carries_the_belt_and_is_loaded_by_it() {
        let ring = |radius: f32, y: f32| {
            (0..12).map(move |k| {
                let angle = k as f32 * std::f32::consts::TAU / 12.0;
                Vec3::new(radius * angle.cos(), y, radius * angle.sin())
            })
        };
        let points: Vec<Vec3> = ring(0.4, -0.3).chain(ring(0.35, 0.3)).collect();
        let trunk = Collider::convex_hull(points).expect("two rings at distinct heights");
        fixed_surface_reaction(trunk, 0.0);
    }

    /// Field-backed geometry is supported once, regardless of its collider representation.
    /// Restoring collider admission for field blocks would add contacts and change the grip pair.
    #[test]
    fn field_geometry_is_never_counted_again_as_a_collider() {
        let reference = bits(&flat_pass(&RubbleBodies::default()));
        let mut app = physics();
        let field = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, -0.5, 0.0),
                RigidBody::Static,
                Collider::cuboid(200.0, 1.0, 200.0),
                FieldSurface,
            ))
            .id();
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let near = rubble.near(Vec3::ZERO, 4.0);
                assert!(
                    !near.contains(&field),
                    "the analytic field owns this surface"
                );
                let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                assert_eq!(bits(&flat_pass(&pieces)), reference);
            })
            .unwrap();
    }

    /// Kinematic motion supplies relative slip, while its imposed velocity takes no impulse.
    /// Treating every non-dynamic surface as motionless would make conveyors grip incorrectly.
    #[test]
    fn a_kinematic_surface_carries_its_motion_without_taking_a_dynamic_reaction() {
        let mut app = physics();
        let body = piece_box(&mut app, Vec3::Y * 0.2, Vec3::new(0.4, 0.4, 4.0));
        app.world_mut()
            .entity_mut(body)
            .insert(RigidBody::Kinematic);
        app.update();
        let velocity = Vec3::new(0.0, 0.0, -0.3);
        app.world_mut().get_mut::<LinearVelocity>(body).unwrap().0 = velocity;
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let mut pieces = RubbleBodies::gather(&rubble, &[body], &bodies);
                assert_eq!(pieces.velocity_at(body, Vec3::ZERO), velocity);
                assert!(!pieces.is_dynamic(body));
                let report = pad_pass(
                    &ground(),
                    &pieces,
                    Affine3A::from_translation(Vec3::Y * 0.4),
                    SideState {
                        speed: 0.4,
                        ..default()
                    },
                    velocity,
                    &mut GripElements::for_links(30),
                );
                assert!(report.contacts.iter().any(|c| c.collider == Some(body)));
                for contact in report
                    .contacts
                    .iter()
                    .filter(|c| c.collider == Some(body) && c.normal.y > 0.99)
                {
                    assert!((contact.slip - 0.4).abs() < 1e-5);
                }
                assert!(piece_impulses(&mut pieces, &report.contacts, &[], DT).is_empty());
            })
            .unwrap();
    }

    /// STATIC CONSTRUCTION IS A SUPPORT SURFACE TOO, AND EMPTY CONVEX CORNERS ARE NOT.
    /// The footprint reader must classify geometry, not rigid-body kind or world AABB overlap.
    /// FIXTURE DATA: a narrow slab beside the corner of a yawed box and below a standing chunk.
    #[test]
    fn the_shape_check_reads_static_construction_and_rejects_empty_corners() {
        let mut app = physics();
        let (_, chunk) = standing_chunk(&mut app, 0.0, 0.0);
        let corner = piece_box(&mut app, Vec3::new(3.0, 0.2, 0.0), Vec3::new(1.0, 0.4, 1.0));
        app.world_mut().get_mut::<Rotation>(corner).unwrap().0 =
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let near = rubble.near(Vec3::ZERO, 5.0);
                let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                let mut slab = Slab {
                    origin: Vec3::new(0.0, 0.1, -0.1),
                    axis: Vec3::Z,
                    lat: Vec3::X,
                    out: Vec3::NEG_Y,
                    len: 0.2,
                    lateral: (-0.1, 0.1),
                    face: 0.0,
                    reach: 0.5,
                    travel: 0.1,
                };
                let hits = pieces.in_slab(&slab);
                assert!(hits.iter().any(|h| pieces.collider(h.piece) == chunk));
                assert!(!pieces.is_dynamic(chunk));
                slab.origin = Vec3::new(3.6, 0.1, 0.5);
                let (lo, hi) = slab.aabb();
                let surface = pieces.surface(corner);
                assert!(
                    !(surface.aabb.min.cmpgt(hi) | surface.aabb.max.cmplt(lo)).any(),
                    "fixture must exercise a world-AABB false positive"
                );
                assert!(
                    pieces.in_slab(&slab).is_empty(),
                    "empty corner counted as support"
                );
            })
            .unwrap();
    }

    /// A LONG PIECE'S REMOTE VERTEX CANNOT COMPRESS AN ANGLED LINK. The hull relies on depth
    /// being constrained to the actual footprint, not a shape-wide support point. A flat top
    /// below this inclined footprint has the same local intrusion at either fixture length.
    /// FIXTURE DATA: unit tangent (0, 0.6, 0.8), top y = 0.1 m, face offset 0.025 m.
    #[test]
    fn support_depth_is_clipped_to_the_footprint_before_it_is_read() {
        let mut app = physics();
        let short = piece_box(
            &mut app,
            Vec3::new(0.0, 0.05, 0.0),
            Vec3::new(0.4, 0.1, 4.0),
        );
        let long = piece_box(
            &mut app,
            Vec3::new(10.0, 0.05, 0.0),
            Vec3::new(0.4, 0.1, 400.0),
        );
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let pieces = RubbleBodies::gather(&rubble, &[short, long], &bodies);
                let mut slab = Slab {
                    origin: Vec3::Y * 0.1,
                    axis: Vec3::new(0.0, 0.6, 0.8),
                    lat: Vec3::X,
                    out: Vec3::new(0.0, -0.8, 0.6),
                    len: 0.2,
                    lateral: (-0.1, 0.1),
                    face: 0.025,
                    reach: 0.5,
                    travel: 0.1,
                };
                let short_depth = pieces.in_slab(&slab)[0].depth;
                slab.origin.x = 10.0;
                let long_depth = pieces.in_slab(&slab)[0].depth;
                assert!(
                    (short_depth - 0.025).abs() < 1e-5,
                    "local depth {short_depth}"
                );
                assert!(
                    (long_depth - short_depth).abs() < 1e-5,
                    "remote geometry changed depth {short_depth} -> {long_depth}"
                );
            })
            .unwrap();
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
            mu: 0.9,
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
                let mut pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                let pushes = piece_impulses(&mut pieces, &contacts, &[], DT);
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

    /// The saturated grip coefficient of every contact in `report` (`|f_lat| / load_elastic`, the
    /// pad sliding sideways from zero strain), and the contacts' summed lateral grip (N).
    fn sliding_grip(report: &SideReport) -> (Vec<f32>, f32) {
        assert!(
            !report.contacts.is_empty(),
            "the sliding pad touched nothing"
        );
        let mus = report
            .contacts
            .iter()
            .map(|c| c.f_lat.abs() / c.load_elastic)
            .collect();
        (mus, report.contacts.iter().map(|c| c.f_lat.abs()).sum())
    }

    /// THE BELT GRIPS BY THE MATERIAL PAIR IT MEETS, ON GROUND AND ON PIECES BY ONE RULE (#238).
    /// One pad sliding sideways on two ground types takes saturated grip in the ratio of the pairs
    /// the data gives, each `√(μ_track · μ_ground)` (`substances::PAIR_RULE`); over a piece it
    /// takes the track's pair with the piece's row, and the piece's reaction is clamped by that
    /// same coefficient. Driving feel and every piece reaction rely on it. A law gripping at one coefficient everywhere, reading one side's row alone, or
    /// leaving a piece's patch on the ground's pair fails here.
    ///
    /// FIXTURE DATA: a 0.7 track on 0.2 and 0.8 fields, the pad, a 0.3 piece, a 5 m/s sideways
    /// slide from zero strain. The class invariant does not require shipped grounds to differ or
    /// a selected vehicle to have nonzero friction.
    #[test]
    fn the_belt_grips_by_the_material_pair_on_ground_and_pieces() {
        let track = contact_friction(0.7);
        let (least, most) = (0.2, 0.8);
        let slide = Vec3::new(5.0, 0.0, 0.0);
        let on_ground = |friction: f32| {
            let grip = GripParams {
                ground: contact_friction(friction),
                ..pad_grip(track)
            };
            let report = pad_pass_gripping(
                grip,
                &ground(),
                &RubbleBodies::default(),
                Affine3A::IDENTITY,
                SideState::default(),
                slide,
                &mut GripElements::for_links(30),
            );
            (grip.on_ground(), sliding_grip(&report))
        };
        let (pair_least, (mus_least, total_least)) = on_ground(least);
        let (pair_most, (mus_most, total_most)) = on_ground(most);
        for (mus, pair) in [(&mus_least, pair_least), (&mus_most, pair_most)] {
            for mu in mus {
                assert!((mu - pair).abs() < 1e-4 * pair, "grip {mu}, pair {pair}");
            }
        }
        let ratio = total_most / total_least;
        let expected = pair_most / pair_least;
        println!(
            "MEASURED sliding grip {total_least:.1} N and {total_most:.1} N, ratio {ratio:.6}; \
             DERIVED pair ratio {expected:.6}"
        );
        assert!((ratio - expected).abs() < 1e-4 * expected);

        let row = 0.3;
        let mut app = physics();
        let piece = piece_box(
            &mut app,
            Vec3::new(0.0, 0.05, 0.0),
            Vec3::new(0.4, 0.1, 2.4),
        );
        app.world_mut()
            .entity_mut(piece)
            .insert(contact_friction(row));
        app.update();
        let grip = pad_grip(track);
        let (report, mus, pushes) = app
            .world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let near = rubble.near(Vec3::new(0.0, 0.1, 0.0), 3.0);
                let mut pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                let report = pad_pass_gripping(
                    grip,
                    &ground(),
                    &pieces,
                    Affine3A::from_translation(Vec3::new(0.0, 0.1, 0.0)),
                    SideState::default(),
                    slide,
                    &mut GripElements::for_links(30),
                );
                let pushes = piece_impulses(&mut pieces, &report.contacts, &[], DT);
                let mus = report.contacts.iter().map(|c| c.mu).collect::<Vec<_>>();
                (sliding_grip(&report), mus, pushes)
            })
            .expect("the piece pass runs");
        let pair = grip.pair(contact_friction(row));
        assert!(
            pair < grip.on_ground(),
            "the fixture piece must grip apart from the ground"
        );
        for mu in report.0.iter().chain(&mus) {
            assert!(
                (mu - pair).abs() < 1e-4 * pair,
                "grip {mu} on the piece, pair {pair}"
            );
        }
        assert!(!pushes.is_empty(), "the sliding pad dragged nothing");
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
    /// EXCEEDS COULOMB'S μ × load × dt: the clamp is what keeps a light piece from being flung, and
    /// the belt's normal force over the tick is what the friction reads, so over 4000 fixture
    /// contacts (pieces of 5 kg to 3 t with skewed inertia, off-centre contacts, any normal,
    /// traction and belt velocity) the normal impulse lies in `[0, load·dt]`, the tangential one
    /// within `|traction|·dt` and `μ·load·dt`; the push alone leaves the piece still closing on the
    /// belt or level with it along the normal, and the whole impulse leaves it still behind the
    /// belt surface or level with it along the drag. A clamp sized without the inertia coupling,
    /// a drag sized against the unpushed piece, a cap on the wrong side, or a friction bound off
    /// the clamped normal impulse (which drags nothing under a hull that is not closing) fails
    /// here; every clamp is seen to bind, and the drag is seen with no push at all.
    #[test]
    fn the_impulse_never_drives_a_piece_past_the_belt() {
        let mut rng = Lcg(0x5eed);
        let (mut by_closing, mut by_load, mut by_cone, mut by_lag, mut unpushed) = (0, 0, 0, 0, 0);
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
                inv_mass: Vec3::splat(1.0 / mass),
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
                mu: rng.range(0.0, 1.2),
            };
            let mu = contact.mu;
            let impulse = contact_impulse(&body, &contact, DT);

            let normal_j = -impulse.dot(normal);
            let tangent_j = (impulse + normal * normal_j).length();
            let tol = 1e-4 * (1.0 + impulse.length());
            assert!(normal_j >= -tol, "case {case}: pulled at {normal_j}");
            assert!(
                normal_j <= contact.load * DT + tol,
                "case {case}: {normal_j}"
            );
            assert!(
                tangent_j <= mu * contact.load * DT + tol,
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
            if normal_j > tol {
                let mut pushed = body;
                pushed.apply(-normal * normal_j, contact.point);
                let pushed = pushed.velocity_at(contact.point) - contact.belt_velocity;
                assert!(
                    pushed.dot(normal) >= -vtol,
                    "case {case}: pushed past the belt along the normal, {} → {}",
                    before.dot(normal),
                    pushed.dot(normal),
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
            } else if tangent_j > tol {
                unpushed += 1;
            }
            if tangent_j > tol && tangent_j > mu * contact.load * DT * 0.999 {
                by_cone += 1;
            } else if tangent_j > tol && tangent_j < contact.traction.length() * DT * 0.999 {
                by_lag += 1;
            }
        }
        for (name, count) in [
            ("closing", by_closing),
            ("load", by_load),
            ("cone", by_cone),
            ("lag", by_lag),
            ("drag with no push", unpushed),
        ] {
            assert!(count > 20, "the {name} bound bound only {count} times");
        }
    }

    /// The fixture pad's grip: `track` on a 0.9 field, at a 30 t vehicle's side weight (fixture
    /// data).
    fn pad_grip(track: Friction) -> GripParams {
        GripParams {
            track,
            ground: contact_friction(0.9),
            side_weight: 30_000.0 * 9.81 / 2.0,
        }
    }

    /// [`pad_pass_gripping`] with a 0.9 track on the 0.9 field (fixture data).
    fn pad_pass<O: TerrainOracle>(
        oracle: &O,
        pieces: &RubbleBodies,
        affine: Affine3A,
        state: SideState,
        vel: Vec3,
        elements: &mut GripElements,
    ) -> SideReport {
        let grip = pad_grip(contact_friction(0.9));
        pad_pass_gripping(grip, oracle, pieces, affine, state, vel, elements)
    }

    /// One contact pass of a fixture pad gripping by `grip`, with its complete shoe footprint and
    /// no suspension travel. The tall return run leaves the piece tests clear of its inward probe
    /// origins.
    fn pad_pass_gripping<O: TerrainOracle>(
        grip: GripParams,
        oracle: &O,
        pieces: &RubbleBodies,
        affine: Affine3A,
        state: SideState,
        vel: Vec3,
        elements: &mut GripElements,
    ) -> SideReport {
        let loop_pts = [
            Vec2::new(-1.0, 0.02),
            Vec2::new(1.0, 0.02),
            Vec2::new(1.0, 1.02),
            Vec2::new(-1.0, 1.02),
            Vec2::new(-1.0, 0.02),
        ];
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 30,
            plane_x: 0.0,
            columns: [(-0.1, 0.25), (0.0, 0.5), (0.1, 0.25)],
            travel: TravelField { knots: &[] },
        };
        let params = ForceParams {
            face_offset: 0.025,
            free_travel: 0.0,
            support_stiffness_per_m: 2.0e6,
            support_damping_per_m: 1.0e5,
            engage_depth: 0.002,
            probe_reach: 0.5,
            slip_saturation: 0.5,
            inertia: 500.0,
            grip: Some(grip),
        };
        contact_side(
            &input,
            state,
            affine,
            DT,
            &params,
            oracle,
            pieces,
            |_| vel,
            elements,
        )
    }

    /// The fixture pad just into flat ground, driving and sliding.
    fn flat_pass(pieces: &RubbleBodies) -> SideReport {
        pad_pass(
            &ground(),
            pieces,
            Affine3A::IDENTITY,
            SideState {
                speed: 0.4,
                phase: 0.0,
                grip: Vec2::ZERO,
            },
            Vec3::new(0.0, -0.01, 0.3),
            &mut GripElements::for_links(30),
        )
    }

    /// Park a fixture hull, integrating heave under the actual support and clamped reactions.
    /// FIXTURE DATA: mass 6 t, duration supplied in fixed ticks, gravity 9.81 m/s².
    fn park(app: &mut App, mut h: f32, ticks: usize) -> f32 {
        const MASS: f32 = 6000.0;
        let mut vy = 0.0;
        let mut elements = GripElements::for_links(30);
        for _ in 0..ticks {
            let (lift, kept) = app
                .world_mut()
                .run_system_once(move |rubble: RubbleQuery, mut bodies: Query<PieceData>| {
                    let mut elements = std::mem::take(&mut elements);
                    let near = rubble.near(Vec3::new(0.0, h, 0.0), 3.0);
                    let mut pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                    let report = pad_pass(
                        &ground(),
                        &pieces,
                        Affine3A::from_translation(Vec3::new(0.0, h, 0.0)),
                        SideState::default(),
                        Vec3::new(0.0, vy, 0.0),
                        &mut elements,
                    );
                    let pushes = piece_impulses(&mut pieces, &report.contacts, &[], DT);
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

    /// THE BELT CARRIES THE HULL OVER A LOOSE PIECE AT THE SAME RIDE HEIGHT AS GROUND, AND THE
    /// PIECE STAYS PUT. Driving relies on support reading the complete footprint and its reaction
    /// using the R2 clamp; a raw opposite force sinks or throws the light piece.
    /// FIXTURE DATA: a 0.1 m tall piece under a 6 t hull, settled for 192 fixed ticks.
    #[test]
    fn a_piece_lifts_the_hull_by_its_height_without_being_thrown() {
        let gravity = Vec3::NEG_Y * 9.81;
        let mut bare = physics_with(gravity);
        bare.update();
        let on_bare = park(&mut bare, 0.0, 192);
        let mut app = physics_with(gravity);
        app.world_mut().spawn((
            Position(Vec3::new(0.0, -0.5, 0.0)),
            RigidBody::Static,
            Collider::cuboid(20.0, 1.0, 20.0),
            FieldSurface,
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
        let speed = app.world().get::<LinearVelocity>(brick).unwrap().0.length();
        let rise = on_brick - on_bare;
        println!(
            "MEASURED hull rise {rise:.6} m; piece displacement {:.6} m, final speed {speed:.6} m/s",
            (after - rest).length()
        );
        assert!(
            (after - rest).length() < 1e-3,
            "piece moved {rest} -> {after}"
        );
        assert!(speed < 1e-3, "piece thrown at {speed} m/s");
        assert!(
            (rise - (after.y + 0.05)).abs() < 5e-4,
            "rise {rise}, top {}",
            after.y + 0.05
        );
    }

    /// A PIECE BETWEEN EVERY PROBE LINE STILL CARRIES ITS TRUE WIDTH'S SHARE, WHILE A PIECE
    /// BESIDE THE SHOE CARRIES NOTHING. Driving over small rubble relies on coverage independent
    /// of the lateral sample columns; widening a hit to the whole column also fails this test.
    /// FIXTURE DATA: shoe width 0.2 m, narrow piece width 0.02 m, centred 0.04 m off the pin plane.
    #[test]
    fn support_reads_the_whole_footprint_and_only_the_footprint() {
        let pass = |x, width| {
            let mut app = physics();
            piece_box(&mut app, Vec3::new(x, 0.1, 0.0), Vec3::new(width, 0.2, 4.0));
            app.update();
            app.world_mut()
                .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                    let near = rubble.near(Vec3::ZERO, 4.0);
                    let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                    let mut elements = GripElements::for_links(30);
                    let report = pad_pass(
                        &ground(),
                        &pieces,
                        Affine3A::from_translation(Vec3::Y * 0.1),
                        SideState::default(),
                        Vec3::ZERO,
                        &mut elements,
                    );
                    (
                        report
                            .contacts
                            .iter()
                            .filter(|c| c.collider.is_some())
                            .map(|c| c.load_elastic * c.normal.y)
                            .sum::<f32>(),
                        elements,
                    )
                })
                .unwrap()
        };
        let (full, _) = pass(0.0, 0.4);
        let (narrow, _) = pass(0.04, 0.02);
        let (beside, _) = pass(0.12, 0.02);
        println!(
            "MEASURED support: full {full:.3} N, between columns {narrow:.3} N, beside {beside:.3} N"
        );
        assert!(full > 0.0);
        assert!(
            (narrow / full - 0.1).abs() < 1e-5,
            "wrong footprint share: {narrow}/{full}"
        );
        assert_eq!(beside, 0.0);
    }

    /// STACKED PIECES DO NOT MULTIPLY SUPPORT, AND A SPLIT ELEMENT ADVANCES ITS GRIP ONCE.
    /// The hull and transmission rely on the deepest surface winning each area, and material
    /// strain must not depend on how many colliders partition an otherwise identical surface.
    #[test]
    fn stacked_and_partitioned_surfaces_preserve_support_and_grip() {
        let pass = |partition: bool, buried: bool| {
            let mut app = physics();
            if partition {
                for x in [-0.05, 0.05] {
                    piece_box(&mut app, Vec3::new(x, 0.1, 0.0), Vec3::new(0.1, 0.2, 4.0));
                }
            } else {
                piece_box(&mut app, Vec3::new(0.0, 0.1, 0.0), Vec3::new(0.2, 0.2, 4.0));
            }
            if buried {
                piece_box(
                    &mut app,
                    Vec3::new(0.0, -0.1, 0.0),
                    Vec3::new(0.2, 0.2, 4.0),
                );
            }
            app.update();
            app.world_mut()
                .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                    let near = rubble.near(Vec3::ZERO, 4.0);
                    let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                    let mut elements = GripElements::for_links(30);
                    let report = pad_pass(
                        &ground(),
                        &pieces,
                        Affine3A::from_translation(Vec3::Y * 0.1),
                        SideState {
                            speed: 0.2,
                            ..Default::default()
                        },
                        Vec3::ZERO,
                        &mut elements,
                    );
                    (
                        report
                            .contacts
                            .iter()
                            .map(|c| c.load_elastic * c.normal.y)
                            .sum::<f32>(),
                        elements,
                    )
                })
                .unwrap()
        };
        let (load, grip) = pass(false, false);
        for (partition, buried) in [(true, false), (false, true), (true, true)] {
            let (other, elements) = pass(partition, buried);
            assert!(
                (other - load).abs() < load * 1e-5,
                "support multiplied: {load} -> {other}"
            );
            for (a, b) in grip.strain.iter().zip(elements.strain) {
                assert!(
                    (*a - b).length() < 1e-6,
                    "grip advanced per patch: {a} -> {b}"
                );
            }
        }
    }

    /// A MOVING PIECE SETS CONTACT SLIP. The belt's damping and grip must see relative motion,
    /// so a carried surface differs from static ground even when their shapes are identical.
    #[test]
    fn the_slip_on_a_piece_is_measured_against_the_piece() {
        let mut app = physics();
        let carried = piece_box(&mut app, Vec3::new(0.0, 0.2, 0.0), Vec3::new(0.4, 0.4, 4.0));
        app.update();
        let hull_vel = Vec3::new(0.0, 0.0, -0.3);
        app.world_mut()
            .get_mut::<LinearVelocity>(carried)
            .unwrap()
            .0 = hull_vel;
        let slips = app
            .world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let near = rubble.near(Vec3::ZERO, 4.0);
                let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                let report = pad_pass(
                    &ground(),
                    &pieces,
                    Affine3A::from_translation(Vec3::Y * 0.4),
                    SideState {
                        speed: 0.4,
                        ..Default::default()
                    },
                    hull_vel,
                    &mut GripElements::for_links(30),
                );
                report
                    .contacts
                    .into_iter()
                    .filter(|c| c.collider.is_some() && c.normal.y > 0.99)
                    .map(|c| c.slip)
                    .collect::<Vec<_>>()
            })
            .unwrap();
        assert!(!slips.is_empty());
        for slip in slips {
            assert!((slip - 0.4).abs() < 1e-5, "slip {slip}");
        }
    }

    /// Every bit of a field-only report's force applications, contacts and belt reaction.
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

    /// OPEN GROUND WITH NO OVERLAPPING PIECES PRESERVES EVERY FORCE BIT. The field's original
    /// arithmetic is the driving contract; gathering a remote piece must not change it.
    #[test]
    fn flat_ground_with_no_pieces_gives_bit_identical_track_forces() {
        let reference = bits(&flat_pass(&RubbleBodies::default()));
        assert!(reference.len() > 1);
        for (at, size) in [
            (Vec3::new(50.0, 0.2, 0.0), Vec3::new(2.0, 0.4, 2.0)),
            (Vec3::new(0.0, -0.051, 0.0), Vec3::new(0.2, 0.1, 4.0)),
        ] {
            let mut app = physics();
            let piece = piece_box(&mut app, at, size);
            app.update();
            let actual = app
                .world_mut()
                .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                    let pieces = RubbleBodies::gather(&rubble, &[piece], &bodies);
                    bits(&flat_pass(&pieces))
                })
                .unwrap();
            assert_eq!(actual, reference);
        }
    }
}
