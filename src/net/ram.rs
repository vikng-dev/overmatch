//! Tracked full-speed ram instrument for the game's environment destruction model, game state only.
//! `scripts/perf/collapse.py --ram` owns `OVERMATCH_RAM`. Default-feature servers derive the
//! run-up from their loaded drive data and resolve ground height at spawn, without view assets.

use avian3d::prelude::{LinearVelocity, Sleeping};
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, ParallelSlice, ParallelSliceMut};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::contact_census::{ContactTick, StepSnapshot};
use super::harness::{LeverError, env_text};
use crate::{
    construction::{
        Body,
        breakage::{Breakage, Piece, SweepAccount},
    },
    contact_transfer::ContactTransfers,
    cost::{
        TickSample, Trace, phases,
        stages::{self, NamedCounts, NamedTimes},
    },
    map::{MapManifest, Prototype},
    scatter::MapInstance,
    spec::TankSpec,
    tank::Tank,
    terrain_grid::{HeightGrid, spawn_pos},
};

const LEVER: &str = "OVERMATCH_RAM";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    instance: String,
    heading_deg: f64,
    /// Operator margin on the ideal constant-top-gear-force acceleration distance.
    runup_margin: f64,
    minimum_speed_fraction: f64,
    /// No-contact limit on the server's ram capture clock (`tick * dt`), from tank spawn;
    /// includes client readiness and the harness lead-in before driving.
    approach_s: f64,
    deadline_s: f64,
    settle_s: f64,
}

impl Recipe {
    fn parse(text: &str) -> Result<Self, LeverError> {
        let invalid = || {
            LeverError::invalid(
                LEVER,
                text,
                "JSON ram recipe: nonempty instance, finite heading_deg, runup_margin >= 1, minimum_speed_fraction in (0,1], positive approach_s/deadline_s/settle_s with settle_s < deadline_s",
            )
        };
        let recipe: Self = serde_json::from_str(text).map_err(|_| invalid())?;
        if recipe.instance.is_empty()
            || !recipe.heading_deg.is_finite()
            || !recipe.runup_margin.is_finite()
            || recipe.runup_margin < 1.0
            || !recipe.minimum_speed_fraction.is_finite()
            || !(0.0 < recipe.minimum_speed_fraction && recipe.minimum_speed_fraction <= 1.0)
            || [recipe.approach_s, recipe.deadline_s, recipe.settle_s]
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0)
            || recipe.settle_s >= recipe.deadline_s
        {
            return Err(invalid());
        }
        Ok(recipe)
    }
}

#[derive(Serialize)]
struct Plan {
    spawn: [f32; 3],
    rotation: [f32; 4],
    runup_m: f32,
    governed_m_s: f32,
    minimum_m_s: f64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum End {
    Rest,
    Deadline,
    WorldChanged,
    ApproachTimeout,
}

#[derive(Resource)]
pub(super) struct Ram {
    recipe: Recipe,
    plan: Option<Plan>,
    started: bool,
    pre_speed: f32,
    impact: Option<Value>,
    impact_tick: Option<u64>,
    resting_s: f64,
    done: Option<End>,
    target: Option<Entity>,
    pub(super) trace: Trace<RamWork>,
}

#[derive(Serialize)]
pub(super) struct RamWork {
    work: NamedCounts,
    contacts: ContactTick,
}

pub(super) fn install(app: &mut App, stats_on: bool) -> Result<(), LeverError> {
    let Some(text) = env_text(LEVER)? else {
        return Ok(());
    };
    let recipe = Recipe::parse(&text)?;
    if super::harness::script().is_some_and(|script| script.spawn_pose.is_some()) {
        return Err(LeverError::invalid(
            LEVER,
            text,
            "one spawn recipe; unset the harness spawn_pose when using OVERMATCH_RAM",
        ));
    }
    if !stats_on {
        return Err(LeverError::invalid(
            LEVER,
            text,
            "set OVERMATCH_SERVER_STATS_S for the ram instrument",
        ));
    }
    install_recipe(app, recipe);
    Ok(())
}

fn install_recipe(app: &mut App, recipe: Recipe) {
    app.insert_resource(Ram::new(recipe));
    app.add_systems(PostStartup, prepare);
    app.add_systems(FixedFirst, sample_speed);
}

/// DERIVED ideal run-up: d = v²/(2a), a = T_peak * top_reduction / (r * mass).
/// It excludes terrain and rolling losses; the recipe's margin buys distance, and measured
/// first-contact speed is the admission gate. It never substitutes an estimated impact speed.
fn runup(spec: &TankSpec, margin: f32) -> (f32, f32) {
    let drive = spec
        .track
        .nominal_transmission_params(&spec.roadwheels)
        .expect("spawn uses an admitted tank transmission");
    let speed = drive.governed_speed(false);
    let top = drive
        .gears_fwd
        .last()
        .expect("transmission admission requires forward gears");
    let acceleration =
        drive.torque_at(drive.peak_torque_rpm) * top / drive.sprocket_radius / spec.mass;
    (margin * speed * speed / (2.0 * acceleration), speed)
}

impl Recipe {
    fn plan(
        &self,
        spec: &TankSpec,
        manifest: &MapManifest,
        height: Option<&HeightGrid>,
    ) -> Result<Plan, LeverError> {
        let invalid = |why| LeverError::invalid(LEVER, &self.instance, why);
        let instance = manifest
            .instances
            .iter()
            .find(|i| i.id == self.instance)
            .ok_or_else(|| invalid("instance must name a construction in the active map"))?;
        if !matches!(
            manifest.prototypes[&instance.prototype],
            Prototype::Construction { .. }
        ) {
            return Err(invalid(
                "instance must name a construction in the active map",
            ));
        }
        let (runup_m, governed_m_s) = runup(spec, self.runup_margin as f32);
        let rotation =
            Quat::from_rotation_y(self.heading_deg.rem_euclid(360.0).to_radians() as f32);
        let direction = rotation * Vec3::NEG_Z;
        let xz = instance.pose.xz() - direction.xz() * runup_m;
        let limit = super::spawn_map::spawn_limit(manifest.extent.half_extent());
        if !runup_m.is_finite() || xz.abs().max_element() > limit {
            return Err(invalid(
                "derived run-up is outside spawn bounds; choose another approach or instance",
            ));
        }
        let spawn = spawn_pos(height, xz);
        Ok(Plan {
            spawn: spawn.to_array(),
            rotation: rotation.to_array(),
            runup_m,
            governed_m_s,
            minimum_m_s: f64::from(governed_m_s) * self.minimum_speed_fraction,
        })
    }
}

/// All synchronous tank/map publishers have flushed before PostStartup; no client is admitted yet.
pub(super) fn prepare(
    mut ram: ResMut<Ram>,
    blueprint: Res<crate::bake::TankBlueprint>,
    manifest: Res<MapManifest>,
    height: Option<Res<HeightGrid>>,
) {
    ram.plan = Some(
        ram.recipe
            .plan(&blueprint.spec, &manifest, height.as_deref())
            .unwrap_or_else(super::harness::refuse_start),
    );
    println!(
        "ram_plan {}",
        json!({"recipe": ram.recipe, "plan": ram.plan})
    );
}

impl Ram {
    /// The capture is armed and unfinished.
    pub(super) fn recording(&self) -> bool {
        self.started && self.done.is_none()
    }

    fn new(recipe: Recipe) -> Self {
        Ram {
            recipe,
            plan: None,
            started: false,
            pre_speed: 0.0,
            impact: None,
            impact_tick: None,
            resting_s: 0.0,
            done: None,
            target: None,
            trace: Trace::default(),
        }
    }

    pub(super) fn pose(&mut self) -> (Vec3, Quat) {
        let plan = self
            .plan
            .as_ref()
            .expect("PostStartup checks the ram plan before the server admits clients");
        self.started = true;
        (plan.spawn.into(), Quat::from_array(plan.rotation))
    }

    pub(super) fn json(&mut self) -> Value {
        let (ticks, sends) = self.trace.drain();
        json!({"recipe": self.recipe, "plan": self.plan, "impact": self.impact,
            "done": self.done, "ticks": ticks, "sends": sends})
    }
}

fn sample_speed(mut ram: ResMut<Ram>, tanks: Query<&LinearVelocity, With<Tank>>) {
    // Before the first tank spawn there is no velocity to sample.
    if let Some(velocity) = tanks.iter().next() {
        ram.pre_speed = velocity.xz().length();
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Census<'w, 's> {
    ram: Option<ResMut<'w, Ram>>,
    tanks: Query<'w, 's, Entity, With<Tank>>,
    pieces: Query<
        'w,
        's,
        (),
        (
            With<Piece>,
            With<Breakage>,
            Without<Sleeping>,
            Without<crate::debris::freeze::Frozen>,
        ),
    >,
    constructions: Query<'w, 's, (Entity, &'static MapInstance, &'static Breakage), With<Body>>,
    contacts: Res<'w, ContactTransfers>,
    sweep: Res<'w, SweepAccount>,
    avian: AvianDiagnostics<'w, 's>,
    capture: ResMut<'w, StepSnapshot>,
}

/// Avian's step diagnostics and the contact state they describe.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct AvianDiagnostics<'w, 's> {
    solver: Res<'w, avian3d::dynamics::solver::SolverDiagnostics>,
    collision: Res<'w, avian3d::collision::CollisionDiagnostics>,
    tree: Res<'w, avian3d::collider_tree::ColliderTreeDiagnostics>,
    graph: Res<'w, avian3d::prelude::ContactGraph>,
    constraints: Res<'w, avian3d::dynamics::solver::constraint_graph::ConstraintGraph>,
    bodies: Query<'w, 's, &'static avian3d::prelude::RigidBody>,
    degrees: Local<'s, bevy::ecs::entity::EntityHashMap<u32>>,
    workers: Local<'s, Vec<AvianContactWork>>,
}

/// Reusable census output owned by one compute-worker batch.
#[derive(Default)]
struct AvianContactWork {
    touching_pairs: u32,
    points: u32,
    degrees: bevy::ecs::entity::EntityHashMap<u32>,
}

impl AvianDiagnostics<'_, '_> {
    /// One collector snapshot of active contacts and prepared constraints, outside the tick clock.
    fn step(&mut self) -> crate::cost::AvianStep {
        use avian3d::dynamics::solver::constraint_graph::COLOR_OVERFLOW_INDEX;
        let us = |d: std::time::Duration| d.as_secs_f64() * 1.0e6;
        let (s, c) = (&*self.solver, &*self.collision);
        let colors = &self.constraints.colors;
        let pairs = self.graph.active_pairs();
        let bodies = &self.bodies;
        let pool = ComputeTaskPool::get();
        let workers = pool.thread_num();
        self.workers.resize_with(workers, AvianContactWork::default);
        // Each batch owns disjoint ranges of both populations and its incidence map.
        self.workers.par_chunk_map_mut(pool, 1, |worker, work| {
            let work = &mut work[0];
            work.touching_pairs = 0;
            work.points = 0;
            work.degrees.clear();
            for pair in &pairs[pairs.len() * worker / workers..pairs.len() * (worker + 1) / workers]
            {
                if pair.is_touching() {
                    work.touching_pairs += 1;
                    work.points += pair
                        .manifolds
                        .iter()
                        .map(|m| m.points.len() as u32)
                        .sum::<u32>();
                }
            }
            for color in colors {
                let constraints = &color.contact_constraints;
                for constraint in &constraints[constraints.len() * worker / workers
                    ..constraints.len() * (worker + 1) / workers]
                {
                    for body in [constraint.body1, constraint.body2] {
                        // Retired bodies can leave the world after solving and before collection.
                        if bodies
                            .get(body)
                            .is_ok_and(avian3d::prelude::RigidBody::is_dynamic)
                        {
                            *work.degrees.entry(body).or_default() += 1;
                        }
                    }
                }
            }
        });
        let (mut touching_pairs, mut points) = (0, 0);
        self.degrees.clear();
        // Incidence for a body can span batches; only the keyed reduction is serial.
        for work in &*self.workers {
            touching_pairs += work.touching_pairs;
            points += work.points;
            for (&body, &degree) in &work.degrees {
                *self.degrees.entry(body).or_default() += degree;
            }
        }
        crate::cost::AvianStep {
            collider_tree_update_us: us(self.tree.update),
            collider_tree_optimize_us: us(self.tree.optimize),
            broad_phase_us: us(c.broad_phase),
            narrow_phase_us: us(c.narrow_phase),
            prepare_constraints_us: us(s.prepare_constraints),
            update_velocity_increments_us: us(s.update_velocity_increments),
            integrate_velocities_us: us(s.integrate_velocities),
            warm_start_us: us(s.warm_start),
            solve_constraints_us: us(s.solve_constraints),
            integrate_positions_us: us(s.integrate_positions),
            relax_velocities_us: us(s.relax_velocities),
            apply_restitution_us: us(s.apply_restitution),
            finalize_us: us(s.finalize),
            store_impulses_us: us(s.store_impulses),
            swept_ccd_us: us(s.swept_ccd),
            broad_phase_candidates: c.broad_phase_candidates,
            pairs: c.contact_count,
            touching_pairs,
            points,
            constraints: s.contact_constraint_count,
            colors: colors
                .iter()
                .filter(|color| !color.contact_constraints.is_empty())
                .count() as u32,
            overflow_constraints: colors[COLOR_OVERFLOW_INDEX].contact_constraints.len() as u32,
            serial_contact_visits: s.serial_contact_visits,
            parallel_contact_visits: s.parallel_contact_visits,
            // No live dynamic endpoint means zero incidence.
            max_dynamic_manifold_degree: self.degrees.values().copied().max().unwrap_or(0),
            stationary_non_touching_pairs: c.stationary_non_touching_pairs,
            stationary_non_touching_queries: c.stationary_non_touching_queries,
            reduction_groups: c.reduction_groups,
            reduction_landing_whole: c.reduction_landing_whole,
            reduction_ceiling_whole: c.reduction_ceiling_whole,
            reduction_overlap_whole: c.reduction_overlap_whole,
            reduction_warm_points_removed: c.reduction_warm_points_removed,
            reduction_cold_points_kept: c.reduction_cold_points_kept,
        }
    }
}

impl Census<'_, '_> {
    /// Whether this tick will reach a row: the census is taken only then.
    pub(super) fn recording(&self) -> bool {
        self.ram.as_deref().is_some_and(Ram::recording)
    }

    pub(super) fn contact_tick(&mut self) -> ContactTick {
        super::contact_census::finish(&mut self.capture, &self.sweep)
    }

    pub(super) fn contact_points(&self) -> usize {
        self.contacts.groups().map(|points| points.len()).sum()
    }

    /// This tick's sample, recorded by the ram and the scheduled collapse alike.
    pub(super) fn sample(
        &mut self,
        tick_us: f64,
        physics_us: f64,
        sleep_us: f64,
        stages: ([f64; stages::COUNT], NamedTimes<{ stages::COUNT }>),
        phases: &phases::Phases,
        dt: f64,
    ) -> TickSample {
        TickSample {
            tick_us,
            physics_us,
            sleep_us,
            stages: NamedTimes::new(&stages::NAMES, stages.0),
            phases_us: phases.times(),
            phases_cpu_us: phases.cpu_times(),
            stages_cpu_us: stages.1,
            tick_cpu_us: stages
                .1
                .values
                .iter()
                .chain(&phases.cpu_times().values)
                .sum(),
            detail: NamedTimes::new(&stages::DETAIL_NAMES, [0.0; stages::DETAIL_COUNT]),
            sweep_sources: self.sweep.sources,
            sweep_casts: self.sweep.work.casts,
            awake: self.pieces.iter().len(),
            tanks: self.tanks.iter().len(),
            dt_s: dt,
            avian: self.avian.step(),
        }
    }

    /// Records a tick of an armed, unfinished capture, returning its row's `collector_us`.
    pub(super) fn record(
        &mut self,
        sample: TickSample,
        work: NamedCounts,
        contacts: ContactTick,
    ) -> Option<&mut f64> {
        let ram = self.ram.as_deref_mut().filter(|ram| ram.recording())?;
        let target = if let Some(target) = ram.target {
            if !self.constructions.contains(target) {
                ram.done = Some(End::WorldChanged);
                return None;
            }
            target
        } else {
            let target = self
                .constructions
                .iter()
                .find(|(_, id, _)| id.id == ram.recipe.instance)
                .expect("the admitted ram construction exists throughout its capture")
                .0;
            ram.target = Some(target);
            target
        };
        let row = ram.trace.push(sample, RamWork { work, contacts });
        let tick = row.tick;
        if ram.impact_tick.is_none() {
            // First contact is the tank's first touch by either path: the hull sweep, which runs
            // before physics and can open the matter it meets so the solver never touches it, or
            // a solver contact. Each reports its closing speed along its contact normal.
            let ordinal = self
                .constructions
                .get(target)
                .expect("the target was checked this tick")
                .2
                .construction;
            let groups: Vec<_> = self.contacts.groups().collect();
            let tanks = &self.tanks;
            let solver = groups
                .par_splat_map(ComputeTaskPool::get(), None, |_, groups| {
                    groups
                        .iter()
                        .flat_map(|points| points.iter())
                        .filter(|point| point.reference == target && tanks.contains(point.other))
                        .map(|point| point.closing_speed)
                        .reduce(f32::max)
                })
                .into_iter()
                .flatten();
            let swept = self
                .sweep
                .contacts
                .iter()
                .filter(|((body, construction), _)| {
                    *construction == ordinal && tanks.contains(*body)
                })
                .map(|(_, &speed)| speed);
            if let Some(normal_speed) = solver.chain(swept).reduce(f32::max) {
                ram.impact_tick = Some(tick);
                ram.impact = Some(
                    json!({"tick": tick, "speed_m_s": ram.pre_speed, "normal_speed_m_s": normal_speed}),
                );
            }
        }
        if let Some(impact_tick) = ram.impact_tick {
            let dt = sample.dt_s;
            ram.resting_s = if sample.awake == 0 {
                ram.resting_s + dt
            } else {
                0.0
            };
            if ram.resting_s >= ram.recipe.settle_s {
                ram.done = Some(End::Rest);
            } else if (tick - impact_tick) as f64 * dt >= ram.recipe.deadline_s {
                ram.done = Some(End::Deadline);
            }
        } else if tick as f64 * sample.dt_s >= ram.recipe.approach_s {
            ram.done = Some(End::ApproachTimeout);
        }
        Some(&mut row.collector_us)
    }
}

#[cfg(test)]
pub(crate) struct FixturePlan {
    pub(crate) instance: String,
    pub(crate) spawn: Vec3,
    pub(crate) rotation: Quat,
    pub(crate) minimum_m_s: f32,
    pub(crate) approach_ticks: usize,
}

/// Headless witnesses use the operator recipe's real parser and planner without enabling pricing.
#[cfg(test)]
pub(crate) fn fixture_plan(app: &App) -> FixturePlan {
    let recipe = Recipe::parse(include_str!("../../scripts/perf/ram.json")).unwrap();
    let plan = recipe
        .plan(
            &app.world().resource::<crate::bake::TankBlueprint>().spec,
            app.world().resource::<MapManifest>(),
            app.world().get_resource::<HeightGrid>(),
        )
        .unwrap();
    FixturePlan {
        instance: recipe.instance,
        spawn: Vec3::from_array(plan.spawn),
        rotation: Quat::from_array(plan.rotation),
        minimum_m_s: plan.minimum_m_s as f32,
        // DERIVED: first contact must arrive within the recipe's approach watchdog.
        approach_ticks: (recipe.approach_s
            / app
                .world()
                .resource::<Time<Fixed>>()
                .timestep()
                .as_secs_f64())
        .ceil() as usize,
    }
}

#[cfg(test)]
mod avian_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Operator typos must refuse the run, rather than produce a different workload.
    #[test]
    fn recipes_reject_unknown_fields_and_invalid_domains() {
        let text = include_str!("../../scripts/perf/ram.json");
        let valid = Recipe::parse(text).unwrap();
        let value: Value = serde_json::from_str(text).unwrap();
        let attested = serde_json::to_value(&valid).unwrap();
        for (key, authored) in value.as_object().unwrap() {
            if authored.is_number() {
                assert_eq!(attested[key].as_f64(), authored.as_f64(), "{key}");
            } else {
                assert_eq!(&attested[key], authored);
            }
        }
        for (field, bad) in [
            ("instance", json!("")),
            ("runup_margin", json!(0.5)),
            ("minimum_speed_fraction", json!(1.1)),
            ("settle_s", json!(0)),
            ("deadline_s", json!(-1)),
            ("approach_s", json!("60")),
            ("drive_s", json!(18)),
            ("typo", json!(1)),
        ] {
            let mut changed = value.clone();
            changed[field] = bad;
            assert!(Recipe::parse(&changed.to_string()).is_err(), "{field}");
        }
        assert_eq!(
            Recipe::parse(&serde_json::to_string(&valid).unwrap()).unwrap(),
            valid
        );
    }

    fn fixture_spec() -> TankSpec {
        ron::from_str(include_str!("../../assets/tiger_1/tiger_1.tank.ron")).unwrap()
    }

    /// Operators get an unusable route refusal before a join can reach the spawn path.
    #[test]
    fn route_admission_refuses_missing_nonconstruction_and_unreachable_instances() {
        let manifest = crate::map::tests::construction_fixture_manifest();
        let mut recipe = Recipe::parse(include_str!("../../scripts/perf/ram.json")).unwrap();
        let spec = fixture_spec();
        for instance in ["missing", "house_0001"] {
            // Fixture: absent and graybox targets.
            recipe.instance = instance.into();
            let error = recipe
                .plan(&spec, &manifest, None)
                .err()
                .expect("unusable target refused")
                .to_string();
            assert!(
                error.contains(LEVER) && error.contains("instance"),
                "{error}"
            );
        }
        recipe.instance = "brick_0001".into(); // Fixture construction from the manifest owner.
        assert!(recipe.plan(&spec, &manifest, None).is_ok());
        recipe.runup_margin = f64::MAX; // Fixture: finite input whose derived run-up cannot fit.
        let error = recipe
            .plan(&spec, &manifest, None)
            .err()
            .expect("unreachable route refused")
            .to_string();
        assert!(error.contains("spawn bounds"), "{error}");
    }

    /// The production installer checks and retains the route without a client, while sampling
    /// waits for spawn. Starting capture at admission would publish a zero-tank prefix to the runner.
    #[test]
    fn startup_retains_the_plan_without_starting_capture() {
        let mut app = App::new();
        let mut recipe = Recipe::parse(include_str!("../../scripts/perf/ram.json")).unwrap();
        recipe.instance = "brick_0001".into(); // Fixture construction in the manifest below.
        app.insert_resource(crate::map::tests::construction_fixture_manifest())
            .insert_resource(crate::bake::TankBlueprint::new(
                "fixture".into(),
                // Fixture: route admission reads spec/terrain only; no rig is spawned.
                crate::bake::CertifiedAsset::fixture(
                    fixture_spec(),
                    crate::bake::TankGeometry {
                        nodes: Vec::new(),
                        by_name: default(),
                        roadwheels: Vec::new(),
                        collision_proxies: Vec::new(),
                        ballistic_volumes: Vec::new(),
                    },
                ),
            ))
            .init_resource::<ContactTransfers>()
            .init_resource::<SweepAccount>();
        install_recipe(&mut app, recipe);
        app.world_mut().run_schedule(PostStartup);
        let ram = app.world().resource::<Ram>();
        assert!(ram.plan.is_some());
        assert!(!ram.started);
        assert!(!record(app.world_mut()));
        let mut ram = app.world_mut().resource_mut::<Ram>();
        let planned = ram.plan.as_ref().unwrap().spawn;
        assert_eq!(ram.pose().0.to_array(), planned);
        assert!(ram.started);
    }

    /// A Battle reset must invalidate an armed observation, rather than certify a quiet new
    /// world as the old construction resting. The runner relies on that refusal.
    #[test]
    fn removed_target_refuses_the_capture() {
        let mut world = World::new();
        let target = world.spawn_empty().id();
        world.despawn(target);
        world.init_resource::<ContactTransfers>();
        world.init_resource::<SweepAccount>();
        let mut ram = armed();
        ram.target = Some(target);
        world.insert_resource(ram);
        assert!(!record(&mut world));
        assert_eq!(world.resource::<Ram>().done, Some(End::WorldChanged));
    }

    /// Runs the census once on an idle tick (fixture: one-second step); whether it captured.
    fn record(world: &mut World) -> bool {
        use bevy::ecs::system::RunSystemOnce;
        world.init_resource::<avian3d::dynamics::solver::SolverDiagnostics>();
        world.init_resource::<avian3d::collision::CollisionDiagnostics>();
        world.init_resource::<avian3d::collider_tree::ColliderTreeDiagnostics>();
        world.init_resource::<avian3d::prelude::ContactGraph>();
        world.init_resource::<avian3d::dynamics::solver::constraint_graph::ConstraintGraph>();
        world.init_resource::<avian3d::prelude::PhysicsLengthUnit>();
        world.init_resource::<avian3d::prelude::TimeToSleep>();
        world.init_resource::<StepSnapshot>();
        world
            .run_system_once(|mut census: Census| {
                let cpu = NamedTimes::new(&stages::NAMES, [0.0; stages::COUNT]);
                let sample = census.sample(
                    0.0,
                    0.0,
                    0.0,
                    ([0.0; stages::COUNT], cpu),
                    &phases::Phases::default(),
                    1.0,
                );
                census
                    .record(
                        sample,
                        crate::cost::stages::Stages::default().take_counts(),
                        ContactTick::default(),
                    )
                    .is_some()
            })
            .unwrap()
    }

    /// A started capture on the shipped recipe; tests set any timing they exercise.
    fn armed() -> Ram {
        let mut ram = Ram::new(Recipe::parse(include_str!("../../scripts/perf/ram.json")).unwrap());
        ram.plan = Some(Plan {
            spawn: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            runup_m: 1.0,
            governed_m_s: 1.0,
            minimum_m_s: 1.0,
        });
        ram.started = true;
        ram
    }

    /// The admission gate reads the speed at first contact, from either path: a tank's sweep
    /// touch on the target alone sets the impact tick, at the sweep's closing speed. Another
    /// body's touch and the tank's touch on another construction are not the ram's impact.
    #[test]
    fn a_sweep_touch_before_any_solver_contact_sets_the_impact_tick() {
        use bevy::tasks::TaskPool;
        ComputeTaskPool::get_or_init(TaskPool::default);
        let mut world = World::new();
        world.init_resource::<ContactTransfers>();
        world.init_resource::<SweepAccount>();
        let mut ram = armed();
        ram.pre_speed = 10.0; // Fixture: FixedFirst's sample of the tank.
        let instance = ram.recipe.instance.clone();
        world.insert_resource(ram);
        let tank = world.spawn(Tank).id();
        let other = world.spawn_empty().id();
        // Fixture: the target is construction 3; construction 4 is another.
        world.spawn((
            Body,
            MapInstance {
                id: instance,
                ordinal: 0,
            },
            Breakage::new(crate::construction::tests::column_construction(), 3),
        ));
        assert!(record(&mut world));
        assert_eq!(world.resource::<Ram>().impact_tick, None);
        // Fixture closing speeds, m/s; only the tank's touch on the target is the impact.
        world.resource_mut::<SweepAccount>().contacts =
            [((tank, 3), 9.5), ((other, 3), 12.0), ((tank, 4), 11.0)].into();
        assert!(record(&mut world));
        let ram = world.resource::<Ram>();
        assert_eq!(ram.impact_tick, Some(2));
        assert_eq!(
            ram.impact,
            Some(json!({"tick": 2, "speed_m_s": 10.0, "normal_speed_m_s": 9.5}))
        );
    }

    /// The runner must receive a named refusal at the approach limit. Contact on the limit
    /// still opens the observation; a mutant that waits or times out after impact fails here.
    #[test]
    fn approach_watchdog_ends_only_an_uncontacted_capture() {
        use bevy::tasks::TaskPool;
        ComputeTaskPool::get_or_init(TaskPool::default);
        for contact_at_limit in [false, true] {
            let mut world = World::new();
            world.init_resource::<ContactTransfers>();
            world.init_resource::<SweepAccount>();
            let mut ram = armed();
            ram.recipe.approach_s = 2.0; // Fixture: two one-second census ticks.
            let instance = ram.recipe.instance.clone();
            world.insert_resource(ram);
            let tank = world.spawn(Tank).id();
            world.spawn((
                Body,
                MapInstance {
                    id: instance,
                    ordinal: 0,
                },
                Breakage::new(crate::construction::tests::column_construction(), 3),
            ));
            assert!(record(&mut world));
            assert_eq!(world.resource::<Ram>().done, None);
            if contact_at_limit {
                // Fixture: first touch arrives exactly at the approach limit.
                world
                    .resource_mut::<SweepAccount>()
                    .contacts
                    .insert((tank, 3), 1.0);
            }
            assert!(record(&mut world));
            if contact_at_limit {
                assert_eq!(world.resource::<Ram>().impact_tick, Some(2));
                assert_ne!(world.resource::<Ram>().done, Some(End::ApproachTimeout));
            } else {
                assert_eq!(
                    world.resource_mut::<Ram>().json()["done"],
                    "approach_timeout"
                );
                assert!(
                    !record(&mut world),
                    "a refused capture records no coasting ticks"
                );
            }
        }
    }

    /// A second vehicle changes distance through its data, without a vehicle-specific route.
    #[test]
    fn runup_scales_with_mass_and_operator_margin() {
        let mut spec = ron::from_str::<crate::spec::TankSpec>(include_str!(
            "../../assets/tiger_1/tiger_1.tank.ron"
        ))
        .unwrap();
        let (distance, speed) = runup(&spec, 1.0); // Fixture: ideal margin.
        spec.mass *= 2.0; // Fixture: otherwise identical heavier vehicle.
        let (heavier, same_speed) = runup(&spec, 1.0);
        assert_eq!(same_speed, speed);
        assert_eq!(heavier, 2.0 * distance);
        // Resolution: allow roundoff across the multiplication and divisions in the distance.
        assert!((runup(&spec, 3.0).0 - 3.0 * heavier).abs() <= 4.0 * f32::EPSILON * heavier);
    }
    /// The game's environment destruction: the shipped tracked recipe accounts for the hull's
    /// sweep exchange and whole-step momentum. A cast body's solver look-ahead against exposed
    /// standing matter must lie in its judged patch; otherwise an unjudged cut could stop the ram.
    /// Overlaps and uncast bodies keep Avian's contacts. Opening latency belongs to
    /// `an_over_capacity_fixture_opens_within_two_contact_ticks`; counts, speeds and advance are observations.
    #[test]
    fn tracked_ram_respects_sweep_contacts_and_accounts_for_momentum() {
        use crate::construction::breakage::{Breakage, SweepAccount};
        use crate::headless_test::{
            FIXED_TICKS_PER_SECOND, booted_sim_on, drive_tick, start_fixed_clock,
        };
        use crate::tank::Controlled;
        use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
        let mut app = booted_sim_on(Some(crate::terrain_grid::tests::shipped_grid()));
        start_fixed_clock(&mut app);
        read_unjudged_contacts(&mut app);
        let tank = app
            .world_mut()
            .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
            .single(app.world())
            .expect("one controlled ram source");
        let plan = fixture_plan(&app);
        let (target, ordinal) = app
            .world_mut()
            .query::<(Entity, &crate::scatter::MapInstance, &Breakage)>()
            .iter(app.world())
            .find(|(_, instance, _)| instance.id == plan.instance)
            .map(|(entity, _, state)| (entity, state.construction))
            .expect("recipe target is spawned");
        app.world_mut().entity_mut(tank).insert((
            Position(plan.spawn),
            Rotation(plan.rotation),
            Transform::from_translation(plan.spawn).with_rotation(plan.rotation),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
        ));
        // Fixture settling after the planner's normal spawn drop, before the driving stimulus.
        for _ in 0..FIXED_TICKS_PER_SECOND * 2 {
            drive_tick(&mut app, tank, 0.0, 0.0);
        }
        let boot = app.world().get::<Breakage>(target).unwrap();
        println!(
            "MEASURED recipe target at rest: chunks={}, joints={}, cracked={}, absent={}",
            boot.live.structure.chunks.len(),
            boot.live.structure.joints.len(),
            boot.live.state.cracked.iter().filter(|&&c| c).count(),
            boot.live
                .state
                .fate
                .iter()
                .filter(|&&f| f != crate::construction::fracture::Fate::Standing)
                .count()
        );
        // Fixture: ticks watched after first contact, throttle held.
        const WATCHED: usize = 60;
        let mut freed = 0;
        let mut freed_by_tick = Vec::new();
        let (mut rounds, mut unswept) = (0, 0);
        let mut entry = None;
        let mut speeds = Vec::new();
        let mut shed = 0;
        let mut counts = (0, 0, 0, 0);
        let mut per_tick = Vec::new();
        let mut solver_contacts = Vec::new();
        let mut initial = None;
        let mut contact = None;
        let mut losses = Vec::new();
        let mut exempted = Vec::new();
        for tick in 0..plan.approach_ticks + WATCHED {
            let before = app.world().get::<LinearVelocity>(tank).unwrap().0;
            let start = app.world().get::<Position>(tank).unwrap().0;
            drive_tick(&mut app, tank, 1.0, 0.0);
            assert_judged_contacts(&app, tick);
            let account = app.world().resource::<SweepAccount>();
            (rounds, unswept) = (rounds.max(account.rounds), unswept + account.unswept);
            if account.contacts.contains_key(&(tank, ordinal)) && contact.is_none() {
                contact = Some(tick);
                let met = account
                    .patches
                    .iter()
                    .filter(|(body, construction, ..)| *body == tank && *construction == ordinal)
                    .map(|(.., witness)| witness.reach)
                    .fold(f32::INFINITY, f32::min);
                entry = Some((start + before * met, before.normalize()));
                println!(
                    "MEASURED first occupied contact: {:?}",
                    account
                        .samples
                        .iter()
                        .filter(
                            |(body, construction, _, _)| *body == tank && *construction == ordinal
                        )
                        .collect::<Vec<_>>()
                );
            }
            if contact.is_some_and(|first| tick <= first + 2) {
                counts.0 += account.work.bounds;
                counts.1 += account.work.casts;
                counts.2 += account.work.joints;
                counts.3 += account.work.queries;
                per_tick.push((tick, account.work));
                let wall_impulse: Vec3 = app
                    .world()
                    .resource::<crate::contact_transfer::ContactTransfers>()
                    .presses()
                    .filter(|(point, _)| point.other == tank && point.reference == target)
                    .map(|(point, _)| point.impulse)
                    .sum();
                solver_contacts.push((
                    tick,
                    app.world().get::<LinearVelocity>(tank).unwrap().0,
                    wall_impulse,
                ));
            }
            // Fixture: the ticks whose hull momentum change is attributed, past the +3 at which
            // the hull meets the house only through rubble.
            if let Some(after) = contact.map(|first| tick - first)
                && (1..=5).contains(&after)
            {
                momentum_sources(&app, tank, (target, ordinal), before).check(after);
            }
            let exempt = app.world().resource::<Unjudged>().exempt;
            if exempt != Vec3::ZERO {
                exempted.push((tick, exempt));
            }
            let chunks = account
                .chunks
                .iter()
                .filter(|(construction, _, _)| *construction == ordinal)
                .count();
            if chunks > 0 {
                let outgoing: Vec3 = account
                    .exchanges
                    .iter()
                    .map(|(_, impulse, _)| *impulse)
                    .sum();
                let scale: f32 = account
                    .exchanges
                    .iter()
                    .map(|(_, impulse, _)| impulse.length())
                    .sum();
                assert!(
                    (account.outgoing_momentum - outgoing).length() < 1e-4 * scale,
                    "actual occupied debris mass carries the exchanged momentum"
                );
                initial.get_or_insert(tick);
                if let Some(after) = contact.map(|first| tick - first)
                    && after <= 5
                {
                    freed_by_tick.push((after, chunks));
                }
                freed += chunks * usize::from(contact.is_some_and(|first| tick <= first + 2));
                shed += account.shed;
                for &(body, before, after, change) in &account.source_changes {
                    if body != tank {
                        continue;
                    }
                    let impulse = account
                        .exchanges
                        .iter()
                        .find(|(source, _, _)| *source == tank)
                        .unwrap()
                        .1;
                    assert!(
                        (change - impulse).length() < 1e-3 * impulse.length(),
                        "swept momentum equals source delta-p before the remaining physics contacts"
                    );
                    if losses.is_empty() {
                        assert!(
                            before.length() >= plan.minimum_m_s,
                            "recipe must reach its declared speed before contact: {} < {}",
                            before.length(),
                            plan.minimum_m_s
                        );
                    }
                    losses.push((before.length(), after.length(), impulse.length()));
                }
            }
            if let Some(after) = contact.map(|first| tick - first)
                && (1..=10).contains(&after)
            {
                let speed = app.world().get::<LinearVelocity>(tank).unwrap().0.length();
                speeds.push((after, speed));
            }
            assert!(
                contact.is_some() || tick + 1 < plan.approach_ticks,
                "the fixture must reach contact within the recipe approach watchdog"
            );
            if contact.is_some_and(|first| tick >= first + WATCHED) {
                break;
            }
        }
        let (met, heading) = entry.expect("the sweep records occupied contact");
        let end = app.world().get::<Position>(tank).unwrap().0;
        println!(
            "MEASURED ram end: advance past first contact by +{WATCHED}={} m, speed={:?}, chunks freed by tick after contact={freed_by_tick:?}, most sweep rounds in a tick={rounds}, departures past the cap={unswept}, first contact={contact:?}, first sweep={initial:?}, counts={counts:?}, work by tick={per_tick:?}",
            (end - met).dot(heading),
            app.world().get::<LinearVelocity>(tank).unwrap().0
        );
        let state = app.world().get::<Breakage>(target).unwrap();
        let gone = state
            .live
            .state
            .fate
            .iter()
            .filter(|&&f| f != crate::construction::fracture::Fate::Standing)
            .count();
        println!(
            "MEASURED tracked ram: first sweep tick={initial:?}, freed in two ticks={freed}, fragments shed as decoration by +{WATCHED}={shed}, speed by ticks after contact={speeds:?}, target chunks absent={gone}, bounds/casts/incident-joint visits/cast-contact queries={counts:?}, speeds and momentum={losses:?}, post-physics velocities and remaining wall impulses={solver_contacts:?}, momentum uncast bodies moved through exposed standing matter, by tick (N s)={exempted:?}"
        );
    }

    /// The hull's momentum change over one tick, N s, by what moved it: the sweep's exchange, its
    /// belts, gravity and each solver contact pair, and what the remainder leaves unattributed.
    /// `through_chunks` is the target's share through chunk colliders rather than element ones, and
    /// `judged` its share through chunks this tick's cast met, whose cut judged them;
    /// `ground` is the rest of the solver's stream into the hull, through bodies that are no
    /// structure: the terrain under its rigid track stops and proxies, which a ramming hull's
    /// pitch can drive into it;
    /// `held` is what the target moved into the rubble bodies over the same step.
    struct MomentumSources {
        change: Vec3,
        sweep: Vec3,
        belts: Vec3,
        gravity: Vec3,
        target: Vec3,
        through_chunks: Vec3,
        judged: Vec3,
        rubble: Vec3,
        ground: Vec3,
        held: Vec3,
        unattributed: Vec3,
    }

    impl MomentumSources {
        fn check(&self, after: usize) {
            println!(
                "MEASURED hull momentum change at impact +{after}, N s: change {}, sweep {}, belts {}, gravity {}, target {} ({} through chunk colliders: {} judged by a cut, {} unjudged), rubble {}, ground {}, unattributed {}; target into rubble {}",
                self.change,
                self.sweep,
                self.belts,
                self.gravity,
                self.target,
                self.through_chunks,
                self.judged,
                self.through_chunks - self.judged,
                self.rubble,
                self.ground,
                self.unattributed,
                self.held
            );
            let terms = [
                self.change,
                self.sweep,
                self.belts,
                self.gravity,
                self.target,
                self.rubble,
                self.ground,
            ];
            let scale: f32 = terms.iter().map(|term| term.length()).sum();
            // Resolution: f32 roundoff over the step's summed contact points.
            assert!(
                self.unattributed.length() < 1e-4 * scale,
                "every source of the hull's momentum change is named at impact +{after}: {} N s left",
                self.unattributed
            );
        }
    }

    /// Solver look-ahead that moved a cast body through exposed standing matter outside its
    /// judged patches, and the impulse into uncast bodies, whose contacts Avian owns. Read before
    /// the laws commit, from the sweep's patches, owner state and Avian's step-start penetration,
    /// independently of the collision hook's predicate. Overlaps remain the solver's responsibility.
    #[derive(Resource, Default)]
    struct Unjudged {
        points: Vec<(Entity, Entity, Vec3, Vec3)>,
        exempt: Vec3,
    }

    fn read_unjudged_contacts(app: &mut App) {
        app.init_resource::<Unjudged>().add_systems(
            FixedPostUpdate,
            read_unjudged
                .after(avian3d::prelude::PhysicsSystems::Writeback)
                .before(crate::construction::breakage::LawSet),
        );
    }

    fn assert_judged_contacts(app: &App, tick: usize) {
        let unjudged = &app.world().resource::<Unjudged>().points;
        assert!(
            unjudged.is_empty(),
            "solver look-ahead resolved standing matter no cut judged at tick {tick}: {unjudged:?}"
        );
    }

    fn read_unjudged(
        transfers: Res<crate::contact_transfer::ContactTransfers>,
        account: Res<crate::construction::breakage::SweepAccount>,
        contacts: Res<avian3d::prelude::ContactGraph>,
        colliders: Query<(
            Option<&crate::construction::Element>,
            Option<&crate::construction::breakage::ChunkCollider>,
        )>,
        standing: Query<
            (
                &crate::construction::breakage::Breakage,
                &avian3d::prelude::Position,
                &avian3d::prelude::Rotation,
            ),
            Without<crate::construction::breakage::Piece>,
        >,
        mut unjudged: ResMut<Unjudged>,
    ) {
        let cast: std::collections::BTreeSet<_> =
            account.drives.iter().map(|(body, _)| *body).collect();
        let (points, exempt): (Vec<crate::contact_transfer::ContactTransfer>, Vec<_>) = transfers
            .groups()
            .flatten()
            .filter(|point| {
                let Ok((breakage, position, rotation)) = standing.get(point.reference) else {
                    return false; // A moving group's contacts are rubble's, not standing matter's.
                };
                if point.other_collider.is_none() || point.impulse == Vec3::ZERO {
                    return false;
                }
                let pose = breakage.physics_frame((position, rotation));
                let at = pose.rotation.inverse() * (point.point - pose.translation);
                let chunk = match colliders
                    .get(point.reference_collider)
                    .expect("a solver point lies on a live collider")
                {
                    (_, Some(chunk)) => usize::from(chunk.0),
                    (Some(element), None) => match crate::construction::breakage::nearest_chunk(
                        &breakage.kind.layout,
                        usize::from(element.0),
                        at,
                    ) {
                        Some(chunk) => chunk,
                        None => return false, // An element that never breaks has no law chunk.
                    },
                    (None, None) => panic!("a standing owner's colliders are elements and chunks"),
                };
                let live = &breakage.live;
                live.state.fate[chunk] == crate::construction::fracture::Fate::Standing
                    && !live.structure.chunks[chunk].buried
                    && !account
                        .patches
                        .iter()
                        .any(|(body, construction, chunks, _)| {
                            *body == point.other
                                && *construction == breakage.construction
                                && chunks.contains(&chunk)
                        })
            })
            .filter(|point| {
                if !cast.contains(&point.other) {
                    return true; // No cast leaves Avian responsible for this body's contacts.
                }
                let (_, pair) = contacts
                    .get(
                        point.reference_collider,
                        point
                            .other_collider
                            .expect("only solver contacts reach this filter"),
                    )
                    .expect("a solver transfer retains its contact pair through writeback");
                let contact = pair
                    .manifolds
                    .iter()
                    .flat_map(|manifold| &manifold.points)
                    .find(|contact| contact.point == point.point)
                    .expect("a solver transfer copies its manifold's world point");
                contact.penetration < 0.0
            })
            .partition(|point| cast.contains(&point.other));
        *unjudged = Unjudged {
            points: (points.into_iter())
                .map(|p| (p.other, p.reference_collider, p.point, p.impulse))
                .collect(),
            exempt: exempt.into_iter().map(|p| p.impulse).sum(),
        };
    }

    fn momentum_sources(
        app: &App,
        tank: Entity,
        (target, ordinal): (Entity, u16),
        before: Vec3,
    ) -> MomentumSources {
        use crate::construction::breakage::SweepAccount;
        let world = app.world();
        let account = world.resource::<SweepAccount>();
        println!(
            "MEASURED cuts the hull met, (incoming m/s, (demand, capacity, threshold, drive)): {:?}",
            account
                .samples
                .iter()
                .filter(|(body, construction, ..)| *body == tank && *construction == ordinal)
                .map(|(_, _, incoming, load)| (incoming, load))
                .collect::<Vec<_>>()
        );
        let transfers = world.resource::<crate::contact_transfer::ContactTransfers>();
        let dt = transfers.dt();
        let mass = world
            .get::<avian3d::prelude::ComputedMass>(tank)
            .unwrap()
            .value();
        let gravity = world.resource::<avian3d::prelude::Gravity>().0 * mass * dt;
        let sweep: Vec3 = account
            .source_impulses
            .iter()
            .filter(|impulse| impulse.body == tank)
            .map(|impulse| impulse.linear)
            .sum();
        let [drive, push] = [&account.drives, &account.pushes].map(|forces| {
            forces
                .iter()
                .find(|(body, _)| *body == tank)
                .expect("the moving hull is a sweep source")
                .1
        });
        let cancelled: Vec3 = account
            .cancelled_belts
            .iter()
            .filter(|belt| belt.body == tank)
            .map(|belt| belt.force)
            .sum();
        // The push enters only the sweep's drive; this step's solver contacts move it.
        let belts = (drive - push - cancelled) * dt - gravity;
        let solver = || {
            transfers
                .groups()
                .flatten()
                .filter(|point| point.other_collider.is_some())
        };
        let on_tank = |structure: bool| -> Vec3 {
            solver()
                .filter(|point| point.other == tank && (point.reference == target) == structure)
                .map(|point| point.impulse)
                .sum()
        };
        let cast: Vec<_> = account
            .patches
            .iter()
            .filter(|(body, construction, ..)| *body == tank && *construction == ordinal)
            .flat_map(|(_, _, chunks, _)| chunks.iter().copied())
            .collect();
        let through = |judged: Option<bool>| -> Vec3 {
            solver()
                .filter(|point| {
                    point.other == tank
                        && point.reference == target
                        && world
                            .get::<crate::construction::breakage::ChunkCollider>(
                                point.reference_collider,
                            )
                            .is_some_and(|chunk| {
                                judged.is_none_or(|j| cast.contains(&usize::from(chunk.0)) == j)
                            })
                })
                .map(|point| point.impulse)
                .sum()
        };
        let (through_chunks, judged) = (through(None), through(Some(true)));
        let held = solver()
            .filter(|point| point.reference == target && point.other != tank)
            .map(|point| point.impulse)
            .sum();
        let solved: Vec3 = transfers
            .solver_groups()
            .flatten()
            .filter_map(|point| {
                if point.other == tank {
                    Some(point.impulse)
                } else if point.reference == tank {
                    Some(-point.impulse)
                } else {
                    None // This constraint moved no momentum into the hull.
                }
            })
            .sum();
        let change = (world.get::<LinearVelocity>(tank).unwrap().0 - before) * mass;
        let (target_impulse, rubble) = (on_tank(true), on_tank(false));
        MomentumSources {
            change,
            sweep,
            belts,
            gravity,
            target: target_impulse,
            through_chunks,
            judged,
            rubble,
            ground: solved - target_impulse - rubble,
            held,
            unattributed: change - sweep - belts - gravity - solved,
        }
    }
}
