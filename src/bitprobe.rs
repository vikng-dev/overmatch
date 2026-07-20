//! Cross-target, single-fixed-tick differential fixture.
//!
//! This module exists only with the `bitprobe` feature. Normal client/server builds do not compile
//! its capture resource, report payloads, writer, or headless composition.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Duration;

use avian3d::prelude::{
    AngularVelocity, IslandPlugin, IslandSleepingPlugin, LinearVelocity,
    PhysicsInterpolationPlugin, PhysicsPlugins, Position, RigidBody, Rotation,
};
use bevy::app::{PluginGroup, PluginsState};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use flate2::Compression;
use flate2::GzBuilder;
use serde::Serialize;
use serde_json::json;

use crate::SimPlugin;
use crate::bake::TankBlueprint;
use crate::command::TankCommand;
use crate::state::AppState;
use crate::tank::{Controlled, TankSimSource, spawn_bitprobe_tank};
use crate::track::sim::{
    ElementGripNetcode, TankTransmission, TrackDrive, TrackGear, TrackGrip, TrackGripEffect,
};
use crate::track::terrain::TrackField;
use crate::track::transmission::{TransmissionProjectionValue, transmission_state_projection};
use crate::world::TerrainMap;

const MAGIC: &[u8; 8] = b"OMBP\x01\r\n\0";
const FORMAT_VERSION: u32 = 1;
const TICK_HZ: f64 = 64.0;
const TICK_COUNT: u32 = 3_072;

const SETTLE_END: u32 = 128;
const RAMP_END: u32 = 256;
const CLIMB_END: u32 = 1_792;
const CRUISE_END: u32 = 2_304;
const WIDE_END: u32 = 2_560;
const TIGHT_END: u32 = 2_816;

const SPAWN_TRANSLATION: Vec3 = Vec3::new(100.0, 2.0, 100.0);
const SPAWN_ROTATION: Quat = Quat::IDENTITY;

const CONTACT_FIELDS: &[(&str, &str)] = &[
    ("side", "u32"),
    ("station", "u32"),
    ("material", "u32"),
    ("column", "u32"),
    ("query_a.x", "f32"),
    ("query_a.y", "f32"),
    ("query_a.z", "f32"),
    ("query_m.x", "f32"),
    ("query_m.y", "f32"),
    ("query_m.z", "f32"),
    ("query_b.x", "f32"),
    ("query_b.y", "f32"),
    ("query_b.z", "f32"),
    ("out.x", "f32"),
    ("out.y", "f32"),
    ("out.z", "f32"),
    ("reach", "f32"),
    ("depth_a", "f32"),
    ("depth_m", "f32"),
    ("depth_b", "f32"),
];

const ELEMENT_FIELDS: &[(&str, &str)] = &[
    ("side", "u32"),
    ("station", "u32"),
    ("material", "u32"),
    ("column", "u32"),
    ("active", "bool"),
    ("point.x", "f32"),
    ("point.y", "f32"),
    ("point.z", "f32"),
    ("normal.x", "f32"),
    ("normal.y", "f32"),
    ("normal.z", "f32"),
    ("load", "f32"),
    ("load_elastic", "f32"),
    ("slip_long", "f32"),
    ("slip_lat", "f32"),
    ("f_long", "f32"),
    ("f_lat", "f32"),
    ("strain.x", "f32"),
    ("strain.y", "f32"),
    ("strain.z", "f32"),
    ("dwell", "u32"),
];

const BELT_FIELDS: &[(&str, &str)] = &[("left", "f32"), ("right", "f32")];

const TRANSMISSION_INPUT_FIELDS: &[(&str, &str)] = &[
    ("command_throttle", "f32"),
    ("command_steer", "f32"),
    ("shaped_throttle", "f32"),
    ("shaped_steer", "f32"),
    ("side_command_left", "f32"),
    ("side_command_right", "f32"),
    ("belt_speed_left", "f32"),
    ("belt_speed_right", "f32"),
    ("reaction_left", "f32"),
    ("reaction_right", "f32"),
    ("dt", "f32"),
    ("direction", "f32"),
    ("demand_sample", "f32"),
    ("demand_pre", "f32"),
    ("demand_post", "f32"),
    ("demand_updated", "bool"),
];

const CLUTCH_ENGINE_FIELDS: &[(&str, &str)] = &[
    ("mean_speed", "f32"),
    ("difference_speed", "f32"),
    ("shaft_speed", "f32"),
    ("gear_reduction", "f32"),
    ("k", "f32"),
    ("omega_idle", "f32"),
    ("omega_floor", "f32"),
    ("omega_pre", "f32"),
    ("u_fuel", "f32"),
    ("rpm", "f32"),
    ("tau_idle", "f32"),
    ("tau_induced", "f32"),
    ("tau_drag", "f32"),
    ("tau_free", "f32"),
    ("power_available", "f32"),
    ("i_mean", "f32"),
    ("f_other", "f32"),
    ("tau_star", "f32"),
    ("tau_clamped", "f32"),
    ("omega_coupled", "f32"),
    ("tau_c", "f32"),
    ("engaged", "bool"),
    ("shifting", "bool"),
    ("f_c_pre_scale", "f32"),
    ("f_s_pre_scale", "f32"),
    ("lambda", "f32"),
    ("j_left", "f32"),
    ("j_right", "f32"),
    ("power_left", "f32"),
    ("power_right", "f32"),
    ("power_positive", "f32"),
    ("power_negative", "f32"),
    ("power_net", "f32"),
    ("power_scale", "f32"),
    ("omega_integrated", "f32"),
    ("force_left", "f32"),
    ("force_right", "f32"),
    ("raw_next_left", "f32"),
    ("raw_next_right", "f32"),
    ("next_speed_left", "f32"),
    ("next_speed_right", "f32"),
    ("reanchor_attempted", "bool"),
    ("reanchor_locked", "f32"),
    ("reanchor_tau_impl", "f32"),
    ("reanchor_feasible", "bool"),
    ("omega_end", "f32"),
];

const TRANSMISSION_STATE_FIELDS: &[(&str, &str)] = &[
    ("gear", "u32"),
    ("shift_ticks", "u32"),
    ("steer_step", "u32"),
    ("reverse", "bool"),
    ("park", "bool"),
    ("last_shift_dir", "i32"),
    ("dwell_ticks", "u32"),
    ("omega_e", "f32"),
    ("clutch_out", "bool"),
    ("demand_n", "f32"),
    ("demand_initialized", "bool"),
    ("grade_confirm_ticks", "u32"),
    ("grade_target", "u32"),
    ("scheduler_tag", "u32"),
    ("scheduler_from", "u32"),
    ("scheduler_to", "u32"),
    ("hill_hold", "bool"),
    ("hold_reengage_ticks", "u32"),
];

const POSE_FIELDS: &[(&str, &str)] = &[
    ("position.x", "f32"),
    ("position.y", "f32"),
    ("position.z", "f32"),
    ("rotation.x", "f32"),
    ("rotation.y", "f32"),
    ("rotation.z", "f32"),
    ("rotation.w", "f32"),
    ("linear_velocity.x", "f32"),
    ("linear_velocity.y", "f32"),
    ("linear_velocity.z", "f32"),
    ("angular_velocity.x", "f32"),
    ("angular_velocity.y", "f32"),
    ("angular_velocity.z", "f32"),
    ("drive.throttle", "f32"),
    ("drive.steer", "f32"),
    ("drive.left.speed", "f32"),
    ("drive.left.phase_lo", "f64_lo"),
    ("drive.left.phase_hi", "f64_hi"),
    ("drive.right.speed", "f32"),
    ("drive.right.phase_lo", "f64_lo"),
    ("drive.right.phase_hi", "f64_hi"),
    ("grip.left.long", "f32"),
    ("grip.left.lat", "f32"),
    ("grip.right.long", "f32"),
    ("grip.right.lat", "f32"),
    ("effect.traction_force.x", "f32"),
    ("effect.traction_force.y", "f32"),
    ("effect.traction_force.z", "f32"),
    ("effect.traction_torque.x", "f32"),
    ("effect.traction_torque.y", "f32"),
    ("effect.traction_torque.z", "f32"),
    ("effect.reaction_left", "f32"),
    ("effect.reaction_right", "f32"),
    ("effect.field_digest", "u32"),
];

const SEAM_NAMES: &[&str] = &[
    "contact_input",
    "element_output",
    "belt_reaction",
    "transmission_input",
    "clutch_engine",
    "transmission_state",
    "pose_velocity",
];

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ContactInputProbe {
    pub station: u32,
    pub material: u32,
    pub column: u32,
    pub query_a: Vec3,
    pub query_m: Vec3,
    pub query_b: Vec3,
    pub out: Vec3,
    pub reach: f32,
    pub depths: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ElementOutputProbe {
    pub station: u32,
    pub material: u32,
    pub column: u32,
    pub active: bool,
    pub point: Vec3,
    pub normal: Vec3,
    pub load: f32,
    pub load_elastic: f32,
    pub slip_long: f32,
    pub slip_lat: f32,
    pub f_long: f32,
    pub f_lat: f32,
    pub strain: Vec3,
    pub dwell: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TransmissionProbe {
    pub throttle: f32,
    pub steer: f32,
    pub side_commands: [f32; 2],
    pub speeds: [f32; 2],
    pub reactions: [f32; 2],
    pub dt: f32,
    pub direction: f32,
    pub demand_sample: f32,
    pub demand_pre: f32,
    pub demand_post: f32,
    pub demand_updated: bool,
    pub mean_speed: f32,
    pub difference_speed: f32,
    pub shaft_speed: f32,
    pub gear_reduction: f32,
    pub k: f32,
    pub omega_idle: f32,
    pub omega_floor: f32,
    pub omega_pre: f32,
    pub u_fuel: f32,
    pub rpm: f32,
    pub tau_idle: f32,
    pub tau_induced: f32,
    pub tau_drag: f32,
    pub tau_free: f32,
    pub power_available: f32,
    pub i_mean: f32,
    pub f_other: f32,
    pub tau_star: f32,
    pub tau_clamped: f32,
    pub omega_coupled: f32,
    pub tau_c: f32,
    pub engaged: bool,
    pub shifting: bool,
    pub f_c_pre_scale: f32,
    pub f_s_pre_scale: f32,
    pub lambda: f32,
    pub j: [f32; 2],
    pub power_left: f32,
    pub power_right: f32,
    pub power_positive: f32,
    pub power_negative: f32,
    pub power_net: f32,
    pub power_scale: f32,
    pub omega_integrated: f32,
    pub forces: [f32; 2],
    pub raw_next: [f32; 2],
    pub next_speeds: [f32; 2],
    pub reanchor_attempted: bool,
    pub reanchor_locked: f32,
    pub reanchor_tau_impl: f32,
    pub reanchor_feasible: bool,
    pub omega_end: f32,
}

#[derive(Resource, Debug, Default)]
pub(crate) struct BitprobeCapture {
    pub tanks_seen: u32,
    pub command: [f32; 2],
    pub contact_inputs: [Vec<ContactInputProbe>; 2],
    pub element_outputs: [Vec<ElementOutputProbe>; 2],
    pub belt_reaction: [f32; 2],
    pub transmission: TransmissionProbe,
}

impl BitprobeCapture {
    pub(crate) fn clear_tick(&mut self) {
        self.tanks_seen = 0;
        self.command = [0.0; 2];
        for side in &mut self.contact_inputs {
            side.clear();
        }
        for side in &mut self.element_outputs {
            side.clear();
        }
        self.belt_reaction = [0.0; 2];
        self.transmission = TransmissionProbe::default();
    }
}

#[derive(Clone, Debug, Serialize)]
struct StartupEntry {
    name: String,
    kind: &'static str,
    bits: u32,
}

#[derive(Default)]
pub(crate) struct StartupBuilder {
    entries: Vec<StartupEntry>,
}

impl StartupBuilder {
    pub(crate) fn f32(&mut self, name: &str, value: f32) {
        self.entries.push(StartupEntry {
            name: name.to_string(),
            kind: "f32",
            bits: value.to_bits(),
        });
    }

    pub(crate) fn u32(&mut self, name: &str, value: u32) {
        self.entries.push(StartupEntry {
            name: name.to_string(),
            kind: "u32",
            bits: value,
        });
    }

    pub(crate) fn bool(&mut self, name: &str, value: bool) {
        self.entries.push(StartupEntry {
            name: name.to_string(),
            kind: "bool",
            bits: u32::from(value),
        });
    }

    pub(crate) fn f64(&mut self, name: &str, value: f64) {
        let bits = value.to_bits();
        self.u32(&format!("{name}.lo"), bits as u32);
        self.u32(&format!("{name}.hi"), (bits >> 32) as u32);
    }

    pub(crate) fn vec2(&mut self, name: &str, value: Vec2) {
        self.f32(&format!("{name}.x"), value.x);
        self.f32(&format!("{name}.y"), value.y);
    }

    pub(crate) fn vec3(&mut self, name: &str, value: Vec3) {
        self.f32(&format!("{name}.x"), value.x);
        self.f32(&format!("{name}.y"), value.y);
        self.f32(&format!("{name}.z"), value.z);
    }

    pub(crate) fn quat(&mut self, name: &str, value: Quat) {
        self.f32(&format!("{name}.x"), value.x);
        self.f32(&format!("{name}.y"), value.y);
        self.f32(&format!("{name}.z"), value.z);
        self.f32(&format!("{name}.w"), value.w);
    }
}

#[derive(Resource)]
struct FixtureEntity(Entity);

fn spawn_fixture(
    mut commands: Commands,
    source: TankSimSource,
    fixture: Option<Res<FixtureEntity>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if fixture.is_some() {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    let tank = spawn_bitprobe_tank(
        &mut commands,
        content,
        (
            Transform::from_translation(SPAWN_TRANSLATION).with_rotation(SPAWN_ROTATION),
            Name::new("bitprobe Tiger I"),
            RigidBody::Dynamic,
            Controlled,
        ),
    );
    commands.insert_resource(FixtureEntity(tank));
    next_state.set(AppState::Playing);
}

fn scripted_input(tick: u32) -> (f32, f32) {
    match tick {
        0..SETTLE_END => (0.0, 0.0),
        SETTLE_END..RAMP_END => {
            let step = (tick - SETTLE_END + 1) as f32;
            (step * (1.0 / (RAMP_END - SETTLE_END) as f32), 0.0)
        }
        RAMP_END..CRUISE_END => (1.0, 0.0),
        CRUISE_END..WIDE_END => (1.0, 0.35),
        WIDE_END..TIGHT_END => (1.0, 0.80),
        _ => (1.0, 0.0),
    }
}

fn build_app() -> App {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(bevy::render::RenderPlugin {
                render_creation: bevy::render::settings::WgpuSettings {
                    backends: None,
                    ..default()
                }
                .into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>(),
    )
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));

    // Match the network authority's no-interpolation/no-sleep Avian policy. The standalone probe
    // retains PhysicsTransformPlugin because Lightyear is intentionally absent and therefore cannot
    // own transform synchronization as it does in the deployed composition.
    let physics = PhysicsPlugins::default()
        .build()
        .disable::<PhysicsInterpolationPlugin>()
        .disable::<IslandPlugin>()
        .disable::<IslandSleepingPlugin>();
    app.add_plugins(physics)
        .add_plugins(SimPlugin)
        .init_resource::<ElementGripNetcode>()
        .init_resource::<BitprobeCapture>()
        .add_systems(PreUpdate, spawn_fixture);
    app
}

fn wait_for_plugins(app: &mut App) -> Result<(), String> {
    for _ in 0..60_000 {
        if app.plugins_state() != PluginsState::Adding {
            app.finish();
            app.cleanup();
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Err("plugins did not finish initialization within 60 seconds".to_string())
}

fn prepare_fixture(app: &mut App) -> Result<Entity, String> {
    for _ in 0..16 {
        app.update();
        let world = app.world();
        let ready = world.get_resource::<TrackGear>().is_some()
            && world
                .get_resource::<TrackField>()
                .is_some_and(|field| field.field.is_some())
            && world.get_resource::<FixtureEntity>().is_some()
            && *world.resource::<State<AppState>>().get() == AppState::Playing;
        if ready {
            return Ok(world.resource::<FixtureEntity>().0);
        }
    }
    Err(
        "fixture did not reach Playing with TrackGear and TrackField after 16 frozen updates"
            .into(),
    )
}

fn startup_dump(app: &App, tank: Entity) -> StartupBuilder {
    let world = app.world();
    let mut out = StartupBuilder::default();
    out.u32("format.version", FORMAT_VERSION);
    out.u32("scenario.tick_hz", TICK_HZ as u32);
    out.u32("scenario.tick_count", TICK_COUNT);
    out.u32("scenario.settle_end", SETTLE_END);
    out.u32("scenario.ramp_end", RAMP_END);
    out.u32("scenario.climb_end", CLIMB_END);
    out.u32("scenario.cruise_end", CRUISE_END);
    out.u32("scenario.wide_end", WIDE_END);
    out.u32("scenario.tight_end", TIGHT_END);
    out.vec3("scenario.spawn.translation", SPAWN_TRANSLATION);
    out.quat("scenario.spawn.rotation", SPAWN_ROTATION);
    out.f64("scenario.fixed_dt", 1.0 / TICK_HZ);

    let terrain = world.resource::<TerrainMap>();
    out.u32("terrain.revision.lo", terrain.revision as u32);
    out.u32("terrain.revision.hi", (terrain.revision >> 32) as u32);
    out.u32("terrain.block_count", terrain.blocks.len() as u32);
    for (index, block) in terrain.blocks.iter().enumerate() {
        out.vec3(
            &format!("terrain.blocks[{index}].translation"),
            block.translation,
        );
        out.quat(&format!("terrain.blocks[{index}].rotation"), block.rotation);
        out.vec3(&format!("terrain.blocks[{index}].scale"), block.scale);
    }
    world
        .resource::<TrackField>()
        .field
        .as_ref()
        .expect("fixture readiness checked the field")
        .bitprobe_startup(&mut out);
    world.resource::<TrackGear>().bitprobe_startup(&mut out);

    let spec = &world.resource::<TankBlueprint>().spec;
    out.f32("spec.mass", spec.mass);
    out.f32("spec.inertia_extents.x", spec.inertia_extents.0);
    out.f32("spec.inertia_extents.y", spec.inertia_extents.1);
    out.f32("spec.inertia_extents.z", spec.inertia_extents.2);
    let track = &spec.track;
    out.f32("spec.track.pitch", track.pitch);
    out.u32("spec.track.link_count", track.link_count as u32);
    out.f32("spec.track.width", track.width);
    out.f32("spec.track.thickness", track.thickness);
    out.f32("spec.track.link_mass", track.link_mass);
    out.f32("spec.track.hinge_torque", track.hinge_torque);
    out.f32("spec.track.max_link_angle", track.max_link_angle);
    out.f32("spec.track.plane_x", track.plane_x);
    out.f32("spec.track.sprocket.center.z", track.sprocket.center.0);
    out.f32("spec.track.sprocket.center.y", track.sprocket.center.1);
    out.u32("spec.track.sprocket.teeth", track.sprocket.teeth);
    out.f32("spec.track.idler.center.z", track.idler.center.0);
    out.f32("spec.track.idler.center.y", track.idler.center.1);
    out.f32("spec.track.idler.radius", track.idler.radius);
    out.f32("spec.track.wheel_radius", track.wheel_radius);
    out.f32(
        "spec.track.powertrain.max_speed",
        track.powertrain.max_speed,
    );
    out.f32("spec.track.powertrain.power", track.powertrain.power);
    out.f32("spec.track.powertrain.force", track.powertrain.force);
    out.f32(
        "spec.track.powertrain.governor_gain",
        track.powertrain.governor_gain,
    );
    out.f32("spec.track.powertrain.inertia", track.powertrain.inertia);
    out.f32(
        "spec.track.support.stiffness_per_m",
        track.support.stiffness_per_m,
    );
    out.f32(
        "spec.track.support.damping_per_m",
        track.support.damping_per_m,
    );
    out.f32("spec.track.support.engage", track.support.engage);

    let position = world.get::<Position>(tank).expect("tank has Position").0;
    let rotation = world.get::<Rotation>(tank).expect("tank has Rotation").0;
    let linear = world
        .get::<LinearVelocity>(tank)
        .expect("tank has LinearVelocity")
        .0;
    let angular = world
        .get::<AngularVelocity>(tank)
        .expect("tank has AngularVelocity")
        .0;
    out.vec3("initial.position", position);
    out.quat("initial.rotation", rotation);
    out.vec3("initial.linear_velocity", linear);
    out.vec3("initial.angular_velocity", angular);
    out
}

fn schema(fields: &[(&str, &str)]) -> Vec<serde_json::Value> {
    fields
        .iter()
        .map(|(name, kind)| json!({"name": name, "type": kind}))
        .collect()
}

fn header(startup: &StartupBuilder) -> serde_json::Value {
    json!({
        "format": "overmatch-bitprobe",
        "version": FORMAT_VERSION,
        "endianness": "little",
        "hash": "fnv1a64 over little-endian u32 payload bytes",
        "tick_hz": TICK_HZ as u32,
        "tick_count": TICK_COUNT,
        "run": {
            "arch": std::env::consts::ARCH,
            "os": std::env::consts::OS,
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        },
        "startup": startup.entries,
        "scenario": [
            {"start": 0, "end_exclusive": SETTLE_END, "name": "settle", "throttle": "0", "steer": "0"},
            {"start": SETTLE_END, "end_exclusive": RAMP_END, "name": "linear throttle ramp", "throttle": "1/128..1", "steer": "0"},
            {"start": RAMP_END, "end_exclusive": CLIMB_END, "name": "full-throttle gear climb", "throttle": "1", "steer": "0"},
            {"start": CLIMB_END, "end_exclusive": CRUISE_END, "name": "straight cruise", "throttle": "1", "steer": "0"},
            {"start": CRUISE_END, "end_exclusive": WIDE_END, "name": "WIDE detent hold", "throttle": "1", "steer": "0.35"},
            {"start": WIDE_END, "end_exclusive": TIGHT_END, "name": "TIGHT detent hold", "throttle": "1", "steer": "0.80"},
            {"start": TIGHT_END, "end_exclusive": TICK_COUNT, "name": "released cruise", "throttle": "1", "steer": "0"},
        ],
        "tick_layout": {
            "tick": "u32",
            "per_seam": ["hash:u64", "word_count:u32", "payload:u32[word_count]"],
        },
        "seams": [
            {"name": SEAM_NAMES[0], "layout": "records", "count_prefix": "u32", "record_fields": schema(CONTACT_FIELDS)},
            {"name": SEAM_NAMES[1], "layout": "records", "count_prefix": "u32", "record_fields": schema(ELEMENT_FIELDS)},
            {"name": SEAM_NAMES[2], "layout": "fields", "fields": schema(BELT_FIELDS)},
            {"name": SEAM_NAMES[3], "layout": "fields", "fields": schema(TRANSMISSION_INPUT_FIELDS)},
            {"name": SEAM_NAMES[4], "layout": "fields", "fields": schema(CLUTCH_ENGINE_FIELDS)},
            {"name": SEAM_NAMES[5], "layout": "fields", "fields": schema(TRANSMISSION_STATE_FIELDS)},
            {"name": SEAM_NAMES[6], "layout": "fields", "fields": schema(POSE_FIELDS)},
        ],
    })
}

struct DumpWriter {
    output: flate2::write::GzEncoder<BufWriter<File>>,
}

impl DumpWriter {
    fn create(path: PathBuf, header: &serde_json::Value) -> Result<Self, String> {
        let file = File::create(&path)
            .map_err(|error| format!("creating {} failed: {error}", path.display()))?;
        // mtime=0 makes the gzip wrapper reproducible; all target/run identity lives in the
        // self-describing inner header where the comparator can deliberately ignore it.
        let mut output = GzBuilder::new()
            .mtime(0)
            .write(BufWriter::new(file), Compression::default());
        let header = serde_json::to_vec(header)
            .map_err(|error| format!("serializing dump header failed: {error}"))?;
        let header_len = u32::try_from(header.len())
            .map_err(|_| "serialized dump header exceeds u32 length".to_string())?;
        output
            .write_all(MAGIC)
            .and_then(|_| output.write_all(&header_len.to_le_bytes()))
            .and_then(|_| output.write_all(&header))
            .map_err(|error| format!("writing dump header failed: {error}"))?;
        Ok(Self { output })
    }

    fn tick(&mut self, tick: u32, seams: &[Vec<u32>]) -> Result<(), String> {
        debug_assert_eq!(seams.len(), SEAM_NAMES.len());
        self.output
            .write_all(&tick.to_le_bytes())
            .map_err(|error| format!("writing tick {tick} failed: {error}"))?;
        for payload in seams {
            let hash = hash_words(payload);
            let len = u32::try_from(payload.len())
                .map_err(|_| format!("tick {tick} seam payload exceeds u32 words"))?;
            self.output
                .write_all(&hash.to_le_bytes())
                .and_then(|_| self.output.write_all(&len.to_le_bytes()))
                .map_err(|error| format!("writing tick {tick} seam header failed: {error}"))?;
            for word in payload {
                self.output
                    .write_all(&word.to_le_bytes())
                    .map_err(|error| format!("writing tick {tick} seam payload failed: {error}"))?;
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<(), String> {
        let mut output = self
            .output
            .finish()
            .map_err(|error| format!("finishing gzip dump failed: {error}"))?;
        output
            .flush()
            .map_err(|error| format!("flushing dump failed: {error}"))
    }
}

fn hash_words(words: &[u32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in words.iter().flat_map(|word| word.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn f32_word(output: &mut Vec<u32>, value: f32) {
    output.push(value.to_bits());
}

fn vec3_words(output: &mut Vec<u32>, value: Vec3) {
    f32_word(output, value.x);
    f32_word(output, value.y);
    f32_word(output, value.z);
}

fn bool_word(output: &mut Vec<u32>, value: bool) {
    output.push(u32::from(value));
}

fn contact_payload(capture: &BitprobeCapture) -> Vec<u32> {
    let count: usize = capture.contact_inputs.iter().map(Vec::len).sum();
    let mut output = Vec::with_capacity(1 + count * CONTACT_FIELDS.len());
    output.push(count as u32);
    for (side, records) in capture.contact_inputs.iter().enumerate() {
        for record in records {
            output.extend([side as u32, record.station, record.material, record.column]);
            vec3_words(&mut output, record.query_a);
            vec3_words(&mut output, record.query_m);
            vec3_words(&mut output, record.query_b);
            vec3_words(&mut output, record.out);
            f32_word(&mut output, record.reach);
            for depth in record.depths {
                f32_word(&mut output, depth);
            }
        }
    }
    output
}

fn element_payload(capture: &BitprobeCapture) -> Vec<u32> {
    let count: usize = capture.element_outputs.iter().map(Vec::len).sum();
    let mut output = Vec::with_capacity(1 + count * ELEMENT_FIELDS.len());
    output.push(count as u32);
    for (side, records) in capture.element_outputs.iter().enumerate() {
        for record in records {
            output.extend([side as u32, record.station, record.material, record.column]);
            bool_word(&mut output, record.active);
            vec3_words(&mut output, record.point);
            vec3_words(&mut output, record.normal);
            for value in [
                record.load,
                record.load_elastic,
                record.slip_long,
                record.slip_lat,
                record.f_long,
                record.f_lat,
            ] {
                f32_word(&mut output, value);
            }
            vec3_words(&mut output, record.strain);
            output.push(record.dwell);
        }
    }
    output
}

fn belt_payload(capture: &BitprobeCapture) -> Vec<u32> {
    capture
        .belt_reaction
        .iter()
        .map(|value| value.to_bits())
        .collect()
}

fn transmission_input_payload(capture: &BitprobeCapture) -> Vec<u32> {
    let probe = capture.transmission;
    let mut output = Vec::with_capacity(TRANSMISSION_INPUT_FIELDS.len());
    for value in [
        capture.command[0],
        capture.command[1],
        probe.throttle,
        probe.steer,
        probe.side_commands[0],
        probe.side_commands[1],
        probe.speeds[0],
        probe.speeds[1],
        probe.reactions[0],
        probe.reactions[1],
        probe.dt,
        probe.direction,
        probe.demand_sample,
        probe.demand_pre,
        probe.demand_post,
    ] {
        f32_word(&mut output, value);
    }
    bool_word(&mut output, probe.demand_updated);
    output
}

fn clutch_engine_payload(probe: TransmissionProbe) -> Vec<u32> {
    let mut output = Vec::with_capacity(CLUTCH_ENGINE_FIELDS.len());
    for value in [
        probe.mean_speed,
        probe.difference_speed,
        probe.shaft_speed,
        probe.gear_reduction,
        probe.k,
        probe.omega_idle,
        probe.omega_floor,
        probe.omega_pre,
        probe.u_fuel,
        probe.rpm,
        probe.tau_idle,
        probe.tau_induced,
        probe.tau_drag,
        probe.tau_free,
        probe.power_available,
        probe.i_mean,
        probe.f_other,
        probe.tau_star,
        probe.tau_clamped,
        probe.omega_coupled,
        probe.tau_c,
    ] {
        f32_word(&mut output, value);
    }
    bool_word(&mut output, probe.engaged);
    bool_word(&mut output, probe.shifting);
    for value in [
        probe.f_c_pre_scale,
        probe.f_s_pre_scale,
        probe.lambda,
        probe.j[0],
        probe.j[1],
        probe.power_left,
        probe.power_right,
        probe.power_positive,
        probe.power_negative,
        probe.power_net,
        probe.power_scale,
        probe.omega_integrated,
        probe.forces[0],
        probe.forces[1],
        probe.raw_next[0],
        probe.raw_next[1],
        probe.next_speeds[0],
        probe.next_speeds[1],
    ] {
        f32_word(&mut output, value);
    }
    bool_word(&mut output, probe.reanchor_attempted);
    f32_word(&mut output, probe.reanchor_locked);
    f32_word(&mut output, probe.reanchor_tau_impl);
    bool_word(&mut output, probe.reanchor_feasible);
    f32_word(&mut output, probe.omega_end);
    output
}

fn transmission_state_payload(transmission: &TankTransmission) -> Vec<u32> {
    let mut output = Vec::with_capacity(TRANSMISSION_STATE_FIELDS.len());
    for field in transmission_state_projection(&transmission.0) {
        match field.value {
            TransmissionProjectionValue::U8(value) => output.push(u32::from(value)),
            TransmissionProjectionValue::I8(value) => output.push((value as i32) as u32),
            TransmissionProjectionValue::Bool(value) => bool_word(&mut output, value),
            TransmissionProjectionValue::F32(value) => f32_word(&mut output, value),
            TransmissionProjectionValue::Scheduler { tag, from, to } => {
                output.extend([u32::from(tag), u32::from(from), u32::from(to)]);
            }
        }
    }
    output
}

fn pose_payload(world: &World, tank: Entity) -> Vec<u32> {
    let position = world.get::<Position>(tank).expect("tank has Position").0;
    let rotation = world.get::<Rotation>(tank).expect("tank has Rotation").0;
    let linear = world
        .get::<LinearVelocity>(tank)
        .expect("tank has LinearVelocity")
        .0;
    let angular = world
        .get::<AngularVelocity>(tank)
        .expect("tank has AngularVelocity")
        .0;
    let drive = world.get::<TrackDrive>(tank).expect("tank has TrackDrive");
    let grip = world.get::<TrackGrip>(tank).expect("tank has TrackGrip");
    let effect = world
        .get::<TrackGripEffect>(tank)
        .expect("tank has TrackGripEffect");
    let mut output = Vec::with_capacity(POSE_FIELDS.len());
    vec3_words(&mut output, position);
    for value in rotation.to_array() {
        f32_word(&mut output, value);
    }
    vec3_words(&mut output, linear);
    vec3_words(&mut output, angular);
    f32_word(&mut output, drive.throttle);
    f32_word(&mut output, drive.steer);
    for side in drive.sides {
        f32_word(&mut output, side.speed);
        let phase = side.phase.to_bits();
        output.push(phase as u32);
        output.push((phase >> 32) as u32);
    }
    for side in grip.sides {
        for value in side {
            f32_word(&mut output, value);
        }
    }
    vec3_words(&mut output, effect.traction_force);
    vec3_words(&mut output, effect.traction_torque);
    f32_word(&mut output, effect.belt_reaction[0]);
    f32_word(&mut output, effect.belt_reaction[1]);
    output.push(effect.field_digest);
    output
}

fn tick_payloads(world: &World, tank: Entity) -> Result<Vec<Vec<u32>>, String> {
    let capture = world.resource::<BitprobeCapture>();
    if capture.tanks_seen != 1 {
        return Err(format!(
            "tick captured {} tanks; the differential fixture requires exactly one",
            capture.tanks_seen
        ));
    }
    let transmission = world
        .get::<TankTransmission>(tank)
        .ok_or_else(|| "fixture tank lost TankTransmission".to_string())?;
    let seams = vec![
        contact_payload(capture),
        element_payload(capture),
        belt_payload(capture),
        transmission_input_payload(capture),
        clutch_engine_payload(capture.transmission),
        transmission_state_payload(transmission),
        pose_payload(world, tank),
    ];
    debug_assert_eq!(seams.len(), SEAM_NAMES.len());
    Ok(seams)
}

/// Run the fixed canned fixture and write one complete raw-bit dump.
pub fn run_bitprobe() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os();
    let executable = args.next().unwrap_or_default();
    let output = args.next().ok_or_else(|| {
        format!(
            "usage: {} <output.obp.gz>",
            PathBuf::from(executable).display()
        )
    })?;
    if args.next().is_some() {
        return Err("bitprobe accepts exactly one output path".into());
    }

    let mut app = build_app();
    wait_for_plugins(&mut app)?;
    let tank = prepare_fixture(&mut app)?;
    app.world_mut()
        .resource_mut::<Time<Fixed>>()
        .set_timestep_hz(TICK_HZ);
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));

    let startup = startup_dump(&app, tank);
    let mut writer = DumpWriter::create(PathBuf::from(output), &header(&startup))?;
    for tick in 0..TICK_COUNT {
        let (throttle, steer) = scripted_input(tick);
        {
            let mut entity = app.world_mut().entity_mut(tank);
            let mut command = entity
                .get_mut::<TankCommand>()
                .ok_or("fixture tank lost TankCommand")?;
            command.throttle = throttle;
            command.steer = steer;
        }
        app.update();
        writer.tick(tick, &tick_payloads(app.world(), tank)?)?;
    }
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_boundaries_are_exact_and_cover_every_tick() {
        assert_eq!(scripted_input(0), (0.0, 0.0));
        assert_eq!(scripted_input(SETTLE_END - 1), (0.0, 0.0));
        assert_eq!(scripted_input(RAMP_END - 1), (1.0, 0.0));
        assert_eq!(scripted_input(CRUISE_END), (1.0, 0.35));
        assert_eq!(scripted_input(WIDE_END), (1.0, 0.80));
        assert_eq!(scripted_input(TIGHT_END), (1.0, 0.0));
        assert_eq!(scripted_input(TICK_COUNT - 1), (1.0, 0.0));
    }

    #[test]
    fn fnv_hash_uses_little_endian_word_bytes() {
        let bytes = [0x04_u8, 0x03, 0x02, 0x01];
        let mut expected = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            expected ^= u64::from(byte);
            expected = expected.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(hash_words(&[0x0102_0304]), expected);
    }

    #[test]
    fn vector_payload_width_matches_the_declared_three_fields() {
        let mut words = Vec::new();
        vec3_words(&mut words, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(
            words,
            [1.0_f32.to_bits(), 2.0_f32.to_bits(), 3.0_f32.to_bits()]
        );
    }
}
