use super::*;
use avian3d::prelude::{
    ConstantForce, ConstantLinearAcceleration, ConstantLocalForce, ConstantLocalLinearAcceleration,
    Sensor,
};
use bevy::math::DMat3;
use bevy::tasks::{ParallelSlice, ParallelSliceMut};

/// A source-side sweep exchange for the test-time contact account. A plastic mass exchange
/// couples the whole body and has no surface normal.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct SourceImpulse {
    pub(crate) body: Entity,
    pub(crate) linear: Vec3,
    /// Free angular impulse about the source COM.
    pub(crate) angular: Vec3,
}

/// The game's environment destruction: this tick's exact cast, the bodies it cast and each
/// (source body, owner root, chunk) a source's sweep met inward and a cut judged;
/// [`SweepContacts`] reads it.
#[derive(Resource, Default, Debug)]
pub(crate) struct SweepCast {
    /// The fixed-clock time the sweep built it at: a contact pass at any other time had no sweep.
    at: Option<std::time::Duration>,
    bodies: BTreeSet<Entity>,
    members: BTreeSet<(Entity, Entity, u16)>,
}

/// The game's environment destruction: the solver resolves no look-ahead point, one whose shapes
/// are still apart, against standing matter that this tick's exact cast says a body does not
/// reach, matter no cut judged. Overlapping points, cast members and uncovered chunks keep the
/// solver's contact, and so does every point of a body the sweep did not cast (a departure past
/// [`DEPARTURE_ROUNDS`], a body woken in the step): no cast is no verdict. A point within Avian's
/// contact tolerance is look-ahead too: a freed chunk stands only [`LOOSE_GAP_M`] off the faces
/// it shared, and the body that freed it can push it across that gap within the solve. Standing
/// colliders enable the hook with [`ActiveCollisionHooks::MODIFY_CONTACTS`].
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SweepContacts<'w, 's> {
    owners: Query<
        'w,
        's,
        (
            &'static ColliderOf,
            Option<&'static Element>,
            Option<&'static ChunkCollider>,
        ),
    >,
    standing:
        Query<'w, 's, (&'static Breakage, &'static Position, &'static Rotation), Without<Piece>>,
    /// Absent in physics-only apps, which run no sweep.
    cast: Option<Res<'w, SweepCast>>,
    fixed: Res<'w, Time<Fixed>>,
}

impl SweepContacts<'_, '_> {
    /// Whether world `point` on `collider` is covered standing matter outside the cast of
    /// `source`, a body this tick's sweep cast. `None` when no sweep ran this tick: replicas and
    /// physics-only apps judge nothing.
    pub(crate) fn unjudged(&self, collider: Entity, source: Entity, point: Vec3) -> Option<bool> {
        let cast = self
            .cast
            .as_deref()
            .filter(|cast| cast.at == Some(self.fixed.elapsed()))?;
        if !cast.bodies.contains(&source) {
            return Some(false);
        }
        let Ok((of, element, chunk)) = self.owners.get(collider) else {
            return Some(false); // Not a collider of a body: no construction matter.
        };
        let Ok((owner, position, rotation)) = self.standing.get(of.body) else {
            return Some(false); // A piece or any other body: no standing matter.
        };
        let pose = owner.physics_frame((position, rotation));
        let at = pose.rotation.inverse() * (point - pose.translation);
        let chunk = match (chunk, element) {
            (Some(chunk), _) => Some(usize::from(chunk.0)),
            (None, Some(element)) => nearest_chunk(&owner.kind.layout, usize::from(element.0), at),
            (None, None) => None, // A standing owner's colliders are elements and chunks.
        };
        Some(chunk.is_some_and(|c| {
            super::super::sweep::covered(&owner.live.structure, &owner.live.state, c)
                && !cast.members.contains(&(source, of.body, c as u16))
        }))
    }
}

impl avian3d::prelude::CollisionHooks for SweepContacts<'_, '_> {
    fn modify_contacts(
        &self,
        contacts: &mut avian3d::prelude::ContactPair,
        _: &mut Commands,
    ) -> bool {
        let sides = [
            (contacts.collider1, contacts.body2),
            (contacts.collider2, contacts.body1),
        ];
        let unjudged = |point: Vec3| {
            sides.iter().any(|&(collider, source)| {
                // A collider with no body is static and meets standing matter in no solve.
                source.is_some_and(|source| self.unjudged(collider, source, point) == Some(true))
            })
        };
        for manifold in &mut contacts.manifolds {
            manifold
                .points
                .retain(|point| point.penetration >= 0.0 || !unjudged(point.point));
        }
        contacts
            .manifolds
            .retain(|manifold| !manifold.points.is_empty());
        !contacts.manifolds.is_empty()
    }
}

/// The sweep's per-tick account, available to headless witnesses and operator instruments.
#[derive(Resource, Default, Debug)]
pub(crate) struct SweepAccount {
    #[cfg(test)]
    pub(crate) source_impulses: Vec<SourceImpulse>,
    #[cfg(test)]
    pub(crate) cancelled_belts: Vec<crate::contact_transfer::AppliedContact>,
    /// Per source body, the force it drives into a cut with (N, world).
    #[cfg(test)]
    pub(crate) drives: Vec<(Entity, Vec3)>,
    /// Sources swept this tick, departures that joined it included.
    pub(crate) sources: usize,
    /// Sweep rounds this tick, the first included ([`DEPARTURE_ROUNDS`]).
    pub(crate) rounds: usize,
    /// Departures past the round cap, swept from the next tick; Avian keeps their contacts.
    pub(crate) unswept: usize,
    pub(crate) freed: usize,
    /// Freed fragments shed to the players as decoration.
    pub(crate) shed: usize,
    #[cfg(test)]
    pub(crate) patches: Vec<(Entity, u16, Vec<usize>, super::super::sweep::Witness)>,
    pub(crate) work: super::super::sweep::Work,
    pub(crate) exchanges: Vec<(Entity, Vec3, f64)>,
    pub(crate) chunks: Vec<(u16, u16, Vec3)>,
    pub(crate) outgoing_kinetic_j: f64,
    pub(crate) outgoing_momentum: Vec3,
    /// Per source body and construction its hull met this tick: the closing speed along the
    /// sweep's contact normal at the first point met, m/s.
    pub(crate) contacts: BTreeMap<(Entity, u16), f32>,
    pub(crate) samples: Vec<(Entity, u16, f32, (f32, f32, f32, f32))>,
    pub(crate) source_changes: Vec<(Entity, Vec3, Vec3, Vec3)>,
}

struct SweepSource {
    body: Entity,
    shape: Arc<super::super::sweep::SourceTree>,
    motion: super::super::sweep::RigidMotion,
    roots: BTreeSet<Entity>,
    mass: f32,
    inertia: avian3d::prelude::ComputedAngularInertia,
    axes: avian3d::prelude::LockedAxes,
    drive: Vec3,
}

impl SweepSource {
    /// DERIVED: Avian 0.7 `ContactNormalPart::generate`
    /// (`dynamics/solver/contact/normal_part.rs`), with a fixed target:
    /// `m_eff = 1 / (n dot M^-1 n + (r cross n) dot I_world^-1 (r cross n))`.
    /// The lever and world inertia are evaluated at the witness time. Avian supplies the
    /// locked-axis response; dominance does not enter the point mass.
    fn effective_mass(&self, witness: super::super::sweep::Witness, pose: &Transform) -> f32 {
        use avian3d::dynamics::solver::solver_body::SolverBodyInertia;
        let normal = pose.rotation * witness.normal;
        let point = pose.translation + pose.rotation * witness.at;
        let m = self.motion;
        let lever = point - m.centre - m.velocity * witness.time;
        let rotation = Quat::from_scaled_axis(m.spin * witness.time) * m.rotation;
        let response = SolverBodyInertia::new(
            self.mass.recip(),
            self.inertia.rotated(rotation).inverse(),
            self.axes,
            0,
            true,
        );
        let torque = lever.cross(normal);
        let inverse = normal.dot(response.effective_inv_mass() * normal)
            + torque.dot(response.effective_inv_angular_inertia() * torque);
        assert!(
            inverse > 0.0,
            "an inward sweep source has a free contact response"
        );
        inverse.recip()
    }
}

/// Sources resolve per construction, in body entity order ([`owner_sequence`]). A source exchanges
/// against all its freed mass once.
type Owners<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Position,
        &'static Rotation,
        &'static mut Breakage,
    ),
    Without<Piece>,
>;
type Bodies<'w, 's> = Query<
    'w,
    's,
    (
        Forces,
        &'static RigidBody,
        &'static avian3d::prelude::ComputedMass,
        &'static avian3d::prelude::ComputedAngularInertia,
        &'static avian3d::prelude::ComputedCenterOfMass,
        Option<&'static avian3d::prelude::LockedAxes>,
        Option<&'static avian3d::prelude::GravityScale>,
    ),
>;
type Hulls<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Collider,
        &'static ColliderOf,
        &'static avian3d::prelude::ColliderTransform,
        &'static CollisionLayers,
        Option<&'static Element>,
        Option<&'static ChunkCollider>,
    ),
    Without<Sensor>,
>;
type Drives<'w, 's> = Query<
    'w,
    's,
    (
        Option<&'static ConstantForce>,
        Option<&'static ConstantLocalForce>,
        Option<&'static ConstantLinearAcceleration>,
        Option<&'static ConstantLocalLinearAcceleration>,
    ),
>;

/// Each world translation axis `axes` leaves free, as 1, and each it locks, as 0.
fn free_translation(axes: avian3d::prelude::LockedAxes) -> Vec3 {
    Vec3::new(
        f32::from(!axes.is_translation_x_locked()),
        f32::from(!axes.is_translation_y_locked()),
        f32::from(!axes.is_translation_z_locked()),
    )
}

fn constant_acceleration(drives: &Drives, body: Entity, rotation: Quat, inverse_mass: f32) -> Vec3 {
    let (force, local_force, acceleration, local_acceleration) = drives
        .get(body)
        .expect("a dynamic source has its optional constant drives");
    force.map_or(Vec3::ZERO, |f| f.0 * inverse_mass)
        + rotation * local_force.map_or(Vec3::ZERO, |f| f.0 * inverse_mass)
        + acceleration.map_or(Vec3::ZERO, |a| a.0)
        + rotation * local_acceleration.map_or(Vec3::ZERO, |a| a.0)
}

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Intake<'w, 's> {
    constructions: Owners<'w, 's>,
    bodies: Bodies<'w, 's>,
    drives: Drives<'w, 's>,
    colliders: Hulls<'w, 's>,
    /// The bodies that can be sources. Standing constructions and sleeping bodies never are:
    /// leaving them out by archetype lets default batching follow the awake movers, not the
    /// static population.
    movers: Query<
        'w,
        's,
        (
            Entity,
            &'static avian3d::prelude::RigidBodyColliders,
            Has<Piece>,
            Option<&'static super::super::sweep::SourceShape>,
        ),
        (
            Without<avian3d::prelude::Sleeping>,
            Or<(With<Piece>, Without<Breakage>)>,
        ),
    >,
    body_colliders: Query<'w, 's, &'static avian3d::prelude::RigidBodyColliders>,
    spatial: SpatialQuery<'w, 's>,
    transfers: ResMut<'w, ContactTransfers>,
    fixed: Res<'w, Time<Fixed>>,
    cast: ResMut<'w, SweepCast>,
    gravity: Res<'w, avian3d::prelude::Gravity>,
    substeps: Res<'w, avian3d::prelude::SubstepCount>,
    clock: Option<Res<'w, ShotClock>>,
    record: ResMut<'w, Record>,
    queue: ResMut<'w, Queue>,
    account: ResMut<'w, SweepAccount>,
    shed: MessageWriter<'w, crate::debris::crumble::Shed>,
    commands: Commands<'w, 's>,
    children: Query<'w, 's, &'static Children>,
    chunk_entities: ChunkChildren<'w, 's>,
}

/// Resolution: how many rounds past the first one tick's departures may join its sweep as
/// sources. Each round sweeps only the matter the round before it freed; past the cap the newest
/// departures spawn unswept, keep Avian's contacts and are swept from the next tick, counted in
/// [`SweepAccount::unswept`]. A bound on one tick's cost, not a law: the tracked ram reaches its
/// fixed point well inside it (MEASURED in its test's report).
const DEPARTURE_ROUNDS: usize = 8;

pub(super) fn run(mut p: Intake) {
    *p.account = SweepAccount::default();
    let dt = p.fixed.timestep().as_secs_f32();
    let tick = p.clock.as_deref().map_or(0, |clock| clock.0.raw());
    *p.cast = SweepCast {
        at: Some(p.fixed.elapsed()),
        ..default()
    };
    // Freed departures join the same tick's sweep: each moves within the tick it leaves, and the
    // remainder it meets is matter only a cut may judge (`SweepContacts`).
    let mut sources = sources(&p, dt);
    let (mut departures, mut recut) = (Vec::new(), BTreeSet::new());
    for round in 0..=DEPARTURE_ROUNDS {
        p.account.sources += sources.len();
        p.account.rounds = round + 1;
        let mut departing: BTreeMap<Entity, Vec<(Entity, Cut)>> = BTreeMap::new();
        let mut exchanges = BTreeMap::new();
        for (body, plugs) in resolve_round(&mut p, dt, tick, &sources, &recut) {
            let exchange = match departures
                .iter_mut()
                .find(|(departure, _)| *departure == body)
            {
                Some((_, departure)) => {
                    let exchange = exchange_departure(&p, departure, &plugs);
                    // Its momentum entered the account when it was freed; [`commit`] adds the
                    // share it hands on as the plugs'.
                    p.account.outgoing_momentum -= exchange.0.impulse.as_vec3();
                    exchange
                }
                None => exchange_source(&mut p, body, &plugs),
            };
            exchanges.insert(body, exchange);
            for (root, cut) in plugs {
                departing.entry(root).or_default().push((body, cut));
            }
        }
        recut.extend(departing.keys().copied());
        let fresh = commit(&mut p, &departing, &exchanges, tick);
        if round == DEPARTURE_ROUNDS {
            p.account.unswept = fresh.len();
        }
        sources = departure_sources(&p, dt, &fresh);
        departures.extend(fresh);
        if sources.is_empty() || round == DEPARTURE_ROUNDS {
            break;
        }
    }
    for (body, departure) in departures {
        spawn_prepared_as(&mut p.commands, body, departure);
    }
    let changed = p
        .queue
        .0
        .iter()
        .flat_map(|deposit| deposit.changed.iter().copied())
        .collect::<BTreeSet<_>>();
    cancel_retired_belts(&mut p, changed);
}

/// One round of the sweep: `sources` resolved against their owners, `recut` the owners an earlier
/// round of this tick cut; the account and cast record it. Returns each source's released cuts.
fn resolve_round(
    p: &mut Intake,
    dt: f32,
    tick: u32,
    sources: &[SweepSource],
    recut: &BTreeSet<Entity>,
) -> BTreeMap<Entity, Vec<(Entity, Cut)>> {
    #[cfg(test)]
    p.account
        .drives
        .extend(sources.iter().map(|source| (source.body, source.drive)));
    p.cast
        .bodies
        .extend(sources.iter().map(|source| source.body));
    let mut resolved = geometry(p, dt, tick, sources, recut);
    resolved.sort_by_key(|resolution| (sources[resolution.source].body, resolution.root));
    p.cast.members.extend(
        resolved
            .iter()
            .filter(|resolution| resolution.swept.witness.is_some())
            .flat_map(|resolution| {
                let body = sources[resolution.source].body;
                (resolution.swept.chunks.iter()).map(move |&c| (body, resolution.root, c as u16))
            }),
    );
    let mut cuts: BTreeMap<Entity, Vec<(Entity, Cut)>> = BTreeMap::new();
    for Resolution {
        source,
        root,
        construction,
        swept,
        cut,
    } in resolved
    {
        let source = &sources[source];
        p.account.work += swept.work;
        if let Some(load) = swept.load() {
            let witness = load.witness;
            p.account
                .contacts
                .entry((source.body, construction))
                .and_modify(|speed| *speed = speed.max(witness.speed))
                .or_insert(witness.speed);
            #[cfg(test)]
            p.account
                .patches
                .push((source.body, construction, swept.chunks.clone(), witness));
            p.account.samples.push((
                source.body,
                construction,
                swept.incoming,
                (load.demand, load.capacity, load.threshold, load.drive),
            ));
        }
        if let Some(cut) = cut {
            cuts.entry(source.body).or_default().push((root, cut));
        }
    }
    cuts
}

/// Cancel each belt reaction staged against a collider this tick's cuts retired.
fn cancel_retired_belts(p: &mut Intake, changed: BTreeSet<Entity>) {
    let retired = changed
        .into_iter()
        .flat_map(|root| {
            let handles = p
                .body_colliders
                .get(root)
                .expect("a swept owner retains its collider relationship until refit");
            handles
                .iter()
                .filter(|&entity| {
                    let Ok((_, _, owner, _, _, element, chunk)) = p.colliders.get(entity) else {
                        return false;
                    };
                    let (_, _, _, breakage) = p
                        .constructions
                        .get(owner.body)
                        .expect("swept collider belongs to its construction");
                    element.is_some_and(|e| breakage.live.swapped[usize::from(e.0)])
                        || chunk.is_some_and(|c| !breakage.live.state.standing(usize::from(c.0)))
                })
                .collect::<Vec<_>>()
        })
        .collect::<BTreeSet<_>>();
    for cancelled in p.transfers.cancel_implicit(&retired) {
        #[cfg(test)]
        p.account.cancelled_belts.push(cancelled);
        let (mut source, ..) = p
            .bodies
            .get_mut(cancelled.body)
            .expect("implicit source survives its same-tick sweep");
        source.apply_force_at_point(-cancelled.force, cancelled.point);
    }
}

fn sources(p: &Intake, dt: f32) -> Vec<SweepSource> {
    let Intake {
        ref constructions,
        ref bodies,
        ref colliders,
        ref movers,
        ref drives,
        ref spatial,
        ref gravity,
        ref substeps,
        ..
    } = *p;
    let travel = travel(substeps, dt);
    let mut gathered = bevy::utils::Parallel::<Vec<SweepSource>>::default();
    movers
        .par_iter()
        .for_each(|(body_entity, handles, piece, cached)| {
            let Ok((forces, body, mass, inertia, com, axes, scale)) = bodies.get(body_entity)
            else {
                return; // Unmarked fixed collider roots have no dynamic force state.
            };
            // The sweep requires finite computed mass. Sleeping contacts retain their force
            // in ContactTransfers; Avian owns any source this intake does not prepare.
            if !body.is_dynamic() || mass.inverse() == 0.0 {
                return;
            }
            let axes = axes.copied().unwrap_or_default(); // Absence is Avian's unrestricted state.
            let rotation = forces.rotation().0;
            // What Avian integrates before its contact solve: accumulated and constant forces and
            // gravity, on the free axes (`pre_process_velocity_increments`).
            let acceleration = free_translation(axes)
                * (forces.accumulated_linear_acceleration()
                    + constant_acceleration(drives, body_entity, rotation, mass.inverse())
                    + gravity.0 * scale.map_or(1.0, |scale| scale.0));
            let velocity = forces.linear_velocity() + acceleration * travel;
            if velocity == Vec3::ZERO && forces.angular_velocity() == Vec3::ZERO {
                return;
            }
            let admitted: Vec<_> = handles
                .iter()
                .filter_map(|handle| {
                    let Ok((_, hull, _, local, layers, _, _)) = colliders.get(handle) else {
                        return None; // Sensors are not contact hulls.
                    };
                    layers
                        .interacts_with(super::super::element_layers())
                        .then_some((hull, local))
                })
                .collect();
            if admitted.is_empty() {
                return;
            } // No construction-contact geometry on this body.
            // Free groups prepare their tree at spawn and split. Articulated bodies need a
            // current-frame snapshot: their collider-local poses can change without a body split.
            let shape = match (piece, cached) {
                (true, Some(shape)) => shape.0.clone(),
                (true, None) => panic!("a free group prepares its source tree at spawn and split"),
                (false, Some(_)) => panic!("only a free group carries a prepared source tree"),
                (false, None) => Arc::new(super::super::sweep::SourceTree::new(
                    admitted
                        .iter()
                        .map(|(hull, local)| super::super::sweep::Hull {
                            hull: (*hull).clone(),
                            position: local.translation,
                            rotation: local.rotation.0,
                        })
                        .collect(),
                )),
            };
            let position = forces.position().0;
            let centre = position + rotation * com.0;
            let motion = super::super::sweep::RigidMotion {
                position,
                rotation,
                centre,
                velocity,
                acceleration,
                lead: travel,
                spin: forces.angular_velocity(),
                dt,
            };
            let roots = owners_met(
                (spatial, colliders, constructions),
                body_entity,
                &shape,
                motion,
            );
            let source = SweepSource {
                body: body_entity,
                shape,
                motion,
                roots,
                mass: mass.value(),
                inertia: *inertia,
                axes,
                drive: acceleration * mass.value(),
            };
            gathered.scope(|local| local.push(source));
        });
    let mut sources: Vec<_> = gathered.drain().collect();
    sources.sort_by_key(|source| source.body);
    sources
}

/// DERIVED: Avian 0.7 adds `a dt / N` to the velocity before each of its `N` substep position
/// steps (`pre_process_velocity_increments`, then `integrate_velocities`), so a body no contact
/// touches travels `v dt + a dt² (N + 1) / (2N)`: the cast's one velocity, `v + a` times this,
/// reaches that pose, and is the body's velocity this far into the tick
/// ([`super::super::sweep::RigidMotion::lead`]).
fn travel(substeps: &avian3d::prelude::SubstepCount, dt: f32) -> f32 {
    (substeps.0 + 1) as f32 / (2 * substeps.0) as f32 * dt
}

/// The standing owners whose colliders `shape`'s swept bounds meet, `body`'s own excepted.
fn owners_met(
    (spatial, colliders, constructions): (&SpatialQuery, &Hulls, &Owners),
    body: Entity,
    shape: &super::super::sweep::SourceTree,
    motion: super::super::sweep::RigidMotion,
) -> BTreeSet<Entity> {
    spatial
        .aabb_intersections_with_aabb(shape.bounds(motion))
        .into_iter()
        .filter_map(|entity| {
            let Ok((_, _, other, _, _, _, _)) = colliders.get(entity) else {
                return None; // Sensors are not contact hulls.
            };
            (other.body != body && constructions.contains(other.body)).then_some(other.body)
        })
        .collect()
}

/// This tick's departures as sweep sources, each under the body id it will spawn as, moving as
/// its launch and gravity leave it: a fresh body bears no other force.
fn departure_sources(
    p: &Intake,
    dt: f32,
    departures: &[(Entity, PreparedDeparture)],
) -> Vec<SweepSource> {
    let travel = travel(&p.substeps, dt);
    departures
        .iter()
        .map(|(body, prepared)| {
            let Departure { root, launch, .. } = &prepared.departure;
            let props = &launch.mass_properties;
            let position = root.transform_point(launch.centroid);
            let motion = super::super::sweep::RigidMotion {
                position,
                rotation: root.rotation,
                centre: position + root.rotation * props.center_of_mass,
                velocity: root.rotation * launch.velocity + p.gravity.0 * travel,
                acceleration: p.gravity.0,
                lead: travel,
                spin: root.rotation * launch.spin,
                dt,
            };
            let shape = prepared.source.0.clone();
            SweepSource {
                body: *body,
                roots: owners_met(
                    (&p.spatial, &p.colliders, &p.constructions),
                    *body,
                    &shape,
                    motion,
                ),
                shape,
                motion,
                mass: props.mass,
                inertia: avian3d::prelude::ComputedAngularInertia::new_with_local_frame(
                    props.principal_angular_inertia,
                    props.local_inertial_frame,
                ),
                axes: default(),
                drive: p.gravity.0 * props.mass,
            }
        })
        .collect()
}

/// One source's resolution on one owner, the source by its index in the tick's sources: what
/// the account reads, and the cut it committed.
struct Resolution {
    source: usize,
    root: Entity,
    construction: u16,
    swept: super::super::sweep::Sweep,
    cut: Option<Cut>,
}

/// A released plug committed to its owner's topology before the exchange ([`cut_fragments`]):
/// the plug, the fragments it and its unsupported remainder leave as, and their ledger entries.
pub(super) struct Cut {
    plug: Vec<usize>,
    fragments: Vec<Vec<usize>>,
    entries: Vec<Entry>,
}

fn geometry(
    p: &mut Intake,
    dt: f32,
    tick: u32,
    sources: &[SweepSource],
    recut: &BTreeSet<Entity>,
) -> Vec<Resolution> {
    let roots = sources
        .iter()
        .flat_map(|source| source.roots.iter().copied())
        .collect::<BTreeSet<_>>();
    let mut gathered = bevy::utils::Parallel::<Vec<_>>::default();
    // One touched owner per task: owners differ widely in size, and the population is not the
    // work set.
    p.constructions
        .par_iter_many_unique_mut(&roots)
        .batching_strategy(bevy::ecs::batching::BatchingStrategy::fixed(1))
        .for_each(|(root, position, rotation, mut breakage)| {
            // Preload storage and cuts change here; a cut owner is marked changed when its
            // departures prepare ([`commit`]).
            let breakage = breakage.bypass_change_detection();
            let pose = breakage.physics_frame((position, rotation));
            let own: Vec<_> = (0..sources.len())
                .filter(|&i| sources[i].roots.contains(&root))
                .collect();
            let resolved = owner_sequence(
                breakage,
                (root, pose),
                (sources, &own),
                (dt, tick),
                recut.contains(&root),
            );
            gathered.scope(|local| local.push(resolved));
        });
    let owners: Vec<_> = gathered.drain().collect();
    assert_eq!(
        owners.len(),
        roots.len(),
        "`sources` admits a root only through this owner query"
    );
    owners.into_iter().flatten().collect()
}

/// One owner's sources in body entity order, each resolved on the state the earlier ones left: a
/// released plug's cut commits at once, and every later source resolves again on its topology
/// with the gravity field re-solved there ([`fracture::parts::Assessment::resolved_on`]), which
/// the owner keeps as its next law stage's factor. A later source meeting only matter an earlier
/// cut freed has no witness and so no cut. Both passes count as work; owners never meet here. An
/// owner an earlier round of this tick cut (`recut`) has no first pass: every source resolves on
/// its current topology.
fn owner_sequence(
    breakage: &mut Breakage,
    (root, pose): (Entity, Transform),
    (sources, own): (&[SweepSource], &[usize]),
    (dt, tick): (f32, u32),
    recut: bool,
) -> Vec<Resolution> {
    // `None` until a cut changes the owner, then that cut's gravity field, solved on first read.
    let mut after: Option<Option<(fracture::parts::Assessment, fracture::parts::Gravity)>> =
        recut.then_some(None);
    let mut resolved = Vec::with_capacity(own.len());
    let mut first_pass = if recut {
        Vec::new()
    } else {
        let own_sources: Vec<_> = own.iter().map(|&i| &sources[i]).collect();
        owner_geometry(breakage, pose, &own_sources, dt, None)
    }
    .into_iter();
    for &index in own {
        let first = first_pass.next().map(|(_, swept)| swept);
        let swept = match &mut after {
            None => first.expect("an owner no cut changed resolves every source in its first pass"),
            Some(field) => {
                let field = field.get_or_insert_with(|| {
                    let live = &breakage.live;
                    (
                        live.assessment.resolved_on(&live.structure, &live.state),
                        fracture::parts::Gravity::default(),
                    )
                });
                let (_, mut swept) =
                    owner_geometry(breakage, pose, &[&sources[index]], dt, Some(field))
                        .pop()
                        .expect("one source resolves one patch");
                if let Some(first) = first {
                    swept.work += first.work;
                }
                swept
            }
        };
        let cut = swept.released().then(|| {
            after = Some(None);
            cut_fragments(breakage, swept.chunks.clone(), tick)
        });
        resolved.push(Resolution {
            source: index,
            root,
            construction: breakage.construction,
            swept,
            cut,
        });
    }
    if let Some(field) = after {
        Arc::make_mut(&mut breakage.live.assessment).next = field.map(|(next, _)| Box::new(next));
    }
    resolved
}

/// Each of `sources`' patches on one owner, resolved against the owner's current state, with the
/// gravity `field` solved on it when given, else the owner's own ([`fracture::parts::Gravity`]).
fn owner_geometry<'s>(
    breakage: &mut Breakage,
    pose: Transform,
    sources: &[&'s SweepSource],
    dt: f32,
    field: Option<&mut (fracture::parts::Assessment, fracture::parts::Gravity)>,
) -> Vec<(&'s SweepSource, super::super::sweep::Sweep)> {
    let inverse = pose.rotation.inverse();
    let pool = ComputeTaskPool::get();
    // Results retain source order.
    let sweeps: Vec<_> = sources
        .par_splat_map(pool, None, |_, sources| {
            sources
                .iter()
                .map(|source| {
                    breakage.kind.sweep.swept(
                        &breakage.live.structure,
                        &breakage.live.state,
                        &source.shape,
                        super::super::sweep::RigidMotion {
                            position: inverse * (source.motion.position - pose.translation),
                            rotation: inverse * source.motion.rotation,
                            velocity: inverse * source.motion.velocity,
                            acceleration: inverse * source.motion.acceleration,
                            lead: source.motion.lead,
                            centre: inverse * (source.motion.centre - pose.translation),
                            spin: inverse * source.motion.spin,
                            dt,
                        },
                    )
                })
                .collect::<Vec<_>>()
        })
        .into_iter()
        .flatten()
        .collect();
    let mut patches: Vec<_> = sources.iter().copied().zip(sweeps).collect();
    let kind = &breakage.kind;
    let Live {
        structure,
        state,
        assessment,
        gravity,
        ..
    } = &mut *breakage.live;
    if patches.iter().any(|(_, swept)| swept.witness.is_some()) {
        let preload = match field {
            Some((current, preloads)) => preloads.get(structure, state, current),
            None => gravity.get(structure, state, assessment),
        };
        // Body patches read the same topology and preload; only their ordered effects interact.
        patches.par_splat_map_mut(pool, None, |_, patches| {
            for (source, swept) in patches {
                if let Some(witness) = swept.witness {
                    kind.sweep.resolve(
                        structure,
                        state,
                        preload,
                        swept,
                        source.effective_mass(witness, &pose),
                        inverse * source.drive,
                    );
                } // An empty patch has no contact response to resolve.
            }
        });
    }
    patches
}

/// `body`'s one plastic exchange against every plug it released this tick, applied to it: the
/// exchange and the source centre it is about.
fn exchange_source(
    p: &mut Intake,
    body: Entity,
    plugs: &[(Entity, Cut)],
) -> (super::super::sweep::Exchange, DVec3) {
    let parts = plug_parts(p, plugs);
    let (mut forces, _, mass, inertia, com, axes, _) = p
        .bodies
        .get_mut(body)
        .expect("sweep source survives intake");
    let centre = forces.position().0 + forces.rotation().0 * com.0;
    let axes = axes.copied().unwrap_or_default(); // Absence is Avian's unrestricted body state.
    let linear = free_translation(axes);
    let angular = Mat3::from_diagonal(Vec3::new(
        f32::from(!axes.is_rotation_x_locked()),
        f32::from(!axes.is_rotation_y_locked()),
        f32::from(!axes.is_rotation_z_locked()),
    ));
    let source = super::super::sweep::Body {
        centre: centre.as_dvec3(),
        inverse_mass: (linear * mass.inverse()).as_dvec3(),
        inverse_inertia: (angular
            * inertia.rotated(forces.rotation().0).inverse().to_mat3()
            * angular)
            .as_dmat3(),
        velocity: forces.linear_velocity().as_dvec3(),
        spin: forces.angular_velocity().as_dvec3(),
    };
    let result = super::super::sweep::exchange(&source, &parts);
    forces.apply_linear_impulse_at_point(-result.impulse.as_vec3(), centre);
    forces.apply_angular_impulse(-result.angular_impulse.as_vec3());
    #[cfg(test)]
    p.account.source_impulses.push(SourceImpulse {
        body,
        linear: -result.impulse.as_vec3(),
        angular: -result.angular_impulse.as_vec3(),
    });
    p.account.source_changes.push((
        body,
        source.velocity.as_vec3(),
        forces.linear_velocity(),
        (source.velocity.as_vec3() - forces.linear_velocity()) * mass.value(),
    ));
    p.account
        .exchanges
        .push((body, result.impulse.as_vec3(), result.dissipated));
    (result, source.centre)
}

/// Every plug's occupied parts, in world frame.
fn plug_parts(p: &Intake, plugs: &[(Entity, Cut)]) -> Vec<super::super::sweep::Part> {
    plugs
        .iter()
        .flat_map(|(root, cut)| {
            let (_, position, rotation, breakage) = p
                .constructions
                .get(*root)
                .expect("sweep owner survives intake");
            let pose = breakage.physics_frame((position, rotation));
            cut.plug
                .iter()
                .map(|&c| breakage.kind.sweep.part(c, &pose))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A departure's one plastic exchange against the plugs it released this tick, applied to its
/// launch before it spawns.
fn exchange_departure(
    p: &Intake,
    prepared: &mut PreparedDeparture,
    plugs: &[(Entity, Cut)],
) -> (super::super::sweep::Exchange, DVec3) {
    let parts = plug_parts(p, plugs);
    let Departure { root, launch, .. } = &mut prepared.departure;
    let props = &launch.mass_properties;
    let rotation = root.rotation;
    let position = root.transform_point(launch.centroid);
    let inertia = avian3d::prelude::ComputedAngularInertia::new_with_local_frame(
        props.principal_angular_inertia,
        props.local_inertial_frame,
    );
    let source = super::super::sweep::Body {
        centre: (position + rotation * props.center_of_mass).as_dvec3(),
        inverse_mass: DVec3::splat(1.0 / f64::from(props.mass)),
        inverse_inertia: inertia.rotated(rotation).inverse().to_mat3().as_dmat3(),
        velocity: (rotation * launch.velocity).as_dvec3(),
        spin: (rotation * launch.spin).as_dvec3(),
    };
    let result = super::super::sweep::exchange(&source, &parts);
    let velocity = source.velocity - source.inverse_mass * result.impulse;
    let spin = source.spin - source.inverse_inertia * result.angular_impulse;
    launch.velocity = rotation.inverse() * velocity.as_vec3();
    launch.spin = rotation.inverse() * spin.as_vec3();
    (result, source.centre)
}

/// Departures prepare per owner, from its cuts and each cut's exchanged motion; ledger, queue
/// and account then commit in (body, owner) order. Returns the departures that leave as server
/// bodies, each under the entity it spawns as.
fn commit(
    p: &mut Intake,
    departing: &BTreeMap<Entity, Vec<(Entity, Cut)>>,
    exchanges: &BTreeMap<Entity, (super::super::sweep::Exchange, DVec3)>,
    tick: u32,
) -> Vec<(Entity, PreparedDeparture)> {
    let mut fresh = Vec::new();
    let mut prepared = bevy::utils::Parallel::<Vec<_>>::default();
    let children = &p.children;
    let chunk_entities = &p.chunk_entities;
    p.constructions
        .par_iter_many_unique_mut(departing.keys())
        .for_each(|(root, position, rotation, mut breakage)| {
            let pose = breakage.physics_frame((position, rotation));
            // A construction without children has no chunk collider to rehome.
            let reusable = reusable_colliders(children.get(root).ok(), chunk_entities);
            let released: Vec<_> = departing[&root]
                .iter()
                .map(|(body, cut)| {
                    let (result, centre) = &exchanges[body];
                    let mut record = Record::default();
                    let (departures, sheds) = sweep_departure(
                        &mut breakage,
                        pose,
                        cut,
                        ReleasedMotion {
                            velocity: result.velocity.as_vec3(),
                            spin: result.spin.as_vec3(),
                            centre: centre.as_vec3(),
                        },
                        tick,
                        &mut record,
                        &reusable,
                    );
                    (
                        *body,
                        root,
                        cut.plug.clone(),
                        departures,
                        sheds,
                        record.entries,
                    )
                })
                .collect();
            prepared.scope(|local| local.extend(released));
        });
    let mut prepared: Vec<_> = prepared.drain().collect();
    assert_eq!(
        prepared.len(),
        departing.values().map(Vec::len).sum::<usize>(),
        "every owner of a committed plug stands for its departure"
    );
    prepared.sort_unstable_by_key(|(body, root, ..)| (*body, *root));
    for (body, root, plug, departures, sheds, entries) in prepared {
        let (_, position, rotation, breakage) = p
            .constructions
            .get(root)
            .expect("a prepared sweep owner survives its ordered commit");
        let pose = breakage.physics_frame((position, rotation));
        p.record.entries.extend(entries);
        p.queue.0.push(RawDeposit {
            changed: vec![root],
            ..default()
        });
        // One ordered reduction accounts for all freed matter, on server bodies or decor.
        let (result, centre) = &exchanges[&body];
        for c in plug {
            let part = breakage.kind.sweep.part(c, &pose);
            let velocity = result.velocity + result.spin.cross(part.centre - *centre);
            p.account.freed += 1;
            p.account
                .chunks
                .push((breakage.construction, c as u16, velocity.as_vec3()));
            p.account.outgoing_momentum += (velocity * part.mass).as_vec3();
            p.account.outgoing_kinetic_j += 0.5
                * (part.mass * velocity.length_squared()
                    + result.spin.dot(part.inertia * result.spin));
        }
        p.account.shed += sheds.len();
        p.shed.write_batch(sheds);
        fresh.extend(
            departures
                .into_iter()
                .map(|departure| (p.commands.spawn_empty().id(), departure)),
        );
    }
    fresh
}

/// A rigid velocity field: `velocity + spin × (x − centre)`, in the frame its user names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ReleasedMotion {
    pub(crate) velocity: Vec3,
    pub(crate) spin: Vec3,
    pub(crate) centre: Vec3,
}

impl ReleasedMotion {
    pub(crate) fn at_rest(centre: Vec3) -> Self {
        Self {
            velocity: Vec3::ZERO,
            spin: Vec3::ZERO,
            centre,
        }
    }
}

/// A cut's swept matter inherits its exchange; matter losing its last support starts at rest.
/// Below-floor merging (`crumble::merge_fragments`) can join both: project their combined linear
/// and angular momentum onto one rigid field, using occupied mass properties. Pure fields require
/// no mass-matrix reduction.
pub(super) fn fragment_motion(
    kind: &ConstructionKind,
    pose: &Transform,
    group: &[usize],
    driven: &BTreeSet<usize>,
    motion: ReleasedMotion,
) -> ReleasedMotion {
    let count = group.iter().filter(|&&c| driven.contains(&c)).count();
    if count == group.len() {
        return motion;
    }
    if count == 0 {
        return ReleasedMotion::at_rest(pose.translation);
    }
    let parts: Vec<_> = group.iter().map(|&c| kind.sweep.part(c, pose)).collect();
    let mass: f64 = parts.iter().map(|p| p.mass).sum();
    let centre: DVec3 = parts.iter().map(|p| p.centre * p.mass).sum::<DVec3>() / mass;
    let mut inertia = DMat3::ZERO;
    let mut momentum = DVec3::ZERO;
    let mut angular = DVec3::ZERO;
    let spin = motion.spin.as_dvec3();
    for (&c, part) in group.iter().zip(parts) {
        let r = part.centre - centre;
        inertia += part.inertia_about(centre);
        if driven.contains(&c) {
            let velocity =
                motion.velocity.as_dvec3() + spin.cross(part.centre - motion.centre.as_dvec3());
            let p = part.mass * velocity;
            momentum += p;
            angular += part.inertia * spin + r.cross(p);
        }
    }
    let inverse = inertia.inverse();
    assert!(
        inverse.is_finite(),
        "admitted occupied solids give a merged fragment invertible inertia"
    );
    ReleasedMotion {
        velocity: (momentum / mass).as_vec3(),
        spin: (inverse * angular).as_vec3(),
        centre: centre.as_vec3(),
    }
}

/// Commit `plug`'s cut before collider reconciliation. Connectivity already identifies
/// unsupported matter; it needs no standing collider or later force solve to establish that it
/// is free.
pub(super) fn cut_fragments(breakage: &mut Breakage, plug: Vec<usize>, tick: u32) -> Cut {
    let kind = &breakage.kind;
    let mut parted = Vec::new();
    let mut entries = Vec::new();
    for j in kind.sweep.cut(&plug) {
        if !breakage.live.state.separated[j] {
            parted.push(j);
            breakage.live.state.cracked[j] = true;
            breakage.live.state.separated[j] = true;
            let joint = &breakage.live.structure.joints[j];
            breakage.live.swapped[usize::from(kind.layout.element_of[joint.a])] = true;
            if let Side::Chunk(b) = joint.b {
                breakage.live.swapped[usize::from(kind.layout.element_of[b])] = true;
            }
            entries.push(Entry {
                construction: breakage.construction,
                event: Event::Separate(j as u16),
                tick,
            });
        }
    }
    let mut fragments =
        crate::debris::crumble::fragments(&breakage.live.structure, &breakage.live.state, &plug);
    for &c in &plug {
        breakage.live.state.fate[c] = Fate::Fallen;
    }
    fragments.extend(fracture::loose_groups(
        &breakage.live.structure,
        &breakage.live.state,
    ));
    let fragments = crate::debris::crumble::merge_fragments(
        &breakage.live.structure,
        fragments,
        &parted,
        &mut breakage.live.rubble_joints,
    );
    for &c in fragments.iter().flatten() {
        breakage.live.state.fate[c] = Fate::Fallen;
        breakage.live.swapped[usize::from(kind.layout.element_of[c])] = true;
    }
    Cut {
        plug,
        fragments,
        entries,
    }
}

/// Prepare a cut's departures ([`cut_fragments`]) before physics: the swept plug `driven`
/// inherits the hull's exchanged `motion`, and matter losing its last support starts at rest.
/// Size routing and below-floor merging share this event; Avian owns later contacts between the
/// resulting bodies.
pub(super) fn sweep_departure(
    breakage: &mut Breakage,
    pose: Transform,
    cut: &Cut,
    motion: ReleasedMotion,
    tick: u32,
    record: &mut Record,
    reusable: &ReusableColliders,
) -> (Vec<PreparedDeparture>, Vec<crate::debris::crumble::Shed>) {
    record.entries.extend(cut.entries.iter().cloned());
    let driven: BTreeSet<_> = cut.plug.iter().copied().collect();
    let kind = &breakage.kind;
    // Routing sums a fragment's mass and size, in fragment order with its ledger entries; only a
    // server body's preparation, which builds a convex hull per chunk, takes a task.
    let mut kept = Vec::new();
    let mut sheds = Vec::new();
    for group in &cut.fragments {
        let motion = fragment_motion(kind, &pose, group, &driven, motion);
        let (bodies, decor) = crate::debris::crumble::route(
            &breakage.live.structure,
            (breakage.construction, pose),
            vec![group.clone()],
            |light| vec![motion; light.len()],
            tick,
            record,
        );
        kept.extend(bodies.into_iter().map(|group| (group, motion)));
        sheds.extend(decor);
    }
    // Root state changes only after every worker has finished reading it.
    let departures = ComputeTaskPool::get().scope(|scope| {
        let breakage = &*breakage;
        for (group, motion) in kept {
            scope.spawn(async move {
                let mut launch = launch(kind, &breakage.live.structure, &group, &[]);
                let at = pose.transform_point(launch.centre);
                launch.velocity = pose.rotation.inverse()
                    * (motion.velocity + motion.spin.cross(at - motion.centre));
                launch.spin = pose.rotation.inverse() * motion.spin;
                prepare_group(
                    Departure {
                        kind: kind.clone(),
                        construction: breakage.construction,
                        root: pose,
                        live: breakage.live.group(&group),
                        group,
                        launch,
                        tick,
                    },
                    reusable,
                )
            });
        }
    });
    crate::debris::crumble::mark_dust(&mut breakage.live.state, &sheds);
    (departures, sheds)
}

pub(super) fn changed(account: Res<SweepAccount>) -> bool {
    account.freed > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless_test::{
        FIXED_TICKS_PER_SECOND, booted_driving_grid, drive_tick, driving_plane, start_fixed_clock,
    };

    /// The game's environment destruction: a cut reads the force its source drives into it, all
    /// Avian integrates besides contacts. A hull its belts hold at rest on level ground drives
    /// nothing up or down, its support cancelling its weight; the belts read without gravity
    /// would press the hull's whole weight up into any matter above it.
    #[test]
    fn a_hull_resting_on_level_ground_drives_no_vertical_force() {
        let mut app = booted_driving_grid(driving_plane(0.0));
        start_fixed_clock(&mut app);
        let tank = app
            .world_mut()
            .query_filtered::<Entity, (With<crate::tank::Tank>, With<crate::tank::Controlled>)>()
            .single(app.world())
            .expect("one controlled tank");
        // Fixture settling after the spawn drop.
        for _ in 0..FIXED_TICKS_PER_SECOND * 2 {
            drive_tick(&mut app, tank, 0.0, 0.0);
        }
        let weight = app
            .world()
            .get::<avian3d::prelude::ComputedMass>(tank)
            .expect("the tank is a dynamic body")
            .value()
            * app
                .world()
                .resource::<avian3d::prelude::Gravity>()
                .0
                .length();
        let mut vertical = Vec::new();
        for _ in 0..FIXED_TICKS_PER_SECOND {
            drive_tick(&mut app, tank, 0.0, 0.0);
            let account = app.world().resource::<SweepAccount>();
            vertical.extend(
                account
                    .drives
                    .iter()
                    .filter(|(body, _)| *body == tank)
                    .map(|(_, drive)| drive.y),
            );
        }
        let mean = vertical.iter().sum::<f32>() / vertical.len() as f32;
        let peak = vertical.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
        println!(
            "MEASURED resting vertical drive over {} ticks: mean {mean} N, peak {peak} N, weight {weight} N",
            vertical.len()
        );
        assert_eq!(
            vertical.len(),
            FIXED_TICKS_PER_SECOND,
            "a resting hull stays a sweep source every tick"
        );
        // Resolution: the belts' support at rest oscillates about the weight (MEASURED over this
        // second: mean 0.8 %, peak 4.4 % of it); a drive read without gravity is 100 % of it.
        assert!(
            mean.abs() < 0.05 * weight,
            "a resting hull's support cancels its weight: {mean} N against {weight} N"
        );
    }
}
