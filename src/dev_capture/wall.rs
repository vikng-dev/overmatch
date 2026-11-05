//! The capture's wall cue, in the game's environment destruction (game state only): on the first
//! frame at or after each shot's app second, a shipped round is fired square at a house wall or a
//! bare charge is set off against the foot of it, and the camera stands in front of that wall.
//!
//! The capture plan's `wall` ([`super::plan::Wall`]), on the offline root
//! ([`offline_wall_plugin`]); each shot goes off at its own second, all at one wall. The dedicated
//! server mounts the same cue from the same plan file ([`server_wall_plugin`]): it fires and draws
//! nothing, and logs the camera its capture clients hold as a plan fragment (a `frames.camera`
//! value), so every peer frames the one break the server made. The wall is chosen by class, the
//! same on every run: the construction nearest the map's centre (the lowest ordinal on a tie), its
//! breakable element of the largest volume (the lowest element on a tie). A round is fired out
//! along the pier's outward normal at the chunk nearest the pier's centre; `assets/dev/capture-wall.ron`
//! owns the staging distances (m). A charge sits on the pier's outer face near its foot, its
//! energy its named burst core's chemical energy per kilogram
//! times its mass, with no casing.
//!
//! When the plan's `frames.camera` places the camera (a pose or a follow), the cue leaves the
//! camera to it.
//!
//! A PREVIEW BESIDE THE HOUSE. The plan's `wall.preview` places shipped constructions (a trunk,
//! say) beside the cue's house on the offline root alone ([`place_preview`]), dev only: the
//! shipped map never places them, and the wall stays the house's. A round with an `aim` is fired
//! at a point the plan names in a named construction's own frame, arriving along the plan's
//! direction, so a shot is chosen by data and never by the kind of element it meets.

use bevy::prelude::*;

use super::plan::{PlanError, Preview};
use crate::ballistics::FireShell;
use crate::construction::breakage::{Breakage, Charge as ConstructionCharge};
use crate::construction::fracture::Blast;
use crate::scatter::MapInstance;
use crate::substances::SubstanceRegistry;

/// The game's damage model: a resolved wall cue, constructed at plan admission.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum What {
    /// Boxed: a fired round carries its whole muzzle emission.
    Round(Box<crate::rounds::Round>),
    Charge(Charge),
}

/// The game's damage model: a complete bare charge whose derived quantities fit simulation units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Charge {
    mass: f32,
    blast: Blast,
}

#[derive(Debug)]
pub(super) enum CueError {
    Round(super::RoundCueError),
    Core(crate::substances::SubstanceError),
    NoBurst { core: String },
    Mass { kg: f32 },
    Derived { kg: f32, quantity: &'static str },
}

impl std::fmt::Display for CueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Round(error) => error.fmt(f),
            Self::Core(error) => write!(
                f,
                "wall.shots.core: {error}; name a materials.ron row with burst facts"
            ),
            Self::NoBurst { core } => write!(
                f,
                "wall.shots.core: {core:?} states no burst facts; name a materials.ron row that does"
            ),
            Self::Mass { kg } => write!(
                f,
                "wall.shots.kg: {kg} is not finite and positive; supply a finite positive mass"
            ),
            Self::Derived { kg, quantity } => write!(
                f,
                "wall.shots.kg: {kg} kg produces {quantity} outside finite positive simulation units; reduce kg for overflow or increase it for underflow"
            ),
        }
    }
}

impl What {
    pub(super) fn round(key: &str, mode: u8) -> Result<Self, CueError> {
        let round = super::resolve_round(key, mode, "wall.shots").map_err(CueError::Round)?;
        Ok(Self::Round(Box::new(round)))
    }

    pub(super) fn charge(kg: f32, name: &str) -> Result<Self, CueError> {
        if !(kg.is_finite() && kg > 0.0) {
            return Err(CueError::Mass { kg });
        }
        let core = SubstanceRegistry::shipped_static()
            .get(name)
            .map_err(CueError::Core)?
            .burst()
            .ok_or_else(|| CueError::NoBurst { core: name.into() })?;
        let radius = (3.0 * kg / (4.0 * std::f32::consts::PI * core.density as f32)).cbrt();
        let energy = kg * core.chemical_energy as f32;
        // The game's damage model: bare products expand at the burst law's no-casing speed, K sqrt(2).
        let products = crate::ballistics::burst::products_momentum(
            kg,
            core.speed_constant as f32 * std::f32::consts::SQRT_2,
        );
        for (quantity, value) in [
            ("radius", radius),
            ("energy", energy),
            ("products momentum", products),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err(CueError::Derived { kg, quantity });
            }
        }
        Ok(Self::Charge(Charge {
            mass: kg,
            blast: Blast::of(energy, radius, products),
        }))
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Staging {
    round_standoff_m: crate::provenance::Sourced<f32>,
    camera_out_m: crate::provenance::Sourced<f32>,
    camera_up_m: crate::provenance::Sourced<f32>,
    inside_inset_m: crate::provenance::Sourced<f32>,
    charge_height_m: crate::provenance::Sourced<f32>,
}

fn staging() -> &'static Staging {
    static STAGING: std::sync::LazyLock<Staging> = std::sync::LazyLock::new(|| {
        crate::provenance::shipped("assets/dev/capture-wall.ron", || {
            let staging: Staging = ron::from_str(include_str!("../../assets/dev/capture-wall.ron"))
                .map_err(|error| error.to_string())?;
            for (field, value) in [
                ("round_standoff_m", staging.round_standoff_m.0),
                ("camera_out_m", staging.camera_out_m.0),
                ("camera_up_m", staging.camera_up_m.0),
                ("inside_inset_m", staging.inside_inset_m.0),
                ("charge_height_m", staging.charge_height_m.0),
            ] {
                if !(value.is_finite() && value > 0.0) {
                    return Err(format!(
                        "{field}: supply a finite positive distance in metres"
                    ));
                }
            }
            Ok(staging)
        })
        .unwrap_or_else(|error| panic!("assets/dev/capture-wall.ron: {error}"))
    });
    &STAGING
}

/// The cue's shots in order, each with its second, its aim and whether it went off; the
/// preview; the framing once found.
#[derive(Resource)]
struct Cue {
    shots: Vec<(What, f32, Option<BoundAim>, bool)>,
    preview: Option<Preview>,
    camera: Option<Transform>,
}

/// Startup binds named targets after preview placement; firing only reads the retained entity.
struct BoundAim {
    target: Option<Entity>,
    at: Vec3,
    toward: Dir3,
}

/// A construction the plan's preview placed: never the cue's house.
#[derive(Component)]
struct Previewed;

/// The map's constructions the cue may take for its house.
type Houses<'w, 's> = Query<
    'w,
    's,
    (&'static MapInstance, &'static Transform, &'static Breakage),
    Without<Previewed>,
>;

pub(crate) fn offline_wall_plugin(app: &mut App) {
    let Some(cue) = cue_from_plan() else {
        return;
    };
    app.insert_resource(cue)
        .insert_resource(crate::camera::CameraCaptive(true))
        .add_systems(PostStartup, (place_preview, check_wall_target).chain())
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
        app.insert_resource(cue)
            .add_systems(PostStartup, check_wall_target)
            .add_systems(Update, fire_on_cue);
    }
}

/// The plan's `wall` cue, `None` when the plan has none.
fn cue_from_plan() -> Option<Cue> {
    let wall = super::plan()?.wall.as_ref()?;
    staging();
    info!("capture: game's environment destruction wall cue {wall:?}");
    Some(Cue {
        shots: Vec::new(), // check_wall_target binds the shots before Update can fire them.
        preview: wall.preview.clone(),
        camera: None,
    })
}

/// The camera looks at the middle of the aim point and foot; [`staging`] owns its offsets (m).
fn framing(outward: Dir3, aim: Vec3, foot: Vec3) -> Transform {
    let look = (aim + foot) / 2.0;
    Transform::from_translation(
        look + outward * staging().camera_out_m.0 + Vec3::Y * staging().camera_up_m.0,
    )
    .looking_at(look, Vec3::Y)
}

/// The wall (module doc): the pier's outward normal, the aim point on its outer face, the foot of
/// that face, and a point inset past the construction's middle from the pier, at the aim's
/// height, all world.
fn the_wall((root, breakage): (&Transform, &Breakage)) -> Option<(Dir3, Vec3, Vec3, Vec3)> {
    let kind = &breakage.kind;
    let layout = &kind.layout;
    let (lo, hi) = kind.bounds();
    let outward_of = |e: usize| {
        let w = layout.element_blocks[e].thinnest().0;
        let centre = layout.element_blocks[e].center();
        if (centre - (lo + hi) / 2.0).dot(w) >= 0.0 {
            w
        } else {
            -w
        }
    };
    // An element's chunks tile it, so their volumes sum to its own.
    let volume = |e: usize| -> f32 {
        let [first, end] = kind.asset.elements[e].chunks;
        kind.asset.chunks[first as usize..end as usize]
            .iter()
            .map(|chunk| chunk.volume_m3)
            .sum()
    };
    let e = (0..kind.asset.elements.len())
        .filter(|&e| layout.elements[e].is_some())
        .max_by(|&a, &b| volume(a).total_cmp(&volume(b)).then(b.cmp(&a)))?;
    let block = &layout.element_blocks[e];
    let centre = block.center();
    let outward = outward_of(e);
    let outer = block
        .points
        .iter()
        .map(|p| p.dot(outward))
        .fold(f32::NEG_INFINITY, f32::max);
    let on_face = |p: Vec3| p + outward * (outer - p.dot(outward));
    let aim = layout
        .chunks_of(e)
        .map(|c| Vec3::from_array(kind.asset.chunks[layout.chunks[c] as usize].centroid))
        .min_by(|a, b| a.distance(centre).total_cmp(&b.distance(centre)))?;
    let foot = Vec3::new(centre.x, block.min.y.max(0.0), centre.z);
    let world = |p: Vec3| root.translation + root.rotation * p;
    let middle = (lo + hi) / 2.0;
    let inside = Vec3::new(middle.x, aim.y, middle.z) - outward * staging().inside_inset_m.0;
    Some((
        Dir3::new(root.rotation * outward)
            .expect("certified construction geometry has a finite nonzero wall normal"),
        world(on_face(aim)),
        world(on_face(foot)),
        world(inside),
    ))
}

/// The construction the cue works on (module doc): its pose and breakable state.
fn the_house<'a>(constructions: &'a Houses) -> Option<(&'a Transform, &'a Breakage)> {
    let reach = |t: &Transform| Vec2::new(t.translation.x, t.translation.z).length_squared();
    constructions
        .iter()
        .min_by(|a, b| {
            reach(a.1)
                .total_cmp(&reach(b.1))
                .then(a.0.ordinal.cmp(&b.0.ordinal))
        })
        .map(|(_, transform, breakage)| (transform, breakage))
}

/// The wall, resolved at boot: the map's constructions spawn at `Startup`, from data, so a plan whose
/// `wall` cue finds no breakable house with a pier fails the run before any frame.
fn check_wall_target(
    constructions: Houses,
    targets: Query<(Entity, &MapInstance), With<Breakage>>,
    mut cue: ResMut<Cue>,
) {
    assert!(
        the_house(&constructions).and_then(the_wall).is_some(),
        "{}: wall: the map has no breakable house with a pier to shoot at",
        super::plan::VARIABLE
    );
    let wall = super::plan()
        .and_then(|plan| plan.wall.as_ref())
        .expect("the wall plugin requires a wall plan");
    cue.shots = wall.shots.iter().map(|shot| {
        let aim = shot.aim.as_ref().map(|aim| {
            let target = aim.instance.as_ref().map(|name| {
                targets.iter().find(|(_, identity)| &identity.id == name).map(|(entity, _)| entity).unwrap_or_else(|| {
                    super::plan::refuse(PlanError::Field(format!("wall.shots.aim.instance: {name:?} is not placed by this root; use offline mode for wall.preview targets or name a map construction")))
                })
            });
            BoundAim { target, at: Vec3::from(aim.at), toward: aim.toward }
        });
        (shot.what.clone(), shot.at, aim, false)
    }).collect();
}

/// Find the wall and frame it (logging the pose), then fire the cue on its second.
///
/// # Panics
///
/// When a shot is due and the wall it was cued at is gone: a run that went on would frame a shot
/// that never happened.
fn fire_on_cue(
    time: Res<Time>,
    mut cue: ResMut<Cue>,
    constructions: Houses,
    targets: Query<(&Transform, &Breakage)>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    let house = the_house(&constructions);
    let Some((outward, aim, foot, inside)) = house.and_then(the_wall) else {
        assert!(
            !cue.shots
                .iter()
                .any(|(_, at, _, fired)| !*fired && now >= *at),
            "{}: wall: a shot is due at {now} s and the house's wall is gone",
            super::plan::VARIABLE
        );
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
    for (what, _, aimed, fired) in cue
        .shots
        .iter_mut()
        .filter(|(_, at, _, fired)| !*fired && now >= *at)
    {
        *fired = true;
        let wall = match aimed {
            Some(aimed) => aimed_at(
                aimed,
                house.expect("the wall was found on this house"),
                &targets,
            ),
            None => (outward, aim, foot),
        };
        shoot(what, wall, &mut commands);
    }
}

/// The game's damage-model aim, bound at startup, transformed through its construction's pose.
fn aimed_at(
    aimed: &BoundAim,
    house: (&Transform, &Breakage),
    targets: &Query<(&Transform, &Breakage)>,
) -> (Dir3, Vec3, Vec3) {
    let (root, _) = match aimed.target {
        None => house,
        Some(entity) => targets
            .get(entity)
            .expect("a bound capture construction stays spawned until its cue fires"),
    };
    let at = root.translation + root.rotation * aimed.at;
    (-(root.rotation * aimed.toward), at, at)
}

/// Stand the plan's preview constructions beside the cue's house (module doc), each on the ground at
/// its own XZ as the map stands its own (the height grid there), drawn with the constructions' one
/// material where the root draws, at unit scale, and frame them with the preview's camera.
///
/// # Panics
///
/// When the map has no house, or a preview asset is not a shipped construction the map's load
/// gate admits: the plan names one.
fn place_preview(
    mut cue: ResMut<Cue>,
    houses: Houses,
    placed: Query<&MapInstance>,
    grid: Res<crate::terrain_grid::HeightGrid>,
    look: Option<Res<crate::construction::view::Look>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Some(preview) = &cue.preview else {
        return; // The plan asks for no preview.
    };
    let (house, _) = the_house(&houses).unwrap_or_else(|| {
        panic!(
            "{}: wall.preview: the map has no house to stand the preview beside",
            super::plan::VARIABLE
        )
    });
    let assets: std::collections::BTreeSet<&str> = preview
        .place
        .iter()
        .map(|placed| placed.asset.as_str())
        .collect();
    let library =
        crate::construction::ConstructionLibrary::load(&crate::assets::asset_root(), assets);
    let first = placed
        .iter()
        .map(|identity| identity.ordinal + 1)
        .max()
        .expect("the map places the house");
    for (ordinal, placed) in (first..).zip(&preview.place) {
        let kind = library
            .get(&placed.asset)
            .expect("the preview library loaded every preview asset");
        let (x, z) = placed.offset;
        let mut at = house.translation + house.rotation * Vec3::new(x, 0.0, z);
        at.y = grid.height_at(at.x, at.z);
        let pose = Isometry3d::new(
            at,
            house.rotation * Quat::from_rotation_y(placed.yaw_deg.to_radians()),
        );
        let identity = MapInstance {
            id: placed.id.clone(),
            ordinal,
        };
        let root = crate::construction::spawn(&mut commands, kind, identity, pose);
        commands.entity(root).insert(Previewed);
        if let Some(look) = &look {
            commands.spawn((
                ChildOf(root),
                Transform::IDENTITY,
                Mesh3d(meshes.add(crate::construction::view::mesh(kind))),
                MeshMaterial3d(look.surface.clone()),
                crate::render_cost::CasterClass::Construction,
                crate::construction::view::View,
            ));
        }
        info!(
            "capture: wall preview {} ({}) stands at {:?}",
            placed.id, placed.asset, pose.translation
        );
    }
    let world = |p: (f32, f32, f32)| house.translation + house.rotation * Vec3::from(p);
    let (eye, look) = (world(preview.eye), world(preview.look));
    info!(
        "capture: wall preview — camera: Pose(eye: ({:?}, {:?}, {:?}), look: ({:?}, {:?}, {:?})),",
        eye.x, eye.y, eye.z, look.x, look.y, look.z
    );
    cue.camera = Some(Transform::from_translation(eye).looking_at(look, Vec3::Y));
}

/// Fire `what` at the wall (its outward normal, aim point and foot, world): a round square to it.
fn shoot(what: &What, (outward, aim, foot): (Dir3, Vec3, Vec3), commands: &mut Commands) {
    match what {
        What::Round(round) => {
            let along = -outward;
            let origin = aim - along * staging().round_standoff_m.0;
            commands.trigger(FireShell::of_round(
                round,
                origin,
                origin,
                along,
                crate::spec::FireMechanism::Single,
                true,
                None,
                None,
            ));
            info!("capture: wall round from {origin:?} at {aim:?}");
        }
        &What::Charge(Charge { mass, blast }) => {
            let at = foot + Vec3::Y * staging().charge_height_m.0 + outward * blast.radius;
            commands.trigger(ConstructionCharge { at, blast });
            info!(
                "capture: wall charge of {mass} kg ({:.0} J) at {at:?}",
                blast.energy
            );
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

/// Capture plugins require the shipped staging file to carry valid sourced distances;
/// a missing field, invalid magnitude or Fixture provenance must refuse this load.
#[cfg(test)]
#[test]
fn shipped_staging_passes_capture_admission() {
    staging();
}
