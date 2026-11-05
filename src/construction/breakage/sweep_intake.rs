use super::*;

/// The sweep's per-tick account, available to headless witnesses and operator instruments.
#[derive(Resource, Default, Debug)]
pub(crate) struct SweepAccount {
    pub(crate) freed: usize,
    pub(crate) work: super::super::sweep::Work,
    pub(crate) exchanges: Vec<(Entity, Vec3, f64)>,
    pub(crate) chunks: Vec<(u16, u16, Vec3)>,
    pub(crate) outgoing_kinetic_j: f64,
    pub(crate) outgoing_momentum: Vec3,
    pub(crate) contacts: BTreeSet<(Entity, u16)>,
    pub(crate) samples: Vec<(Entity, u16, f32, Vec<(usize, f32, f32, f32)>)>,
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
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Intake<'w, 's> {
    constructions: Owners<'w, 's>,
    bodies: Bodies<'w, 's>,
    colliders: Hulls<'w, 's>,
    body_colliders: Query<'w, 's, (Entity, &'static avian3d::prelude::RigidBodyColliders)>,
    spatial: SpatialQuery<'w, 's>,
    transfers: ResMut<'w, ContactTransfers>,
    fixed: Res<'w, Time<Fixed>>,
    clock: Option<Res<'w, ShotClock>>,
    record: ResMut<'w, Record>,
    queue: ResMut<'w, Queue>,
    account: ResMut<'w, SweepAccount>,
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
        if swept.contacts > 0 {
            let (_, _, _, owner) = p
                .constructions
                .get(root)
                .expect("swept owner survives geometry intake");
            p.account.contacts.insert((body, owner.construction));
            p.account
                .samples
                .push((body, owner.construction, swept.incoming, swept.samples));
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
            .extend(swept.chunks);
    }
    let mut departures = Vec::new();
    for (body, owners) in by_source {
        commit_source(&mut p, body, owners, tick, &mut departures);
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
            let (_, handles) = p
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
    let prepared = ComputeTaskPool::get().scope(|scope| {
        for departure in departures {
            scope.spawn(async move { prepare_group(departure) });
        }
    });
    for departure in prepared {
        let launch = &departure.departure.launch;
        p.account.outgoing_momentum +=
            departure.departure.root.rotation * launch.velocity * launch.mass;
        p.account.outgoing_kinetic_j += 0.5
            * (f64::from(launch.mass) * launch.velocity.as_dvec3().length_squared()
                + launch
                    .spin
                    .as_dvec3()
                    .dot(launch.inertia.as_dmat3() * launch.spin.as_dvec3()));
        spawn_prepared(&mut p.commands, departure);
    }
}

fn sources(p: &Intake, dt: f32) -> Vec<SweepSource> {
    let Intake {
        ref constructions,
        ref bodies,
        ref colliders,
        ref body_colliders,
        ref spatial,
        ..
    } = *p;
    let mut gathered = bevy::utils::Parallel::<Vec<SweepSource>>::default();
    body_colliders
        .par_iter()
        .for_each(|(body_entity, handles)| {
            let mut sources = Vec::new();
            let Ok((forces, body, _, _, com, _)) = bodies.get(body_entity) else {
                return; // Unmarked fixed collider roots have no dynamic force state.
            };
            if !body.is_dynamic()
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
                sources
                    .iter()
                    .filter(|source| {
                        source.roots.contains(&root)
                            && source.body != root
                            && source.layers.interacts_with(super::super::element_layers())
                    })
                    .map(|source| {
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
                        (source.body, root, swept)
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
    departures: &mut Vec<Departure>,
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
        for c in chunks {
            let velocity = (result.velocity
                + result
                    .spin
                    .cross(breakage.kind.sweep.part(c, &pose).centre - source.centre))
            .as_vec3();
            departures.push(sweep_departure(
                &mut breakage,
                pose,
                c,
                velocity,
                result.spin.as_vec3(),
                tick,
                &mut p.record,
            ));
            p.account.freed += 1;
            p.account
                .chunks
                .push((breakage.construction, c as u16, velocity));
        }
    }
}

/// Departure boundary for the decor layer: stable chunk identity, world velocity and birth tick.
/// Until presentation routing replaces it, each chunk uses the existing debris body path.
pub(super) fn sweep_departure(
    breakage: &mut Breakage,
    pose: Transform,
    c: usize,
    velocity: Vec3,
    spin: Vec3,
    tick: u32,
    record: &mut Record,
) -> Departure {
    let kind = breakage.kind.clone();
    let group = vec![c];
    let mut live = breakage.live.group(&group);
    record.entries.push(Entry {
        construction: breakage.construction,
        event: Event::Wear(c as u16),
        tick,
    });
    for &j in kind.sweep.joints(c) {
        if !breakage.live.state.separated[j] {
            breakage.live.state.cracked[j] = true;
            breakage.live.state.separated[j] = true;
            live.state.cracked[j] = true;
            live.state.separated[j] = true;
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
    breakage.live.state.fate[c] = Fate::Fallen;
    breakage.live.swapped[usize::from(kind.layout.element_of[c])] = true;
    let mut launch = launch(&kind, &live.structure, &group, &[]);
    launch.velocity = pose.rotation.inverse() * velocity;
    launch.spin = pose.rotation.inverse() * spin;
    Departure {
        kind,
        construction: breakage.construction,
        root: pose,
        live,
        group,
        launch,
        tick,
    }
}

pub(super) fn changed(account: Res<SweepAccount>) -> bool {
    account.freed > 0
}
