//! Constructions and rubble under the running gear, in the game's tank driving physics (game state
//! only). The terrain field ([`super::oracle::BlockField`]) is the heightmap and the authored
//! blocks; a construction's elements and standing chunks and every loose piece are Avian colliders
//! the field never holds. [`RubbleBodies`] snapshots the nearby colliders shared by every driving
//! source, and the belt's sampled support reads each through a convex shape check over every
//! link's footprint ([`RubbleBodies::in_slabs`]) at the ground's own per-metre spring.
//!
//! **The boundary, one rule from construction facts: the layer a collider was built on.**
//! - **On `Layer::Debris`** (a construction's fallen pieces and the debris stress lever's
//!   fragments, at any mass, [`RubbleBodies::loose`]): a solver body. It meets the belt's sprung
//!   segments (`super::belt_segment`), so Avian resolves the equal and opposite reaction and
//!   whatever holds the piece. The sampled support keeps it in the footprint partition, across
//!   the whole shoe over the length it covers (a link is stiff across its width), so that length
//!   takes no ground support as well; the Driver perceives it, but no spring force acts on it.
//! - **Every other solid** (standing construction, map blocks, another tank's hull; terrain is the
//!   field): the sampled support carries the belt and the solid takes the whole opposite reaction,
//!   fixed construction into its structural load reading
//!   ([`RubbleBodies::record_static_contacts`]), a dynamic body as an impulse ([`reactions`]). The
//!   dynamic bodies here are the hulls and a launched turret, on `Layer::Vehicle`. The explicit
//!   spring is stable for a body while `dt² k / m + 2 dt c / m` stays under 4: DERIVED, about 0.2
//!   for the shipped belt law on a 50 t body at 64 Hz, so the bound holds down to about 2.5 t.
//!
//! - **What the belt bears on** ([`RubbleQuery::supports`]): solid, layer-compatible colliders,
//!   except [`super::terrain::FieldSurface`] geometry already represented by the field. Callers
//!   exclude self-support by identity ([`RubbleBodies::select_source`]).
//! - **The footprint check**: a link's slab ([`Slab`]) is the volume its field probes sweep, made
//!   solid across the true shoe width. Six support points reject separated shapes; the convex
//!   adapter then clips entry faces to that envelope. Narrow pieces and empty corners are
//!   resolved by their actual projected coverage, independently of the probe columns.
//! - **One support law**: `super::world_contact::convex` partitions overlapping footprints into
//!   their visible upper envelope; each visible face region enters the field's own support law at
//!   its covered share of the shoe. Stacked pieces cannot multiply the hull's support.
//! - **Work while held against rubble** (DERIVED from the loops): one AABB query per tank
//!   ([`RubbleQuery::near`], [`footprint_radius`]) selects its N candidates. Each side rejects N
//!   candidates against its slabs' union bounds. For L stations and P survivors this means
//!   N + L × P bounds checks, with one station dispatch when P > 0. Every slab AABB pass makes
//!   six support-map queries; only a positive-volume pass prepares the boundary triangles, once
//!   per surface in the shared tick-start snapshot. Support-map and clipping work also depend on
//!   the shape's complexity. `forces::collect_column_contacts` partitions the covered faces
//!   only where a face rises above the field; each visible face region keeps its own depth plane.
//!   With no nearby collider no shape is read, and field-only driving retains its force arithmetic.
//! - **Tick truth**: avian refits dynamic bodies' AABBs at the end of its physics step
//!   (`update_solver_body_aabbs`) and writes each collider's `Position`/`Rotation`, so a snapshot
//!   in `FixedUpdate` reads the poses the last step wrote.
//! - **Rejected alternatives**: probe rays folded onto the colliders (each probe also a ray
//!   against the rubble, the deepest hit wins) see only the three column lines, so a brick between
//!   them sits under the belt unseen. A custom reaction law on pieces outside the solver, a
//!   clamped impulse sized against each piece's effective mass, gave the tank ground support while
//!   a resting piece took no normal push: nothing pressed the piece into what held it, so the belt
//!   swept a heap apart, and the law was the larger code path. The belt's solver segments give the
//!   piece the equal and opposite reaction instead.

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{borrow::Cow, sync::OnceLock};

use avian3d::prelude::{
    Collider, ColliderAabb, ColliderOf, CollisionLayers, ComputedAngularInertia,
    ComputedCenterOfMass, ComputedMass, DefaultFriction, Forces, Friction, Position,
    ReadRigidBodyForces, RigidBody, Rotation, Sensor, SpatialQuery, WriteRigidBodyForces,
};
use bevy::ecs::query::{QueryData, QueryFilter};
use bevy::ecs::system::SystemParam;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, ParallelSlice, TaskPool};

use super::terrain::FieldSurface;
use crate::contact_transfer::{AppliedContact, ContactTransfers, StructureBody};
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

    /// Whether `collider` is a loose piece the belt segments carry (`super::belt_segment`).
    fn loose(&self, collider: Entity) -> bool {
        self.layers
            .get(collider)
            .expect("avian requires CollisionLayers on every Collider")
            .interacts_with(super::belt_segment::segment_layers())
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

/// What a moving support's motion and response read and a hull's reaction writes.
pub(crate) type PieceData = (
    Forces,
    &'static ComputedCenterOfMass,
    &'static ComputedMass,
    &'static ComputedAngularInertia,
);

pub(crate) type PieceReadData = <PieceData as QueryData>::ReadOnly;

/// One rubble collider near the tank, as the footprint check reads it: its convex shape at its
/// tick-start world pose and bounds, its owning rigid body, whether that body is dynamic (only a
/// dynamic one takes an impulse), the structure whose load reading takes a fixed one's reaction,
/// and the friction its surface brings to a pair.
#[derive(Clone, Debug)]
struct Surface {
    collider: Entity,
    owner: Entity,
    shape: Collider,
    position: Vec3,
    rotation: Quat,
    aabb: ColliderAabb,
    /// Boundary triangles at the tick-start world pose.
    triangles: OnceLock<Vec<[Vec3; 3]>>,
    dynamic: bool,
    friction: Friction,
    structure: Option<Entity>,
    /// The body's motion at the tick's start, dynamic or kinematic; static surfaces have none.
    motion: Option<SurfaceMotion>,
    /// A loose piece: the belt segments carry it, the spring channel only partitions over it.
    loose: bool,
}

#[derive(Clone, Debug)]
struct SurfaceMotion {
    com: Vec3,
    linear: Vec3,
    angular: Vec3,
    /// The body's response to an impulse: 1/kg, and its world inverse inertia tensor. Zero for a
    /// kinematic body, which Avian's solver also treats as of infinite mass.
    inverse_mass: f32,
    inverse_inertia: Mat3,
}

impl SurfaceMotion {
    /// The largest speed (m/s per N·s) a unit tangent impulse at `point`, in the plane normal to
    /// `normal`, gives the surface there: the top eigenvalue of 1/m + (r×t)·I⁻¹(r×t) over the
    /// plane's tangents t, r from the centre of mass.
    fn tangent_mobility(&self, point: Vec3, normal: Vec3) -> f32 {
        let r = point - self.com;
        let (t1, t2) = normal.any_orthonormal_pair();
        let (a1, a2) = (r.cross(t1), r.cross(t2));
        let (i1, i2) = (self.inverse_inertia * a1, self.inverse_inertia * a2);
        let w11 = self.inverse_mass + a1.dot(i1);
        let w22 = self.inverse_mass + a2.dot(i2);
        let w12 = a1.dot(i2);
        (w11 + w22) / 2.0 + (((w11 - w22) / 2.0).powi(2) + w12 * w12).sqrt()
    }
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
                    let motion = if kind.is_kinematic() || kind.is_dynamic() {
                        let (forces, com, mass, inertia) = bodies.get(body).expect(
                            "an admitted moving support body carries Avian force state its caller can read",
                        );
                        let (inverse_mass, inverse_inertia) = if kind.is_dynamic() {
                            (
                                mass.inverse(),
                                inertia.rotated(forces.rotation().0).inverse().to_mat3(),
                            )
                        } else {
                            (0.0, Mat3::ZERO)
                        };
                        Some(SurfaceMotion {
                            com: forces.position().0 + forces.rotation().0 * com.0,
                            linear: forces.linear_velocity(),
                            angular: forces.angular_velocity(),
                            inverse_mass,
                            inverse_inertia,
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
                    Surface {
                            collider,
                            owner: body,
                            shape: shape.clone(),
                            position: position.0,
                            rotation: rotation.0,
                            aabb: *aabb,
                            triangles: OnceLock::new(),
                            dynamic: kind.is_dynamic(),
                            friction: rubble.friction(collider),
                            structure: rubble.structures.contains(body).then_some(body),
                            motion,
                            loose: rubble.loose(collider),
                        }
                })
                .collect::<Vec<_>>()
        });
        let mut out = Self::default();
        // Collider indices follow gather order, never task assignment.
        for surface in prepared.into_iter().flatten() {
            let index = out.surfaces.len();
            out.by_collider.to_mut().insert(surface.collider, index);
            out.surfaces.to_mut().push(surface);
            out.active.push(index);
        }
        out
    }

    /// Borrow the common tick-start geometry and motion; only candidate membership is local.
    pub(crate) fn for_source(&self, source: Entity, near: &[Entity]) -> RubbleBodies<'_> {
        let mut selected = RubbleBodies {
            surfaces: Cow::Borrowed(&self.surfaces),
            by_collider: Cow::Borrowed(&self.by_collider),
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
        let _stage = Stages::start(self.stages.as_ref(), Stage::BeltContacts);
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

    pub(crate) fn count(&self, work: Work, count: usize) {
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

    /// Whether `collider` belongs to a gathered dynamic body.
    #[cfg(test)]
    pub(crate) fn is_dynamic(&self, collider: Entity) -> bool {
        self.surface(collider).dynamic
    }

    /// The friction `collider`'s surface brings to a pair, as avian's narrow phase resolves it.
    /// Only a collider from this snapshot may be read.
    pub(crate) fn friction(&self, collider: Entity) -> Friction {
        self.surface(collider).friction
    }

    /// Whether `collider` is a loose piece, carried by the belt segments' solver contacts rather
    /// than the spring channel. Only a collider from this snapshot may be read.
    pub(crate) fn loose(&self, collider: Entity) -> bool {
        self.surface(collider).loose
    }

    /// The collider of a [`SlabOverlap::piece`].
    pub(crate) fn collider(&self, piece: usize) -> Entity {
        self.surfaces[piece].collider
    }

    /// The surface's world velocity at `point`: dynamic or kinematic motion, zero on fixed
    /// construction. Only a collider from this snapshot may be read.
    pub(crate) fn velocity_at(&self, collider: Entity, point: Vec3) -> Vec3 {
        self.surface(collider)
            .motion
            .as_ref()
            .map_or(Vec3::ZERO, |motion| {
                motion.linear + motion.angular.cross(point - motion.com)
            })
    }

    /// How far the surface at `point` yields along its contact plane: its tangent mobility
    /// (m/s per N·s), zero on fixed construction. Only a collider from this snapshot may be read.
    pub(crate) fn mobility_at(&self, collider: Entity, point: Vec3, normal: Vec3) -> f32 {
        self.surface(collider)
            .motion
            .as_ref()
            .map_or(0.0, |motion| motion.tangent_mobility(point, normal))
    }

    /// Stage the equal-and-opposite reaction on fixed structural construction. The field has no
    /// collider contact, dynamic bodies take [`reactions`], and permanent surfaces have no
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
            if surface.dynamic {
                continue;
            } // Dynamic recipients take `reactions`.
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

    /// Each station's overlaps in gather order. One side owns the station batch; its union
    /// bounds reject candidates before station dispatch, without changing an individual slab.
    pub(crate) fn in_slabs(&self, slabs: &[Slab]) -> Vec<Vec<SlabOverlap>> {
        if self.active.is_empty() || slabs.is_empty() {
            return vec![Vec::new(); slabs.len()];
        }
        self.in_slabs_on_pool(slabs, ComputeTaskPool::get())
    }

    fn in_slabs_on_pool(&self, slabs: &[Slab], pool: &TaskPool) -> Vec<Vec<SlabOverlap>> {
        let _stage = Stages::start(self.stages.as_ref(), Stage::BeltContacts);
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
            self.count(Work::BeltBoundaryBuilds, 1);
            Surface::triangles(&s.shape, s.position, s.rotation)
        });
        let mut faces = super::world_contact::convex::clip_faces(triangles, slab);
        if s.loose {
            faces = faces
                .into_iter()
                .flat_map(|face| face.across_shoe(slab))
                .collect();
        }
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

/// One impulse a dynamic body took from a belt: `impulse` (N·s, world) at `point`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Push {
    pub(crate) body: Entity,
    pub(crate) impulse: Vec3,
    pub(crate) point: Vec3,
}

/// The force the belt law applied to its source through `contact` (N): the support along the
/// normal and both tangent contributions. A recipient held by its own laws takes its opposite.
fn source_force(contact: &BeltContact) -> Vec3 {
    contact.normal * contact.load + contact.tangential_force()
}

/// Every dynamic recipient's side of `contacts` over a tick of `dt` (module doc): the whole
/// opposite of the belt's force on it. Impulses add, so their order decides nothing.
pub(crate) fn reactions<'c>(
    bodies: &RubbleBodies,
    contacts: impl IntoIterator<Item = &'c BeltContact>,
    dt: f32,
) -> Vec<Push> {
    let _stage = Stages::start(bodies.stages.as_ref(), Stage::BeltContacts);
    let pushes: Vec<Push> = contacts
        .into_iter()
        .filter_map(|contact| {
            // Field support has no recipient, and static or kinematic recipients take no impulse.
            let surface = bodies.surface(contact.collider?);
            surface.dynamic.then(|| Push {
                body: surface.owner,
                impulse: -source_force(contact) * dt,
                point: contact.point,
            })
        })
        .collect();
    bodies.count(Work::BeltDynamicContacts, pushes.len());
    pushes
}

/// Put every push on its body through avian, which wakes a sleeping body for a non-zero impulse.
pub(crate) fn apply_pushes<F: QueryFilter>(pushes: &[Push], bodies: &mut Query<PieceData, F>) {
    for push in pushes {
        let (mut forces, ..) = bodies
            .get_mut(push.body)
            .expect("a gathered dynamic body remains alive during the synchronous force pass");
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
                assert_eq!(
                    reference.surface(shared).owner,
                    reference.surface(near[7]).owner
                );
                assert!(reference.surface(moving).motion.is_some());
                assert!(!reference.surface(fixed).dynamic);
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
                    assert_eq!(actual.active, reference.active);
                    assert_eq!(actual.by_collider, reference.by_collider);
                    for (a, b) in actual.surfaces.iter().zip(reference.surfaces.iter()) {
                        assert_eq!(
                            (a.collider, a.owner, a.dynamic, a.structure),
                            (b.collider, b.owner, b.dynamic, b.structure),
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

    /// Both adapters must read the same planar surface and material. Their pressure quadratures
    /// differ: grid columns sample a plane; solids integrate exact footprint coverage.
    /// Fixture: tangent translation and normal closing on the same finite ramp.
    #[test]
    fn grid_and_convex_adapters_read_the_same_surface_and_material() {
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
                shear_length_m: 0.0, // The solid's: it stores no shear of its own.
            };
            let pose = Affine3A::from_translation(Vec3::Y * 0.1);
            let tangent = Vec3::new(0.4, 0.03 * 0.4 + 0.02 * 0.3, 0.3);
            let run = |field: &BlockField, pieces: &RubbleBodies, velocity: Vec3| pad_pass_gripping(
                grip, field, pieces, pose, SideState { speed: 0.6, ..default() },
                velocity, &mut GripElements::for_stations(30, 3),
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
                for contact in &reference.contacts {
                    assert!((contact.normal - expected_normal).length() < 1e-5);
                    assert!((contact.point.y - height(contact.point.x, contact.point.z)).abs() < 1e-5);
                    assert_eq!(contact.mu, ground.grip);
                }
                let both = totals(&run(&field, &pieces, velocity));
                println!("MEASURED grid/convex quadrature {size}, velocity {velocity}: force delta {} N, torque delta {} N m, load delta {} N; combined force delta {} N, torque delta {} N m",
                    (f-force).length(), (t-torque).length(), l-load, (both.0-force).length(), (both.1-torque).length());
            }
        }).unwrap();
    }

    /// THE LAYER DECIDES THE CHANNEL, WHATEVER THE BODY WEIGHS (module doc). A debris-layer body
    /// is loose at 2.9 t as at 16 kg, so a heavy chunk cannot fall between the channels; a dynamic
    /// body off that layer takes the whole opposite of the belt's force, grip and rolling
    /// included. A mutant that gates `loose` on mass, or clamps or drops a part of the reaction,
    /// fails here. FIXTURE DATA: boxes at brick density, one contact pressing 100 N with 8 N grip
    /// and 6 N rolling.
    #[test]
    fn the_layer_decides_the_channel_and_a_hull_takes_the_whole_reaction() {
        let mut app = physics();
        let light = piece_box(&mut app, Vec3::new(0.0, 0.1, 0.0), Vec3::splat(0.2));
        let heavy = piece_box(&mut app, Vec3::new(4.0, 0.5, 0.0), Vec3::new(2.0, 0.8, 1.0));
        for piece in [light, heavy] {
            app.world_mut()
                .entity_mut(piece)
                .insert(crate::construction::breakage::piece_layers());
        }
        let hull = piece_box(&mut app, Vec3::new(8.0, 0.5, 0.0), Vec3::ONE);
        app.update();
        app.update();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let near = [light, heavy, hull];
                let gathered = RubbleBodies::gather(&rubble, &near, &bodies);
                assert!(gathered.loose(light) && gathered.loose(heavy));
                assert!(!gathered.loose(hull));
                let contact = BeltContact {
                    traction: Vec3::X * 8.0,
                    rolling_force: Vec3::Z * 6.0,
                    ..press(Vec3::new(8.0, 1.0, 0.0), hull, 100.0)
                };
                let pushes = reactions(&gathered, [&contact], DT);
                assert_eq!(pushes.len(), 1);
                assert_eq!(pushes[0].body, hull);
                let expected = -(Vec3::Y * 100.0 + contact.tangential_force()) * DT;
                assert!(pushes[0].impulse.distance(expected) < 1e-6);
            })
            .expect("the gathered bodies are read");
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
    #[should_panic(expected = "an admitted moving support body carries Avian force state")]
    fn an_admitted_dynamic_body_with_missing_force_state_is_a_bug() {
        let mut app = physics();
        let body = piece(&mut app, 0.0, 0.0);
        app.update();
        app.world_mut()
            .entity_mut(body)
            .remove::<ComputedCenterOfMass>();
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
                      masses: Query<&avian3d::prelude::ComputedMass>,
                      mut transfers: ResMut<ContactTransfers>| {
                    let near = rubble.near(Vec3::ZERO, 4.0);
                    assert!(
                        near.contains(&collider),
                        "unmarked fixed geometry must carry the belt"
                    );
                    let mut pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                    pieces.select_source(source, &near);
                    let (mut forces, ..) = bodies.get_mut(source).unwrap();
                    let mass = masses.get(source).unwrap();
                    let before = forces.linear_velocity();
                    let report = pad_pass(
                        &ground(),
                        &pieces,
                        Affine3A::from_translation(Vec3::Y * 0.6),
                        SideState::default(),
                        before,
                        &mut GripElements::for_stations(30, 3),
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
            .filter(|(point, ..)| point.other == source)
        {
            assert_eq!(point.reference, root);
            assert_eq!(point.reference_collider, collider);
            assert_eq!(point.other_collider, None);
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
                .any(|(point, ..)| point.other == source),
            "an implicit contact must be refreshed each tick, never retained as a sleeping pair"
        );
    }

    #[test]
    fn a_belt_pressing_a_wall_delivers_its_reaction_to_the_walls_reading() {
        for closing in [0.0, 1.0] {
            fixed_surface_reaction(Collider::cuboid(2.0, 0.6, 2.0), closing);
        }
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
                let pieces = RubbleBodies::gather(&rubble, &[body], &bodies);
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
                    &mut GripElements::for_stations(30, 3),
                );
                assert!(report.contacts.iter().any(|c| c.collider == Some(body)));
                for contact in report
                    .contacts
                    .iter()
                    .filter(|c| c.collider == Some(body) && c.normal.y > 0.99)
                {
                    assert!((contact.slip - 0.4).abs() < 1e-5);
                }
                assert!(reactions(&pieces, &report.contacts, DT).is_empty());
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
            rolling_force: Vec3::ZERO,
            belt_velocity: Vec3::ZERO,
            collider: Some(collider),
            mu: 0.9,
        }
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
    /// Fixture: a 0.7 track sliding sideways over a 0.3 piece of the pad's 30 t, a hull's weight
    /// like the channel's dynamic recipients, so the step floor's recipient term leaves one tick
    /// of slide past breakaway; ground grip has its own witness.
    #[test]
    fn the_belt_grips_solid_recipients_by_the_material_pair() {
        let track = contact_friction(0.7);
        let slide = Vec3::new(5.0, 0.0, 0.0);
        let row = 0.3;
        let mut app = physics();
        let size = Vec3::new(0.4, 0.1, 2.4);
        let hull_mass = 2.0 * pad_grip(track).side_weight / super::super::derive::G;
        app.world_mut().spawn((
            Position(Vec3::new(0.0, 0.05, 0.0)),
            Rotation::default(),
            RigidBody::Dynamic,
            Collider::cuboid(size.x, size.y, size.z),
            ColliderDensity(hull_mass / (size.x * size.y * size.z)),
            contact_friction(row),
        ));
        app.update();
        let grip = pad_grip(track);
        let (report, mus, pushes) = app
            .world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let near = rubble.near(Vec3::new(0.0, 0.1, 0.0), 3.0);
                let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                let report = pad_pass_gripping(
                    grip,
                    &ground(),
                    &pieces,
                    Affine3A::from_translation(Vec3::new(0.0, 0.1, 0.0)),
                    SideState::default(),
                    slide,
                    &mut GripElements::for_stations(30, 3),
                );
                let pushes = reactions(&pieces, &report.contacts, DT);
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

    /// The fixture pad's grip: `track` on a 0.9 field, at a 30 t vehicle's side weight (fixture
    /// data).
    fn pad_grip(track: Friction) -> GripParams {
        GripParams {
            track,
            nominal_pressure_kpa: super::super::forces::rolling_reference_kpa(),
            side_weight: 30_000.0 * 9.81 / 2.0,
            hull_coupling: 1.0, // Fixture point-mass hull.
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
        let travel = 0.01; // Fixture spring travel (m).
        let loop_pts = [
            Vec2::new(-1.0, 0.02 + travel),
            Vec2::new(1.0, 0.02 + travel),
            Vec2::new(1.0, 1.02 + travel),
            Vec2::new(-1.0, 1.02 + travel),
            Vec2::new(-1.0, 0.02 + travel),
        ];
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 30,
            plane_x: 0.0,
            columns: [(-0.1, 0.25), (0.0, 0.5), (0.1, 0.25)].into(),
            travel: TravelField {
                knots: &[(0.0, travel)],
            },
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
            &mut GripElements::for_stations(30, 3),
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
                        &mut GripElements::for_stations(30, 3),
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
                        SideState::default(), Vec3::Z * speed, &mut GripElements::for_stations(30, 3),
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
                    let mut elements = GripElements::for_stations(30, 3);
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

    /// A LINK IS STIFF ACROSS ITS WIDTH, SO A LOOSE PIECE UNDER PART OF IT LIFTS ALL OF IT. The
    /// ground beside the piece, over the covered length, is not touching the plate; the belt
    /// segment carries that length whole, so ground left under it would count it twice. Past the
    /// piece's length the ground keeps the belt. FIXTURE DATA: a 2 m pad 0.2 m wide, 5 mm into
    /// the ground; the piece stands 15 mm proud of that, under half the width.
    #[test]
    fn a_loose_piece_under_part_of_the_width_lifts_the_whole_link_off_the_ground() {
        let ground_load = |piece: Option<(Vec3, Vec3)>| {
            let mut app = physics();
            if let Some((at, size)) = piece {
                let piece = piece_box(&mut app, at, size);
                app.world_mut()
                    .entity_mut(piece)
                    .insert(crate::construction::breakage::piece_layers());
            }
            app.update();
            app.world_mut()
                .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                    let near = rubble.near(Vec3::ZERO, 4.0);
                    let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                    pad_pass(
                        &ground(),
                        &pieces,
                        Affine3A::IDENTITY,
                        SideState::default(),
                        Vec3::ZERO,
                        &mut GripElements::for_stations(30, 3),
                    )
                    .contacts
                    .iter()
                    .filter(|c| c.collider.is_none())
                    .map(|c| c.load_elastic * c.normal.y)
                    .sum::<f32>()
                })
                .unwrap()
        };
        let box_at = |x: f32, z: f32| Vec3::new(x, 0.015 - 0.2, z);
        let bare = ground_load(None);
        let whole_length = ground_load(Some((box_at(0.05, 0.0), Vec3::new(0.1, 0.4, 4.0))));
        let half_length = ground_load(Some((box_at(0.05, 0.5), Vec3::new(0.1, 0.4, 1.0))));
        let beside = ground_load(Some((box_at(0.2, 0.0), Vec3::new(0.1, 0.4, 4.0))));
        println!(
            "MEASURED ground load: bare {bare:.1} N, piece the whole length {whole_length:.1} N, \
             half the length {half_length:.1} N, beside the shoe {beside:.1} N"
        );
        assert!(bare > 0.0);
        assert_eq!(whole_length, 0.0, "ground left beside the piece");
        assert!(
            (half_length / bare - 0.5).abs() < 0.1,
            "{half_length}/{bare}"
        );
        assert_eq!(beside, bare);
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
                    let mut elements = GripElements::for_stations(30, 3);
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
                        &mut GripElements::for_stations(30, 3),
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

    /// Grip on a dynamic recipient acts on the slip between the belt and that body's surface, and
    /// a tick of force changes it at the hull's footprint mobility plus the recipient's. A tank
    /// parked on another, or on a launched turret, relies on the step floor counting both: with
    /// the hull's alone, the pair's grip mode alternates at half the tick rate and grows.
    /// FIXTURE: the pad as a point-mass hull of the weight it carries, on a free 4 × 0.4 × 4 m
    /// slab of that mass, kicked sideways below breakaway. The test integrates both bodies
    /// itself; the slab's contact line yields 1/m + h²/I to a lateral push at its top face, h its
    /// half height and I = m(w² + t²)/12 its roll moment, DERIVED from the box.
    #[test]
    fn grip_on_a_free_body_settles_without_half_rate_ringing() {
        use super::super::derive::G;
        let (width, thickness) = (4.0, 0.4);
        let at_rest = pad_pass(
            &ground(),
            &RubbleBodies::default(),
            Affine3A::IDENTITY,
            SideState::default(),
            Vec3::ZERO,
            &mut GripElements::for_stations(30, 3),
        );
        let mass = at_rest.contacts.iter().map(|c| c.load_elastic).sum::<f32>() / G;
        let grip = GripParams {
            side_weight: mass * G / 2.0,
            ..pad_grip(contact_friction(0.9))
        };
        let mut app = physics();
        let slab = app
            .world_mut()
            .spawn((
                Position(Vec3::NEG_Y * (thickness / 2.0)),
                Rotation::default(),
                RigidBody::Dynamic,
                Collider::cuboid(width, thickness, width),
                ColliderDensity(mass / (width * thickness * width)),
                contact_friction(0.9),
            ))
            .id();
        app.update();
        let computed = app.world().get::<ComputedMass>(slab).unwrap().value();
        assert!(
            (computed - mass).abs() < 1e-3 * mass,
            "the slab weighs {computed} kg"
        );
        let roll = mass * (width * width + thickness * thickness) / 12.0;
        let slab_mobility = 1.0 / mass + (thickness / 2.0).powi(2) / roll;
        let mu = grip.pair(contact_friction(0.9));
        let kick = 0.1 * (mu * G * mu * grip.step_shear_per_grip_m(DT)).sqrt();
        let (mut hull, mut under, mut history) = (kick, 0.0_f32, Vec::new());
        let mut elements = GripElements::for_stations(30, 3);
        for _ in 0..64 {
            app.world_mut().get_mut::<LinearVelocity>(slab).unwrap().0 = Vec3::X * under;
            let carried = elements.clone();
            let (force, carried) = app
                .world_mut()
                .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                    let near = rubble.near(Vec3::ZERO, width);
                    let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                    let mut carried = carried.clone();
                    let report = pad_pass_gripping(
                        grip,
                        &BlockField::default(),
                        &pieces,
                        Affine3A::IDENTITY,
                        SideState::default(),
                        Vec3::X * hull,
                        &mut carried,
                    );
                    assert!(report.contacts.iter().all(|c| c.collider == Some(slab)));
                    let force: f32 = report.contacts.iter().map(|c| c.tangential_force().x).sum();
                    (force, carried)
                })
                .unwrap();
            elements = carried;
            hull += grip.hull_coupling * force / mass * DT;
            under -= slab_mobility * force * DT;
            history.push(hull - under);
        }
        // FIXTURE observation floor: f32 residue far below the kick is not motion.
        let moving = |v: f32| v.abs() > kick * 1e-4;
        let alternations = history
            .windows(3)
            .filter(|w| w.iter().all(|v| moving(*v)) && w[0] * w[1] < 0.0 && w[1] * w[2] < 0.0)
            .count();
        println!("MEASURED relative slip after a kick on a free slab: {history:?}");
        assert_eq!(
            alternations, 0,
            "the pair's grip mode alternates tick to tick"
        );
        assert!(
            !moving(*history.last().unwrap()),
            "the pair's grip mode does not settle"
        );
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
                    "coarse-query neighbours outside the side must not prepare boundary triangles"
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
