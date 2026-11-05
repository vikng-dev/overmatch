//! A released part crumbling where it lands, in the game's environment destruction model (game
//! state only). The construction design's "Crumble on landing" owns the narrative.
//!
//! THE LAW ([`crumble`]). A landing is a physics step in which a moving chunk group took a blow
//! past its rest speed (Avian's `SleepThreshold`, [`landings`]). Each contact point's blow peaks as
//! two elastic bodies stopping each other do ([`peak_force`], Hertz), from the two sides'
//! stiffness and curvature, never from the solver's step; with the point's press it is a force on
//! the chunk it touches. The group's chunk kernel
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

use avian3d::prelude::{
    AngularVelocity, CollisionLayers, ComputedCenterOfMass, LinearVelocity, PhysicsLengthUnit,
    Position, Rotation, SleepThreshold,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::construction::Element;
use crate::construction::breakage::{
    Breakage, ChunkCollider, Entry, Event, PendingSplit, Piece, Record,
};
use crate::construction::fracture::bondsolve::Wrench;
use crate::construction::fracture::parts::Kernel;
use crate::construction::fracture::{Cause, Failure, Side, State, Structure};
use crate::contact_transfer::ContactTransfers;
use crate::physical_laws::PhysicalLaws;
use crate::{Layer, ShotClock};

/// One side of a contact: its plane-strain compliance `(1 − ν²) / E` (1/Pa, zero where rigid) and
/// its radius of curvature (m, `None` where flat).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Contactor {
    pub(crate) compliance: f64,
    pub(crate) radius: Option<f64>,
}

impl Contactor {
    /// Chunk `c`: its material's compliance (`ν = E / 2G − 1`, isotropic, from its two moduli) and
    /// the radius of the sphere of its volume (a StandIn for the contact patch's own curvature,
    /// which the narrow phase does not report).
    pub(crate) fn chunk(structure: &Structure, c: usize) -> Self {
        let chunk = &structure.chunks[c];
        let (young, shear) = (chunk.material.young, chunk.material.shear_modulus);
        let poisson = young / (2.0 * shear) - 1.0;
        Self {
            compliance: (1.0 - poisson * poisson) / young,
            radius: Some((3.0 * f64::from(chunk.volume) / (4.0 * std::f64::consts::PI)).cbrt()),
        }
    }

    /// The ground, flat, with its data ([`PhysicalLaws::ground_youngs_modulus_pa`]).
    pub(crate) fn ground() -> Self {
        let laws = PhysicalLaws::shipped();
        let poisson = laws.ground_poisson_ratio;
        Self {
            compliance: (1.0 - poisson * poisson) / laws.ground_youngs_modulus_pa,
            radius: None,
        }
    }

    /// A body the game states no stiffness for (a hull): rigid and flat, so the chunk's own
    /// compliance governs. StandIn for the body's material stiffness, which no body data carries.
    pub(crate) const RIGID: Self = Self {
        compliance: 0.0,
        radius: None,
    };
}

/// `E*` (Pa) and `√R` (√m) of the contact between `a` and `b` (Hertz; K. L. Johnson, *Contact
/// Mechanics*, Cambridge 1985, §11.4): `1/E* = Σ (1 − ν²)/E`, `1/R = Σ 1/R`.
fn hertz(a: Contactor, b: Contactor) -> (f64, f64) {
    let curvature: f64 = [a.radius, b.radius]
        .iter()
        .flatten()
        .map(|r| r.recip())
        .sum();
    assert!(
        curvature > 0.0,
        "a contact's chunk side is curved: two flat sides make no Hertz contact"
    );
    (
        (a.compliance + b.compliance).recip(),
        curvature.recip().sqrt(),
    )
}

/// The peak force (N) of an elastic impact closing at `speed` m/s with effective mass `mass` kg
/// between `a` and `b` ([`hertz`]): maximum approach `δ = (15 m v² / (16 E* √R))^(2/5)` and peak
/// force `P = (4/3) E* √R δ^(3/2)`. The contact's own stiffness sets how hard it stops, so no
/// solver step enters it. One law for a landing and a hull's ram (`construction::sweep`).
pub(crate) fn peak_force(mass: f64, speed: f64, a: Contactor, b: Contactor) -> f64 {
    let (stiffness, root) = hertz(a, b);
    let approach = (15.0 * mass * speed * speed / (16.0 * stiffness * root)).powf(0.4);
    4.0 / 3.0 * stiffness * root * approach.powf(1.5)
}

/// The closing speed (m/s) at which [`peak_force`] reaches `force` N, its inverse:
/// `δ = (3P / (4 E* √R))^(2/3)` and `v = √(16 E* √R δ^(5/2) / (15 m))`.
pub(crate) fn peak_speed(mass: f64, force: f64, a: Contactor, b: Contactor) -> f64 {
    let (stiffness, root) = hertz(a, b);
    let approach = (3.0 * force / (4.0 * stiffness * root)).powf(2.0 / 3.0);
    (16.0 * stiffness * root * approach.powf(2.5) / (15.0 * mass)).sqrt()
}

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
pub(crate) fn fragments(structure: &Structure, state: &State, group: &[usize]) -> Vec<Vec<usize>> {
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

/// One contact point of a landing: the group's collider, the other side's (`None` for a track
/// belt), the world point, the blow (N·s) and press (N) the group took there, and its closing speed
/// (m/s).
struct Point {
    collider: Entity,
    other: Option<Entity>,
    at: Vec3,
    blow: Vec3,
    press: Vec3,
    closing: f32,
}

/// One physics step's landing on a moving group: its points and the energy its blows dissipated
/// (J, module doc).
#[derive(Default)]
struct Landing {
    points: Vec<Point>,
    energy: f32,
}

/// Each moving group's landing this physics step; a group whose contacts brought no blow only
/// rests or slides, and lands nowhere.
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
            let blow = -point.normal * struck;
            let press = -(point.impulse - point.normal * struck) / dt;
            landing.energy += 0.5 * struck * point.closing_speed.max(0.0);
            landing.points.push(Point {
                collider: point.structure_collider,
                other: point.body_collider,
                at: point.point,
                blow,
                press,
                closing: point.closing_speed,
            });
        }
    }
    landings.retain(|_, landing| landing.energy > 0.0);
    landings
}

/// The force (N) a landing's point puts on chunk `chunk`: its blow's peak against `other`, with the
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
    /// What a landing's point meets through `collider`: a construction's chunk (curved) or element
    /// (flat) by its material, the ground by its data, and any other body (a hull, a track belt
    /// with no collider) as [`Contactor::RIGID`].
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
        match (chunk, element, breakage) {
            (Some(&ChunkCollider(c)), _, Some(breakage)) => {
                Contactor::chunk(&breakage.live.structure, usize::from(c))
            }
            (None, Some(&Element(e)), Some(breakage)) => {
                let first = breakage
                    .kind
                    .layout
                    .element_of
                    .iter()
                    .position(|&of| of == e)
                    .expect("an element has chunks");
                Contactor {
                    radius: None, // A whole element's face is flat to the chunk landing on it.
                    ..Contactor::chunk(&breakage.live.structure, first)
                }
            }
            _ if layers.is_some_and(|layers| layers.memberships.has_all(Layer::Terrain)) => {
                Contactor::ground()
            }
            _ => Contactor::RIGID,
        }
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
    mut commands: Commands,
) {
    if contacts.transfers.steps() == *contacts.read {
        return;
    }
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
    let landings: Vec<(Entity, f32, Vec<Touch>)> = landings
        .into_iter()
        .map(|(entity, landing)| {
            let (_, _, breakage, pose, _, _) = groups
                .get(entity)
                .expect("the landings were filtered to these groups");
            let frame = breakage.physics_frame(pose);
            let structure = &breakage.live.structure;
            let touches = landing
                .points
                .iter()
                .map(|point| {
                    let chunk = usize::from(
                        contacts
                            .colliders
                            .get(point.collider)
                            .expect("a moving group's contact lies on a live collider")
                            .0
                            .expect("a moving group's colliders are its chunks'")
                            .0,
                    );
                    let other = contacts.other(point.other, &groups);
                    let force = landing_force(
                        structure,
                        chunk,
                        other,
                        (point.blow, point.press, point.closing),
                    );
                    Touch {
                        chunk,
                        at: frame.rotation.inverse() * (point.at - frame.translation),
                        force: frame.rotation.inverse() * force,
                    }
                })
                .collect();
            (entity, landing.energy, touches)
        })
        .collect();
    for (entity, energy, touches) in landings {
        let (_, piece, mut breakage, pose, (velocity, spin, centre), _) = groups
            .get_mut(entity)
            .expect("the landings were filtered to these groups");
        let frame = breakage.physics_frame(pose);
        let owned: Vec<usize> = piece.chunks.iter().map(|&c| usize::from(c)).collect();
        let live = breakage.live.as_mut();
        let result = crumble(&live.structure, &live.state, &owned, &touches, energy);
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
        let (kept, sheds) = route(
            &structure,
            (piece.construction, frame),
            result.fragments,
            motion,
            tick,
            &mut record,
        );
        shed.write_batch(sheds);
        commands
            .entity(entity)
            .insert(PendingSplit::of(&breakage, kept, tick));
    }
}

/// The size split of `fragments` of `construction` at `tick` (module doc), each moving with the
/// rigid field `motion` (as [`Shed::of`]): a light one goes to the record as dust and to the
/// players as one [`Shed`], returned for the caller to send; the rest, returned, stay server bodies.
pub(crate) fn route(
    structure: &Structure,
    (construction, frame): (u16, Transform),
    fragments: Vec<Vec<usize>>,
    motion: (Vec3, Vec3, Vec3),
    tick: u32,
    record: &mut Record,
) -> (Vec<Vec<usize>>, Vec<Shed>) {
    let (kept, light): (Vec<_>, Vec<_>) = fragments
        .into_iter()
        .partition(|fragment| server_body(structure, fragment));
    let sheds = light
        .iter()
        .map(|fragment| {
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
