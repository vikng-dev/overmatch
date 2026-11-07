//! Constructions and rubble under the running gear, in the game's tank driving physics (game state
//! only). The terrain field ([`super::oracle::BlockField`]) is the heightmap and the authored
//! blocks; a construction's elements and standing chunks and every loose piece are Avian colliders the
//! field never holds. [`RubbleBodies`] snapshots the nearby colliders shared by every driving source, the belt's support
//! reads each through a convex shape check over every link's footprint ([`RubbleBodies::in_slabs`])
//! at the ground's own per-metre spring. In the same tick [`piece_impulses`] sizes what a loose
//! piece takes back, [`RubbleBodies::record_static_contacts`] stages the whole opposite
//! reaction on fixed structural construction, and [`RubbleBodies::record_moving_contacts`] the
//! impulse a moving structure took.
//!
//! - **What the belt bears on** ([`RubbleQuery::supports`]): solid, layer-compatible colliders,
//!   except [`super::terrain::FieldSurface`] geometry already represented by the field. Callers
//!   exclude self-support by identity ([`RubbleBodies::select_source`]). Every admitted dynamic
//!   body carries Avian force state; static construction transmits the full opposite belt
//!   reaction to [`crate::contact_transfer::ContactTransfers`] when it has structural laws.
//! - **The footprint check**: a link's slab ([`Slab`]) is the volume its field probes sweep, made
//!   solid across the true shoe width. Six support points reject separated shapes; the convex
//!   adapter then clips entry faces to that envelope. Narrow pieces and empty corners are
//!   resolved by their actual projected coverage, independently of the probe columns.
//! - **One support law**: `forces::contact_side` partitions overlapping footprints; the deepest
//!   surface in each cell enters the field's own support law at its covered share of the shoe.
//!   Stacked pieces cannot multiply the hull's support.
//! - **Work while held against rubble** (DERIVED from the loops): one AABB query per tank
//!   ([`RubbleQuery::near`], [`footprint_radius`]) selects its N candidates. Each side rejects N
//!   candidates against its slabs' union bounds. For L stations and P survivors this means
//!   N + L × P bounds checks, with one station dispatch when P > 0. Every slab AABB pass makes
//!   six support-map queries; only a positive-volume pass builds the boundary triangles, once
//!   per surface in the shared tick-start snapshot. Support-map and clipping work also depend on the shape's complexity.
//!   `forces::collect_column_contacts` partitions the covered faces without a contact cap, so
//!   face arrangements and their emitted patches still cost work on every held tick.
//! - **Recipient work**: for C dynamic contacts and D gathered dynamic bodies, grouping sorts
//!   D + C endpoint entries and the reaction pass sorts C contact keys. Each grouping pass walks
//!   C contacts and the deduplicated endpoint slots, using sorted endpoint lookups, path-halved
//!   roots and indexed failed-component membership. Each repeat clamps at least one body; none is
//!   re-enabled: at most D + 1 passes (DERIVED). The query bounds space, not population;
//!   local pile density, shape complexity and contact partitions remain cost inputs. With no
//!   nearby collider no shape is read, and field-only driving retains its force arithmetic.
//! - **Tick truth**: avian refits dynamic bodies' AABBs at the end of its physics step
//!   (`update_solver_body_aabbs`) and writes each collider's `Position`/`Rotation`, so a snapshot
//!   in `FixedUpdate` reads the poses the last step wrote.
//!
//! **One reaction law for every recipient, from its construction and motion.** The belt's
//! support is an explicit penalty spring and damper read at the tick-start pose. A dynamic body
//! the explicit step integrates them against stably ([`explicit_recipients`]: the connected
//! system of its source and the source's other recipients, by every patch's stiffness, damping
//! and both bodies' inertia) takes the whole opposite reaction,
//! `−(normal × load + grip + rolling) × dt`, whether or not it closes: a hull under another
//! tank's belt, a heavy slab. Any other is a *piece*: the explicit spring would drive it past the
//! belt within the step, so Avian alone resolves what holds it and its side is clamped, deliberately asymmetric
//! with the belt's support force: a resting piece takes no normal impulse. That clamp does not
//! conserve the belt-piece pair's momentum.
//! - *The tank's side* is the track law: a piece supports and grips the tank as ground does, with
//!   the slip and the support damping measured against the piece's own velocity at the contact
//!   ([`RubbleBodies::velocity_at`]).
//! - *The piece's side* is the clamped contact impulse of sequential-impulse solvers (Catto, the
//!   family avian's own solver belongs to), one pass over all sources' contacts in canonical
//!   geometry order, each sized after the impulses before it. The push, along the normal: the support reaction's
//!   impulse `load × dt`, at most what stops the piece closing on the belt. Then the drag, along
//!   the total tangent reaction's direction: its impulse, at most Coulomb's `μ × load × dt` (the
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

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{borrow::Cow, sync::OnceLock};

use avian3d::dynamics::solver::solver_body::SolverBodyInertia;
use avian3d::prelude::{
    Collider, ColliderAabb, ColliderOf, CollisionLayers, ComputedAngularInertia,
    ComputedCenterOfMass, ComputedMass, DefaultFriction, Forces, Friction, LockedAxes, Position,
    ReadRigidBodyForces, RigidBody, Rotation, Sensor, SpatialQuery, WriteRigidBodyForces,
};
use bevy::ecs::query::{QueryData, QueryFilter};
use bevy::ecs::system::SystemParam;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, ParallelSlice, TaskPool};

use super::terrain::FieldSurface;
use crate::contact_transfer::{AppliedContact, BodyInertia, ContactTransfers, StructureBody};
use crate::cost::stages::{Stage, Stages, Work};

use super::forces::BeltContact;

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
        super::world_contact::solid(layers, self.sensors.contains(collider))
            && !self.field_surfaces.contains(collider)
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

pub(crate) type PieceReadData = <PieceData as QueryData>::ReadOnly;

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
    /// Outward-wound convex boundary at the tick-start pose.
    triangles: OnceLock<Vec<[Vec3; 3]>>,
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
    pub(crate) fn aabb(&self) -> (Vec3, Vec3) {
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

/// A convex solid's actual entry faces clipped to one belt element's envelope.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SlabOverlap {
    pub(crate) piece: usize,
    pub(crate) faces: Vec<super::world_contact::convex::ClippedFace>,
}

/// The colliders near every driving tank this tick, read once at tick start: each collider's
/// surface and its dynamic bodies' states. Empty where no tank has any near it, and for every
/// pass that reads the field alone (the envelope calibration).
#[derive(Default)]
pub struct RubbleBodies<'a> {
    surfaces: Cow<'a, [Surface]>,
    by_collider: Cow<'a, HashMap<Entity, usize>>,
    /// Indices in gather order, restricted once per source using its tick-start AABB query.
    active: Vec<usize>,
    pub(crate) stages: Option<Stages>,
    #[cfg(test)]
    surface_checks: AtomicUsize,
    #[cfg(test)]
    reaction_sort_keys: AtomicUsize,
    pub(crate) bodies: Cow<'a, [PieceBody]>,
}

impl RubbleBodies<'_> {
    /// Read the `near` colliders' shapes, frictions and bodies. Every dynamic body must carry the
    /// force state Avian constructs for it, readable through `bodies`.
    pub(crate) fn gather<D: QueryData<ReadOnly = PieceReadData>, F: QueryFilter>(
        rubble: &RubbleQuery,
        near: &[Entity],
        bodies: &Query<D, F>,
    ) -> Self {
        Self::gather_on_pool(rubble, near, bodies, ComputeTaskPool::get())
    }

    fn gather_on_pool<D: QueryData<ReadOnly = PieceReadData>, F: QueryFilter>(
        rubble: &RubbleQuery,
        near: &[Entity],
        bodies: &Query<D, F>,
        pool: &TaskPool,
    ) -> Self {
        // Bevy's slice map returns chunks in input order, independently of worker completion.
        let prepared = near.par_splat_map(pool, None, |_, colliders| {
            colliders
                .iter()
                .map(|&collider| {
                    let body = rubble.body_of(collider);
                    // A collider without a rigid body is fixed construction in Avian.
                    let kind = rubble
                        .rigid_bodies
                        .get(body)
                        .copied()
                        .unwrap_or(RigidBody::Static);
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
                    assert!(
                        shape.shape_scaled().as_cuboid().is_some()
                            || shape.shape_scaled().as_convex_polyhedron().is_some(),
                        "belt support shapes are boxes or convex polyhedra; fix the collider constructor"
                    );
                    (
                        Surface {
                            collider,
                            owner: body,
                            shape: shape.clone(),
                            position: position.0,
                            rotation: rotation.0,
                            aabb: *aabb,
                            triangles: OnceLock::new(),
                            body: None,
                            friction: rubble.friction(collider),
                            structure: rubble.structures.contains(body).then_some(body),
                            motion,
                        },
                        kind.is_dynamic(),
                    )
                })
                .collect::<Vec<_>>()
        });
        let mut out = Self::default();
        let mut owners = Vec::new();
        let mut slots = HashMap::<Entity, usize>::default();
        // Collider indices and shared body slots follow gather order, never task assignment.
        for (mut surface, dynamic) in prepared.into_iter().flatten() {
            if dynamic {
                surface.body = Some(*slots.entry(surface.owner).or_insert_with(|| {
                    let slot = owners.len();
                    owners.push(surface.owner);
                    slot
                }));
            }
            let index = out.surfaces.len();
            out.by_collider.to_mut().insert(surface.collider, index);
            out.surfaces.to_mut().push(surface);
            out.active.push(index);
        }
        let prepared_bodies = owners.par_splat_map(pool, None, |_, owners| {
            owners.iter().map(|&body| {
                let (forces, _, mass, inertia, com, locked) = bodies.get(body).expect(
                    "an admitted dynamic support body carries Avian force state its caller can read",
                );
                let rotation = forces.rotation().0;
                // Avian's solver constructor applies the same axis locks as point impulses.
                let inertia = SolverBodyInertia::new(
                    mass.inverse(),
                    inertia.rotated(rotation).inverse(),
                    locked.copied().unwrap_or_default(),
                    0,
                    true,
                );
                PieceBody {
                    entity: body,
                    com: forces.position().0 + rotation * com.0,
                    inv_mass: inertia.effective_inv_mass(),
                    inv_inertia: inertia.effective_inv_angular_inertia().to_mat3(),
                    linear: forces.linear_velocity(),
                    angular: forces.angular_velocity(),
                }
            }).collect::<Vec<_>>()
        });
        out.bodies = prepared_bodies
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .into();
        out
    }

    /// Borrow the common tick-start geometry and motion; only candidate membership is local.
    pub(crate) fn for_source(&self, source: Entity, near: &[Entity]) -> RubbleBodies<'_> {
        let mut selected = RubbleBodies {
            surfaces: Cow::Borrowed(&self.surfaces),
            by_collider: Cow::Borrowed(&self.by_collider),
            bodies: Cow::Borrowed(&self.bodies),
            stages: self.stages.clone(),
            ..Default::default()
        };
        selected.select_source(source, near);
        selected
    }

    /// Select the source's tick-start nearby colliders and exclude its own body. The shared
    /// snapshot retains all recipients for reaction sizing, but each link tests only this tank's
    /// candidates. Sorting indices preserves gather order, including for an unordered input list.
    pub(crate) fn select_source(&mut self, source: Entity, near: &[Entity]) {
        let _stage = Stages::start(self.stages.as_ref(), Stage::BeltRubble);
        self.active.clear();
        for collider in near {
            let index = *self
                .by_collider
                .get(collider)
                .expect("a source's nearby collider belongs to the shared tick-start snapshot");
            if self.surfaces[index].owner != source {
                self.active.push(index);
            }
        }
        self.active.sort_unstable();
        self.active.dedup();
        self.count(Work::BeltCandidates, self.active.len());
    }

    fn count(&self, work: Work, count: usize) {
        if let Some(stages) = &self.stages {
            stages.add(work, count as u64);
        }
    }

    fn surface(&self, collider: Entity) -> &Surface {
        let index = self
            .by_collider
            .get(&collider)
            .expect("a belt collider contact belongs to the gathered footprint surfaces");
        &self.surfaces[*index]
    }

    /// Whether `collider` belongs to a gathered dynamic piece.
    #[cfg(any(feature = "dev_ui", test))]
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

    /// Stage the impulse each moving structure took (a loose chunk group) as an implicit contact
    /// named with its source, through the reading fixed construction's reaction joins
    /// ([`Self::record_static_contacts`]): its laws judge the load the group actually took, and
    /// the reading sizes its blow against the pair's masses, the group's from this tick-start
    /// snapshot. `dt` is the tick the pushes were sized over.
    pub(crate) fn record_moving_contacts(
        &self,
        pushes: &[Push],
        transfers: &mut ContactTransfers,
        dt: f32,
    ) {
        for push in pushes {
            let surface = self.surface(push.collider);
            let Some(structure) = surface.structure else {
                continue;
            }; // No structural law.
            let piece = &self.bodies[surface
                .body
                .expect("a push lands on a gathered dynamic body")];
            transfers.stage_on_moving(
                AppliedContact {
                    structure,
                    structure_collider: push.collider,
                    body: push.source,
                    point: push.point,
                    normal: push.normal,
                    force: -push.impulse / dt,
                    closing_speed: push.closing_speed,
                },
                BodyInertia {
                    centre: piece.com,
                    inverse_mass: piece.inv_mass,
                    inverse_inertia: piece.inv_inertia,
                },
            );
        }
    }

    /// Each station's overlaps in gather order. One side owns the station batch; its union
    /// bounds reject candidates before station dispatch, without changing an individual slab.
    pub(crate) fn in_slabs(&self, slabs: &[Slab]) -> Vec<Vec<SlabOverlap>> {
        if self.active.is_empty() || slabs.is_empty() {
            return vec![Vec::new(); slabs.len()];
        }
        self.in_slabs_on_pool(slabs, ComputeTaskPool::get())
    }

    fn in_slabs_on_pool(&self, slabs: &[Slab], pool: &TaskPool) -> Vec<Vec<SlabOverlap>> {
        let _stage = Stages::start(self.stages.as_ref(), Stage::BeltRubble);
        let prepared: Vec<_> = slabs.iter().map(|slab| (slab, slab.aabb())).collect();
        let (lo, hi) = prepared.iter().fold(
            (Vec3::INFINITY, Vec3::NEG_INFINITY),
            |(lo, hi), &(_, (a, b))| (lo.min(a), hi.max(b)),
        );
        #[cfg(test)]
        self.surface_checks
            .fetch_add(self.active.len(), Ordering::Relaxed);
        let candidates: Vec<_> = self
            .active
            .iter()
            .copied()
            .filter(|&piece| {
                let aabb = self.surfaces[piece].aabb;
                !(aabb.min.cmpgt(hi) | aabb.max.cmplt(lo)).any()
            })
            .collect();
        if candidates.is_empty() {
            return vec![Vec::new(); slabs.len()]; // The side's entire envelope is clear.
        }
        self.count(Work::BeltDispatches, 1);
        self.count(Work::BeltSlabChecks, slabs.len() * candidates.len());
        // Bevy 0.19.1 ParallelSlice preserves input order across station chunks.
        prepared
            .par_splat_map(pool, None, |_, stations| {
                stations
                    .iter()
                    .map(|&(slab, bounds)| {
                        candidates
                            .iter()
                            .filter_map(|&piece| self.overlap(piece, slab, bounds))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten()
            .collect()
    }

    fn overlap(&self, piece: usize, slab: &Slab, (lo, hi): (Vec3, Vec3)) -> Option<SlabOverlap> {
        let s = &self.surfaces[piece];
        if (s.aabb.min.cmpgt(hi) | s.aabb.max.cmplt(lo)).any() {
            return None; // Candidate lies outside this contact envelope.
        }
        self.count(Work::BeltSlabAabbPasses, 1);
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
        let radial = (slab.face - slab.reach, slab.face + slab.travel);
        if u.1 <= u.0 || l.1 <= l.0 || outer <= radial.0 || inner >= radial.1 {
            return None; // No positive volume reaches the probe envelope.
        }
        let triangles = s.triangles.get_or_init(|| {
            self.count(Work::BeltTriangulations, 1);
            Surface::triangles(&s.shape, s.position, s.rotation)
        });
        let faces = super::world_contact::convex::clip_faces(triangles, slab);
        self.count(Work::BeltOverlaps, usize::from(!faces.is_empty()));
        (!faces.is_empty()).then_some(SlabOverlap { piece, faces })
    }

    #[cfg(test)]
    pub(crate) fn in_slab(&self, slab: &Slab) -> Vec<SlabOverlap> {
        self.in_slabs(std::slice::from_ref(slab))
            .pop()
            .expect("one slab has one result")
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

/// A belt source's inertia before driving forces. The reaction bound includes both sides;
/// source identity alone cannot supply its mass or its permitted rigid modes.
#[derive(Clone, Copy)]
pub(crate) struct BeltSource {
    pub(crate) entity: Entity,
    inertia: BodyInertia,
}

impl BeltSource {
    pub(crate) fn new(
        entity: Entity,
        centre: Vec3,
        rotation: Quat,
        mass: &ComputedMass,
        angular: &ComputedAngularInertia,
        locked: Option<&LockedAxes>,
    ) -> Self {
        let inertia = SolverBodyInertia::new(
            mass.inverse(),
            angular.rotated(rotation).inverse(),
            locked.copied().unwrap_or_default(),
            0,
            true,
        );
        Self {
            entity,
            inertia: BodyInertia {
                centre,
                inverse_mass: inertia.effective_inv_mass(),
                inverse_inertia: inertia.effective_inv_angular_inertia().to_mat3(),
            },
        }
    }

    pub(crate) fn read<D: QueryData<ReadOnly = PieceReadData>, F: QueryFilter>(
        entity: Entity,
        bodies: &Query<D, F>,
    ) -> Self {
        let (forces, _, mass, angular, com, locked) = bodies
            .get(entity)
            .expect("every driving source has Avian mass, pose and force state at tick start");
        let rotation = forces.rotation().0;
        Self::new(
            entity,
            forces.position().0 + rotation * com.0,
            rotation,
            mass,
            angular,
            locked,
        )
    }

    fn inverse_at(self, point: Vec3, normal: Vec3) -> f32 {
        let arm = (point - self.inertia.centre).cross(normal);
        normal.dot(self.inertia.inverse_mass * normal) + arm.dot(self.inertia.inverse_inertia * arm)
    }

    /// FIXTURE: an imposed belt velocity has no finite source response.
    #[cfg(test)]
    pub(crate) fn fixed(entity: Entity) -> Self {
        Self {
            entity,
            inertia: BodyInertia {
                centre: Vec3::ZERO,
                inverse_mass: Vec3::ZERO,
                inverse_inertia: Mat3::ZERO,
            },
        }
    }
}

/// The root of one connected spring/damper component, including dynamic belt sources.
fn reaction_root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

/// The explicit step's two rates on a spring/damper system (module doc, [`EXPLICIT_LIMIT`]):
/// `dt² k / m` and `dt c / m`, each summed over its patches' inverse responses.
#[derive(Clone, Copy, Default)]
struct Rates {
    spring: f32,
    damper: f32,
}

impl Rates {
    fn add(&mut self, contact: &BeltContact, inverse: f32, dt: f32) {
        self.spring += contact.stiffness * dt * dt * inverse;
        self.damper += contact.damping * dt * inverse;
    }

    /// Stable: DERIVED from the amplification matrix, `spring + 2 damper < 4`. A damper rate
    /// between 1 and 2 still turns a closing velocity back within the step, `v' = v (1 −
    /// damper)`, at a smaller speed: no energy is created, but an overdamped pair can part.
    fn explicit(self) -> bool {
        self.spring + 2.0 * self.damper < EXPLICIT_LIMIT
    }
}

/// Which dynamic recipients take the whole opposite reaction: those of each connected
/// spring/damper system whose trace bound passes [`Rates::explicit`]. A system is a dynamic
/// source with every recipient it presses; a clamped recipient's spring, which the source
/// still takes whole, counts on the source's side alone. A recipient that fails on its own
/// patches is clamped first, so one light piece leaves its source's other recipients to be
/// judged without it; a system still failing clamps all of them. The trace bounds the stiffest
/// rigid mode. The source's field and fixed-construction springs are not counted: with them,
/// two parked hulls read over the limit and the clamp would give the lower one no reaction.
fn explicit_recipients(
    bodies: &RubbleBodies,
    dynamic: &[(&BeltContact, usize, Entity, BeltSource, f32)],
    dt: f32,
) -> Vec<bool> {
    let mut entities: Vec<_> = bodies
        .bodies
        .iter()
        .map(|b| b.entity)
        .chain(dynamic.iter().map(|c| c.3.entity))
        .collect();
    entities.sort_unstable();
    entities.dedup();
    let index = |entity| {
        entities
            .binary_search(&entity)
            .expect("every reaction endpoint is indexed")
    };
    let receiver_inverse = |contact: &BeltContact, slot: usize| {
        bodies.bodies[slot]
            .effective_mass(contact.point, contact.normal)
            .recip()
    };
    let mut own = vec![Rates::default(); bodies.bodies.len()];
    for &(contact, slot, _, source, _) in dynamic {
        let inverse =
            source.inverse_at(contact.point, contact.normal) + receiver_inverse(contact, slot);
        own[slot].add(contact, inverse, dt);
    }
    let mut explicit: Vec<bool> = own.iter().map(|rates| rates.explicit()).collect();
    loop {
        let mut parents: Vec<_> = (0..entities.len()).collect();
        let mut rates = vec![Rates::default(); entities.len()];
        for &(contact, slot, _, source, _) in dynamic {
            let source_inverse = source.inverse_at(contact.point, contact.normal);
            if !explicit[slot] {
                rates[index(source.entity)].add(contact, source_inverse, dt);
                continue;
            }
            let r = index(bodies.bodies[slot].entity);
            // A fully fixed source imposes its motion; it couples no recipients to one another.
            if source.inertia.inverse_mass != Vec3::ZERO
                || source.inertia.inverse_inertia != Mat3::ZERO
            {
                let a = reaction_root(&mut parents, r);
                let b = reaction_root(&mut parents, index(source.entity));
                parents[b] = a;
            }
            rates[r].add(
                contact,
                source_inverse + receiver_inverse(contact, slot),
                dt,
            );
        }
        let mut totals = vec![Rates::default(); entities.len()];
        for (i, rate) in rates.into_iter().enumerate() {
            let total = &mut totals[reaction_root(&mut parents, i)];
            total.spring += rate.spring;
            total.damper += rate.damper;
        }
        let mut failing_roots = vec![false; entities.len()];
        for (slot, &whole) in explicit.iter().enumerate() {
            if whole {
                let component = reaction_root(&mut parents, index(bodies.bodies[slot].entity));
                failing_roots[component] = !totals[component].explicit();
            }
        }
        if !failing_roots.iter().any(|&failing| failing) {
            return explicit;
        }
        // A failing coupled system clamps all its explicit recipients together.
        for (whole, body) in explicit.iter_mut().zip(bodies.bodies.iter()) {
            if failing_roots[reaction_root(&mut parents, index(body.entity))] {
                *whole = false;
            }
        }
    }
}

/// One impulse a dynamic piece took: `impulse` (N·s, world) at `point`, through `collider`, from
/// the belt of `source`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Push {
    pub(crate) body: Entity,
    pub(crate) collider: Entity,
    pub(crate) source: Entity,
    pub(crate) impulse: Vec3,
    pub(crate) point: Vec3,
    /// The contact's normal, unit, from the piece into the source.
    pub(crate) normal: Vec3,
    /// How fast the piece closed on the belt along `normal` before any recipient impulse (m/s);
    /// negative when parting.
    pub(crate) closing_speed: f32,
}

/// The force the belt law applied to its source through `contact` (N): the support along the
/// normal and both tangent contributions. A recipient held by its own laws takes its opposite.
fn source_force(contact: &BeltContact) -> Vec3 {
    contact.normal * contact.load + contact.tangential_force()
}

/// The explicit spring/damper stability limit. RESOLUTION: symplectic Euler with explicit
/// damping is stable while `dt² k/m + 2 dt c/m < 4` (DERIVED from its amplification matrix).
/// The sum over a system's patches is a trace bound on its stiffest rigid mode
/// ([`Rates::explicit`]).
const EXPLICIT_LIMIT: f32 = 4.0;

/// Every dynamic recipient's side of `contacts`, each with its source body (module doc), by one law for every body: the
/// whole opposite reaction on a recipient the explicit step integrates the belt's springs against
/// stably ([`EXPLICIT_LIMIT`], over all its patches from every source this tick), the clamped
/// impulse on any other. Pieces are sized in one canonical order of the contacts' geometry, each
/// impulse taken into `bodies` before the next is sized, so the order the contacts were gathered
/// in, and which tank produced them, decide nothing. One push per contact on a dynamic body; a
/// clamped push's normal part is zero where the piece is not closing on the belt.
pub(crate) fn piece_impulses<'c>(
    bodies: &mut RubbleBodies,
    contacts: impl IntoIterator<Item = (BeltSource, &'c BeltContact)>,
    dt: f32,
) -> Vec<Push> {
    let _stage = Stages::start(bodies.stages.as_ref(), Stage::BeltRubble);
    let mut dynamic = Vec::new();
    for (source, contact) in contacts {
        // Field support has no recipient, and static or kinematic recipients take no impulses.
        let Some(collider) = contact.collider else {
            continue;
        };
        let surface = bodies.surface(collider);
        let Some(slot) = surface.body else { continue };
        let closing_speed = (bodies.bodies[slot].velocity_at(contact.point)
            - contact.belt_velocity)
            .dot(contact.normal);
        dynamic.push((contact, slot, collider, source, closing_speed));
    }
    bodies.count(Work::BeltDynamicContacts, dynamic.len());
    if bodies.stages.is_some() {
        let mut recipients = vec![false; bodies.bodies.len()];
        for &(_, slot, ..) in &dynamic {
            recipients[slot] = true;
        }
        bodies.count(
            Work::BeltRecipients,
            recipients.into_iter().filter(|seen| *seen).count(),
        );
    }
    let explicit = explicit_recipients(bodies, &dynamic, dt);
    dynamic.sort_by_cached_key(|(contact, ..)| {
        #[cfg(test)]
        bodies.reaction_sort_keys.fetch_add(1, Ordering::Relaxed);
        // Lexicographic over the contact's geometry and forces, as bits.
        [
            contact.point.x.to_bits(),
            contact.point.y.to_bits(),
            contact.point.z.to_bits(),
            contact.normal.x.to_bits(),
            contact.normal.y.to_bits(),
            contact.normal.z.to_bits(),
            contact.tangential_force().x.to_bits(),
            contact.tangential_force().y.to_bits(),
            contact.tangential_force().z.to_bits(),
            contact.belt_velocity.x.to_bits(),
            contact.belt_velocity.y.to_bits(),
            contact.belt_velocity.z.to_bits(),
            contact.load.to_bits(),
            contact.mu.to_bits(),
        ]
    });
    let mut pushes = Vec::new();
    for (contact, slot, collider, source, closing_speed) in dynamic {
        let body = &mut bodies.bodies.to_mut()[slot];
        let impulse = if explicit[slot] {
            -source_force(contact) * dt
        } else {
            contact_impulse(body, contact, dt)
        };
        body.apply(impulse, contact.point);
        pushes.push(Push {
            body: body.entity,
            collider,
            source: source.entity,
            impulse,
            point: contact.point,
            normal: contact.normal,
            closing_speed,
        });
    }
    pushes
}

/// The impulse one contact puts on `body` (module doc): the push, then the drag sized against the
/// pushed piece. `contact.normal` points from the piece to the belt; the piece's side runs
/// along `-normal` and opposite the hull's total tangent force. The drag's
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
    let Ok(dir) = Dir3::new(-contact.tangential_force()) else {
        return push;
    };
    // How far the pushed piece lags the belt surface along the drag.
    let lag = -(rel + body.response(push, point)).dot(*dir);
    let catch_up = if lag > 0.0 {
        body.effective_mass(point, *dir) * lag
    } else {
        0.0
    };
    let drag = (contact.tangential_force().length() * dt)
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
        Collider, ColliderDensity, Gravity, LayerMask, LinearVelocity, PhysicsPlugins, Position,
        Rotation,
    };
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::{Affine3A, Quat, Vec2};
    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::Layer;
    use crate::construction::breakage::ChunkCollider;
    use crate::substances::contact_friction;
    use crate::track::forces::{
        ForceParams, GripElements, GripParams, SideInput, SideReport, SideState, TravelField,
        contact_side,
    };
    use crate::track::oracle::{BlockField, TerrainBlock, TerrainOracle};

    /// Fixture: one fixed tick (s).
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
            super::super::world_contact::plugin,
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

    /// A static construction root holding one 2 × 0.6 × 2 m standing chunk whose top is at y = 0.6,
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
                ChunkCollider(0),
            ))
            .id();
        (root, chunk)
    }

    /// Driving and reaction commit require the same geometry, motion and shared body slots
    /// under any worker assignment. Fixture: separate boxes, a compound dynamic body, a
    /// kinematic surface and fixed construction; pool sizes vary the preparation batches.
    #[test]
    fn support_snapshot_is_independent_of_preparation_workers() {
        use bevy::tasks::TaskPoolBuilder;
        let mut app = physics();
        let mut near: Vec<_> = (0..8)
            .map(|i| piece(&mut app, i as f32 * 3.0, 0.0))
            .collect();
        let shared = app
            .world_mut()
            .spawn((
                ChildOf(near[0]),
                Transform::from_xyz(0.0, 0.5, 0.0),
                Collider::cuboid(0.2, 0.2, 0.2),
            ))
            .id();
        app.world_mut()
            .entity_mut(near[0])
            .insert(LockedAxes::ROTATION_LOCKED);
        let moving = piece(&mut app, 30.0, 0.0);
        app.world_mut().entity_mut(moving).insert((
            RigidBody::Kinematic,
            LinearVelocity(Vec3::new(0.2, 0.0, 0.3)),
        ));
        let (_, fixed) = standing_chunk(&mut app, 40.0, 0.0);
        // A shared owner's second collider crosses batch boundaries in the fixture order.
        near.reverse();
        near.extend([moving, fixed, shared]);
        app.update();
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let reference = RubbleBodies::gather(&rubble, &near, &bodies);
                assert_eq!(reference.bodies.len(), 8, "one state per dynamic owner");
                assert_eq!(
                    reference.surface(shared).body,
                    reference.surface(near[7]).body
                );
                assert!(reference.surface(moving).motion.is_some());
                assert!(reference.surface(fixed).body.is_none());
                // Fixture worker counts exercise different chunk boundaries and completion orders.
                for workers in [1, 2, 4] {
                    let pool = TaskPoolBuilder::new().num_threads(workers).build();
                    let actual = RubbleBodies::gather_on_pool(&rubble, &near, &bodies, &pool);
                    let slab = Slab {
                        origin: Vec3::new(10.0, 0.3, -0.1),
                        axis: Vec3::X,
                        lat: Vec3::Z,
                        out: Vec3::NEG_Y,
                        len: 35.0,
                        lateral: (-0.2, 0.2),
                        face: 0.025,
                        reach: 0.5,
                        travel: 0.1,
                    };
                    let hits = reference.in_slab(&slab);
                    assert!(
                        hits.len() > 1,
                        "fixture must cross candidate batch boundaries"
                    );
                    assert_eq!(
                        actual.in_slabs_on_pool(std::slice::from_ref(&slab), &pool),
                        vec![hits]
                    );
                    // Fixture: disjoint station envelopes and an empty station exercise the
                    // side union and station chunk boundaries. Batching cannot change a station.
                    let stations = [0.0, 9.0, 18.0, 40.0, 100.0].map(|x| Slab {
                        origin: Vec3::new(x, 0.3, -0.1),
                        len: 1.0,
                        ..slab
                    });
                    let independent: Vec<_> =
                        stations.iter().map(|s| reference.in_slab(s)).collect();
                    assert!(independent.iter().any(Vec::is_empty));
                    assert!(independent.iter().any(|hits| !hits.is_empty()));
                    assert_eq!(actual.in_slabs_on_pool(&stations, &pool), independent);
                    assert_eq!(actual.bodies, reference.bodies);
                    assert_eq!(actual.active, reference.active);
                    assert_eq!(actual.by_collider, reference.by_collider);
                    for (a, b) in actual.surfaces.iter().zip(reference.surfaces.iter()) {
                        assert_eq!(
                            (a.collider, a.owner, a.body, a.structure),
                            (b.collider, b.owner, b.body, b.structure),
                        );
                        assert_eq!(
                            (a.position, a.rotation, a.aabb),
                            (b.position, b.rotation, b.aabb)
                        );
                        assert_eq!(a.triangles, b.triangles);
                        assert_eq!(a.friction, b.friction);
                        let motion = |m: &SurfaceMotion| (m.com, m.linear, m.angular);
                        assert_eq!(a.motion.as_ref().map(motion), b.motion.as_ref().map(motion));
                    }
                }
                println!(
                    "MEASURED identical support snapshots with 1, 2 and 4 preparation workers"
                );
            })
            .unwrap();
    }

    /// Flat terrain field, top face at y = 0.
    fn ground() -> BlockField {
        BlockField::new(vec![TerrainBlock::new(
            Vec3::new(0.0, -0.5, 0.0),
            Quat::IDENTITY,
            Vec3::new(200.0, 1.0, 200.0),
        )])
    }

    /// The mechanical law must depend on geometry and material, not their adapter.
    /// Fixture: the same planar ramp as a grid and a fixed convex solid, with equal effective
    /// grip and rolling facts. Tangent translation tests tessellation-independent pressure;
    /// normal closing tests both adapters with the same facets.
    #[test]
    fn grid_and_convex_adapters_give_equivalent_contact_for_equal_surfaces() {
        use crate::ground_field::{ContactGround, GroundField};
        use crate::terrain_grid::{HeightGrid, TerrainExtent};
        let mut app = physics();
        let height = |x: f32, z: f32| 0.2 + 0.03 * x + 0.02 * z;
        let mut vertices = Vec::new();
        for x in [-2.0, 2.0] {
            for z in [-2.0, 2.0] {
                vertices.extend([Vec3::new(x, height(x, z), z), Vec3::new(x, -1.0, z)]);
            }
        }
        let solid = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                Collider::convex_hull(vertices).expect("fixture ramp is a finite convex prism"),
                contact_friction(0.7),
            ))
            .id();
        app.update();
        app.world_mut().run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
            let pieces = RubbleBodies::gather(&rubble, &[solid], &bodies);
            let grip = pad_grip(contact_friction(0.8));
            let ground = ContactGround {
                grip: grip.pair(pieces.friction(solid)),
                rolling_resistance: super::super::forces::rigid_rolling_resistance(),
            };
            let pose = Affine3A::from_translation(Vec3::Y * 0.1);
            let tangent = Vec3::new(0.4, 0.03 * 0.4 + 0.02 * 0.3, 0.3);
            let run = |field: &BlockField, pieces: &RubbleBodies, velocity: Vec3| pad_pass_gripping(
                grip, field, pieces, pose, SideState { speed: 0.6, ..default() },
                velocity, &mut GripElements::for_links(30),
            );
            let empty = BlockField::default();
            let totals = |report: &SideReport| {
                let mut force = Vec3::ZERO;
                let mut torque = Vec3::ZERO;
                for app in &report.apps {
                    force += app.force;
                    torque += app.point.cross(app.force);
                }
                (force, torque, report.contacts.iter().map(|c| c.load).sum::<f32>())
            };
            for (size, velocity) in [(2, tangent), (3, tangent), (5, tangent), (9, tangent), (2, Vec3::new(0.4, -0.2, 0.3))] {
                let reference = run(&empty, &pieces, velocity);
                assert!(!reference.contacts.is_empty());
                assert!(reference.contacts.iter().all(|c| c.collider == Some(solid)));
                let (force, torque, load) = totals(&reference);
                let mut samples = Vec::new();
                for j in 0..size {
                    for i in 0..size {
                        samples.push(height(-2.0 + 4.0 * i as f32 / (size - 1) as f32,
                                            -2.0 + 4.0 * j as f32 / (size - 1) as f32));
                    }
                }
                let grid = HeightGrid::new(samples.into(), size, TerrainExtent {
                    world_size_m: 4.0, height_offset_m: -1.0, height_span_m: 2.0,
                });
                let field = BlockField::default().with_height(Some(grid))
                    .with_ground(Some(GroundField::uniform(ground)));
                let report = run(&field, &RubbleBodies::default(), velocity);
                assert!(!report.contacts.is_empty());
                let expected_normal = Vec3::new(-0.03, 1.0, -0.02).normalize();
                for contact in &report.contacts {
                    assert_eq!(contact.collider, None);
                    assert!((contact.normal - expected_normal).length() < 1e-5);
                    assert!((contact.point.y - height(contact.point.x, contact.point.z)).abs() < 1e-5);
                    assert_eq!(contact.mu, ground.grip);
                }
                let (f, t, l) = totals(&report);
                // Resolution: accumulated f32 clipping/moments across differing tessellations.
                assert!((f - force).length() < load * 1e-5, "{size}: {f} vs {force}");
                assert!((t - torque).length() < load * 1e-5, "{size}: {t} vs {torque}");
                assert!((l - load).abs() < load * 1e-5);
                let both = totals(&run(&field, &pieces, velocity));
                assert!((both.0-force).length() < load*1e-5, "coincident adapters doubled support");
                assert!((both.1-torque).length() < load*1e-5, "coincident adapters changed the lever arm");
                println!("MEASURED grid/convex equivalence {size}, velocity {velocity}: force error {} N, torque error {} N m, load error {} N",
                    (f-force).length(), (t-torque).length(), (l-load).abs());
            }
        }).unwrap();
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
    /// adapter relies on every admitted recipient carrying its spawn-established force state.
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

    /// The world interface must deliver load on unmarked static geometry, including a map-edge
    /// wall. Self-owned children, sensors and query volumes cannot contribute a patch.
    #[test]
    fn unmarked_solids_carry_load_without_admitting_self_or_observations() {
        let mut app = physics_with(Vec3::ZERO);
        // Fixture: coincident static boxes under a calibration pad, no gameplay-kind markers.
        let solid = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.2, 0.0),
                Collider::cuboid(2.0, 0.4, 2.0),
            ))
            .id();
        let owner = app
            .world_mut()
            .spawn((Transform::IDENTITY, RigidBody::Static))
            .id();
        let own = app
            .world_mut()
            .spawn((
                ChildOf(owner),
                Transform::from_xyz(0.0, 0.2, 0.0),
                Collider::cuboid(2.0, 0.4, 2.0),
            ))
            .id();
        let sensor = app
            .world_mut()
            .spawn((Transform::IDENTITY, Collider::cuboid(2.0, 0.4, 2.0), Sensor))
            .id();
        let query = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                Collider::cuboid(2.0, 0.4, 2.0),
                CollisionLayers::new([Layer::Armor], LayerMask::NONE),
            ))
            .id();
        app.update();
        app.update();
        app.world_mut()
            .run_system_once(move |world: RubbleQuery, bodies: Query<PieceData>| {
                let near = world.near(Vec3::ZERO, 4.0);
                assert!(near.contains(&solid) && near.contains(&own));
                assert!(!near.contains(&sensor) && !near.contains(&query));
                let snapshot = RubbleBodies::gather(&world, &near, &bodies);
                let selected = snapshot.for_source(owner, &near);
                let report = flat_pass(&selected);
                assert!(
                    report
                        .contacts
                        .iter()
                        .any(|c| c.collider == Some(solid) && c.load > 0.0),
                    "a compatible unmarked solid must carry belt load"
                );
                assert!(
                    report.contacts.iter().all(|c| c.collider != Some(own)
                        && c.collider != Some(sensor)
                        && c.collider != Some(query)),
                    "self and observations cannot enter the mechanical law"
                );
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
                let state = &mut snapshot.bodies.to_mut()[0];
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
                    pieces.select_source(source, &near);
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
        let mut seen = 0;
        for (point, press) in transfers
            .presses()
            .filter(|(point, ..)| point.body == source)
        {
            assert_eq!(point.structure, root);
            assert_eq!(point.structure_collider, collider);
            assert_eq!(point.body_collider, None);
            reaction += press * transfers.dt();
            seen += 1;
        }
        assert!(seen > 1, "the reading must include the footprint's patches");
        let impulse = expected * transfers.dt();
        assert!((reaction + impulse).length() < 1e-4 * impulse.length());
        let after = app.world().get::<LinearVelocity>(source).unwrap().0;
        assert!(((after - before) * mass + reaction).length() < 1e-4 * impulse.length());
        app.update();
        assert!(
            !app.world()
                .resource::<ContactTransfers>()
                .presses()
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

    /// A belt driving over a moving structure (a fallen chunk group) loads that structure's laws
    /// with the impulse the structure actually took, named with its source: the reading holds a
    /// contact force whose time integral equals the pushes. Moving groups rely on this
    /// applied load to assess their internal joints without a second momentum exchange. FIXTURE DATA: a 14.4 kg source box closing at 1 m/s on a free 2 × 0.6 × 2 m slab
    /// of the same mass, topped at 0.6 m.
    #[test]
    fn a_belt_on_a_moving_structure_loads_its_reading_by_the_impulse_it_took() {
        let mut app = physics();
        let slab = app
            .world_mut()
            .spawn((
                Position(Vec3::Y * 0.3),
                Rotation::default(),
                RigidBody::Dynamic,
                StructureBody,
                Collider::cuboid(2.0, 0.6, 2.0),
                ColliderDensity(6.0),
            ))
            .id();
        let source = piece_box(&mut app, Vec3::Y * 1.5, Vec3::splat(0.2));
        let closing = 1.0;
        app.world_mut()
            .entity_mut(source)
            .insert(LinearVelocity(Vec3::NEG_Y * closing));
        app.update();
        app.update();
        let pushes = app
            .world_mut()
            .run_system_once(
                move |rubble: RubbleQuery,
                      mut bodies: Query<PieceData>,
                      mut transfers: ResMut<ContactTransfers>| {
                    let near = rubble.near(Vec3::ZERO, 4.0);
                    let mut pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                    pieces.select_source(source, &near);
                    let before = bodies.get(source).unwrap().0.linear_velocity();
                    let report = pad_pass(
                        &ground(),
                        &pieces,
                        Affine3A::from_translation(Vec3::Y * 0.6),
                        SideState::default(),
                        before,
                        &mut GripElements::for_links(30),
                    );
                    pieces.record_static_contacts(source, &report.contacts, &mut transfers);
                    let pushes = piece_impulses(
                        &mut pieces,
                        report
                            .contacts
                            .iter()
                            .map(|contact| (BeltSource::read(source, &bodies), contact)),
                        DT,
                    );
                    pieces.record_moving_contacts(&pushes, &mut transfers, DT);
                    apply_pushes(&pushes, &mut bodies);
                    pushes
                },
            )
            .unwrap();
        assert!(
            pushes.iter().any(|push| push.body == slab),
            "the belt must bear on the slab"
        );
        app.update();
        let transfers = app.world().resource::<ContactTransfers>();
        let mut load = Vec3::ZERO;
        for (point, press) in transfers.presses().filter(|(p, ..)| p.structure == slab) {
            assert_eq!(point.body, source, "the contact is named with its source");
            assert_eq!(point.body_collider, None);
            load += press * transfers.dt();
        }
        let took: Vec3 = pushes.iter().map(|push| push.impulse).sum();
        assert!(
            (load - took).length() <= 1e-4 * took.length(),
            "the slab's reading {load} must be the impulse it took, {took}"
        );
    }

    /// A convex frustum supports the belt and receives its reaction, independently of boxes.
    /// Fixture: two twelve-point rings, radii 0.4/0.35 m, separated by 0.6 m.
    #[test]
    fn a_convex_frustum_carries_the_belt_and_is_loaded_by_it() {
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
                assert!(piece_impulses(&mut pieces, sourced(&report.contacts), DT).is_empty());
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
                let depth = |hits: Vec<SlabOverlap>| {
                    hits.iter()
                        .flat_map(|h| &h.faces)
                        .flat_map(|f| f.polygon.iter().map(|p| f.depth.dot(*p)))
                        .fold(f32::NEG_INFINITY, f32::max)
                };
                let short_depth = depth(pieces.in_slab(&slab));
                slab.origin.x = 10.0;
                let long_depth = depth(pieces.in_slab(&slab));
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

    /// `contacts` from one source body no reading names.
    fn sourced(contacts: &[BeltContact]) -> impl Iterator<Item = (BeltSource, &BeltContact)> {
        contacts
            .iter()
            .map(|contact| (BeltSource::fixed(Entity::PLACEHOLDER), contact))
    }

    /// A contact pressing `load` (N) along +Y on `collider` at `point`, belt at rest, no traction.
    /// FIXTURE DATA: its spring holds that load at a 1 cm press.
    fn press(point: Vec3, collider: Entity, load: f32) -> BeltContact {
        BeltContact {
            point,
            load,
            load_elastic: load,
            stiffness: load / 0.01,
            damping: 0.0,
            slip: 0.0,
            slip_lat: 0.0,
            f_long: 0.0,
            f_lat: 0.0,
            normal: Vec3::Y,
            traction: Vec3::ZERO,
            rolling_force: Vec3::ZERO,
            belt_velocity: Vec3::ZERO,
            collider: Some(collider),
            mu: 0.9,
        }
    }

    /// A damped support must not return more speed than came in to a closing recipient the
    /// unstable band clamps. Driving and moving-structure loads rely on the stability verdict
    /// including the damper; a stable band with a damper rate over 1 still parts the pair at a
    /// lower speed ([`Rates::explicit`]).
    /// FIXTURE: k=2 MN/m, c=100 kN s/m, penetration=1 cm, closing=1 m/s at 64 Hz.
    /// DERIVED: the old undamped gate passes both 500 kg and 200 kg and returns 2.75 and
    /// 8.375 m/s separation; the implicit clamp stops at the stationary belt instead.
    #[test]
    fn damped_reactions_do_not_fling_closing_recipients() {
        let mut app = physics();
        let collider = piece(&mut app, 0.0, 0.0);
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                for mass in [500.0, 200.0] {
                    let mut pieces = RubbleBodies::gather(&rubble, &[collider], &bodies);
                    let body = &mut pieces.bodies.to_mut()[0];
                    body.inv_mass = Vec3::splat(1.0 / mass);
                    body.linear = Vec3::Y;
                    body.angular = Vec3::ZERO;
                    let mut contact = press(body.com, collider, 120_000.0);
                    contact.stiffness = 2.0e6;
                    contact.damping = 1.0e5;
                    piece_impulses(&mut pieces, sourced(&[contact]), DT);
                    let speed = pieces.bodies[0].linear.y;
                    assert!(speed.abs() < 1e-5, "mass {mass}: left at {speed} m/s");
                }
            })
            .unwrap();
    }

    /// Footprint subdivision must not alter the structure's transferred momentum. The contact reader relies on
    /// every patch retaining the closing speed before any recipient impulse, even though the
    /// clamp sizes each impulse against the updated velocity. FIXTURE: two 1000 kg bodies,
    /// COM-normal reaction 800 N s, closing 1 m/s. Each structure force must integrate to the actual transferred impulse.
    #[test]
    fn subdividing_a_reaction_preserves_the_pre_solve_closing_and_transfer() {
        for copies in [1, 2] {
            let mut app = physics();
            let slab = piece(&mut app, 0.0, 0.0);
            let source = piece_box(&mut app, Vec3::Y * 5.0, Vec3::splat(1.0));
            for body in [slab, source] {
                app.world_mut()
                    .entity_mut(body)
                    .insert((avian3d::prelude::Mass(1000.0), avian3d::prelude::NoAutoMass));
            }
            app.world_mut()
                .entity_mut(slab)
                .insert((StructureBody, LinearVelocity(Vec3::Y)));
            app.update();
            app.update();
            let pushes = app
                .world_mut()
                .run_system_once(
                    move |rubble: RubbleQuery,
                          bodies: Query<PieceData>,
                          mut transfers: ResMut<ContactTransfers>| {
                        let mut pieces = RubbleBodies::gather(&rubble, &[slab], &bodies);
                        let centre = pieces.bodies[0].com;
                        let contacts: Vec<_> = (0..copies)
                            .map(|_| {
                                let mut contact = press(centre, slab, 800.0 / DT / copies as f32);
                                // FIXTURE: a weak undamped spring keeps the recipient in the explicit band.
                                contact.stiffness = 1000.0 / copies as f32;
                                contact
                            })
                            .collect();
                        let pushes = piece_impulses(
                            &mut pieces,
                            contacts
                                .iter()
                                .map(|c| (BeltSource::read(source, &bodies), c)),
                            DT,
                        );
                        pieces.record_moving_contacts(&pushes, &mut transfers, DT);
                        pushes
                    },
                )
                .unwrap();
            app.update();
            let reading = app.world().resource::<ContactTransfers>();
            let transferred: Vec3 = reading
                .presses()
                .filter(|(p, _)| p.structure == slab && p.body == source)
                .map(|(_, press)| press * reading.dt())
                .sum();
            let took: Vec3 = pushes.iter().map(|p| p.impulse).sum();
            assert!((transferred - took).length() < 1e-3);
            assert!(pushes.iter().all(|p| (p.closing_speed - 1.0).abs() < 1e-6));
        }
    }

    /// A shared contact spring acts on the pair's rigid mode. Driving relies on a light source
    /// entering the implicit band even when the recipient alone would integrate stably.
    /// FIXTURE: k=10 MN/m, no damper, source/recipient 1000 kg, 1 m/s closing, 64 Hz.
    /// DERIVED: recipient-only k dt²/m=2.4414; pair=4.8828 exceeds the stability limit.
    #[test]
    fn coupled_reaction_bound_includes_the_source_response() {
        let mut app = physics();
        let collider = piece(&mut app, 0.0, 0.0);
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let mut pieces = RubbleBodies::gather(&rubble, &[collider], &bodies);
                let body = &mut pieces.bodies.to_mut()[0];
                body.inv_mass = Vec3::splat(0.001);
                body.linear = Vec3::Y;
                body.angular = Vec3::ZERO;
                let mut contact = press(body.com, collider, 200_000.0);
                contact.stiffness = 1.0e7;
                let source = BeltSource {
                    entity: Entity::PLACEHOLDER,
                    inertia: BodyInertia {
                        centre: body.com,
                        inverse_mass: Vec3::splat(0.001),
                        inverse_inertia: Mat3::ZERO,
                    },
                };
                piece_impulses(&mut pieces, [(source, &contact)], DT);
                let speed = pieces.bodies[0].linear.y;
                println!("MEASURED coupled stability fixture: recipient speed {speed} m/s");
                assert!(
                    speed.abs() < 1e-5,
                    "pair must take the implicit clamp: {speed}"
                );
            })
            .unwrap();
    }

    /// A slab must carry its source's whole reaction even when another recipient is clamped.
    /// FIXTURE: a 57 t source; a 2 t slab at rest under a 2 MN/m, 100 kN s/m patch pressing
    /// 120 kN; a 16 kg
    /// brick under a 0.2 MN/m, 10 kN s/m patch. DERIVED rates: slab 1.87, brick 22.6.
    #[test]
    fn a_light_piece_leaves_a_co_sourced_slab_its_whole_reaction() {
        let mut app = physics();
        let slab = piece(&mut app, 0.0, 0.0);
        let brick = piece_box(
            &mut app,
            Vec3::new(5.0, 0.05, 0.0),
            Vec3::new(0.3, 0.1, 0.3),
        );
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                for with_brick in [false, true] {
                    let mut pieces = RubbleBodies::gather(&rubble, &[slab, brick], &bodies);
                    let mut points = [Vec3::ZERO; 2];
                    for (body, point) in pieces.bodies.to_mut().iter_mut().zip(&mut points) {
                        let mass = if body.entity == slab { 2000.0 } else { 16.0 };
                        body.inv_mass = Vec3::splat(1.0 / mass);
                        body.inv_inertia = Mat3::ZERO;
                        body.linear = Vec3::ZERO;
                        body.angular = Vec3::ZERO;
                        *point = body.com;
                    }
                    let slab_at = if pieces.bodies[0].entity == slab {
                        points[0]
                    } else {
                        points[1]
                    };
                    let brick_at = if pieces.bodies[0].entity == slab {
                        points[1]
                    } else {
                        points[0]
                    };
                    let mut on_slab = press(slab_at, slab, 120_000.0);
                    on_slab.stiffness = 2.0e6;
                    on_slab.damping = 1.0e5;
                    let mut on_brick = press(brick_at, brick, 2_000.0);
                    on_brick.stiffness = 2.0e5;
                    on_brick.damping = 1.0e4;
                    let source = BeltSource {
                        entity: Entity::PLACEHOLDER,
                        inertia: BodyInertia {
                            centre: slab_at,
                            inverse_mass: Vec3::splat(1.0 / 57_000.0),
                            inverse_inertia: Mat3::ZERO,
                        },
                    };
                    let contacts: Vec<_> = if with_brick {
                        vec![(source, &on_slab), (source, &on_brick)]
                    } else {
                        vec![(source, &on_slab)]
                    };
                    let pushes = piece_impulses(&mut pieces, contacts, DT);
                    let taken = pushes
                        .iter()
                        .find(|push| push.body == slab)
                        .expect("the slab takes a push")
                        .impulse
                        .y;
                    println!("MEASURED slab reaction, brick {with_brick}: {taken} N s");
                    assert!(
                        (taken + 120_000.0 * DT).abs() < 1e-2,
                        "brick {with_brick}: the slab took {taken} N s"
                    );
                }
            })
            .unwrap();
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
                let pushes = piece_impulses(&mut pieces, sourced(&contacts), DT);
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

    /// Solid recipients retain track/material pair grip in both belt and recipient reactions.
    /// Fixture: a 0.7 track sliding sideways over a 0.3 piece; ground grip has its own witness.
    #[test]
    fn the_belt_grips_solid_recipients_by_the_material_pair() {
        let track = contact_friction(0.7);
        let slide = Vec3::new(5.0, 0.0, 0.0);
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
                let pushes = piece_impulses(&mut pieces, sourced(&report.contacts), DT);
                let mus = report.contacts.iter().map(|c| c.mu).collect::<Vec<_>>();
                (sliding_grip(&report), mus, pushes)
            })
            .expect("the piece pass runs");
        let pair = grip.pair(contact_friction(row));
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

    /// Both recipient routes must include rolling in the source reaction. The clamped fixture
    /// is force-limited, so neither Coulomb nor catch-up can hide a dropped component.
    /// FIXTURE: 100 kg at its COM, closing at 1 m/s normally and moving along both tangent axes;
    /// 100 N support, 8 N grip and perpendicular 6 N rolling, no angular coupling.
    #[test]
    fn recipient_reactions_include_rolling_distinct_from_grip() {
        let mut contact = press(Vec3::ZERO, Entity::PLACEHOLDER, 100.0);
        contact.traction = Vec3::X * 8.0;
        contact.rolling_force = Vec3::Z * 6.0;
        let body = PieceBody {
            entity: Entity::PLACEHOLDER,
            com: Vec3::ZERO,
            inv_mass: Vec3::splat(1.0 / 100.0),
            inv_inertia: Mat3::ZERO,
            linear: Vec3::new(2.0, 1.0, 3.0),
            angular: Vec3::ZERO,
        };
        let expected = Vec3::new(8.0, 100.0, 6.0);
        let whole = source_force(&contact);
        let clamped = contact_impulse(&body, &contact, DT);
        // Resolution: f32 direction normalization and impulse arithmetic.
        assert!(
            (whole - expected).length() < 1e-5 && (clamped + expected * DT).length() < 1e-5,
            "recipient force {whole:?}, clamped impulse {clamped:?}; expected {expected:?} N and its opposite times dt"
        );
    }

    /// THE IMPULSE ON A PIECE NEVER DRIVES IT PAST THE BELT, AND ITS TANGENTIAL PART NEVER
    /// EXCEEDS COULOMB'S μ × load × dt: the clamp is what keeps a light piece from being flung, and
    /// the belt's normal force over the tick is what the friction reads, so over 4000 fixture
    /// contacts (pieces of 5 kg to 3 t with skewed inertia, off-centre contacts, any normal,
    /// grip, rolling and belt velocity) the normal impulse lies in `[0, load·dt]`, the tangential
    /// one within `|grip + rolling|·dt` and `μ·load·dt`; the push alone leaves the piece still
    /// closing on the belt or level with it along the normal, and the whole impulse leaves it behind the
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
                stiffness: 0.0,
                damping: 0.0,
                slip: 0.0,
                slip_lat: 0.0,
                f_long: 0.0,
                f_lat: 0.0,
                normal,
                traction: tangent,
                rolling_force: tangent * 0.1, // Fixture: a separate ground-drag contribution.
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
                tangent_j <= contact.tangential_force().length() * DT + tol,
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
            if let Ok(dir) = Dir3::new(-contact.tangential_force())
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
            } else if tangent_j > tol
                && tangent_j < contact.tangential_force().length() * DT * 0.999
            {
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
            nominal_pressure_kpa: super::super::forces::rolling_reference_kpa(),
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

    /// The Driver must read the supporting piece's plane when ride pitches the hull.
    /// Fixture: a level broad slab, a pad pitched by one degree, and nominal weight.
    #[test]
    fn perceived_piece_grade_is_independent_of_hull_pitch() {
        let mut app = physics();
        piece_box(&mut app, Vec3::Y * 0.1, Vec3::new(4.0, 0.2, 4.0));
        app.update();
        app.world_mut()
            .run_system_once(|rubble: RubbleQuery, bodies: Query<PieceData>| {
                let near = rubble.near(Vec3::ZERO, 4.0);
                let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                for pitch in [-1.0_f32, 1.0] {
                    let rotation = Quat::from_rotation_x(pitch.to_radians());
                    let report = pad_pass(
                        &ground(),
                        &pieces,
                        Affine3A::from_rotation_translation(rotation, Vec3::Y * 0.1),
                        SideState::default(),
                        Vec3::ZERO,
                        &mut GripElements::for_links(30),
                    );
                    assert!(!report.perceived_support.is_empty());
                    let load = super::super::load::observe(
                        report.perceived_support.iter(),
                        rotation * Vec3::NEG_Z,
                        560_000.0,
                        None,
                    );
                    assert!(
                        load.grade_n.abs() < 1.0,
                        "hull pitch entered piece grade: {} N",
                        load.grade_n
                    );
                }
            })
            .unwrap();
    }

    /// The normal damper must spend no work on motion tangent to the supporting solid. The
    /// hull and drivetrain rely on this even when the belt element is inclined. Fixture: broad
    /// level solid, inclined pad, and opposite tangential velocities, with no vertical motion.
    #[test]
    fn inclined_belt_over_level_solid_has_no_fictitious_normal_damping() {
        let mut app = physics();
        piece_box(&mut app, Vec3::Y * 0.1, Vec3::new(4.0, 0.2, 4.0));
        app.update();
        app.world_mut().run_system_once(|rubble: RubbleQuery, bodies: Query<PieceData>| {
            let near = rubble.near(Vec3::ZERO, 4.0);
            let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
            for pitch in [-15.0_f32, 15.0] {
                let mut loads = Vec::new();
                for speed in [-3.0, 0.0, 3.0] {
                    let report = pad_pass(
                        &ground(), &pieces,
                        Affine3A::from_rotation_translation(Quat::from_rotation_x(pitch.to_radians()), Vec3::Y * 0.1),
                        SideState::default(), Vec3::Z * speed, &mut GripElements::for_links(30),
                    );
                    let contacts: Vec<_> = report.contacts.iter().filter(|c| c.collider.is_some()).collect();
                    assert!(!contacts.is_empty());
                    for c in &contacts {
                        assert!((c.normal - Vec3::Y).length() < 1e-6);
                        assert!((c.load - c.load_elastic).abs() < c.load_elastic * 1e-6,
                            "tangential motion entered normal damping: {} vs {}", c.load, c.load_elastic);
                        assert!((c.point.y - 0.2).abs() < 1e-5, "centroid left supporting plane");
                    }
                    loads.push(contacts.iter().map(|c| c.load).sum::<f32>());
                }
                assert_eq!(loads[0], loads[1]);
                assert_eq!(loads[1], loads[2]);
                println!("MEASURED pitch {pitch} deg: normal load {} N, tangential-speed normal damping 0 N", loads[1]);
            }
        }).unwrap();
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
                    let pushes = piece_impulses(&mut pieces, sourced(&report.contacts), DT);
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
        let pass = |partition: bool, buried: bool, duplicate: bool| {
            let mut app = physics();
            if partition {
                for x in [-0.05, 0.05] {
                    piece_box(&mut app, Vec3::new(x, 0.1, 0.0), Vec3::new(0.1, 0.2, 4.0));
                }
            } else {
                piece_box(&mut app, Vec3::new(0.0, 0.1, 0.0), Vec3::new(0.2, 0.2, 4.0));
            }
            if duplicate {
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
        let (load, grip) = pass(false, false, false);
        for (partition, buried, duplicate) in [
            (true, false, false),
            (false, true, false),
            (true, true, false),
            (false, false, true),
        ] {
            let (other, elements) = pass(partition, buried, duplicate);
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

    /// Moving support sets both slip and rolling. Driving relies on common hull/support
    /// translation producing no drag, and a moving support dragging a stationary hull.
    /// FIXTURE: support translates at twice the fade band; hull carried with it or stationary.
    #[test]
    fn the_slip_and_rolling_on_a_piece_are_measured_against_the_piece() {
        let mut app = physics();
        let carried = piece_box(&mut app, Vec3::new(0.0, 0.2, 0.0), Vec3::new(0.4, 0.4, 4.0));
        app.update();
        let piece_velocity = Vec3::NEG_Z * (2.0 * super::super::forces::ROLLING_FADE_SPEED);
        app.world_mut()
            .get_mut::<LinearVelocity>(carried)
            .unwrap()
            .0 = piece_velocity;
        for hull_velocity in [piece_velocity, Vec3::ZERO] {
            let contacts = app
                .world_mut()
                .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                    let near = rubble.near(Vec3::ZERO, 4.0);
                    let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                    pad_pass(
                        &ground(),
                        &pieces,
                        Affine3A::from_translation(Vec3::Y * 0.4),
                        SideState {
                            speed: 0.4,
                            ..Default::default()
                        },
                        hull_velocity,
                        &mut GripElements::for_links(30),
                    )
                    .contacts
                    .into_iter()
                    .filter(|c| c.collider == Some(carried) && c.normal.y > 0.99)
                    .collect::<Vec<_>>()
                })
                .unwrap();
            assert!(!contacts.is_empty());
            let relative = hull_velocity - piece_velocity;
            // Resolution: f32 contact-frame projection and force assembly.
            for contact in contacts {
                assert!((contact.slip - (0.4 + relative.z)).abs() < 1e-5);
                let expected = if relative == Vec3::ZERO {
                    Vec3::ZERO
                } else {
                    // Relative motion exceeds the owned fade band: full rigid-support drag.
                    assert!(relative.z > super::super::forces::ROLLING_FADE_SPEED);
                    Vec3::NEG_Z * super::super::forces::rigid_rolling_resistance() * contact.load
                };
                assert!(
                    (contact.rolling_force - expected).length() < contact.load * 1e-5,
                    "support-relative rolling {:?}, expected {expected:?}",
                    contact.rolling_force
                );
            }
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

    /// Ground and fixed construction have no dynamic impulse to size, so they must never enter
    /// the geometry-key sort. The dedicated server relies on field-only driving doing no reaction
    /// key construction. Fixture contacts alternate the ground and one standing chunk.
    #[test]
    fn field_and_fixed_support_never_enter_the_dynamic_reaction_sort() {
        let mut app = physics();
        let (_, chunk) = standing_chunk(&mut app, 0.0, 0.0);
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let mut pieces = RubbleBodies::gather(&rubble, &[chunk], &bodies);
                let contacts: Vec<_> = (0..32)
                    .map(|i| {
                        let mut contact = press(Vec3::Z * i as f32, chunk, 100.0);
                        if i % 2 == 0 {
                            contact.collider = None;
                        }
                        contact
                    })
                    .collect();
                let pushes = piece_impulses(&mut pieces, sourced(&contacts), DT);
                assert!(pushes.is_empty());
                assert_eq!(
                    pieces.reaction_sort_keys.load(Ordering::Relaxed),
                    0,
                    "contacts without dynamic recipients must not construct sort keys"
                );
                println!(
                    "MEASURED fixture reaction keys: {}",
                    pieces.reaction_sort_keys.load(Ordering::Relaxed)
                );
            })
            .unwrap();
    }

    /// Each tank's links visit only its own tick-start candidates, in the original gather order.
    /// The server relies on other tanks' nearby construction adding no work to this footprint;
    /// support intersections and the whole snapshot used for reactions must stay identical.
    /// Fixture boxes are separated horizontally, with a small slab inside the first footprint.
    #[test]
    fn each_source_reuses_only_its_own_nearby_surfaces_without_changing_support() {
        let mut app = physics();
        let local = piece(&mut app, 0.0, 0.0);
        let remote = piece(&mut app, 50.0, 0.0);
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let mut pieces = RubbleBodies::gather(&rubble, &[local, remote], &bodies);
                let slab = Slab {
                    origin: Vec3::new(0.0, 0.3, -0.1),
                    axis: Vec3::Z,
                    lat: Vec3::X,
                    out: Vec3::NEG_Y,
                    len: 0.2,
                    lateral: (-0.1, 0.1),
                    face: 0.025,
                    reach: 0.5,
                    travel: 0.0,
                };
                let full = pieces.in_slab(&slab);
                assert_eq!(full.len(), 1);
                assert_eq!(pieces.surface_checks.swap(0, Ordering::Relaxed), 2);
                assert!(pieces.surface(local).triangles.get().is_some());
                assert!(
                    pieces.surface(remote).triangles.get().is_none(),
                    "coarse-query neighbours outside the side must not be triangulated"
                );
                {
                    let local_view = pieces.for_source(Entity::PLACEHOLDER, &[local]);
                    let remote_view = pieces.for_source(Entity::PLACEHOLDER, &[remote]);
                    assert_eq!(local_view.in_slab(&slab), full);
                    assert!(remote_view.in_slab(&slab).is_empty());
                    assert_eq!(
                        local_view.in_slab(&slab),
                        full,
                        "another source's selection cannot change this source's membership"
                    );
                }
                assert_eq!(pieces.in_slab(&slab), full);
                assert_eq!(
                    pieces.surface_checks.swap(0, Ordering::Relaxed),
                    2,
                    "borrowed selections leave the common snapshot's membership intact"
                );
                pieces.select_source(Entity::PLACEHOLDER, &[local]);
                assert_eq!(pieces.in_slab(&slab), full);
                assert_eq!(pieces.surface_checks.swap(0, Ordering::Relaxed), 1);
                assert!(
                    pieces.is_dynamic(remote),
                    "unselected recipients stay in the snapshot"
                );
                pieces.select_source(Entity::PLACEHOLDER, &[remote, local, local]);
                assert_eq!(
                    pieces.in_slab(&slab),
                    full,
                    "gather order and membership are unchanged"
                );
                assert_eq!(pieces.surface_checks.swap(0, Ordering::Relaxed), 2);
                pieces.select_source(local, &[local]);
                assert!(
                    pieces.in_slab(&slab).is_empty(),
                    "a hull cannot support its own belt"
                );
                assert_eq!(pieces.surface_checks.load(Ordering::Relaxed), 0);
                println!("MEASURED candidate checks per fixture slab: global=2 local=1 empty=0");
            })
            .unwrap();
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
