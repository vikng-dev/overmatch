use super::*;
use avian3d::prelude::{
    ConstantForce, ConstantLinearAcceleration, ConstantLocalForce, ConstantLocalLinearAcceleration,
};

/// The sweep's per-tick account, available to headless witnesses and operator instruments.
#[derive(Resource, Default, Debug)]
pub(crate) struct SweepAccount {
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
    pub(crate) contacts: BTreeSet<(Entity, u16)>,
    pub(crate) samples: Vec<(Entity, u16, f32, (f32, f32, f32, f32))>,
    pub(super) held: Vec<(Entity, Load)>,
    pub(crate) source_changes: Vec<(Entity, Vec3, Vec3, Vec3)>,
}

struct SweepSource {
    body: Entity,
    hull: Collider,
    position: Vec3,
    rotation: Quat,
    velocity: Vec3,
    centre: Vec3,
    spin: Vec3,
    layers: CollisionLayers,
    roots: BTreeSet<Entity>,
    mass: f32,
    drive: Vec3,
}

/// Independent geometry queries run per construction. Sources commit in entity order because
/// they can compete for the same chunks; a source exchanges against all its freed mass once.
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
}

pub(super) fn run(mut p: Intake) {
    *p.account = SweepAccount::default();
    let dt = p.fixed.timestep().as_secs_f32();
    let tick = p.clock.as_deref().map_or(0, |clock| clock.0.raw());
    let mut candidates = geometry(&p, dt);
    candidates.sort_by_key(|(body, root, _)| (*body, *root));
    let mut by_source: BTreeMap<Entity, BTreeMap<Entity, BTreeSet<usize>>> = BTreeMap::new();
    for (body, root, swept) in candidates {
        if let Some(load) = swept.load {
            let (_, _, _, owner) = p
                .constructions
                .get(root)
                .expect("swept owner survives geometry intake");
            p.account.contacts.insert((body, owner.construction));
            #[cfg(test)]
            p.account.patches.push((
                body,
                owner.construction,
                swept.chunks.clone(),
                swept
                    .witness
                    .expect("a resolved patch has an inward witness"),
            ));
            p.account.samples.push((
                body,
                owner.construction,
                swept.incoming,
                (load.demand, load.capacity, load.threshold, load.drive),
            ));
        }
        if !swept.broken && !swept.chunks.is_empty() {
            hold(&mut p, body, root, &swept, dt);
        }
        p.account.work.bounds += swept.work.bounds;
        p.account.work.casts += swept.work.casts;
        p.account.work.joints += swept.work.joints;
        p.account.work.queries += swept.work.queries;
        by_source
            .entry(body)
            .or_default()
            .entry(root)
            .or_default()
            .extend(swept.chunks.into_iter().filter(|_| swept.broken));
    }
    for (body, owners) in by_source {
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
            let (_, handles, _) = p
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
        .for_each(|(body_entity, handles, sleeping)| {
            let mut sources = Vec::new();
            let Ok((forces, body, mass, _, com, _)) = bodies.get(body_entity) else {
                return; // Unmarked fixed collider roots have no dynamic force state.
            };
            // A newly spawned body's mass is prepared by Avian after this hook; it has no
            // rigid-body response yet. Sleeping contacts retain their force in ContactTransfers.
            if !body.is_dynamic()
                || mass.inverse() == 0.0
                || sleeping
                || (forces.linear_velocity() == Vec3::ZERO
                    && forces.angular_velocity() == Vec3::ZERO)
            {
                return;
            }
            for handle in handles {
                let Ok((_, hull, owner, local, layers, _, _)) = colliders.get(handle) else {
                    continue;
                }; // Sensors are presentation/ballistic occupancy, not contact hulls.
                if !layers.interacts_with(super::super::element_layers()) {
                    continue;
                }
                let position = forces.position().0 + forces.rotation().0 * local.translation;
                let rotation = forces.rotation().0 * local.rotation.0;
                let centre = forces.position().0 + forces.rotation().0 * com.0;
                let motion = super::super::sweep::Motion {
                    hull,
                    position,
                    rotation,
                    centre,
                    velocity: forces.linear_velocity(),
                    spin: forces.angular_velocity(),
                    dt,
                };
                let roots = spatial
                    .aabb_intersections_with_aabb(motion.bounds())
                    .into_iter()
                    .filter_map(|entity| {
                        let Ok((_, _, other, _, _, _, _)) = colliders.get(entity) else {
                            return None;
                        };
                        (other.body != body_entity && constructions.contains(other.body))
                            .then_some(other.body)
                    })
                    .collect::<BTreeSet<_>>();
                if roots.is_empty() {
                    continue;
                }
                sources.push(SweepSource {
                    body: owner.body,
                    hull: hull.clone(),
                    position,
                    rotation,
                    centre,
                    velocity: forces.linear_velocity(),
                    spin: forces.angular_velocity(),
                    layers: *layers,
                    roots,
                    mass: mass.value(),
                    drive: (forces.accumulated_linear_acceleration()
                        + constant_acceleration(
                            drives,
                            body_entity,
                            forces.rotation().0,
                            mass.inverse(),
                        ))
                        * mass.value(),
                });
            }
            gathered.scope(|local| local.extend(sources));
        });
    let mut sources: Vec<_> = gathered.drain().collect();
    sources.sort_by_key(|source| source.body);
    sources
}

fn geometry(p: &Intake, dt: f32) -> Vec<(Entity, Entity, super::super::sweep::Sweep)> {
    let sources = sources(p, dt);
    let constructions = &p.constructions;
    let pool = ComputeTaskPool::get();
    pool.scope(|scope| {
        let roots = sources
            .iter()
            .flat_map(|source| source.roots.iter().copied())
            .collect::<BTreeSet<_>>();
        for (root, position, rotation, breakage) in constructions.iter_many(roots) {
            let sources = &sources;
            scope.spawn(async move {
                let pose = breakage.physics_frame((position, rotation));
                let inverse = pose.rotation.inverse();
                let mut patches: BTreeMap<Entity, super::super::sweep::Sweep> = BTreeMap::new();
                for source in sources.iter().filter(|source| {
                    source.roots.contains(&root)
                        && source.body != root
                        && source.layers.interacts_with(super::super::element_layers())
                }) {
                    let swept = breakage.kind.sweep.swept(
                        &breakage.live.structure,
                        &breakage.live.state,
                        super::super::sweep::Motion {
                            hull: &source.hull,
                            position: inverse * (source.position - pose.translation),
                            rotation: inverse * source.rotation,
                            velocity: inverse * source.velocity,
                            centre: inverse * (source.centre - pose.translation),
                            spin: inverse * source.spin,
                            dt,
                        },
                    );
                    patches.entry(source.body).or_default().merge(swept);
                }
                patches
                    .into_iter()
                    .map(|(body, mut swept)| {
                        let source = sources
                            .iter()
                            .find(|s| s.body == body)
                            .expect("a patch has its source body");
                        breakage.kind.sweep.resolve(
                            &breakage.live.structure,
                            &breakage.live.state,
                            &mut swept,
                            source.mass,
                            inverse * source.drive,
                        );
                        (body, root, swept)
                    })
                    .collect::<Vec<_>>()
            });
        }
    })
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
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
            chunks
                .iter()
                .filter(|&&c| breakage.live.state.standing(c))
                .map(|&c| breakage.kind.sweep.part(c, &pose))
                .collect::<Vec<_>>()
        })
        .collect();
    if parts.is_empty() {
        return;
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
    p.account.source_changes.push((
        body,
        source.velocity.as_vec3(),
        forces.linear_velocity(),
        (source.velocity.as_vec3() - forces.linear_velocity()) * mass.value(),
    ));
    p.account
        .exchanges
        .push((body, result.impulse.as_vec3(), result.dissipated));
    for (root, chunks) in owners {
        let (_, position, rotation, mut breakage) = p
            .constructions
            .get_mut(root)
            .expect("sweep owner survives intake");
        let pose = breakage.physics_frame((position, rotation));
        let chunks: Vec<_> = chunks
            .into_iter()
            .filter(|&c| breakage.live.state.standing(c))
            .collect();
        if chunks.is_empty() {
            continue;
        }
        p.queue.0.push(RawDeposit {
            changed: vec![root],
            ..default()
        });
        for &c in &chunks {
            let velocity = (result.velocity
                + result
                    .spin
                    .cross(breakage.kind.sweep.part(c, &pose).centre - source.centre))
            .as_vec3();
            p.account.freed += 1;
            p.account
                .chunks
                .push((breakage.construction, c as u16, velocity));
        }
        let sheds = sweep_departure(
            &mut breakage,
            pose,
            chunks.clone(),
            ReleasedMotion {
                velocity: result.velocity.as_vec3(),
                spin: result.spin.as_vec3(),
                centre: source.centre.as_vec3(),
            },
            tick,
            &mut p.record,
        );
        // The patch's momentum and energy leave with the players' decoration, on the rigid field
        // the exchange gave every freed chunk.
        for c in chunks {
            let part = breakage.kind.sweep.part(c, &pose);
            let velocity = result.velocity + result.spin.cross(part.centre - source.centre);
            p.account.outgoing_momentum += (velocity * part.mass).as_vec3();
            p.account.outgoing_kinetic_j += 0.5
                * (part.mass * velocity.length_squared()
                    + result.spin.dot(part.inertia * result.spin));
        }
        p.account.shed += sheds.len();
        p.shed.write_batch(sheds);
    }
}

pub(super) struct ReleasedMotion {
    pub(super) velocity: Vec3,
    pub(super) spin: Vec3,
    pub(super) centre: Vec3,
}

/// One freed patch, its rigid velocity field and authoritative tick: the hull makes the hole.
/// Whatever its mass, the patch leaves the server as dust in the record and reaches the players
/// as decoration, one [`crate::debris::crumble::Shed`] per fragment that stays connected; the
/// hull has paid its exchange. The size route is for what the structure law releases and for
/// landings, never for a swept hole.
pub(super) fn sweep_departure(
    breakage: &mut Breakage,
    pose: Transform,
    group: Vec<usize>,
    motion: ReleasedMotion,
    tick: u32,
    record: &mut Record,
) -> Vec<crate::debris::crumble::Shed> {
    let ReleasedMotion {
        velocity,
        spin,
        centre,
    } = motion;
    let kind = breakage.kind.clone();
    for j in kind.sweep.cut(&group) {
        if !breakage.live.state.separated[j] {
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
    let fragments =
        crate::debris::crumble::fragments(&breakage.live.structure, &breakage.live.state, &group);
    for &c in &group {
        record.entries.push(Entry {
            construction: breakage.construction,
            event: Event::Dust(c as u16),
            tick,
        });
        breakage.live.state.fate[c] = Fate::Dust;
        breakage.live.swapped[usize::from(kind.layout.element_of[c])] = true;
    }
    fragments
        .iter()
        .map(|fragment| {
            crate::debris::crumble::Shed::of(
                &breakage.live.structure,
                (breakage.construction, pose),
                fragment,
                (velocity, spin, centre),
                tick,
            )
        })
        .collect()
}

/// A held cut transmits its stopping press through the same intake as solver contacts. Moving
/// to the first surface before braking prevents a strong wall being crossed between solver ticks.
fn hold(p: &mut Intake, body: Entity, root: Entity, swept: &super::super::sweep::Sweep, dt: f32) {
    let witness = swept.witness.expect("a held patch has an occupied contact");
    let (_, _, mass, ..) = p.bodies.get(body).expect("a held source survives intake");
    // The structure check reads every contact as its mean force over the tick, as it reads the
    // solver's; the stop's peak force decided only that the cut holds (`sweep::Geometry::resolve`).
    let stop = mass.value() * witness.speed / dt;
    let (_, position, rotation, owner) = p
        .constructions
        .get(root)
        .expect("a held construction survives intake");
    let pose = owner.physics_frame((position, rotation));
    let normal = pose.rotation * witness.normal;
    let point = pose.translation + pose.rotation * witness.at;
    // Ongoing drive reaches the wall through Avian's contacts. Only the impulse removed
    // before physics needs an extra intake load; counting drive here would duplicate its press.
    let share = stop / swept.chunks.len() as f32;
    for &c in &swept.chunks {
        p.account.held.push((
            root,
            Load {
                chunk: c,
                at: owner.live.structure.chunks[c].centroid,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::ZERO,
                press: witness.normal * share,
                peak: None,
                origin: Origin::Contact {
                    other: body,
                    step: dt,
                },
            },
        ));
    }
    let (mut forces, _, mass, inertia, com, axes) = p
        .bodies
        .get_mut(body)
        .expect("a held source survives intake");
    let mut forces = forces.non_waking();
    let position = forces.position().0;
    let rotation = forces.rotation().0;
    let centre = position + rotation * com.0;
    let velocity = forces.linear_velocity();
    let spin = forces.angular_velocity();
    let turn = Quat::from_scaled_axis(spin * witness.time);
    let lever = turn.inverse() * (point - centre - velocity * witness.time);
    let axes = axes.copied().unwrap_or_default(); // Absence is Avian's unrestricted state.
    let inverse_mass = Vec3::new(
        f32::from(!axes.is_translation_x_locked()),
        f32::from(!axes.is_translation_y_locked()),
        f32::from(!axes.is_translation_z_locked()),
    ) * mass.inverse();
    let response = normal.dot(inverse_mass * normal);
    let closing = velocity.dot(normal).max(0.0);
    if closing > 0.0 {
        assert!(
            response > 0.0,
            "inward translation has an unlocked response"
        );
        forces.apply_linear_impulse(-normal * closing / response);
    }
    let angular_mask = Mat3::from_diagonal(Vec3::new(
        f32::from(!axes.is_rotation_x_locked()),
        f32::from(!axes.is_rotation_y_locked()),
        f32::from(!axes.is_rotation_z_locked()),
    ));
    let inverse_inertia =
        angular_mask * inertia.rotated(rotation).inverse().to_mat3() * angular_mask;
    let torque = lever.cross(normal);
    let angular_response = torque.dot(inverse_inertia * torque);
    let angular_closing = (forces.linear_velocity() + spin.cross(lever))
        .dot(normal)
        .max(0.0);
    if angular_closing > 0.0 {
        assert!(
            angular_response > 0.0,
            "inward rotation has an unlocked response"
        );
        forces.apply_angular_impulse(-torque * angular_closing / angular_response);
    }
    // Pre-integration pose accounts only for motion removed by the surface constraint. Avian
    // still integrates the surviving tangential twist over the complete step.
    let next_rotation =
        Quat::from_scaled_axis(-forces.angular_velocity() * witness.time) * turn * rotation;
    let next_centre = centre + (velocity - forces.linear_velocity()) * witness.time;
    let next_position = next_centre + next_rotation * (rotation.inverse() * (position - centre));
    if next_position != position || next_rotation != rotation {
        p.commands
            .entity(body)
            .insert((Position(next_position), Rotation(next_rotation)));
    }
}

pub(super) fn changed(account: Res<SweepAccount>) -> bool {
    account.freed > 0
}
