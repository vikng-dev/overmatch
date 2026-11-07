//! A released part crumbling where it lands, in the game's environment destruction model (game
//! state only). The construction design's "Crumble on landing" owns the narrative.
//!
//! THE LAW ([`crumble`]). A landing is a physics step in which a moving chunk group took a blow
//! past its rest speed (Avian's `SleepThreshold`, [`landings`]). The blows one chunk took from one
//! other collider are one patch, summed before any force law reads them, so the narrow phase's
//! sampling changes nothing. Each patch's blow peaks as two elastic bodies stopping each other do
//! ([`peak_force`], [`crate::contact_law`]), from the two sides' stiffness and curvature, never
//! from the solver's step; it and each point's press are forces on the chunk they touch
//! ([`touches`]). The group's chunk kernel
//! ([`Kernel::hold`]) balances those forces by the group's rigid acceleration and judges every
//! joint in the field that leaves: a bond past its strength cracks, a joint pulled apart parts,
//! and the field is solved again on what is left until it holds. Cracks therefore lie where the
//! force runs. Energy conserves: the cracks absorb their fracture
//! work, each bond's fracture energy in its failing mode over its area ([`crack_work`]), and all of
//! them together never more than the kinetic energy the landing dissipated, `E = ½ Σ |J_i| v_i`
//! over its blows and closing speeds (DERIVED, a plastic impact), shared with a moving group on the
//! other side by compliance ([`touches`]). Past that, the most overloaded
//! crack first, it stops at the first the energy left cannot pay. What stays connected over the
//! joints that did not part is a fragment.
//!
//! THE SIZE SPLIT ([`route`]). A fragment stays a server body through the construction's split
//! path ([`crate::construction::breakage::PendingSplit`]) when its size, the cube root of its
//! occupied volume, reaches the server's smallest body ([`PhysicalLaws::server_rubble_min_size_m`]).
//! Any other leaves the simulation: the record writes its chunks as dust, and one [`Shed`] carries
//! it to the players' decoration ([`super::decor`]), which only draws it.

use std::collections::BTreeMap;

use avian3d::prelude::{
    AngularVelocity, CollisionLayers, ComputedCenterOfMass, LinearVelocity, PhysicsLengthUnit,
    Position, Rotation, SleepThreshold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::construction::Element;
use crate::construction::breakage::{
    Breakage, ChunkCollider, Entry, Event, PendingSplit, Piece, Record, ReleasedMotion,
};
use crate::construction::fracture::bondsolve::Wrench;
use crate::construction::fracture::parts::Kernel;
use crate::construction::fracture::{Cause, Failure, Fate, Side, State, Structure};
use crate::contact_law::{Contactor, peak_force};
use crate::contact_transfer::ContactTransfers;
use crate::cost::stages::{Stage, Stages, Work};
use crate::physical_laws::PhysicalLaws;
use crate::{Layer, ShotClock};

/// One contact on a group, in the construction frame: the chunk it touches, the point (m) and the
/// force the group takes there (N).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Touch {
    pub(crate) chunk: usize,
    pub(crate) at: Vec3,
    pub(crate) force: Vec3,
}

/// What one landing broke in a group.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Crumble {
    /// The bonds it cracked, most overloaded first.
    pub(crate) cracked: Vec<usize>,
    /// The joints it parted, ascending.
    pub(crate) separated: Vec<usize>,
    /// The cracks' fracture work, J: never more than the landing dissipated.
    pub(crate) spent: f32,
    /// What stays connected over the joints that did not part, each ascending, ordered by its
    /// first chunk.
    pub(crate) fragments: Vec<Vec<usize>>,
    /// Scalar operations of its passes, each kernel's factorization and solve, a DERIVED count.
    pub(crate) work: u64,
    /// Field evaluations attempted by this landing, including its final holding pass.
    pub(crate) passes: usize,
}

/// The fracture work to fail joint `j` by `cause`, J: its seam's fracture energy in that mode
/// (mode I for tension, mode II for shear, both data) over the joint's area. A joint that only
/// parts holds by no cohesion and costs nothing.
pub(crate) fn crack_work(structure: &Structure, j: usize, cause: Cause) -> f32 {
    let joint = &structure.joints[j];
    match (joint.seam(), cause) {
        (Some(seam), Cause::Tension) => seam.fracture_energy * joint.area,
        (Some(seam), Cause::Shear) => seam.shear.energy * joint.area,
        _ => 0.0,
    }
}

/// The kernel's loads for `touches` on `group` (ascending): each touch's force about its chunk's
/// centroid, summed per chunk.
fn loads(structure: &Structure, group: &[usize], touches: &[Touch]) -> Vec<Wrench> {
    let mut loads = vec![Wrench::default(); group.len()];
    for touch in touches {
        let node = group
            .binary_search(&touch.chunk)
            .expect("a landing touches the group's own chunks");
        let arm = touch.at - structure.chunks[touch.chunk].centroid;
        loads[node].add(Wrench::at(touch.force.as_dvec3(), arm.as_dvec3()), 1.0);
    }
    loads
}

/// The overload a failure carries: demand over capacity, unbounded for a joint that only parts.
fn overload(failure: &Failure) -> f32 {
    if failure.capacity > 0.0 {
        failure.demand / failure.capacity
    } else {
        f32::INFINITY
    }
}

/// Resolution: solves one landing spends letting its field move off what it cracked. Each pass
/// fails at least one joint or ends the landing, and a part's joints are finite; a field still
/// cracking after them leaves the rest standing for the next landing.
const PASSES: usize = 16;

/// Union-find: the representative of `i`, halving the path to it.
fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// What stays connected of `group` (ascending) over its joints `state` has not parted, each
/// ascending, ordered by its first chunk.
pub(crate) fn fragments(structure: &Structure, state: &State, group: &[usize]) -> Vec<Vec<usize>> {
    let index: BTreeMap<usize, usize> = group.iter().enumerate().map(|(i, &c)| (c, i)).collect();
    let mut parent: Vec<usize> = (0..group.len()).collect();
    for (j, joint) in structure.joints.iter().enumerate() {
        let Side::Chunk(b) = joint.b else {
            continue; // A ground joint joins no two chunks.
        };
        let (Some(&x), Some(&y)) = (index.get(&joint.a), index.get(&b)) else {
            continue; // A boundary to a chunk outside the group.
        };
        if state.separated[j] || !state.live(joint) {
            continue; // Parted: it holds nothing together.
        }
        let (x, y) = (root(&mut parent, x), root(&mut parent, y));
        parent[x.max(y)] = x.min(y);
    }
    let mut fragments: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &c) in group.iter().enumerate() {
        fragments.entry(root(&mut parent, i)).or_default().push(c);
    }
    fragments.into_values().collect()
}

/// The landing `touches` on `group` (ascending, owned by `state`), which dissipated `energy` J
/// (module doc): the field is solved again after each pass's failures, the cracked bonds bearing
/// only push, until it holds, the energy is spent or [`PASSES`] end. Once a joint parts, a touched
/// fragment takes only its share `m_f / M` of the landing's force: the momentum a plastic stop of
/// that fragment alone needs (DERIVED).
pub(crate) fn crumble(
    structure: &Structure,
    state: &State,
    group: &[usize],
    touches: &[Touch],
    energy: f32,
) -> Crumble {
    assert!(
        energy.is_finite() && energy >= 0.0,
        "a landing's dissipated energy is finite and nonnegative"
    );
    let mass = |chunks: &[usize]| -> f32 { chunks.iter().map(|&c| structure.chunks[c].mass).sum() };
    let whole = mass(group);
    let mut after = state.clone();
    let (mut cracked, mut separated) = (Vec::new(), Vec::new());
    let (mut spent, mut work) = (0.0_f32, 0);
    let mut passes = 0;
    'passes: for _ in 0..PASSES {
        passes += 1;
        let pieces = fragments(structure, &after, group);
        let share = |chunk: usize| {
            let piece = pieces
                .iter()
                .find(|piece| piece.binary_search(&chunk).is_ok())
                .expect("the fragments partition the group");
            mass(piece) / whole
        };
        let shared: Vec<Touch> = touches
            .iter()
            .map(|touch| Touch {
                force: touch.force * share(touch.chunk),
                ..*touch
            })
            .collect();
        let kernel = Kernel::of(structure, &after, group);
        let hold = kernel.hold(structure, &loads(structure, group, &shared));
        work += kernel.factor.ops + hold.work;
        let mut failures = hold.failures;
        if failures.is_empty() {
            break; // The field holds.
        }
        failures.sort_by(|a, b| {
            overload(b)
                .total_cmp(&overload(a))
                .then(a.joint.cmp(&b.joint))
        });
        for failure in failures {
            let cost = crack_work(structure, failure.joint, failure.cause);
            if spent + cost > energy {
                break 'passes; // The energy is spent.
            }
            spent += cost;
            let j = failure.joint;
            if matches!(failure.cause, Cause::Tension | Cause::Shear) {
                after.cracked[j] = true;
                cracked.push(j);
            }
            if hold.separated.contains(&j) {
                after.separated[j] = true;
                separated.push(j);
            }
        }
    }
    separated.sort_unstable();
    Crumble {
        cracked,
        separated,
        spent,
        fragments: fragments(structure, &after, group),
        work,
        passes,
    }
}

/// Whether `fragment` stays a server body (module doc): its size reaches the server's smallest
/// body, whatever its mass.
pub(crate) fn server_body(structure: &Structure, fragment: &[usize]) -> bool {
    let volume: f32 = fragment.iter().map(|&c| structure.chunks[c].volume).sum();
    volume.cbrt() >= PhysicalLaws::shipped().server_rubble_min_size_m
}

/// Coarsen to the server's smallest body ([`server_body`]): a fragment too small to stay a
/// body alone joins one leaving with it across a joint parted in this event, so its matter stays
/// present for hulls and rounds. The largest parted joint decides first, joint index breaking ties.
/// Retained edges preserve earlier absorption without restoring material strength. Runs within
/// each owner's worker; chunks and returned groups stay ascending, independent of worker
/// assignment.
pub(crate) fn merge_fragments(
    structure: &Structure,
    mut fragments: Vec<Vec<usize>>,
    parted: &[usize],
    retained: &mut Vec<usize>,
) -> Vec<Vec<usize>> {
    for fragment in &mut fragments {
        fragment.sort_unstable();
        assert!(!fragment.is_empty(), "a departing fragment contains matter");
    }
    fragments.sort_by_key(|fragment| fragment[0]);
    if parted.is_empty() && retained.is_empty() {
        return fragments; // No eligible edge can change this partition.
    }
    let mut owner = vec![None; structure.chunks.len()];
    for (g, fragment) in fragments.iter().enumerate() {
        for &c in fragment {
            assert!(
                owner[c].replace(g).is_none(),
                "departing fragments partition matter"
            );
        }
    }
    let mut parent: Vec<_> = (0..fragments.len()).collect();
    retained.retain(|&j| {
        let joint = &structure.joints[j];
        matches!(joint.b, Side::Chunk(b) if owner[joint.a].is_some() && owner[b].is_some())
    });
    let mut edges: Vec<_> = retained
        .iter()
        .map(|&j| (false, j))
        .chain(parted.iter().map(|&j| (true, j)))
        .collect();
    edges.sort_unstable_by(|&(new_a, a), &(new_b, b)| {
        let area = |j: usize| structure.joints[j].area;
        new_a
            .cmp(&new_b)
            .then(area(b).total_cmp(&area(a)))
            .then(a.cmp(&b))
    });
    for (new, j) in edges {
        let joint = &structure.joints[j];
        let Side::Chunk(b) = joint.b else { continue }; // Ground is no departing neighbour.
        let (Some(a), Some(b)) = (owner[joint.a], owner[b]) else {
            continue; // Only fragments leaving together may join.
        };
        let (a, b) = (root(&mut parent, a), root(&mut parent, b));
        if a == b
            || (new
                && server_body(structure, &fragments[a])
                && server_body(structure, &fragments[b]))
        {
            continue;
        }
        let (a, b) = (a.min(b), a.max(b));
        parent[b] = a;
        let absorbed = std::mem::take(&mut fragments[b]);
        fragments[a].extend(absorbed);
        fragments[a].sort_unstable();
        if new {
            retained.push(j);
        }
    }
    retained.sort_unstable();
    retained.dedup();
    fragments
        .into_iter()
        .filter(|fragment| !fragment.is_empty())
        .collect()
}

/// A fragment leaving the simulation for the players' decoration, sent once at `tick`: its
/// construction and chunks (ascending), the construction frame's world pose then, the fragment's
/// centre-of-mass velocity (m/s) and spin (rad/s) in world terms, and a seed that fixes every
/// choice its decoration makes, so every player draws it alike.
#[derive(Message, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct Shed {
    pub(crate) construction: u16,
    pub(crate) chunks: Vec<u16>,
    pub(crate) at: [f32; 3],
    pub(crate) rotation: [f32; 4],
    pub(crate) velocity: [f32; 3],
    pub(crate) spin: [f32; 3],
    pub(crate) seed: u32,
    pub(crate) tick: u32,
}

impl Shed {
    /// `fragment` of `construction` at `tick`, in the construction frame `frame` (world), moving
    /// with the rigid field `velocity` + `spin` × (x − `centre`) of the group it left (world).
    pub(crate) fn of(
        structure: &Structure,
        (construction, frame): (u16, Transform),
        fragment: &[usize],
        ReleasedMotion {
            velocity,
            spin,
            centre,
        }: ReleasedMotion,
        tick: u32,
    ) -> Self {
        let mass: f32 = fragment.iter().map(|&c| structure.chunks[c].mass).sum();
        let local = fragment
            .iter()
            .map(|&c| structure.chunks[c].centroid * structure.chunks[c].mass)
            .sum::<Vec3>()
            / mass;
        let own = velocity + spin.cross(frame.transform_point(local) - centre);
        let first = u32::from(fragment[0] as u16);
        Self {
            construction,
            chunks: fragment.iter().map(|&c| c as u16).collect(),
            at: frame.translation.to_array(),
            rotation: frame.rotation.to_array(),
            velocity: own.to_array(),
            spin: spin.to_array(),
            // Any fixed mix of the identities works; it only has to be the same on every peer.
            seed: u32::from(construction).rotate_left(16) ^ first.wrapping_mul(0x9E37_79B9) ^ tick,
            tick,
        }
    }
}

/// One patch of a landing: the blow (N·s) one of a group's colliders took from one other collider
/// (`None`, a track belt) over the step, summed over every point the narrow phase sampled it with,
/// at their blow-weighted centre and closing at their blow-weighted speed (m/s), so its force law
/// sees one blow however many points sampled it. `struck` is the momentum its points struck with
/// (N·s), and `shared` says whether the other side is a moving group crumbling from the same
/// collision.
struct Patch {
    collider: Entity,
    other: Option<Entity>,
    at: Vec3,
    blow: Vec3,
    struck: f32,
    closing: f32,
    shared: bool,
}

/// What a point held through the step rather than struck: its press (N) on the group's collider
/// at the point.
struct Press {
    collider: Entity,
    at: Vec3,
    force: Vec3,
}

/// One physics step's landing on a moving group: its patches and its points' presses.
#[derive(Default)]
struct Landing {
    patches: Vec<Patch>,
    presses: Vec<Press>,
}

/// Each moving group's landing this physics step; a group whose contacts brought no blow only
/// rests or slides, and lands nowhere. `rest_speed` is a moving group's rest speed (m/s), `None`
/// for a body that is not one free to crumble this step.
fn landings(
    transfers: &ContactTransfers,
    rest_speed: impl Fn(Entity) -> Option<f32>,
) -> BTreeMap<Entity, Landing> {
    let dt = transfers.dt();
    let mut landings: BTreeMap<Entity, Landing> = BTreeMap::new();
    for points in transfers.groups() {
        let structure = points[0].structure;
        let Some(rest) = rest_speed(structure) else {
            continue; // Not a moving group free to crumble this step.
        };
        let landing = landings.entry(structure).or_default();
        for point in points {
            // A point closing past the group's rest speed (Avian's `SleepThreshold`) strikes with
            // the momentum its closing brings, `m v`, never more than the solver moved along the
            // normal; the rest of its transfer is held through the step, a press.
            let moved = point.impulse.dot(point.normal).max(0.0);
            let struck = if point.closing_speed > rest {
                (point.effective_mass * point.closing_speed).min(moved)
            } else {
                0.0
            };
            landing.presses.push(Press {
                collider: point.structure_collider,
                at: point.point,
                force: -(point.impulse - point.normal * struck) / dt,
            });
            if struck <= 0.0 {
                continue;
            }
            let at = landing.patches.iter().position(|patch| {
                patch.collider == point.structure_collider && patch.other == point.body_collider
            });
            let patch = match at {
                Some(at) => &mut landing.patches[at],
                None => {
                    landing.patches.push(Patch {
                        collider: point.structure_collider,
                        other: point.body_collider,
                        at: Vec3::ZERO,
                        blow: Vec3::ZERO,
                        struck: 0.0,
                        closing: 0.0,
                        shared: rest_speed(point.body).is_some(),
                    });
                    landing.patches.last_mut().expect("the patch just pushed")
                }
            };
            patch.at += point.point * struck;
            patch.blow -= point.normal * struck;
            patch.struck += struck;
            patch.closing += point.closing_speed * struck;
        }
    }
    landings.retain(|_, landing| !landing.patches.is_empty());
    for patch in landings
        .values_mut()
        .flat_map(|landing| &mut landing.patches)
    {
        patch.at /= patch.struck;
        patch.closing /= patch.struck;
    }
    landings
}

/// `landing`'s touches on its group's chunks (`chunk` of each of its colliders), in the
/// construction frame `frame`, and the energy its blows dissipated that the group may spend (J):
/// `½ J v` per patch, the whole of it against a side that spends none (the ground, a hull, a
/// placed construction, whose contact loads carry no energy), and against a moving group
/// crumbling from the same collision its own compliance's share (DERIVED from Hertz: under one
/// force each side's approach, and so the work done on it, is in proportion to its compliance;
/// K. L. Johnson, *Contact Mechanics*, §4.2), so the two spend one collision's energy between
/// them. `other` is what each patch meets ([`Contacts::other`]).
fn touches(
    structure: &Structure,
    frame: Transform,
    landing: &Landing,
    chunk: impl Fn(Entity) -> usize,
    other: impl Fn(Option<Entity>) -> Contactor,
) -> (Vec<Touch>, f32) {
    let local = |chunk: usize, at: Vec3, force: Vec3| Touch {
        chunk,
        at: frame.rotation.inverse() * (at - frame.translation),
        force: frame.rotation.inverse() * force,
    };
    let mut touches: Vec<Touch> = landing
        .presses
        .iter()
        .map(|press| local(chunk(press.collider), press.at, press.force))
        .collect();
    let mut energy = 0.0;
    for patch in &landing.patches {
        let c = chunk(patch.collider);
        let (own, against) = (Contactor::chunk(structure, c), other(patch.other));
        let share = if patch.shared {
            own.compliance / (own.compliance + against.compliance)
        } else {
            1.0
        };
        energy += 0.5 * patch.struck * patch.closing * share as f32;
        let blow = (patch.blow, Vec3::ZERO, patch.closing);
        touches.push(local(
            c,
            patch.at,
            landing_force(structure, c, against, blow),
        ));
    }
    (touches, energy)
}

/// The force (N) a landing's patch puts on chunk `chunk`: its blow's peak against `other`, with the
/// effective mass a plastic stop of it moved, `m = |J| / v` (DERIVED), and its press.
pub(crate) fn landing_force(
    structure: &Structure,
    chunk: usize,
    other: Contactor,
    (blow, press, closing): (Vec3, Vec3, f32),
) -> Vec3 {
    if blow == Vec3::ZERO || closing <= 0.0 {
        return press; // No blow at this point: only what it holds.
    }
    let mass = f64::from(blow.length() / closing);
    let peak = peak_force(
        mass,
        f64::from(closing),
        Contactor::chunk(structure, chunk),
        other,
    );
    blow.normalize() * peak as f32 + press
}

type Moving<'a> = (
    Entity,
    &'a Piece,
    &'a mut Breakage,
    (&'a Position, &'a Rotation),
    (
        &'a LinearVelocity,
        &'a AngularVelocity,
        &'a ComputedCenterOfMass,
    ),
    Has<PendingSplit>,
);

type ColliderOf<'a> = (
    Option<&'a ChunkCollider>,
    Option<&'a Element>,
    Option<&'a CollisionLayers>,
    Option<&'a ChildOf>,
);

/// The physics steps' contacts, read once each, what each collider stands for, and the placed
/// constructions a landing may meet.
#[derive(bevy::ecs::system::SystemParam)]
struct Contacts<'w, 's> {
    transfers: Res<'w, ContactTransfers>,
    read: Local<'s, u64>,
    colliders: Query<'w, 's, ColliderOf<'static>>,
    placed: Query<'w, 's, &'static Breakage, Without<Piece>>,
    rests: Query<'w, 's, &'static SleepThreshold>,
    length_unit: Res<'w, PhysicsLengthUnit>,
}

impl Contacts<'_, '_> {
    /// What a landing's patch meets through `collider` ([`meets`]); a track belt, with no
    /// collider, is a hull's, [`Contactor::RIGID`].
    fn other(&self, collider: Option<Entity>, moving: &Query<Moving>) -> Contactor {
        let Some(collider) = collider else {
            return Contactor::RIGID; // A track belt: a hull's.
        };
        let (chunk, element, layers, parent) = self
            .colliders
            .get(collider)
            .expect("a solver contact's other side is a live collider");
        let owner = parent.map(ChildOf::parent);
        let breakage = owner.and_then(|owner| match self.placed.get(owner) {
            Ok(breakage) => Some(breakage),
            Err(_) => moving.get(owner).ok().map(|item| item.2),
        });
        let terrain = layers.is_some_and(|layers| layers.memberships.has_all(Layer::Terrain));
        meets(chunk, element, terrain, breakage)
    }
}

/// What a landing meets on a collider standing for `chunk` or `element` of `breakage`'s
/// construction, or on the ground (`terrain`): a chunk curved, by its material; a whole element
/// flat, by its substance's stiffness, whether or not it breaks; the ground by its data; any other
/// body (a hull) as [`Contactor::RIGID`].
fn meets(
    chunk: Option<&ChunkCollider>,
    element: Option<&Element>,
    terrain: bool,
    breakage: Option<&Breakage>,
) -> Contactor {
    match (chunk, element, breakage) {
        (Some(&ChunkCollider(c)), _, Some(breakage)) => {
            Contactor::chunk(&breakage.live.structure, usize::from(c))
        }
        (None, Some(&Element(e)), Some(breakage)) => breakage.kind.element_face(usize::from(e)),
        _ if terrain => Contactor::ground(),
        _ => Contactor::RIGID,
    }
}

/// AUTHORITY: each physics step's landings crumble the moving groups they reach (module doc). A
/// group a field load already splits this tick keeps that split; its landing is not read twice.
fn crumble_landings(
    mut contacts: Contacts,
    mut groups: Query<Moving>,
    clock: Option<Res<ShotClock>>,
    mut record: ResMut<Record>,
    mut shed: MessageWriter<Shed>,
    stages: Option<Res<Stages>>,
    mut commands: Commands,
) {
    if contacts.transfers.steps() == *contacts.read {
        return;
    }
    let _crumble = Stages::start(stages.as_deref(), Stage::Crumble);
    *contacts.read = contacts.transfers.steps();
    let landings = landings(&contacts.transfers, |entity| {
        groups.get(entity).is_ok_and(|item| !item.5).then(|| {
            contacts
                .rests
                .get(entity)
                .expect("a moving group's dynamic body carries its sleep threshold")
                .linear
                * contacts.length_unit.0
        })
    });
    // An authority without a shot clock (a test fixture's bare sim) is at tick zero.
    let tick = clock.as_deref().map_or(0, |clock| clock.0.raw());
    let landings: std::collections::BTreeMap<_, _> = landings
        .into_iter()
        .map(|(entity, landing)| {
            let (_, _, breakage, pose, _, _) = groups
                .get(entity)
                .expect("the landings were filtered to these groups");
            let chunk = |collider: Entity| {
                usize::from(
                    contacts
                        .colliders
                        .get(collider)
                        .expect("a moving group's contact lies on a live collider")
                        .0
                        .expect("a moving group's colliders are its chunks'")
                        .0,
                )
            };
            let (touches, energy) = touches(
                &breakage.live.structure,
                breakage.physics_frame(pose),
                &landing,
                chunk,
                |other| contacts.other(other, &groups),
            );
            (entity, (energy, touches))
        })
        .collect();
    let mut prepared = bevy::utils::Parallel::<Vec<_>>::default();
    // Owners update their own seams and prepare children together. Only the ledger, messages
    // and structural changes commit in entity order after every owner's calculation completes.
    // One landing per task: its passes and kernel scale with its group, from a few chunks to over
    // a hundred, so contiguous batches cannot balance them.
    groups
        .par_iter_many_unique_mut(landings.keys())
        .batching_strategy(bevy::ecs::batching::BatchingStrategy::fixed(1))
        .for_each(
            |(entity, piece, mut breakage, pose, (velocity, spin, centre), _)| {
                let (energy, touches) = &landings[&entity];
                let owned: Vec<usize> = piece.chunks.iter().map(|&c| usize::from(c)).collect();
                let frame = breakage.physics_frame(pose);
                let live = breakage.live.as_mut();
                let mut result = crumble(&live.structure, &live.state, &owned, touches, *energy);
                if let Some(stages) = stages.as_deref() {
                    stages.add(Work::Landings, 1);
                    stages.add(Work::LandingPasses, result.passes as u64);
                    stages.add(Work::LandingOps, result.work);
                }
                result.fragments = merge_fragments(
                    &live.structure,
                    result.fragments,
                    &result.separated,
                    &mut live.rubble_joints,
                );
                for &j in &result.cracked {
                    live.state.cracked[j] = true;
                }
                for &j in &result.separated {
                    live.state.separated[j] = true;
                }
                if result.fragments.len() == 1 && server_body(&live.structure, &result.fragments[0])
                {
                    return; // Still one body: its seams took the energy, or none could.
                }
                let motion = ReleasedMotion {
                    velocity: velocity.0,
                    spin: spin.0,
                    centre: pose.0.0 + pose.1.0 * centre.0,
                };
                let mut record = Record::default();
                let (kept, sheds) = route(
                    &live.structure,
                    (piece.construction, frame),
                    result.fragments,
                    |light| vec![motion; light.len()],
                    tick,
                    &mut record,
                );
                let pending = PendingSplit::of(&breakage, kept, tick);
                prepared.scope(|local| local.push((entity, pending, sheds, record.entries)));
            },
        );
    let mut prepared: Vec<_> = prepared.drain().collect();
    prepared.sort_unstable_by_key(|(entity, ..)| *entity);
    for (entity, pending, sheds, entries) in prepared {
        record.entries.extend(entries);
        shed.write_batch(sheds);
        commands.entity(entity).insert(pending);
    }
}

/// The size split after [`merge_fragments`], at `tick`, for hull cuts, structure releases and
/// landings: a fragment that is no server body goes to the record as dust and to the players as
/// one [`Shed`], returned for the caller to send, moving with the rigid field (as [`Shed::of`])
/// that `motions` gives each such fragment, in order; the rest, returned, stay server bodies.
pub(crate) fn route(
    structure: &Structure,
    (construction, frame): (u16, Transform),
    fragments: Vec<Vec<usize>>,
    motions: impl FnOnce(&[Vec<usize>]) -> Vec<ReleasedMotion>,
    tick: u32,
    record: &mut Record,
) -> (Vec<Vec<usize>>, Vec<Shed>) {
    let (kept, light): (Vec<_>, Vec<_>) = fragments
        .into_iter()
        .partition(|fragment| server_body(structure, fragment));
    let motions = motions(&light);
    assert_eq!(
        motions.len(),
        light.len(),
        "each decor fragment has one rigid field"
    );
    let sheds = light
        .iter()
        .zip(motions)
        .map(|(fragment, motion)| {
            record.entries.extend(fragment.iter().map(|&c| Entry {
                construction,
                event: Event::Dust(c as u16),
                tick,
            }));
            Shed::of(structure, (construction, frame), fragment, motion, tick)
        })
        .collect();
    (kept, sheds)
}

/// Mark `sheds`' chunks as dust in `state`: what [`route`] sheds leaves the simulation. Landings
/// mark nothing, as their split keeps only owned chunks.
pub(crate) fn mark_dust(state: &mut State, sheds: &[Shed]) {
    for &c in sheds.iter().flat_map(|shed| &shed.chunks) {
        state.fate[usize::from(c)] = Fate::Dust;
    }
}

/// Mount the crumble on the authority in [`crate::construction::breakage::LandingSet`]: after the
/// deposits whose splits it keeps, before the refit that despawns colliders its contacts name.
/// [`Shed`] is registered with the breakage, whose sweep routes too.
pub(crate) fn sim_plugin(app: &mut App) {
    app.add_systems(
        FixedPostUpdate,
        crumble_landings
            .in_set(crate::construction::breakage::LandingSet)
            .run_if(not(resource_exists::<crate::ClientReplica>)),
    );
}

#[cfg(test)]
mod tests;
