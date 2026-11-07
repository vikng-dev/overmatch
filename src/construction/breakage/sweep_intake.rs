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
    /// Free angular impulse, about the source COM, rather than `point cross linear`.
    pub(crate) angular: Vec3,
}

/// The sweep's per-tick account, available to headless witnesses and operator instruments.
#[derive(Resource, Default, Debug)]
pub(crate) struct SweepAccount {
    #[cfg(test)]
    pub(crate) source_impulses: Vec<SourceImpulse>,
    #[cfg(test)]
    pub(crate) cancelled_belts: Vec<crate::contact_transfer::AppliedContact>,
    pub(crate) sources: usize,
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

/// Independent geometry queries run per construction. Sources commit in entity order; a later
/// source's plug on an owner an earlier one changed this tick is resolved again on the state that
/// commit left, so every committed plug is a minimum cut for its own demand. A source exchanges
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
    body_colliders: Query<
        'w,
        's,
        (
            Entity,
            &'static avian3d::prelude::RigidBodyColliders,
            Has<avian3d::prelude::Sleeping>,
            Has<Piece>,
            Option<&'static super::super::sweep::SourceShape>,
        ),
    >,
    spatial: SpatialQuery<'w, 's>,
    transfers: ResMut<'w, ContactTransfers>,
    fixed: Res<'w, Time<Fixed>>,
    clock: Option<Res<'w, ShotClock>>,
    record: ResMut<'w, Record>,
    queue: ResMut<'w, Queue>,
    account: ResMut<'w, SweepAccount>,
    shed: MessageWriter<'w, crate::debris::crumble::Shed>,
    commands: Commands<'w, 's>,
    children: Query<'w, 's, &'static Children>,
    chunk_entities: ChunkChildren<'w, 's>,
}

pub(super) fn run(mut p: Intake) {
    *p.account = SweepAccount::default();
    let dt = p.fixed.timestep().as_secs_f32();
    let tick = p.clock.as_deref().map_or(0, |clock| clock.0.raw());
    let sources = sources(&p, dt);
    p.account.sources = sources.len();
    let mut candidates = geometry(&mut p, dt, &sources);
    candidates.sort_by_key(|(body, root, _)| (*body, *root));
    let mut by_source: BTreeMap<Entity, BTreeMap<Entity, BTreeSet<usize>>> = BTreeMap::new();
    for (body, root, swept) in candidates {
        if let Some(load) = swept.load {
            let (_, _, _, owner) = p
                .constructions
                .get(root)
                .expect("swept owner survives geometry intake");
            let witness = load.witness;
            p.account
                .contacts
                .entry((body, owner.construction))
                .and_modify(|speed| *speed = speed.max(witness.speed))
                .or_insert(witness.speed);
            #[cfg(test)]
            p.account
                .patches
                .push((body, owner.construction, swept.chunks.clone(), witness));
            p.account.samples.push((
                body,
                owner.construction,
                swept.incoming,
                (load.demand, load.capacity, load.threshold, load.drive),
            ));
        }
        p.account.work.bounds += swept.work.bounds;
        p.account.work.casts += swept.work.casts;
        p.account.work.joints += swept.work.joints;
        p.account.work.queries += swept.work.queries;
        p.account.work.flow += swept.work.flow;
        by_source
            .entry(body)
            .or_default()
            .entry(root)
            .or_default()
            .extend(swept.chunks.into_iter().filter(|_| swept.broken));
    }
    let mut committed = BTreeSet::new();
    for (body, mut owners) in by_source {
        let source = sources
            .iter()
            .find(|source| source.body == body)
            .expect("a candidate body is a sweep source");
        for (&root, plug) in &mut owners {
            if committed.contains(&root) {
                *plug = resolve_again(&mut p, root, source, dt);
            }
        }
        owners.retain(|_, plug| !plug.is_empty());
        committed.extend(owners.keys().copied());
        commit_source(&mut p, body, owners, tick);
    }
    let changed = p
        .queue
        .0
        .iter()
        .flat_map(|deposit| deposit.changed.iter().copied())
        .collect::<BTreeSet<_>>();
    let retired = changed
        .into_iter()
        .flat_map(|root| {
            let (_, handles, ..) = p
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
        ref body_colliders,
        ref drives,
        ref spatial,
        ..
    } = *p;
    let mut gathered = bevy::utils::Parallel::<Vec<SweepSource>>::default();
    body_colliders
        .par_iter()
        .for_each(|(body_entity, handles, sleeping, piece, cached)| {
            let Ok((forces, body, mass, inertia, com, axes)) = bodies.get(body_entity) else {
                return; // Unmarked fixed collider roots have no dynamic force state.
            };
            // The sweep requires finite computed mass. Sleeping contacts retain their force
            // in ContactTransfers; Avian owns any source this intake does not prepare.
            if !body.is_dynamic()
                || mass.inverse() == 0.0
                || sleeping
                || (forces.linear_velocity() == Vec3::ZERO
                    && forces.angular_velocity() == Vec3::ZERO)
            {
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
            let rotation = forces.rotation().0;
            let centre = position + rotation * com.0;
            let motion = super::super::sweep::RigidMotion {
                position,
                rotation,
                centre,
                velocity: forces.linear_velocity(),
                spin: forces.angular_velocity(),
                dt,
            };
            let roots = spatial
                .aabb_intersections_with_aabb(shape.bounds(motion))
                .into_iter()
                .filter_map(|entity| {
                    let Ok((_, _, other, _, _, _, _)) = colliders.get(entity) else {
                        return None;
                    };
                    (other.body != body_entity && constructions.contains(other.body))
                        .then_some(other.body)
                })
                .collect();
            let source = SweepSource {
                body: body_entity,
                shape,
                motion,
                roots,
                mass: mass.value(),
                inertia: *inertia,
                axes: axes.copied().unwrap_or_default(), // Absence is Avian's unrestricted state.
                drive: (forces.accumulated_linear_acceleration()
                    + constant_acceleration(drives, body_entity, rotation, mass.inverse()))
                    * mass.value(),
            };
            gathered.scope(|local| local.push(source));
        });
    let mut sources: Vec<_> = gathered.drain().collect();
    sources.sort_by_key(|source| source.body);
    sources
}

fn geometry(
    p: &mut Intake,
    dt: f32,
    sources: &[SweepSource],
) -> Vec<(Entity, Entity, super::super::sweep::Sweep)> {
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
            // This pass mutates only derived preload storage, never collider or ledger state.
            let breakage = breakage.bypass_change_detection();
            let pose = breakage.physics_frame((position, rotation));
            let own: Vec<_> = sources
                .iter()
                .filter(|source| source.roots.contains(&root))
                .collect();
            let results = owner_geometry(breakage, pose, &own, dt)
                .into_iter()
                .map(|(body, swept)| (body, root, swept));
            gathered.scope(|local| local.extend(results));
        });
    gathered.drain().collect()
}

/// Each of `sources`' patches on one owner, resolved against the owner's current state.
fn owner_geometry(
    breakage: &mut Breakage,
    pose: Transform,
    sources: &[&SweepSource],
    dt: f32,
) -> Vec<(Entity, super::super::sweep::Sweep)> {
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
        let preload = gravity.get(structure, state, assessment);
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
        .into_iter()
        .map(|(source, swept)| (source.body, swept))
        .collect()
}

/// `body`'s plug on an owner an earlier source changed this tick, resolved again on the state that
/// commit left: empty when its cut now holds.
fn resolve_again(p: &mut Intake, root: Entity, source: &SweepSource, dt: f32) -> BTreeSet<usize> {
    let (_, position, rotation, mut breakage) = p
        .constructions
        .get_mut(root)
        .expect("a committed owner stands until refit");
    let breakage = breakage.bypass_change_detection();
    let pose = breakage.physics_frame((position, rotation));
    let (_, swept) = owner_geometry(breakage, pose, &[source], dt)
        .pop()
        .expect("one source resolves one patch");
    p.account.work.bounds += swept.work.bounds;
    p.account.work.casts += swept.work.casts;
    p.account.work.joints += swept.work.joints;
    p.account.work.queries += swept.work.queries;
    p.account.work.flow += swept.work.flow;
    if swept.broken {
        swept.chunks.into_iter().collect()
    } else {
        BTreeSet::new()
    }
}

fn commit_source(
    p: &mut Intake,
    body: Entity,
    owners: BTreeMap<Entity, BTreeSet<usize>>,
    tick: u32,
) {
    let parts: Vec<_> = owners
        .iter()
        .flat_map(|(root, chunks)| {
            let (_, position, rotation, breakage) = p
                .constructions
                .get(*root)
                .expect("sweep owner survives intake");
            let pose = breakage.physics_frame((position, rotation));
            assert!(
                chunks.iter().all(|&c| breakage.live.state.standing(c)),
                "a committed plug was resolved on its owner's current state"
            );
            chunks
                .iter()
                .map(|&c| breakage.kind.sweep.part(c, &pose))
                .collect::<Vec<_>>()
        })
        .collect();
    if parts.is_empty() {
        return; // Every cut this body met held.
    }
    let (mut forces, _, mass, inertia, com, axes) = p
        .bodies
        .get_mut(body)
        .expect("sweep source survives intake");
    let centre = forces.position().0 + forces.rotation().0 * com.0;
    let axes = axes.copied().unwrap_or_default(); // Absence is Avian's unrestricted body state.
    let linear = Vec3::new(
        f32::from(!axes.is_translation_x_locked()),
        f32::from(!axes.is_translation_y_locked()),
        f32::from(!axes.is_translation_z_locked()),
    );
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
    // The source's common exchange is known. Owners prepare their own topology and geometry
    // independently; ledger and structural effects commit in owner order before physics.
    let mut released = bevy::utils::Parallel::<Vec<_>>::default();
    let children = &p.children;
    let chunk_entities = &p.chunk_entities;
    p.constructions
        .par_iter_many_unique_mut(owners.keys())
        .for_each(|(root, position, rotation, mut breakage)| {
            let pose = breakage.physics_frame((position, rotation));
            let chunks: Vec<_> = owners[&root].iter().copied().collect();
            let mut record = Record::default();
            let reusable = reusable_colliders(children.get(root).ok(), chunk_entities);
            let departure = sweep_departure(
                &mut breakage,
                pose,
                chunks.clone(),
                ReleasedMotion {
                    velocity: result.velocity.as_vec3(),
                    spin: result.spin.as_vec3(),
                    centre: source.centre.as_vec3(),
                },
                tick,
                &mut record,
                &reusable,
            );
            released.scope(|local| local.push((root, chunks, departure, record.entries)));
        });
    let mut released: Vec<_> = released.drain().collect();
    assert_eq!(
        released.len(),
        owners.len(),
        "every owner of a committed plug stands for its departure"
    );
    released.sort_unstable_by_key(|(root, ..)| *root);
    for (root, chunks, (departures, sheds), entries) in released {
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
        for c in chunks {
            let part = breakage.kind.sweep.part(c, &pose);
            let velocity = result.velocity + result.spin.cross(part.centre - source.centre);
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
        for departure in departures {
            spawn_prepared(&mut p.commands, departure);
        }
    }
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

/// Commit the cut before collider reconciliation. Connectivity already identifies unsupported
/// matter; it needs no standing collider or later force solve to establish that it is free.
fn cut_fragments(
    breakage: &mut Breakage,
    group: &[usize],
    tick: u32,
    record: &mut Record,
) -> Vec<Vec<usize>> {
    let kind = &breakage.kind;
    let mut parted = Vec::new();
    for j in kind.sweep.cut(group) {
        if !breakage.live.state.separated[j] {
            parted.push(j);
            breakage.live.state.cracked[j] = true;
            breakage.live.state.separated[j] = true;
            let joint = &breakage.live.structure.joints[j];
            breakage.live.swapped[usize::from(kind.layout.element_of[joint.a])] = true;
            if let Side::Chunk(b) = joint.b {
                breakage.live.swapped[usize::from(kind.layout.element_of[b])] = true;
            }
            record.entries.push(Entry {
                construction: breakage.construction,
                event: Event::Separate(j as u16),
                tick,
            });
        }
    }
    let mut fragments =
        crate::debris::crumble::fragments(&breakage.live.structure, &breakage.live.state, group);
    for &c in group {
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
    fragments
}

/// Free the cut and its unsupported remainder together before physics. Size routing and
/// below-floor merging share this event; Avian owns later contacts between the resulting bodies.
pub(super) fn sweep_departure(
    breakage: &mut Breakage,
    pose: Transform,
    group: Vec<usize>,
    motion: ReleasedMotion,
    tick: u32,
    record: &mut Record,
    reusable: &ReusableColliders,
) -> (Vec<PreparedDeparture>, Vec<crate::debris::crumble::Shed>) {
    let fragments = cut_fragments(breakage, &group, tick, record);
    let driven: BTreeSet<_> = group.into_iter().collect();
    // Each fragment prepares independently; scoped results and their ledger entries retain
    // fragment order. Root state changes only after all workers have finished reading it.
    let results = ComputeTaskPool::get().scope(|scope| {
        let breakage = &*breakage;
        let driven = &driven;
        for group in fragments {
            scope.spawn(async move {
                let kind = &breakage.kind;
                let motion = fragment_motion(kind, &pose, &group, driven, motion);
                let mut record = Record::default();
                let (kept, sheds) = crate::debris::crumble::route(
                    &breakage.live.structure,
                    (breakage.construction, pose),
                    vec![group],
                    |light| vec![motion; light.len()],
                    tick,
                    &mut record,
                );
                let departures = kept
                    .into_iter()
                    .map(|group| {
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
                    })
                    .collect::<Vec<_>>();
                (departures, sheds, record.entries)
            });
        }
    });
    let mut departures = Vec::new();
    let mut sheds = Vec::new();
    for (bodies, decor, entries) in results {
        crate::debris::crumble::mark_dust(&mut breakage.live.state, &decor);
        record.entries.extend(entries);
        departures.extend(bodies);
        sheds.extend(decor);
    }
    (departures, sheds)
}

pub(super) fn changed(account: Res<SweepAccount>) -> bool {
    account.freed > 0
}
