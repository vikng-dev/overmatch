//! A released part crumbling where it lands, in the game's environment destruction model (game
//! state only). The construction design's "Crumble on landing" owns the narrative.
//!
//! THE LAW ([`crumble`]). An impact on a moving chunk group (a contact whose blow its solver
//! delivered past the body's rest speed, [`ContactTransfers::split_points`]) spends its energy
//! `E = ½ Σ |J_i| v_i` on the group's own seams, cheapest first: each costs its opening work, its
//! seam's mode-I fracture energy over its area, and a rest or an opened seam costs nothing. It stops
//! at the first seam the energy left cannot pay. What stays connected is a fragment.
//!
//! THE SIZE SPLIT ([`server_body`]). A fragment at least [`PhysicalLaws::server_fragment_kg`] stays
//! a server body, split off through the construction's split path
//! ([`crate::construction::breakage::PendingSplit`]). A lighter one leaves the simulation: the
//! record writes its chunks as dust, and one [`Shed`] carries it to the players' decoration
//! ([`super::decor`]), which only draws it.

use std::collections::BTreeMap;

use avian3d::prelude::{AngularVelocity, ComputedCenterOfMass, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ShotClock;
use crate::construction::breakage::{Breakage, Entry, Event, PendingSplit, Piece, Record};
use crate::construction::fracture::{Bond, Side, State, Structure};
use crate::contact_transfer::ContactTransfers;
use crate::physical_laws::PhysicalLaws;

/// What one impact broke in a group.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Crumble {
    /// The joints it opened, cheapest first.
    pub(crate) broken: Vec<usize>,
    /// Their opening work, J: never more than the impact's energy.
    pub(crate) spent: f32,
    /// What stays connected, each ascending, ordered by its first chunk.
    pub(crate) fragments: Vec<Vec<usize>>,
}

/// The work to open joint `j`, J: its seam's mode-I fracture energy over its area. A rest, a
/// cracked seam and a separated contact hold by no cohesion and open for nothing.
fn opening_work(structure: &Structure, state: &State, j: usize) -> f32 {
    let joint = &structure.joints[j];
    match joint.bond {
        Bond::Bonded(seam) | Bond::AreaShare { seam, .. }
            if !state.cracked[j] && !state.separated[j] =>
        {
            seam.fracture_energy * joint.area
        }
        _ => 0.0,
    }
}

/// Spend `energy` (J) on `group`'s internal seams in `state`, cheapest first (module doc).
/// `group` is ascending and owned by `state`.
pub(crate) fn crumble(
    structure: &Structure,
    state: &State,
    group: &[usize],
    energy: f32,
) -> Crumble {
    assert!(
        energy.is_finite() && energy >= 0.0,
        "an impact's energy is finite and nonnegative"
    );
    let inside = |c: usize| group.binary_search(&c).is_ok();
    let mut seams: Vec<(f32, usize)> = structure
        .joints
        .iter()
        .enumerate()
        .filter_map(|(j, joint)| match joint.b {
            Side::Chunk(b) if inside(joint.a) && inside(b) && state.live(joint) => {
                Some((opening_work(structure, state, j), j))
            }
            _ => None,
        })
        .collect();
    seams.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut spent = 0.0_f32;
    let mut broken = Vec::new();
    let mut whole = Vec::new();
    let mut seams = seams.into_iter();
    for (work, j) in seams.by_ref() {
        if spent + work > energy {
            whole.push(j);
            break;
        }
        spent += work;
        broken.push(j);
    }
    whole.extend(seams.map(|(_, j)| j));
    let index: BTreeMap<usize, usize> = group.iter().enumerate().map(|(i, &c)| (c, i)).collect();
    let mut parent: Vec<usize> = (0..group.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for j in whole {
        let joint = &structure.joints[j];
        let Side::Chunk(b) = joint.b else {
            unreachable!("an internal seam joins two chunks");
        };
        let (x, y) = (
            root(&mut parent, index[&joint.a]),
            root(&mut parent, index[&b]),
        );
        parent[x.max(y)] = x.min(y);
    }
    let mut fragments: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &c) in group.iter().enumerate() {
        fragments.entry(root(&mut parent, i)).or_default().push(c);
    }
    Crumble {
        broken,
        spent,
        fragments: fragments.into_values().collect(),
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

/// The energy (J) one physics step's impacts brought each moving group: `½ |J_i| v_i` per point,
/// its blow and closing speed (module doc).
fn impacts(transfers: &ContactTransfers, moving: impl Fn(Entity) -> bool) -> BTreeMap<Entity, f32> {
    let mut energy = BTreeMap::new();
    for points in transfers.groups() {
        let structure = points[0].structure;
        if !moving(structure) {
            continue;
        }
        let brought: f32 = transfers
            .split_points(points)
            .map(|(point, blow, _, _)| 0.5 * blow.length() * point.closing_speed.max(0.0))
            .sum();
        if brought > 0.0 {
            *energy.entry(structure).or_default() += brought;
        }
    }
    energy
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

/// AUTHORITY: each physics step's impacts crumble the moving groups they reach (module doc). A
/// group a field load already splits this tick keeps that split; its impact is not read twice.
fn crumble_impacts(
    transfers: Res<ContactTransfers>,
    mut read: Local<u64>,
    mut groups: Query<Moving, Without<PendingSplit>>,
    clock: Option<Res<ShotClock>>,
    mut record: ResMut<Record>,
    mut shed: MessageWriter<Shed>,
    mut commands: Commands,
) {
    if transfers.steps() == *read {
        return;
    }
    *read = transfers.steps();
    let energy = impacts(&transfers, |entity| groups.contains(entity));
    // An authority without a shot clock (a test fixture's bare sim) is at tick zero.
    let tick = clock.as_deref().map_or(0, |clock| clock.0.raw());
    for (entity, energy) in energy {
        let (_, piece, mut breakage, pose, (velocity, spin, centre)) = groups
            .get_mut(entity)
            .expect("the impacts were filtered to these groups");
        let owned: Vec<usize> = piece.chunks.iter().map(|&c| usize::from(c)).collect();
        let live = breakage.live.as_mut();
        let result = crumble(&live.structure, &live.state, &owned, energy);
        for &j in &result.broken {
            live.state.cracked[j] = true;
        }
        if result.fragments.len() == 1 && server_body(&live.structure, &result.fragments[0]) {
            continue; // Still one body: its seams took the energy, or none could.
        }
        let structure = live.structure.clone();
        let frame = breakage.physics_frame(pose);
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
        crumble_impacts
            .after(crate::construction::breakage::LawSet)
            .before(super::launch::LaunchSet)
            .run_if(not(resource_exists::<crate::ClientReplica>)),
    );
}

#[cfg(test)]
mod tests;
