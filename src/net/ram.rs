//! Tracked full-speed ram instrument for the game's environment destruction model, game state only.
//! `scripts/perf/collapse.py --ram` owns `OVERMATCH_RAM`. Default-feature servers derive the
//! run-up from their loaded drive data and resolve ground height at spawn, without view assets.

use avian3d::prelude::{LinearVelocity, Sleeping};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::harness::{LeverError, env_text};
use crate::{
    construction::{
        Body,
        breakage::{Breakage, Piece},
    },
    contact_transfer::ContactTransfers,
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
    app.insert_resource(Ram {
        recipe,
        plan: None,
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
    });
    app.add_systems(FixedFirst, sample_speed);
    Ok(())
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

impl Ram {
    pub(super) fn pose(
        &mut self,
        spec: &TankSpec,
        manifest: &MapManifest,
        height: Option<&HeightGrid>,
    ) -> Result<(Vec3, Quat), LeverError> {
        if let Some(plan) = &self.plan {
            return Ok((plan.spawn.into(), Quat::from_array(plan.rotation)));
        }
        let invalid = |why| LeverError::invalid(LEVER, &self.recipe.instance, why);
        let instance = manifest
            .instances
            .iter()
            .find(|i| i.id == self.recipe.instance)
            .ok_or_else(|| invalid("instance must name a construction in the active map"))?;
        if !matches!(
            manifest.prototypes[&instance.prototype],
            Prototype::Construction { .. }
        ) {
            return Err(invalid(
                "instance must name a construction in the active map",
            ));
        }
        let (runup_m, governed_m_s) = runup(spec, self.recipe.runup_margin as f32);
        let rotation =
            Quat::from_rotation_y(self.recipe.heading_deg.rem_euclid(360.0).to_radians() as f32);
        let direction = rotation * Vec3::NEG_Z;
        let target = Vec3::from_array(instance.translation);
        let xz = (target - direction * runup_m).xz();
        let limit = super::spawn_map::spawn_limit(manifest.extent.half_extent());
        if !runup_m.is_finite() || xz.abs().max_element() > limit {
            return Err(invalid(
                "derived run-up is outside spawn bounds; choose another approach or instance",
            ));
        }
        let spawn = spawn_pos(height, xz);
        let plan = Plan {
            spawn: spawn.to_array(),
            rotation: rotation.to_array(),
            runup_m,
            governed_m_s,
            minimum_m_s: f64::from(governed_m_s) * self.recipe.minimum_speed_fraction,
        };
        println!("ram_plan {}", json!({"recipe": self.recipe, "plan": plan}));
        self.plan = Some(plan);
        Ok((spawn, rotation))
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
    // Before the first tank spawn the capture has no plan or velocity.
    if let Some(velocity) = tanks.iter().next() {
        ram.pre_speed = velocity.xz().length();
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Census<'w, 's> {
    ram: Option<ResMut<'w, Ram>>,
    tanks: Query<'w, 's, Entity, With<Tank>>,
    pieces: Query<'w, 's, Option<&'static Sleeping>, (With<Piece>, With<Breakage>)>,
    constructions: Query<'w, 's, (Entity, &'static MapInstance), With<Body>>,
    contacts: Res<'w, ContactTransfers>,
}

impl Census<'_, '_> {
    /// The collector runs after TickClosed, so its measured cost is reported separately.
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
            .filter(|r| r.plan.is_some() && r.done.is_none())
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
                .find(|(_, id)| id.id == ram.recipe.instance)
                .expect("the admitted ram construction exists throughout its capture")
                .0;
            ram.target = Some(target);
            target
        };
        if ram.impact_tick.is_none() {
            let normal_speed = self
                .contacts
                .split()
                .filter(|(point, _, _, _)| {
                    point.structure == target && self.tanks.contains(point.body)
                })
                .map(|(point, _, _, _)| point.closing_speed)
                .reduce(f32::max);
            if let Some(normal_speed) = normal_speed {
                ram.impact_tick = Some(ram.tick);
                ram.impact = Some(
                    json!({"tick": ram.tick, "speed_m_s": ram.pre_speed, "normal_speed_m_s": normal_speed}),
                );
            }
        }
        let awake = self.pieces.iter().filter(Option::is_none).count();
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
            intake_us: stages[0],
            structure_us: stages[1],
            refit_us: stages[2],
            publish_us: stages[3],
            contacts_us: stages[4],
            collector_us: 0.0,
            awake,
            tanks: self.tanks.iter().count(),
            dt_s: dt,
        });
        true
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

    /// A Battle reset must invalidate an armed observation, rather than certify a quiet new
    /// world as the old construction resting. The runner relies on that refusal.
    #[test]
    fn removed_target_refuses_the_capture() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let target = world.spawn_empty().id();
        world.despawn(target);
        world.init_resource::<ContactTransfers>();
        // Fixture: the route and timing values are irrelevant once its owner is gone.
        world.insert_resource(Ram {
            recipe: Recipe::parse(include_str!("../../scripts/perf/ram.json")).unwrap(),
            plan: Some(Plan {
                spawn: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                runup_m: 1.0,
                governed_m_s: 1.0,
                minimum_m_s: 1.0,
            }),
            tick: 0,
            pre_speed: 0.0,
            impact: None,
            impact_tick: None,
            resting_s: 0.0,
            done: None,
            target: Some(target),
            rows: Vec::new(),
            sends: Vec::new(),
            last_send_tick: None,
        });
        let captured = world
            .run_system_once(|mut census: Census| {
                census.record(0.0, 0.0, 0.0, [0.0; crate::cost::stages::COUNT], 1.0)
            })
            .unwrap();
        assert!(!captured);
        assert_eq!(world.resource::<Ram>().done, Some(End::WorldChanged));
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
}
