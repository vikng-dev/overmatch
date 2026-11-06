//! Tracked full-speed ram instrument for the game's environment destruction model, game state only.
//! `scripts/perf/collapse.py --ram` owns `OVERMATCH_RAM`. Default-feature servers derive the
//! run-up from their loaded drive data and resolve ground height at spawn, without view assets.

use avian3d::prelude::{LinearVelocity, Sleeping};
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, ParallelSlice};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::harness::{LeverError, env_text};
use crate::{
    construction::{
        Body,
        breakage::{Breakage, Piece, SweepAccount},
    },
    contact_transfer::ContactTransfers,
    cost::stages::Stage,
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
    drive_s: f64,
    deadline_s: f64,
    settle_s: f64,
}

impl Recipe {
    fn parse(text: &str) -> Result<Self, LeverError> {
        let invalid = || {
            LeverError::invalid(
                LEVER,
                text,
                "JSON ram recipe: nonempty instance, finite heading_deg, runup_margin >= 1, minimum_speed_fraction in (0,1], positive drive_s/deadline_s/settle_s with settle_s < deadline_s",
            )
        };
        let recipe: Self = serde_json::from_str(text).map_err(|_| invalid())?;
        if recipe.instance.is_empty()
            || !recipe.heading_deg.is_finite()
            || !recipe.runup_margin.is_finite()
            || recipe.runup_margin < 1.0
            || !recipe.minimum_speed_fraction.is_finite()
            || !(0.0 < recipe.minimum_speed_fraction && recipe.minimum_speed_fraction <= 1.0)
            || [recipe.drive_s, recipe.deadline_s, recipe.settle_s]
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

#[derive(Serialize)]
struct Tick {
    tick: u64,
    tick_us: f64,
    physics_us: f64,
    sleep_us: f64,
    intake_us: f64,
    structure_us: f64,
    refit_us: f64,
    publish_us: f64,
    contacts_us: f64,
    sweep_us: f64,
    crumble_us: f64,
    collector_us: f64,
    awake: usize,
    tanks: usize,
    dt_s: f64,
}

#[derive(Serialize)]
struct Send {
    tick: u64,
    us: f64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum End {
    Rest,
    Deadline,
    WorldChanged,
}

#[derive(Resource)]
pub(super) struct Ram {
    recipe: Recipe,
    plan: Option<Plan>,
    started: bool,
    tick: u64,
    pre_speed: f32,
    impact: Option<Value>,
    impact_tick: Option<u64>,
    resting_s: f64,
    done: Option<End>,
    target: Option<Entity>,
    rows: Vec<Tick>,
    sends: Vec<Send>,
    last_send_tick: Option<u64>,
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
    fn new(recipe: Recipe) -> Self {
        Ram {
            recipe,
            plan: None,
            started: false,
            tick: 0,
            pre_speed: 0.0,
            impact: None,
            impact_tick: None,
            resting_s: 0.0,
            done: None,
            target: None,
            rows: Vec::new(),
            sends: Vec::new(),
            last_send_tick: None,
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

    pub(super) fn send(&mut self, us: f64) {
        if self.plan.is_some() && self.tick > 0 && self.last_send_tick != Some(self.tick) {
            self.last_send_tick = Some(self.tick);
            self.sends.push(Send {
                tick: self.tick,
                us,
            });
        }
    }

    pub(super) fn json(&mut self) -> Value {
        json!({"recipe": self.recipe, "plan": self.plan, "impact": self.impact,
            "done": self.done, "ticks": std::mem::take(&mut self.rows), "sends": std::mem::take(&mut self.sends)})
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
    pieces: Query<'w, 's, (), (With<Piece>, With<Breakage>, Without<Sleeping>)>,
    constructions: Query<'w, 's, (Entity, &'static MapInstance, &'static Breakage), With<Body>>,
    contacts: Res<'w, ContactTransfers>,
    sweep: Res<'w, SweepAccount>,
}

impl Census<'_, '_> {
    pub(super) fn collector(&mut self, us: f64) {
        if let Some(row) = self.ram.as_deref_mut().and_then(|ram| ram.rows.last_mut()) {
            row.collector_us = us;
        }
    }

    pub(super) fn record(
        &mut self,
        tick_us: f64,
        physics_us: f64,
        sleep_us: f64,
        stages: [f64; crate::cost::stages::COUNT],
        dt: f64,
    ) -> bool {
        let Some(ram) = self
            .ram
            .as_deref_mut()
            .filter(|r| r.started && r.done.is_none())
        else {
            return false;
        };
        ram.tick += 1;
        let target = if let Some(target) = ram.target {
            if !self.constructions.contains(target) {
                ram.done = Some(End::WorldChanged);
                return false;
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
                        .filter(|point| point.structure == target && tanks.contains(point.body))
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
                ram.impact_tick = Some(ram.tick);
                ram.impact = Some(
                    json!({"tick": ram.tick, "speed_m_s": ram.pre_speed, "normal_speed_m_s": normal_speed}),
                );
            }
        }
        let awake = self.pieces.iter().len();
        if let Some(impact_tick) = ram.impact_tick {
            ram.resting_s = if awake == 0 { ram.resting_s + dt } else { 0.0 };
            if ram.resting_s >= ram.recipe.settle_s {
                ram.done = Some(End::Rest);
            } else if (ram.tick - impact_tick) as f64 * dt >= ram.recipe.deadline_s {
                ram.done = Some(End::Deadline);
            }
        }
        ram.rows.push(Tick {
            tick: ram.tick,
            tick_us,
            physics_us,
            sleep_us,
            intake_us: stages[Stage::Intake as usize],
            structure_us: stages[Stage::Structure as usize],
            refit_us: stages[Stage::Refit as usize],
            publish_us: stages[Stage::Publish as usize],
            contacts_us: stages[Stage::Contacts as usize],
            sweep_us: stages[Stage::Sweep as usize],
            crumble_us: stages[Stage::Crumble as usize],
            collector_us: 0.0,
            awake,
            tanks: self.tanks.iter().len(),
            dt_s: dt,
        });
        true
    }
}

#[cfg(test)]
pub(crate) struct FixturePlan {
    pub(crate) instance: String,
    pub(crate) spawn: Vec3,
    pub(crate) rotation: Quat,
    pub(crate) minimum_m_s: f32,
    pub(crate) drive_ticks: usize,
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
        drive_ticks: (recipe.drive_s
            / app
                .world()
                .resource::<Time<Fixed>>()
                .timestep()
                .as_secs_f64())
        .ceil() as usize,
    }
}

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
            ("drive_s", json!("60")),
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
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        let mut recipe = Recipe::parse(include_str!("../../scripts/perf/ram.json")).unwrap();
        recipe.instance = "brick_0001".into(); // Fixture construction in the manifest below.
        app.insert_resource(crate::map::tests::construction_fixture_manifest())
            .insert_resource(crate::bake::TankBlueprint {
                file: "fixture".into(),
                // Fixture: route admission reads spec/terrain only; no rig is spawned.
                geometry: std::sync::Arc::new(crate::bake::TankGeometry {
                    nodes: Vec::new(),
                    by_name: default(),
                    roadwheels: Vec::new(),
                    collision_proxies: Vec::new(),
                    ballistic_volumes: Vec::new(),
                }),
                spec: std::sync::Arc::new(fixture_spec()),
            })
            .init_resource::<ContactTransfers>()
            .init_resource::<SweepAccount>();
        install_recipe(&mut app, recipe);
        app.world_mut().run_schedule(PostStartup);
        let ram = app.world().resource::<Ram>();
        assert!(ram.plan.is_some());
        assert!(!ram.started);
        assert!(
            !app.world_mut()
                .run_system_once(|mut census: Census| {
                    census.record(0.0, 0.0, 0.0, [0.0; crate::cost::stages::COUNT], 1.0)
                })
                .unwrap()
        );
        let mut ram = app.world_mut().resource_mut::<Ram>();
        let planned = ram.plan.as_ref().unwrap().spawn;
        assert_eq!(ram.pose().0.to_array(), planned);
        assert!(ram.started);
    }

    /// A Battle reset must invalidate an armed observation, rather than certify a quiet new
    /// world as the old construction resting. The runner relies on that refusal.
    #[test]
    fn removed_target_refuses_the_capture() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let target = world.spawn_empty().id();
        world.despawn(target);
        world.init_resource::<ContactTransfers>();
        world.init_resource::<SweepAccount>();
        let mut ram = armed();
        ram.target = Some(target);
        world.insert_resource(ram);
        let captured = world
            .run_system_once(|mut census: Census| {
                census.record(0.0, 0.0, 0.0, [0.0; crate::cost::stages::COUNT], 1.0)
            })
            .unwrap();
        assert!(!captured);
        assert_eq!(world.resource::<Ram>().done, Some(End::WorldChanged));
    }

    /// A started capture on the shipped recipe. Fixture: the route and timing values are
    /// irrelevant to the census's impact and end decisions.
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

    /// The admission gate reads the speed at first contact. The hull sweep runs before physics
    /// and can open all the matter it meets, so the solver touches the target later and slower,
    /// or never: a tank's sweep touch on the target sets the impact tick alone, at the sweep's
    /// closing speed along its normal. Another body's touch and the tank's touch on another
    /// construction are not the ram's impact.
    #[test]
    fn a_sweep_touch_before_any_solver_contact_sets_the_impact_tick() {
        use bevy::ecs::system::RunSystemOnce;
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
        let record = |world: &mut World| {
            world
                .run_system_once(|mut census: Census| {
                    census.record(0.0, 0.0, 0.0, [0.0; crate::cost::stages::COUNT], 1.0)
                })
                .unwrap()
        };
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
    /// The game's environment destruction: the tracked ram recipe must open occupied hull space
    /// within the owner's two-tick contact bar, and the source must lose the momentum its chunks take.
    /// Counts and resulting speeds are observations; the gate pins the owner's latency and conservation.
    #[test]
    fn tracked_ram_sweep_opens_within_two_ticks_and_accounts_for_momentum() {
        use crate::construction::breakage::{Breakage, SweepAccount};
        use crate::headless_test::{
            FIXED_TICKS_PER_SECOND, booted_sim_on, drive_tick, start_fixed_clock,
        };
        use crate::tank::Controlled;
        use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
        let mut app = booted_sim_on(Some(crate::terrain_grid::tests::shipped_grid()));
        start_fixed_clock(&mut app);
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
        let mut freed = 0;
        let mut speeds = Vec::new();
        let mut shed = 0;
        let mut counts = (0, 0, 0, 0);
        let mut per_tick = Vec::new();
        let mut solver_contacts = Vec::new();
        let mut initial = None;
        let mut contact = None;
        let mut losses = Vec::new();
        for tick in 0..plan.drive_ticks {
            drive_tick(&mut app, tank, 1.0, 0.0);
            let account = app.world().resource::<SweepAccount>();
            if account.contacts.contains_key(&(tank, ordinal)) && contact.is_none() {
                contact = Some(tick);
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
                    .filter(|(point, _)| point.body == tank && point.structure == target)
                    .map(|(point, _)| point.impulse)
                    .sum();
                solver_contacts.push((
                    tick,
                    app.world().get::<LinearVelocity>(tank).unwrap().0,
                    wall_impulse,
                ));
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
                && [1, 2, 5, 10].contains(&after)
            {
                let speed = app.world().get::<LinearVelocity>(tank).unwrap().0.length();
                speeds.push((after, speed));
            }
            if contact.is_some_and(|first| tick >= first + 10) {
                break;
            }
        }
        println!(
            "MEASURED ram end: position={:?}, speed={:?}, first contact={contact:?}, first sweep={initial:?}, counts={counts:?}, work by tick={per_tick:?}",
            app.world().get::<Position>(tank).unwrap().0,
            app.world().get::<LinearVelocity>(tank).unwrap().0
        );
        assert!(
            freed > 0,
            "the tracked recipe must reach and open its target"
        );
        let first = initial.unwrap();
        assert!(
            first <= contact.expect("the sweep records occupied contact") + 2,
            "hole appears within two contact ticks"
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
            "MEASURED tracked ram: first sweep tick={first}, freed in two ticks={freed}, fragments shed as decoration by +10={shed}, speed by ticks after contact={speeds:?}, target chunks absent={gone}, bounds/casts/incident-joint visits/cast-contact queries={counts:?}, speeds and momentum={losses:?}, post-physics velocities and remaining wall impulses={solver_contacts:?}"
        );
    }
}
