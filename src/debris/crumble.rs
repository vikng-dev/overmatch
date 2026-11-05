//! A released part crumbling where it lands, in the game's environment destruction model (game
//! state only). The construction design's "Crumble on landing" owns the narrative.
//!
//! THE LAW ([`crumble`]). A landing is a physics step in which a moving chunk group took a blow
//! past its rest speed ([`ContactTransfers::split_points`]). Each contact point's transfer over
//! that step, `F = J / Δt`, is a force on the chunk it touches; the group's chunk kernel
//! ([`Kernel::hold`]) balances those forces by the group's rigid acceleration and judges every
//! joint in the field that leaves: a bond past its strength cracks, a joint pulled apart parts,
//! and the field is solved again on what is left until it holds. Cracks therefore lie where the
//! force runs. Energy conserves: the cracks absorb their fracture
//! work, each bond's fracture energy in its failing mode over its area ([`crack_work`]), and all of
//! them together never more than the kinetic energy the landing dissipated, `E = ½ Σ |J_i| v_i`
//! over its blows and closing speeds (DERIVED, a plastic impact). Past that, the most overloaded
//! crack first, it stops at the first the energy left cannot pay. What stays connected over the
//! joints that did not part is a fragment.
//!
//! THE SIZE SPLIT ([`route`]). A fragment at least [`PhysicalLaws::server_fragment_kg`] stays
//! a server body, split off through the construction's split path
//! ([`crate::construction::breakage::PendingSplit`]). A lighter one leaves the simulation: the
//! record writes its chunks as dust, and one [`Shed`] carries it to the players' decoration
//! ([`super::decor`]), which only draws it.

use std::collections::BTreeMap;

use avian3d::prelude::{AngularVelocity, ComputedCenterOfMass, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ShotClock;
use crate::construction::breakage::{
    Breakage, ChunkCollider, Entry, Event, PendingSplit, Piece, Record,
};
use crate::construction::fracture::bondsolve::Wrench;
use crate::construction::fracture::parts::Kernel;
use crate::construction::fracture::{Cause, Failure, Side, State, Structure};
use crate::contact_transfer::ContactTransfers;
use crate::physical_laws::PhysicalLaws;

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
    /// Scalar operations of the group's solve, a DERIVED count ([`Kernel::hold`]).
    pub(crate) work: u64,
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

/// What stays connected of `group` (ascending) over its joints `state` has not parted, each
/// ascending, ordered by its first chunk.
fn fragments(structure: &Structure, state: &State, group: &[usize]) -> Vec<Vec<usize>> {
    let index: BTreeMap<usize, usize> = group.iter().enumerate().map(|(i, &c)| (c, i)).collect();
    let mut parent: Vec<usize> = (0..group.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
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
    'passes: for _ in 0..PASSES {
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
        let hold =
            Kernel::of(structure, &after, group).hold(structure, &loads(structure, group, &shared));
        work += hold.work;
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
    }
}

/// Whether `fragment` stays a server body: its mass reaches the cut-off (module doc).
pub(crate) fn server_body(structure: &Structure, fragment: &[usize]) -> bool {
    let mass: f32 = fragment.iter().map(|&c| structure.chunks[c].mass).sum();
    mass >= PhysicalLaws::shipped().server_fragment_kg
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
        (velocity, spin, centre): (Vec3, Vec3, Vec3),
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

/// One physics step's landing on a moving group: each point's collider, world point and the force
/// the group took there (N, world), and the energy its blows dissipated (J, module doc).
#[derive(Default)]
struct Landing {
    points: Vec<(Entity, Vec3, Vec3)>,
    energy: f32,
}

/// Each moving group's landing this physics step; a group whose contacts brought no blow only
/// rests or slides, and lands nowhere.
fn landings(
    transfers: &ContactTransfers,
    moving: impl Fn(Entity) -> bool,
) -> BTreeMap<Entity, Landing> {
    let dt = transfers.dt();
    let mut landings: BTreeMap<Entity, Landing> = BTreeMap::new();
    for points in transfers.groups() {
        let structure = points[0].structure;
        if !moving(structure) {
            continue;
        }
        let landing = landings.entry(structure).or_default();
        for (point, blow, press, _) in transfers.split_points(points) {
            landing.energy += 0.5 * blow.length() * point.closing_speed.max(0.0);
            landing
                .points
                .push((point.structure_collider, point.point, blow / dt + press));
        }
    }
    landings.retain(|_, landing| landing.energy > 0.0);
    landings
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
);

/// The physics steps' contacts, read once each, and the chunk each moving collider stands for.
#[derive(bevy::ecs::system::SystemParam)]
struct Contacts<'w, 's> {
    transfers: Res<'w, ContactTransfers>,
    read: Local<'s, u64>,
    colliders: Query<'w, 's, &'static ChunkCollider>,
}

/// AUTHORITY: each physics step's landings crumble the moving groups they reach (module doc). A
/// group a field load already splits this tick keeps that split; its landing is not read twice.
fn crumble_landings(
    mut contacts: Contacts,
    mut groups: Query<Moving, Without<PendingSplit>>,
    clock: Option<Res<ShotClock>>,
    mut record: ResMut<Record>,
    mut shed: MessageWriter<Shed>,
    mut commands: Commands,
) {
    if contacts.transfers.steps() == *contacts.read {
        return;
    }
    *contacts.read = contacts.transfers.steps();
    let colliders = &contacts.colliders;
    let landings = landings(&contacts.transfers, |entity| groups.contains(entity));
    // An authority without a shot clock (a test fixture's bare sim) is at tick zero.
    let tick = clock.as_deref().map_or(0, |clock| clock.0.raw());
    for (entity, landing) in landings {
        let (_, piece, mut breakage, pose, (velocity, spin, centre)) = groups
            .get_mut(entity)
            .expect("the landings were filtered to these groups");
        let frame = breakage.physics_frame(pose);
        let touches: Vec<Touch> = landing
            .points
            .iter()
            .map(|&(collider, at, force)| Touch {
                chunk: usize::from(
                    colliders
                        .get(collider)
                        .expect("a moving group's contact lies on one of its chunk colliders")
                        .0,
                ),
                at: frame.rotation.inverse() * (at - frame.translation),
                force: frame.rotation.inverse() * force,
            })
            .collect();
        let owned: Vec<usize> = piece.chunks.iter().map(|&c| usize::from(c)).collect();
        let live = breakage.live.as_mut();
        let result = crumble(
            &live.structure,
            &live.state,
            &owned,
            &touches,
            landing.energy,
        );
        for &j in &result.cracked {
            live.state.cracked[j] = true;
        }
        for &j in &result.separated {
            live.state.separated[j] = true;
        }
        if result.fragments.len() == 1 && server_body(&live.structure, &result.fragments[0]) {
            continue; // Still one body: its seams took the energy, or none could.
        }
        let structure = live.structure.clone();
        let motion = (velocity.0, spin.0, pose.0.0 + pose.1.0 * centre.0);
        let kept = route(
            &structure,
            (piece.construction, frame),
            result.fragments,
            motion,
            tick,
            &mut record,
            &mut shed,
        );
        commands
            .entity(entity)
            .insert(PendingSplit::of(&breakage, kept, tick));
    }
}

/// The size split of `fragments` of `construction` at `tick` (module doc), each moving with the
/// rigid field `motion` (as [`Shed::of`]): a light one goes to the record as dust and to the
/// players as one [`Shed`]; the rest, returned, stay server bodies.
pub(crate) fn route(
    structure: &Structure,
    (construction, frame): (u16, Transform),
    fragments: Vec<Vec<usize>>,
    motion: (Vec3, Vec3, Vec3),
    tick: u32,
    record: &mut Record,
    shed: &mut MessageWriter<Shed>,
) -> Vec<Vec<usize>> {
    let (kept, light): (Vec<_>, Vec<_>) = fragments
        .into_iter()
        .partition(|fragment| server_body(structure, fragment));
    for fragment in &light {
        record.entries.extend(fragment.iter().map(|&c| Entry {
            construction,
            event: Event::Dust(c as u16),
            tick,
        }));
        shed.write(Shed::of(
            structure,
            (construction, frame),
            fragment,
            motion,
            tick,
        ));
    }
    kept
}

/// Mount the crumble on the authority: after the construction laws, before launches and splits.
pub(crate) fn sim_plugin(app: &mut App) {
    app.add_message::<Shed>().add_systems(
        FixedPostUpdate,
        crumble_landings
            .after(crate::construction::breakage::LawSet)
            .before(super::launch::LaunchSet)
            .run_if(not(resource_exists::<crate::ClientReplica>)),
    );
}

#[cfg(test)]
mod tests;
