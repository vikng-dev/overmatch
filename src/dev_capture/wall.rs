//! The capture's wall cue, in the game's environment destruction (game state only): on the first
//! frame at or after each shot's app second, a shipped round is fired square at a house wall or a
//! bare charge is set off against the foot of it, and the camera stands in front of that wall.
//!
//! The capture plan's `wall` ([`super::plan::Wall`]), on the offline root
//! ([`offline_wall_plugin`]); each shot goes off at its own second, all at one wall. The dedicated
//! server mounts the same cue from the same plan file ([`server_wall_plugin`]): it fires and draws
//! nothing, and logs the camera its capture clients hold as a plan fragment (a `frames.camera`
//! value), so every peer frames the one break the server made. The wall is chosen by class, the
//! same on every run: the building nearest the map's centre (the lowest ordinal on a tie), its
//! breakable pier of the largest volume (the lowest element on a tie). A round is fired 25 m out
//! along the pier's outward normal at the chunk nearest the pier's centre; a charge sits on the
//! pier's outer face at its foot, its energy the first shipped burst core's chemical energy per
//! kilogram times its mass, with no casing.
//!
//! When the plan's `frames.camera` places the camera (a pose or a follow), the cue leaves the
//! camera to it.

use bevy::prelude::*;

use super::plan::WallShot;
use crate::ballistics::FireShell;
use crate::buildings::breakage::{Breakage, WallCharge};
use crate::buildings::fracture::Blast;
use crate::buildings::schema::ElementKind;
use crate::scatter::MapInstance;
use crate::substances::SubstanceRegistry;

/// How far out along the wall's normal a round is fired from, m (fixture).
const STANDOFF_M: f32 = 25.0;
/// The camera's distance out from the wall's outer face and its height over the aim point, m
/// (ASSUMED framing: the pier and its neighbours fill the frame).
const CAMERA_OUT_M: f32 = 7.0;
const CAMERA_UP_M: f32 = 0.6;

#[derive(Clone, Debug)]
enum What {
    Round(crate::rounds::Round),
    Charge { mass: f32 },
}

/// The cue's shots in order, each with its second and whether it went off; the wall's framing
/// once found.
#[derive(Resource)]
struct Cue {
    shots: Vec<(What, f32, bool)>,
    camera: Option<Transform>,
}

pub(crate) fn offline_wall_plugin(app: &mut App) {
    let Some(cue) = cue_from_plan() else {
        return;
    };
    app.insert_resource(cue)
        .insert_resource(crate::camera::CameraCaptive(true))
        .add_systems(Update, fire_on_cue);
    let placed = super::plan()
        .and_then(|plan| plan.frames.as_ref())
        .is_some_and(|frames| frames.camera != super::plan::Camera::Player);
    if !placed {
        app.add_systems(
            PostUpdate,
            hold_camera.in_set(crate::camera::CaptiveCameraPlaced),
        );
    }
}

/// The dedicated server's cue (module doc): it fires, and its clients frame the wall.
pub(crate) fn server_wall_plugin(app: &mut App) {
    if let Some(cue) = cue_from_plan() {
        app.insert_resource(cue).add_systems(Update, fire_on_cue);
    }
}

/// The plan's `wall` cue, `None` when the plan has none.
fn cue_from_plan() -> Option<Cue> {
    let wall = super::plan()?.wall.as_ref()?;
    let shots = wall
        .shots
        .iter()
        .map(|shot| match shot {
            WallShot::Round { round, mode, at } => (
                What::Round(super::resolve_round(round, *mode, "wall.shots")),
                *at,
                false,
            ),
            WallShot::Charge { kg, at } => (What::Charge { mass: *kg }, *at, false),
        })
        .collect();
    info!("capture: game's environment destruction wall cue {wall:?}");
    Some(Cue {
        shots,
        camera: None,
    })
}

/// The camera that frames the wall: [`CAMERA_OUT_M`] out from the middle of the aim point and the
/// foot, [`CAMERA_UP_M`] up, looking at that middle.
fn framing(outward: Vec3, aim: Vec3, foot: Vec3) -> Transform {
    let look = (aim + foot) / 2.0;
    Transform::from_translation(look + outward * CAMERA_OUT_M + Vec3::Y * CAMERA_UP_M)
        .looking_at(look, Vec3::Y)
}

/// The wall (module doc): the pier's outward normal, the aim point on its outer face, the foot of
/// that face, and a point inside the building a metre past its middle from the pier, at the aim's
/// height, all world.
fn the_wall((root, breakage): (&Transform, &Breakage)) -> Option<(Vec3, Vec3, Vec3, Vec3)> {
    let kind = &breakage.kind;
    let layout = &kind.layout;
    let (lo, hi) = kind.bounds();
    let outward_of = |e: usize| {
        let w = kind.asset.elements[e].frame.w().normalize();
        let centre = layout.element_blocks[e].center();
        if (centre - (lo + hi) / 2.0).dot(w) >= 0.0 {
            w
        } else {
            -w
        }
    };
    let e = (0..kind.asset.elements.len())
        .filter(|&e| {
            kind.asset.elements[e].kind == ElementKind::Pier && layout.elements[e].is_some()
        })
        .max_by(|&a, &b| {
            kind.asset.elements[a]
                .volume()
                .total_cmp(&kind.asset.elements[b].volume())
                .then(b.cmp(&a))
        })?;
    let element = &kind.asset.elements[e];
    let w = element.frame.w().normalize();
    let origin = Vec3::from_array(element.frame.origin);
    let centre = layout.element_blocks[e].center();
    let outward = outward_of(e);
    let outer = if outward == w {
        element.depth[1]
    } else {
        element.depth[0]
    };
    let on_face = |p: Vec3| p + w * (outer - (p - origin).dot(w));
    let aim = layout
        .chunks_of(e)
        .map(|c| Vec3::from_array(kind.asset.chunks[layout.chunks[c] as usize].centroid))
        .min_by(|a, b| a.distance(centre).total_cmp(&b.distance(centre)))?;
    let block = &layout.element_blocks[e];
    let foot = Vec3::new(centre.x, block.min.y.max(0.0), centre.z);
    let world = |p: Vec3| root.translation + root.rotation * (p * breakage.scale);
    let middle = (lo + hi) / 2.0;
    let inside = Vec3::new(middle.x, aim.y, middle.z) - outward;
    Some((
        root.rotation * outward,
        world(on_face(aim)),
        world(on_face(foot)),
        world(inside),
    ))
}

/// The building the cue works on (module doc): its pose and breakable state.
fn the_house<'a>(
    buildings: &'a Query<(&MapInstance, &Transform, &Breakage)>,
) -> Option<(&'a Transform, &'a Breakage)> {
    let reach = |t: &Transform| Vec2::new(t.translation.x, t.translation.z).length_squared();
    buildings
        .iter()
        .min_by(|a, b| {
            reach(a.1)
                .total_cmp(&reach(b.1))
                .then(a.0.ordinal.cmp(&b.0.ordinal))
        })
        .map(|(_, transform, breakage)| (transform, breakage))
}

/// Find the wall and frame it (logging the pose), then fire the cue on its second.
fn fire_on_cue(
    time: Res<Time>,
    mut cue: ResMut<Cue>,
    buildings: Query<(&MapInstance, &Transform, &Breakage)>,
    mut commands: Commands,
) {
    let Some((transform, breakage)) = the_house(&buildings) else {
        return;
    };
    let Some((outward, aim, foot, inside)) = the_wall((transform, breakage)) else {
        return;
    };
    if cue.camera.is_none() {
        let camera = framing(outward, aim, foot);
        let (at, look) = (camera.translation, (aim + foot) / 2.0);
        info!(
            "capture: wall outside — camera: Pose(eye: ({:?}, {:?}, {:?}), look: ({:?}, {:?}, {:?})),",
            at.x, at.y, at.z, look.x, look.y, look.z
        );
        info!(
            "capture: wall inside — camera: Pose(eye: ({:?}, {:?}, {:?}), look: ({:?}, {:?}, {:?})),",
            inside.x, inside.y, inside.z, look.x, look.y, look.z
        );
        cue.camera = Some(camera);
    }
    let now = time.elapsed_secs();
    for (what, _, fired) in cue
        .shots
        .iter_mut()
        .filter(|(_, at, fired)| !*fired && now >= *at)
    {
        *fired = true;
        shoot(what, (outward, aim, foot), &mut commands);
    }
}

/// Fire `what` at the wall (its outward normal, aim point and foot, world): a round square to it.
fn shoot(what: &What, (outward, aim, foot): (Vec3, Vec3, Vec3), commands: &mut Commands) {
    match what {
        What::Round(round) => {
            let along = -outward;
            let origin = aim - along * STANDOFF_M;
            commands.trigger(FireShell::of_round(
                round,
                origin,
                Dir3::new(along).unwrap_or(Dir3::NEG_Z),
                crate::spec::FireMechanism::Single,
                true,
                None,
                None,
            ));
            info!("capture: wall round from {origin:?} at {aim:?}");
        }
        &What::Charge { mass } => {
            let registry = SubstanceRegistry::shipped_static();
            let core = registry
                .keys()
                .into_iter()
                .find_map(|name| registry.get(name).ok().and_then(|s| s.burst()))
                .filter(|core| core.chemical_energy.is_some());
            let (Some(core), at) = (core, foot + Vec3::Y * 0.3) else {
                error!("capture: no shipped burst core states a chemical energy");
                return;
            };
            let radius = (3.0 * mass / (4.0 * std::f32::consts::PI * core.density as f32)).cbrt();
            let energy = mass * core.chemical_energy.unwrap_or(0.0) as f32;
            let at = at + outward * radius;
            commands.trigger(WallCharge {
                at,
                blast: Blast::of(energy),
            });
            info!("capture: wall charge of {mass} kg ({energy:.0} J) at {at:?}");
        }
    }
}

/// Hold the player's camera on the wall from the frame the cue found it.
fn hold_camera(
    cue: Res<Cue>,
    mut cameras: Query<
        &mut Transform,
        (
            With<crate::view::PlayerView>,
            Without<crate::tank::Controlled>,
        ),
    >,
) {
    if let Some(pose) = cue.camera {
        for mut camera in &mut cameras {
            *camera = pose;
        }
    }
}
