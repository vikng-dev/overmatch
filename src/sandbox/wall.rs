//! The wall prototype, about the game's environment destruction (game state only): a brick wall on
//! the range whose chunks the range's rounds and a blast turn to dust or crack loose. A prototype
//! isolated to the sandbox: nothing here is wired into the game's sim or its net.
//!
//! * **The wall.** One wall of the baked house (`assets/buildings/house_1/house_1.building.ron`,
//!   [`WALL`]'s brick elements), set down at [`PLACE`] by [`baked::wall_cut`]: the bake's convex
//!   chunks, each a static collider with the `Brick` substance, so the walk crosses them as it
//!   crosses any hard volume, and the bake's joints and ground anchors.
//! * **Deposits.** A round's crossing deposits, in each chunk, the energy the walk says it lost
//!   there ([`ArmourMark::Crossing`]), and pushes it with the momentum the round lost. The blast
//!   tool (O) bursts a charge of 0.1–5 kg at 0–5 m standoff: a sphere of rays ([`blast_field`])
//!   each stopping on the first chunk it meets with its share of the energy, pushing the face it
//!   meets with the blast's reflected impulse ([`model::Blast::push`]).
//! * **Responses.** The game's fracture laws ([`crate::buildings::fracture`], through [`model`])
//!   turn each deposit into dust (the pools), local cracks (the push rule) and structural cracks
//!   (the structural check, then the weight check on what changed), then let every group with no
//!   path to the ground fall with the momentum pushed into it.
//! * **The view.** Each chunk's pool as heat, local cracks in cyan and structural ones in
//!   magenta, a dust puff where a chunk went, loose groups as falling bodies on the default layer
//!   (out of ballistics; grey-blue when a local crack freed them, purple when a structural one
//!   did), a one-line readout (the last shot and its energy) and, on H, a help panel of the keys
//!   and the colour legend. Every value of a resolved shot goes to the log. The readout shows only
//!   while the camera stands within [`READOUT_RANGE`] of the wall, so the range's own view carries
//!   no more text than its round's name.
//! * **Spall.** Every exit cone the wall throws is read off the shell's record and drawn, and its
//!   rays' doses are summed on a witness panel behind the wall: the game's own dose.
//!
//! Keys: B round, F fire it, O blast and `,`/`.` its filler mass, `;`/`'` its standoff, V its
//! casing, G camera (front, back, witness), N new wall, H help.
//!
//! `OVERMATCH_WALL_CASE=<round key | x<kg>@<standoff m>>` (`88mm_AP`, `x1@3`) runs one case for a
//! hidden capture ([`drive_script`]): the camera stands at the front, then the back, fires (or
//! bursts) at [`SCRIPT_FIRE`] s from the front, and stands front then back again for the after
//! frames. `OVERMATCH_WALL_CASING=bare`, `OVERMATCH_WALL_VIEW=witness` (the last frames from the
//! witness pose) and `OVERMATCH_WALL_HELP=1` (the help panel open) set the rest.

mod baked;
mod model;
#[cfg(test)]
mod tests;

use avian3d::prelude::{
    AngularVelocity, Collider, ColliderDensity, CollisionLayers, LayerMask, LinearVelocity,
    RigidBody,
};
use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::SystemParam;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use self::baked::Placement;
use self::model::{Block, Cut, Loading, Masonry, Push, SectionCache, WallState};
use super::{Autopilot, FreeFlyCam, SEED_SPEC, UI_LEFT, panel_capturing};
use crate::ballistics::{
    ArmourMark, BallisticVolume, CrossingEnd, FireShell, PenetrationMarks, SpallMarks,
};
use crate::buildings::schema::BuildingAsset;
use crate::substances::SubstanceRegistry;
use crate::{Layer, spec};

/// The baked building the wall comes from, and which of its walls (an element's name begins
/// `<wall>/`): the brick elements of that wall.
const BUILDING: &str = include_str!("../../assets/buildings/house_1/house_1.building.ron");
const WALL: &str = "east";
/// Where the wall stands, world metres: clear of the range's plates and target (x within ±7.5)
/// and of the test course (z ≤ −30). Its front face looks down +Z.
const PLACE: Placement = Placement {
    centre_x: 16.0,
    back_z: 3.875,
};
/// The substance every chunk wears; its values are its row's ([`Masonry::of`]).
const SUBSTANCE: &str = "Brick";
/// The blast tool's filler masses, kg, and standoffs, m (the owner's 0.1–5 kg and 0–5 m).
const MASSES: [f32; 6] = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0];
const STANDOFFS: [f32; 6] = [0.0, 0.5, 1.0, 2.0, 3.0, 5.0];
/// Rounds an automatic gun fires per F (the brief's burst of 50).
const BURST: usize = 50;
/// The burst's spread half-angle, rad. ASSUMED: no gun file states a dispersion; a tight group so
/// a burst's wear lands on a few chunks.
const BURST_SPREAD: f32 = 0.003;
/// How far the drawn chunk falls short of its collider on every side, m: the drawn seam.
const SEAM: f32 = 0.004;
/// How far a loose chunk's collider falls short of its box on every side, m, so a group freed in
/// place does not start inside its standing neighbours.
const LOOSE_GAP: f32 = 0.005;
/// The witness panel: its distance behind the back face (past the back view's camera), its tile
/// size and count (m, m, x × y).
const WITNESS_GAP: f32 = 4.4;
const TILE: f32 = 0.25;
const TILES: (usize, usize) = (24, 14);
/// The camera's distance from the face it frames, in wall heights: the wall fills two thirds of a
/// 45 degree 16:9 view's height, below the readout.
const VIEW_FILL: f32 = 1.8;
/// A dust puff's life, s.
const PUFF_LIFE: f32 = 1.6;
/// How near the wall's centre the camera must stand for the readout to show, m: the wall's own
/// views stand 5–9 m off it, the range's boot pose 21 m.
const READOUT_RANGE: f32 = 12.0;
/// Blast rays drawn of the last blast: every this-many-th hit.
const RAY_DRAW_STRIDE: usize = 6;
/// The capture script's timeline, app seconds: back view from, front view from, the shot, and the
/// after back view from. Frames read at 3, 4, 11.5 and 12.5 s fall inside each stand.
const SCRIPT_BACK: f32 = 3.5;
const SCRIPT_FRONT: f32 = 4.5;
const SCRIPT_FIRE: f32 = 5.0;
const SCRIPT_BACK_AGAIN: f32 = 12.0;

pub(super) fn plugin(app: &mut App) {
    let rounds = wall_rounds();
    let script = std::env::var("OVERMATCH_WALL_CASE")
        .ok()
        .and_then(|case| parse_case(&case, &rounds));
    let flag = |name: &str, value: &str| std::env::var(name).as_deref() == Ok(value);
    let controls = WallControls {
        mass: 3,
        standoff: 0,
        cased: !flag("OVERMATCH_WALL_CASING", "bare"),
        round: 0,
        view: View::Front,
        help: flag("OVERMATCH_WALL_HELP", "1"),
    };
    if let Some(case) = script {
        info!("wall prototype: capture case {case:?}");
        // The range keeps the cursor free under an autopilot; the script is one.
        if !app.world().contains_resource::<Autopilot>() {
            app.insert_resource(Autopilot {
                pose: None,
                fly: None,
                fire: Default::default(),
            });
        }
        app.insert_resource(WallScript {
            case,
            witness: flag("OVERMATCH_WALL_VIEW", "witness"),
            done: false,
        })
        .add_systems(Update, drive_script.before(fire_burst));
    }
    let charge = Charge::of(&rounds);
    app.insert_resource(WallRounds(rounds))
        .insert_resource(charge)
        .insert_resource(BlastField(blast_field()))
        .insert_resource(controls)
        .init_resource::<WallQueue>()
        .init_resource::<WallLog>()
        .init_resource::<WallBurst>()
        .init_gizmo_group::<WallGizmos>()
        .add_systems(Startup, (setup_wall, spawn_readout))
        .add_systems(
            Update,
            (
                wall_keys.run_if(not(panel_capturing)),
                fire_burst,
                read_hits,
                apply_deposits,
                (draw_wall, paint_witness, animate_puffs, update_readout),
            )
                .chain(),
        );
}

/// The wall's gizmos: drawn a little toward the camera so a crack on a face beats the face.
#[derive(Default, Reflect, GizmoConfigGroup)]
struct WallGizmos;

/// One chunk's static entity: its index in the cut.
#[derive(Component)]
struct WallChunk(usize);

/// A loose group's body, gone at a new wall.
#[derive(Component)]
struct WallPiece;

/// A witness tile: its index, row-major from the panel's low corner.
#[derive(Component)]
struct WitnessTile(usize);

/// A dust puff and the material it fades.
#[derive(Component)]
struct DustPuff {
    born: f32,
    material: Handle<StandardMaterial>,
}

/// How much of a shell's record the wall has read: its armour log entries and spall cones.
#[derive(Component)]
struct WallRead {
    log: usize,
    cones: usize,
}

/// The readout line: the active rule, the structural check and the last shot.
#[derive(Component)]
struct WallReadout;

/// The help panel (H): the keys and the colour legend.
#[derive(Component)]
struct WallHelp;

/// The wall as it stands: its cut, its state, which cracks the structural check made and the
/// check's sections of the cut, each standing chunk's entity and drawn mesh, and its material.
#[derive(Resource)]
struct Wall {
    cut: Cut,
    state: WallState,
    structural: Vec<bool>,
    sections: SectionCache,
    entities: Vec<Option<Entity>>,
    meshes: Vec<Handle<Mesh>>,
    material: Masonry,
}

/// The wall as the bake gives it, set down in the world: its cut and its bounding box.
struct Layout {
    cut: Cut,
    min: Vec3,
    size: Vec3,
}

impl Layout {
    fn centre(&self) -> Vec3 {
        self.min + self.size / 2.0
    }
}

/// The wall's layout, read once from [`BUILDING`]. A building the bake wrote and the build
/// shipped: one that does not parse is a broken build.
fn layout() -> &'static Layout {
    static LAYOUT: std::sync::LazyLock<Layout> = std::sync::LazyLock::new(|| {
        let asset = BuildingAsset::from_ron(BUILDING).expect("the shipped building parses");
        let cut = baked::wall_cut(&asset, WALL, SUBSTANCE, brick(), PLACE)
            .expect("the shipped building has the wall");
        let (min, max) = cut.blocks.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), block| (lo.min(block.min), hi.max(block.max)),
        );
        Layout {
            cut,
            min,
            size: max - min,
        }
    });
    &LAYOUT
}

/// The camera stand G steps through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Front,
    Back,
    Witness,
}

/// The wall's settings: the blast tool's mass and standoff (indices into [`MASSES`],
/// [`STANDOFFS`]), whether the charge is cased, the round F fires (an index into
/// [`WallRounds`]), the camera stand and whether the help panel is open.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
struct WallControls {
    mass: usize,
    standoff: usize,
    cased: bool,
    round: usize,
    view: View,
    help: bool,
}

/// One round F fires: its data key and display name, the round, its gun's fire mechanism, and how
/// many it fires per F at what interval (s).
#[derive(Clone, Debug)]
struct WallRound {
    key: String,
    name: String,
    round: crate::rounds::Round,
    mechanism: spec::FireMechanism,
    count: usize,
    interval: f32,
}

/// Every round each of the seed tank's guns chambers, the primary's first.
#[derive(Resource)]
struct WallRounds(Vec<WallRound>);

/// The blast tool's charge, from data: the filler's density (kg/m³) and chemical energy (J/kg),
/// and the casing of the seed tank's burst round as the burst law derives it — its mass per kg of
/// filler `M/C` and the speed it is driven to, `v_s = K (M/C + ½)^−½` (m/s). A charge of `C` kg
/// carries `M/C × C` kg of it.
#[derive(Resource, Clone, Copy, Debug)]
struct Charge {
    density: f32,
    chemical: f32,
    casing_ratio: f32,
    casing_speed: f32,
}

impl Charge {
    /// The filler is the registry's burst-core substance; the casing is the first burst round's
    /// among the wall's rounds, read back from its burst (`M = 2E / v_s²`, `M/C = (K / v_s)² − ½`).
    /// A bare charge where the data names none.
    fn of(rounds: &[WallRound]) -> Self {
        let registry = SubstanceRegistry::shipped_static();
        let core = registry
            .keys()
            .into_iter()
            .find_map(|name| registry.get(name).ok().and_then(|s| s.burst()));
        let burst = rounds.iter().find_map(|round| round.round.burst);
        match (core, burst) {
            (Some(core), Some(burst)) => {
                let speed = f64::from(burst.sideways_speed);
                Self {
                    density: core.density as f32,
                    chemical: core.chemical_energy.unwrap_or(0.0) as f32,
                    casing_ratio: ((core.speed_constant / speed).powi(2) - 0.5) as f32,
                    casing_speed: burst.sideways_speed,
                }
            }
            (Some(core), None) => Self {
                density: core.density as f32,
                chemical: core.chemical_energy.unwrap_or(0.0) as f32,
                casing_ratio: 0.0,
                casing_speed: 0.0,
            },
            _ => Self {
                density: 1650.0,
                chemical: 0.0,
                casing_ratio: 0.0,
                casing_speed: 0.0,
            },
        }
    }

    /// A charge of `mass` kg: its chemical energy, its casing's driven energy (none when bare) and
    /// the blast's energy (the difference), all J, the TNT charge that carries it (kg,
    /// [`model::Blast::of`]), and its radius (m).
    fn blast(&self, mass: f32, cased: bool) -> Blast {
        let chemical = self.chemical * mass;
        let casing = if cased {
            0.5 * self.casing_ratio * mass * self.casing_speed * self.casing_speed
        } else {
            0.0
        };
        let energy = (chemical - casing).max(0.0);
        Blast {
            mass,
            chemical,
            casing,
            energy,
            charge: model::Blast::of(energy).charge,
            radius: (3.0 * mass / (4.0 * std::f32::consts::PI * self.density)).cbrt(),
        }
    }
}

/// One charge's energies (J), TNT charge (kg) and radius (m), filler `mass` kg.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Blast {
    mass: f32,
    chemical: f32,
    casing: f32,
    energy: f32,
    charge: f32,
    radius: f32,
}

/// The blast field's directions with their shares (summing to 1): the field machinery's ball at
/// ratio 0, uniform over the sphere ([`crate::ballistics::spall_cone`]).
#[derive(Resource)]
struct BlastField(Vec<(Vec3, f32)>);

fn blast_field() -> Vec<(Vec3, f32)> {
    model::blast_directions().to_vec()
}

/// The rounds of a burst still to fire: which round, how many fired, when the next goes (app s).
#[derive(Resource, Default)]
struct WallBurst {
    round: usize,
    fired: usize,
    left: usize,
    next: f32,
}

/// A deposit waiting for the model.
#[derive(Debug)]
enum Deposit {
    /// One round's crossing of the wall: per chunk the line it cut (entry, end) and the energy it
    /// lost there (J); its travel and the momentum it lost across the wall (N·s).
    Line {
        what: String,
        lines: Vec<(usize, Vec3, Vec3, f32)>,
        along: Vec3,
        momentum: f32,
        /// The strength (J) of the exit cones it threw.
        spall: f32,
    },
    /// A charge of filler `mass` kg, its surface `standoff` m before the face point `at`, along
    /// the face's inward normal `into`.
    Blast {
        mass: f32,
        standoff: f32,
        at: Vec3,
        into: Vec3,
    },
}

#[derive(Resource, Default)]
struct WallQueue(Vec<Deposit>);

/// One exit cone the wall threw: its origin, and its rays' ends and strengths (J).
struct WallCone {
    origin: Vec3,
    rays: Vec<(Vec3, f32)>,
}

/// What the readout and the drawing keep: the last outcome, the cones thrown, each witness tile's
/// dose (J), and the last blast's origin, radius and drawn rays.
#[derive(Resource, Default)]
struct WallLog {
    last: Option<Outcome>,
    cones: Vec<WallCone>,
    witness: Vec<f32>,
    blast: Option<(Vec3, f32, Vec<Vec3>)>,
}

/// One deposit's outcome, for the readout.
#[derive(Clone, Debug, Default)]
struct Outcome {
    what: String,
    deposited: f32,
    chunks: usize,
    dust: usize,
    local: usize,
    structural: usize,
    sections: usize,
    /// The first push failure's and the first weight failure's demand and capacity.
    worst: Option<(f32, f32)>,
    weight: Option<(f32, f32)>,
    unassessed: usize,
    weight_steps: usize,
    groups: usize,
    fallen: usize,
    spall: f32,
    blast: Option<(Blast, f32, f32)>,
}

/// The capture script: its case, whether its last frames stand at the witness, and whether it has
/// fired.
#[derive(Resource)]
pub(super) struct WallScript {
    case: Case,
    witness: bool,
    done: bool,
}

/// What a script case fires.
#[derive(Clone, Copy, Debug)]
enum Case {
    /// The [`WallRounds`] entry at this index.
    Round(usize),
    /// A blast of filler `mass` kg at `standoff` m.
    Blast { mass: f32, standoff: f32 },
}

/// Materials and meshes the wall draws with: the chunks' heat ladder (full pool first), a loose
/// chunk's when a local crack freed it and when a structural one did, the witness tiles' ladder
/// (no dose first), and the puff mesh.
#[derive(Resource)]
struct WallAssets {
    heat: Vec<Handle<StandardMaterial>>,
    loose: Handle<StandardMaterial>,
    toppled: Handle<StandardMaterial>,
    witness: Vec<Handle<StandardMaterial>>,
    puff: Handle<Mesh>,
}

/// The rounds of every gun the seed tank mounts, each gun's in key order, the primary's gun first;
/// an automatic gun's fire a [`BURST`] at its cyclic interval.
fn wall_rounds() -> Vec<WallRound> {
    let sheet: spec::TankSpec =
        ron::de::from_str(SEED_SPEC).expect("the shipped spec sheet parses");
    let mut guns: Vec<(bool, &str)> = sheet
        .weapons
        .values()
        .map(|weapon| {
            (
                weapon.trigger != spec::Trigger::Primary,
                weapon.gun.as_str(),
            )
        })
        .collect();
    guns.sort_unstable();
    guns.dedup();
    let registry = crate::guns::GunRegistry::shipped();
    let names = crate::rounds::RoundRegistry::shipped();
    let mut rounds = Vec::new();
    for (_, gun_key) in guns {
        let Ok(gun) = registry.gun(gun_key) else {
            continue;
        };
        let (count, interval) = match gun.mechanism() {
            crate::guns::Mechanism::Single => (1, 0.0),
            crate::guns::Mechanism::Automatic { cyclic_rpm } => (BURST, 60.0 / cyclic_rpm),
        };
        for key in names.keys() {
            if let Ok(round) = registry.round(gun_key, key) {
                rounds.push(WallRound {
                    key: key.to_owned(),
                    name: ascii(names.name(key).unwrap_or(key)),
                    round,
                    mechanism: gun.mechanism().family(),
                    count,
                    interval,
                });
            }
        }
    }
    rounds
}

/// `text` with every character outside printable ASCII replaced: the sandbox's default font draws
/// ASCII only.
fn ascii(text: &str) -> String {
    text.chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .collect()
}

/// `<round key | x<kg>@<standoff m>>`.
fn parse_case(case: &str, rounds: &[WallRound]) -> Option<Case> {
    let what = case.trim();
    if let Some(index) = rounds.iter().position(|round| round.key == what) {
        return Some(Case::Round(index));
    }
    let (mass, standoff) = what.strip_prefix('x')?.split_once('@')?;
    Some(Case::Blast {
        mass: mass.parse().ok()?,
        standoff: standoff.parse().ok()?,
    })
}

/// The brick as the wall reads it: the shipped registry's `Brick` row ([`Masonry::of`]).
fn brick() -> Masonry {
    Masonry::of(SubstanceRegistry::shipped_static(), SUBSTANCE)
        .expect("the shipped registry names Brick")
        .expect("the shipped Brick states its seam")
}

/// The front face's z and the back face's, world metres.
fn faces() -> (f32, f32) {
    let layout = layout();
    (layout.min.z + layout.size.z, layout.min.z)
}

/// Witness tile `index`'s box, row-major from the panel's low corner.
fn witness_tile(index: usize) -> model::Block {
    let (_, back) = faces();
    let low = Vec3::new(
        layout().centre().x - TILES.0 as f32 * TILE / 2.0,
        0.1,
        back - WITNESS_GAP - 0.01,
    );
    let (column, row) = (index % TILES.0, index / TILES.0);
    let min = low + Vec3::new(column as f32 * TILE, row as f32 * TILE, 0.0);
    model::Block::cuboid(min, min + Vec3::new(TILE, TILE, 0.01))
}

/// The tile a point lies on, within a millimetre.
fn witness_at(point: Vec3) -> Option<usize> {
    (0..TILES.0 * TILES.1).find(|&index| {
        let tile = witness_tile(index);
        (point.cmpge(tile.min - 1e-3) & point.cmple(tile.max + 1e-3)).all()
    })
}

/// The aim point: the front-face centre of the chunk whose centre is nearest the wall's centre.
fn aim_point(wall: &Wall) -> Vec3 {
    let centre = layout().centre();
    let block = wall
        .cut
        .blocks
        .iter()
        .min_by(|a, b| {
            a.center()
                .distance(centre)
                .total_cmp(&b.center().distance(centre))
        })
        .map_or(centre, Block::center);
    Vec3::new(block.x, block.y, faces().0)
}

/// The camera's pose at `view`, framing the wall about `aim`.
fn view_pose(view: View, aim: Vec3) -> Transform {
    let (front, back) = faces();
    let distance = VIEW_FILL * layout().size.y;
    let (eye, look) = match view {
        View::Front => (Vec3::new(aim.x, aim.y, front + distance), aim),
        View::Back => (
            Vec3::new(aim.x, aim.y, back - distance),
            Vec3::new(aim.x, aim.y, back),
        ),
        View::Witness => (
            Vec3::new(aim.x + 1.6, aim.y + 0.8, back - WITNESS_GAP - 2.8),
            Vec3::new(aim.x, aim.y, back - WITNESS_GAP),
        ),
    };
    Transform::from_translation(eye).looking_at(look, Vec3::Y)
}

fn setup_wall(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut gizmos: ResMut<GizmoConfigStore>,
) {
    let (config, _) = gizmos.config_mut::<WallGizmos>();
    config.line.width = 3.0;
    config.depth_bias = -0.05;
    let brick_colour = LinearRgba::from(Color::srgb(0.42, 0.16, 0.11));
    let hot = LinearRgba::from(Color::srgb(1.0, 0.85, 0.3));
    let heat = (0..=10)
        .map(|level| {
            let t = level as f32 / 10.0;
            materials.add(StandardMaterial {
                base_color: Color::LinearRgba(brick_colour.mix(&hot, t)),
                emissive: hot * (0.8 * t),
                perceptual_roughness: 0.9,
                ..default()
            })
        })
        .collect();
    let witness = (0..=8)
        .map(|level| {
            let colour = if level == 0 {
                Color::srgba(0.8, 0.8, 0.85, 0.12)
            } else {
                let t = (level - 1) as f32 / 7.0;
                Color::srgba(1.0, 0.9 - 0.75 * t, 0.2 - 0.15 * t, 0.65)
            };
            materials.add(StandardMaterial {
                base_color: colour,
                alpha_mode: AlphaMode::Blend,
                unlit: level > 0,
                ..default()
            })
        })
        .collect::<Vec<_>>();
    let assets = WallAssets {
        heat,
        loose: materials.add(StandardMaterial {
            base_color: Color::srgb(0.47, 0.5, 0.58),
            perceptual_roughness: 0.9,
            ..default()
        }),
        toppled: materials.add(StandardMaterial {
            base_color: Color::srgb(0.6, 0.3, 0.62),
            perceptual_roughness: 0.9,
            ..default()
        }),
        witness: witness.clone(),
        puff: meshes.add(Sphere::new(1.0)),
    };
    let wood = crate::ballistics::VolumeMaterial::of(SubstanceRegistry::shipped_static(), "Wood")
        .expect("the shipped registry names Wood");
    let tile_mesh = meshes.add(Cuboid::new(TILE - 0.01, TILE - 0.01, 0.01));
    for index in 0..TILES.0 * TILES.1 {
        let tile = witness_tile(index);
        let collider = Collider::cuboid(TILE, TILE, 0.01);
        commands.spawn((
            Name::new("witness tile"),
            WitnessTile(index),
            Mesh3d(tile_mesh.clone()),
            MeshMaterial3d(witness[0].clone()),
            Transform::from_translation(tile.center()),
            RigidBody::Static,
            envelope_of(&collider),
            collider,
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: wood,
                substance: "Wood".to_string(),
            },
        ));
    }
    // A fill behind the wall so its back face, in the sun's shadow, reads. A view light only; its
    // intensity is chosen to light the back face at about a third of the sun's illuminance.
    commands.spawn((
        Name::new("wall back fill"),
        PointLight {
            intensity: 1.5e6,
            range: 12.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_translation(
            layout().min + layout().size * Vec3::new(0.5, 0.7, 0.0) - Vec3::Z * 3.5,
        ),
    ));
    let wall = spawn_wall(&mut commands, &mut meshes, &assets);
    commands.insert_resource(wall);
    commands.insert_resource(assets);
}

/// A chunk's drawn mesh, vertices about its bounding box's centre: its faces inset by [`SEAM`].
fn chunk_mesh(block: &Block) -> Mesh {
    let inset = inset(block, SEAM);
    let (mut positions, mut normals, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for &(normal, offset) in &block.planes {
        let face = block.face_on(normal * offset, normal);
        let base = positions.len() as u32;
        for corner in &face {
            positions.push(inset(*corner).to_array());
            normals.push(normal.to_array());
        }
        for k in 1..face.len().saturating_sub(1) as u32 {
            indices.extend([base, base + k, base + k + 1]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_indices(Indices::U32(indices))
}

/// A map from a world point to its place about `block`'s bounding-box centre with every face moved
/// in by `gap` m: the corners scaled about their centroid.
fn inset(block: &Block, gap: f32) -> impl Fn(Vec3) -> Vec3 {
    let centre = block.center();
    let middle = block.points.iter().sum::<Vec3>() / block.points.len().max(1) as f32;
    let scale = (1.0 - gap / block.inradius().max(1e-6)).max(0.2);
    move |p| middle + (p - middle) * scale - centre
}

/// A convex collider of `block` with every face moved in by `gap` m, about its bounding box's
/// centre.
fn chunk_collider(block: &Block, gap: f32) -> Collider {
    let inset = inset(block, gap);
    Collider::convex_hull(block.points.iter().map(|p| inset(*p)).collect())
        .expect("a bake chunk has volume")
}

/// The envelope of a static body whose one collider is `collider`, at the body's origin.
fn envelope_of(collider: &Collider) -> crate::ballistics::Envelope {
    crate::ballistics::Envelope::holding([(collider, Vec3::ZERO, Quat::IDENTITY)])
        .expect("one part")
}

/// Spawn a new wall: one static chunk per block, full pools, nothing cracked.
fn spawn_wall(commands: &mut Commands, meshes: &mut Assets<Mesh>, assets: &WallAssets) -> Wall {
    let cut = layout().cut.clone();
    let brick_material =
        crate::ballistics::VolumeMaterial::of(SubstanceRegistry::shipped_static(), SUBSTANCE)
            .expect("the shipped registry names Brick");
    let mut entities = Vec::with_capacity(cut.blocks.len());
    let mut chunk_meshes = Vec::with_capacity(cut.blocks.len());
    for (index, block) in cut.blocks.iter().enumerate() {
        let mesh = meshes.add(chunk_mesh(block));
        chunk_meshes.push(mesh.clone());
        let collider = chunk_collider(block, 0.0);
        entities.push(Some(
            commands
                .spawn((
                    Name::new("wall chunk"),
                    WallChunk(index),
                    Mesh3d(mesh),
                    MeshMaterial3d(assets.heat[0].clone()),
                    Transform::from_translation(block.center()),
                    RigidBody::Static,
                    envelope_of(&collider),
                    collider,
                    CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                    BallisticVolume {
                        material: brick_material,
                        substance: SUBSTANCE.to_string(),
                    },
                ))
                .id(),
        ));
    }
    Wall {
        state: WallState::new(&cut.structure),
        structural: vec![false; cut.structure.joints.len()],
        sections: SectionCache::default(),
        cut,
        entities,
        meshes: chunk_meshes,
        material: brick(),
    }
}

/// The wall's keys (module doc).
#[derive(SystemParam)]
struct KeysParam<'w, 's> {
    keys: Res<'w, ButtonInput<KeyCode>>,
    controls: ResMut<'w, WallControls>,
    rounds: Res<'w, WallRounds>,
    wall: Option<Res<'w, Wall>>,
    queue: ResMut<'w, WallQueue>,
    burst: ResMut<'w, WallBurst>,
    camera: Single<'w, 's, &'static mut Transform, With<FreeFlyCam>>,
    time: Res<'w, Time>,
    rebuild: Rebuild<'w, 's>,
}

/// Step `index` down or up a ladder of `len` rungs on `down` / `up`.
fn step(keys: &ButtonInput<KeyCode>, down: KeyCode, up: KeyCode, index: &mut usize, len: usize) {
    if keys.just_pressed(down) {
        *index = index.saturating_sub(1);
    }
    if keys.just_pressed(up) {
        *index = (*index + 1).min(len - 1);
    }
}

fn wall_keys(mut p: KeysParam) {
    let keys = &p.keys;
    let mut controls = *p.controls;
    if keys.just_pressed(KeyCode::KeyH) {
        controls.help = !controls.help;
    }
    if keys.just_pressed(KeyCode::KeyV) {
        controls.cased = !controls.cased;
    }
    if keys.just_pressed(KeyCode::KeyB) && !p.rounds.0.is_empty() {
        controls.round = (controls.round + 1) % p.rounds.0.len();
    }
    step(
        keys,
        KeyCode::Comma,
        KeyCode::Period,
        &mut controls.mass,
        MASSES.len(),
    );
    step(
        keys,
        KeyCode::Semicolon,
        KeyCode::Quote,
        &mut controls.standoff,
        STANDOFFS.len(),
    );
    if keys.just_pressed(KeyCode::KeyG) {
        controls.view = match controls.view {
            View::Front => View::Back,
            View::Back => View::Witness,
            View::Witness => View::Front,
        };
        if let Some(wall) = p.wall.as_deref() {
            **p.camera = view_pose(controls.view, aim_point(wall));
        }
    }
    if keys.just_pressed(KeyCode::KeyF) {
        start_burst(
            &mut p.burst,
            controls.round,
            &p.rounds,
            p.time.elapsed_secs(),
        );
    }
    if keys.just_pressed(KeyCode::KeyO)
        && let Some(wall) = p.wall.as_deref()
    {
        let eye = p.camera.translation;
        let dir = p.camera.forward().as_vec3();
        let state = &wall.state;
        if let Some((_, t, _)) = model::first_hit(&wall.cut.blocks, |i| state.standing(i), eye, dir)
        {
            let at = eye + dir * t;
            p.queue.0.push(Deposit::Blast {
                mass: MASSES[controls.mass],
                standoff: STANDOFFS[controls.standoff],
                at,
                into: dir,
            });
        }
    }
    if keys.just_pressed(KeyCode::KeyN) {
        p.rebuild.rebuild();
    }
    p.controls.set_if_neq(controls);
}

/// Arm a burst of `round` from now.
fn start_burst(burst: &mut WallBurst, round: usize, rounds: &WallRounds, now: f32) {
    let Some(entry) = rounds.0.get(round) else {
        return;
    };
    *burst = WallBurst {
        round,
        fired: 0,
        left: entry.count,
        next: now,
    };
}

/// A new wall: the old one's chunks, pieces and puffs go, the log and the witness clear.
#[derive(SystemParam)]
struct Rebuild<'w, 's> {
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    assets: Option<Res<'w, WallAssets>>,
    log: ResMut<'w, WallLog>,
    old: Query<'w, 's, Entity, Or<(With<WallChunk>, With<WallPiece>, With<DustPuff>)>>,
}

impl Rebuild<'_, '_> {
    fn rebuild(&mut self) {
        let Some(assets) = self.assets.as_deref() else {
            return;
        };
        for entity in &self.old {
            self.commands.entity(entity).despawn();
        }
        let wall = spawn_wall(&mut self.commands, &mut self.meshes, assets);
        self.commands.insert_resource(wall);
        *self.log = WallLog::default();
    }
}

/// Fire the burst's rounds that are due, down the camera's view axis, an automatic gun's spread
/// over a sunflower of [`BURST_SPREAD`].
fn fire_burst(
    mut burst: ResMut<WallBurst>,
    rounds: Res<WallRounds>,
    time: Res<Time>,
    camera: Single<&Transform, With<FreeFlyCam>>,
    mut commands: Commands,
) {
    let Some(entry) = rounds.0.get(burst.round) else {
        return;
    };
    while burst.left > 0 && time.elapsed_secs() >= burst.next {
        let k = burst.fired;
        let forward = camera.forward().as_vec3();
        let direction = if entry.count > 1 {
            let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
            let r = BURST_SPREAD * ((k as f32 + 0.5) / entry.count as f32).sqrt();
            let angle = k as f32 * golden;
            forward
                + camera.right().as_vec3() * (r * angle.cos())
                + camera.up().as_vec3() * (r * angle.sin())
        } else {
            forward
        };
        commands.trigger(FireShell::of_round(
            &entry.round,
            camera.translation,
            Dir3::new(direction).unwrap_or(Dir3::NEG_Z),
            entry.mechanism,
            true,
            None,
            None,
        ));
        burst.fired += 1;
        burst.left -= 1;
        burst.next += entry.interval;
    }
}

/// Read every shell's new armour log entries: a crossing that cut a chunk queues its line deposit,
/// and the cones it threw are the wall's, each ray's dose summed on the witness tile it ended on.
#[derive(SystemParam)]
struct HitsParam<'w, 's> {
    shells: Query<
        'w,
        's,
        (
            Entity,
            &'static PenetrationMarks,
            Option<&'static SpallMarks>,
            Option<&'static WallRead>,
        ),
    >,
    chunks: Query<'w, 's, &'static WallChunk>,
    queue: ResMut<'w, WallQueue>,
    log: ResMut<'w, WallLog>,
    rounds: Res<'w, WallRounds>,
    commands: Commands<'w, 's>,
}

fn read_hits(mut p: HitsParam) {
    let tiles = TILES.0 * TILES.1;
    for (shell, marks, spall, read) in &p.shells {
        let (log_at, mut cone_at) = read.map_or((0, 0), |read| (read.log, read.cones));
        if log_at >= marks.log.len() {
            continue;
        }
        for mark in &marks.log[log_at..] {
            let ArmourMark::Crossing {
                volumes,
                cones,
                incoming,
                speed_in,
                end,
                ..
            } = mark
            else {
                continue;
            };
            let lines: Vec<(usize, Vec3, Vec3, f32)> = volumes
                .iter()
                .filter_map(|crossing| {
                    p.chunks
                        .get(crossing.volume)
                        .ok()
                        .map(|chunk| (chunk.0, crossing.entry, crossing.end, crossing.energy))
                })
                .collect();
            let thrown = spall
                .map(|spall| &spall.cones[cone_at.min(spall.cones.len())..])
                .unwrap_or_default();
            let thrown = &thrown[..(*cones).min(thrown.len())];
            cone_at += cones;
            if lines.is_empty() {
                continue;
            }
            // The round is the wall round of the nearest muzzle speed: what it is called, and the
            // mass that meets the plate, whose speed change is the momentum it lost.
            let round = p.rounds.0.iter().min_by(|a, b| {
                (a.round.muzzle_speed - speed_in)
                    .abs()
                    .total_cmp(&(b.round.muzzle_speed - speed_in).abs())
            });
            let speed_out = match end {
                CrossingEnd::Exit { speed, .. } => *speed,
                CrossingEnd::Rest { .. } => 0.0,
            };
            p.log.witness.resize(tiles, 0.0);
            let mut strength = 0.0;
            for cone in thrown {
                let weight = 1.0 / cone.rays.len().max(1) as f32;
                for ray in &cone.rays {
                    if let Some(tile) = witness_at(ray.end) {
                        p.log.witness[tile] += ray.strength * weight;
                    }
                }
                p.log.cones.push(WallCone {
                    origin: cone.origin,
                    rays: cone
                        .rays
                        .iter()
                        .map(|ray| (ray.end, ray.strength))
                        .collect(),
                });
                strength += cone.strength;
            }
            p.queue.0.push(Deposit::Line {
                what: round.map_or_else(|| "a round".to_string(), |round| round.name.clone()),
                lines,
                along: incoming.normalize_or(Vec3::NEG_Z),
                momentum: round.map_or(0.0, |round| {
                    round.round.penetrator.mass * (speed_in - speed_out).max(0.0)
                }),
                spall: strength,
            });
        }
        p.commands.entity(shell).insert(WallRead {
            log: marks.log.len(),
            cones: cone_at,
        });
    }
}

/// What one deposit did to the wall.
#[derive(Debug, Default)]
struct Applied {
    deposited: f32,
    chunks: usize,
    worn: Vec<usize>,
    dusted: Vec<usize>,
    local: usize,
    failures: Vec<model::Failure>,
    /// The weight check's unassessed touched chunks and its operation count ([`model::Weighed`]).
    unassessed: usize,
    weight_steps: usize,
    /// Each loose group, its centroid, linear and angular velocity, and whether a structural crack
    /// bounds it.
    groups: Vec<(Vec<usize>, Vec3, Vec3, Vec3, bool)>,
    /// A blast's charge, origin, coupled energy, the impulse its rays pushed with (N·s, summed
    /// unsigned), and its drawn ray ends.
    blast: Option<(Blast, Vec3, f32, f32, Vec<Vec3>)>,
}

/// A round's crossing as the laws read it: its lines ([`Loading::lines`]), and the round's lost
/// momentum shared among the chunks by the energy each took, at its chord's midpoint.
fn line_loading(
    lines: &[(usize, Vec3, Vec3, f32)],
    along: Vec3,
    momentum: f32,
) -> (Loading, Vec<Push>) {
    let total: f32 = lines.iter().map(|line| line.3).sum();
    let pushes = if total > 0.0 {
        lines
            .iter()
            .map(|&(chunk, entry, end, energy)| Push {
                chunk,
                at: (entry + end) / 2.0,
                impulse: along * (momentum * energy / total),
            })
            .collect()
    } else {
        Vec::new()
    };
    (Loading::lines(lines), pushes)
}

/// Run one deposit through the game's laws: the wear and dust, the local cracks, the structural
/// check, and the loose groups with the velocities the pushes on them give.
fn apply_deposit(
    wall: &mut Wall,
    controls: &WallControls,
    charge: &Charge,
    field: &[(Vec3, f32)],
    deposit: &Deposit,
) -> Applied {
    let mut applied = Applied::default();
    let (loading, pushes) = match deposit {
        Deposit::Line {
            lines,
            along,
            momentum,
            ..
        } => line_loading(lines, *along, *momentum),
        &Deposit::Blast {
            mass,
            standoff,
            at,
            into,
        } => {
            let blast = charge.blast(mass, controls.cased);
            let origin = at - into * (standoff + blast.radius);
            let state = &wall.state;
            let hits = model::blast_rays(
                &wall.cut.blocks,
                |i| state.standing(i),
                origin,
                model::Blast::of(blast.energy),
                field,
            );
            let coupled: f32 = hits.iter().map(|hit| hit.energy).sum();
            let pushed: f32 = hits.iter().map(|hit| hit.impulse.length()).sum();
            let drawn = hits
                .iter()
                .step_by(RAY_DRAW_STRIDE)
                .map(|hit| hit.at)
                .collect();
            applied.blast = Some((blast, origin, coupled, pushed, drawn));
            Loading::blast(&hits)
        }
    };
    applied.deposited = loading.energies.iter().map(|(_, e)| e).sum();
    applied.chunks = loading.energies.len();
    let struck = model::strike(&wall.cut.structure, &mut wall.state, &loading);
    applied.local = struck.cracked.len();
    // A push on a chunk turned to dust went into crushing it, and moves nothing.
    let pushes: Vec<Push> = pushes
        .into_iter()
        .filter(|push| wall.state.standing(push.chunk))
        .collect();
    applied.failures = model::structural(
        &wall.cut.structure,
        &mut wall.state,
        &pushes,
        &mut wall.sections,
    );
    let pushed: Vec<usize> = struck
        .cracked
        .iter()
        .chain(applied.failures.iter().flat_map(|failure| &failure.joints))
        .copied()
        .collect();
    let weighed = model::weigh(
        &wall.cut.structure,
        &mut wall.state,
        &struck.dusted,
        &pushed,
        &mut wall.sections,
    );
    applied.failures.extend(weighed.failures);
    applied.unassessed = weighed.unassessed.len();
    applied.weight_steps = weighed.steps;
    applied.worn = struck.worn;
    applied.dusted = struck.dusted;
    for failure in &applied.failures {
        for &j in &failure.joints {
            wall.structural[j] = true;
        }
    }
    for group in model::fall(&wall.cut.structure, &mut wall.state) {
        let chunks = &wall.cut.structure.chunks;
        let mass: f32 = group.iter().map(|&i| chunks[i].mass).sum();
        let centroid = group
            .iter()
            .map(|&i| chunks[i].centroid * chunks[i].mass)
            .sum::<Vec3>()
            / mass.max(f32::EPSILON);
        // A group's least moment of inertia: a ball of its mean chunk's size per chunk.
        let piece =
            (group.iter().map(|&i| chunks[i].volume).sum::<f32>() / group.len() as f32).cbrt();
        let on_group = || {
            pushes
                .iter()
                .filter(|push| group.binary_search(&push.chunk).is_ok())
        };
        let impulse: Vec3 = on_group().map(|push| push.impulse).sum();
        let angular: Vec3 = on_group()
            .map(|push| (push.at - centroid).cross(push.impulse))
            .sum();
        let inertia: f32 = group
            .iter()
            .map(|&i| chunks[i].mass * (chunks[i].centroid - centroid).length_squared())
            .sum::<f32>()
            .max(mass * piece * piece / 6.0);
        // Momentum conserved: a group leaves with the impulse pushed into it, no more. (A
        // free-surface velocity `2σ/(ρc)` describes a thin spalled layer, and credited to a whole
        // group it hands a 3.8 t wall tens of m/s.)
        let velocity = impulse / mass.max(f32::EPSILON);
        let structural = wall
            .cut
            .structure
            .joints
            .iter()
            .enumerate()
            .any(|(j, joint)| {
                wall.structural[j]
                    && (group.binary_search(&joint.a).is_ok()
                        || matches!(joint.b, model::Side::Chunk(b) if group.binary_search(&b).is_ok()))
            });
        applied
            .groups
            .push((group, centroid, velocity, angular / inertia, structural));
    }
    applied
}

/// The deposits' entity work: a dust puff where a chunk went, a body per loose group, the heat of
/// every worn chunk, and the outcome for the readout.
#[derive(SystemParam)]
struct ApplyParam<'w, 's> {
    wall: Option<ResMut<'w, Wall>>,
    controls: Res<'w, WallControls>,
    charge: Res<'w, Charge>,
    field: Res<'w, BlastField>,
    queue: ResMut<'w, WallQueue>,
    log: ResMut<'w, WallLog>,
    assets: Option<Res<'w, WallAssets>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    time: Res<'w, Time>,
    commands: Commands<'w, 's>,
}

fn apply_deposits(mut p: ApplyParam) {
    let (Some(wall), Some(assets)) = (p.wall.as_deref_mut(), p.assets.as_deref()) else {
        return;
    };
    for deposit in std::mem::take(&mut p.queue.0) {
        let applied = apply_deposit(wall, &p.controls, &p.charge, &p.field.0, &deposit);
        for &chunk in &applied.worn {
            if let Some(entity) = wall.entities[chunk] {
                let left =
                    wall.state.pool[chunk] / model::full_pool(&wall.cut.structure.chunks[chunk]);
                let level = ((1.0 - left) * 10.0).round().clamp(0.0, 10.0) as usize;
                p.commands
                    .entity(entity)
                    .insert(MeshMaterial3d(assets.heat[level].clone()));
            }
        }
        for &chunk in &applied.dusted {
            if let Some(entity) = wall.entities[chunk].take() {
                p.commands.entity(entity).despawn();
            }
            let material = p.materials.add(StandardMaterial {
                base_color: Color::srgba(0.78, 0.7, 0.6, 0.75),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                ..default()
            });
            p.commands.spawn((
                Name::new("dust puff"),
                DustPuff {
                    born: p.time.elapsed_secs(),
                    material: material.clone(),
                },
                Mesh3d(assets.puff.clone()),
                MeshMaterial3d(material),
                Transform::from_translation(wall.cut.blocks[chunk].center())
                    .with_scale(Vec3::splat(0.12)),
            ));
        }
        for (group, centroid, velocity, spin, structural) in &applied.groups {
            for &chunk in group {
                if let Some(entity) = wall.entities[chunk].take() {
                    p.commands.entity(entity).despawn();
                }
            }
            let tint = if *structural {
                assets.toppled.clone()
            } else {
                assets.loose.clone()
            };
            p.commands
                .spawn((
                    Name::new("wall piece"),
                    WallPiece,
                    Transform::from_translation(*centroid),
                    Visibility::default(),
                    crate::eased_dynamic_body(),
                    LinearVelocity(*velocity),
                    AngularVelocity(*spin),
                ))
                .with_children(|parent| {
                    for &chunk in group {
                        let block = &wall.cut.blocks[chunk];
                        parent.spawn((
                            Mesh3d(wall.meshes[chunk].clone()),
                            MeshMaterial3d(tint.clone()),
                            Transform::from_translation(block.center() - *centroid),
                            chunk_collider(block, LOOSE_GAP),
                            ColliderDensity(wall.material.density),
                            CollisionLayers::new(
                                [Layer::Default],
                                [Layer::Default, Layer::Terrain, Layer::Armor],
                            ),
                        ));
                    }
                });
        }
        let what = match &deposit {
            Deposit::Line { what, .. } => what.clone(),
            Deposit::Blast { mass, standoff, .. } => format!(
                "blast {mass:.1} kg {} at {standoff:.1} m",
                if p.controls.cased { "cased" } else { "bare" }
            ),
        };
        if let Some((_, origin, _, _, rays)) = &applied.blast {
            p.log.blast = Some((
                *origin,
                applied.blast.as_ref().map_or(0.0, |b| b.0.radius),
                rays.clone(),
            ));
        }
        let first = |load: model::Load| {
            applied
                .failures
                .iter()
                .find(|failure| failure.load == load)
                .map(|failure| (failure.demand, failure.capacity))
        };
        p.log.last = Some(Outcome {
            what,
            deposited: applied.deposited,
            chunks: applied.chunks,
            dust: applied.dusted.len(),
            local: applied.local,
            structural: applied.failures.iter().map(|f| f.joints.len()).sum(),
            sections: applied.failures.len(),
            worst: first(model::Load::Push),
            weight: first(model::Load::Weight),
            unassessed: applied.unassessed,
            weight_steps: applied.weight_steps,
            groups: applied.groups.len(),
            fallen: applied.groups.iter().map(|(group, ..)| group.len()).sum(),
            spall: match &deposit {
                Deposit::Line { spall, .. } => *spall,
                _ => 0.0,
            },
            blast: applied
                .blast
                .as_ref()
                .map(|(blast, _, coupled, pushed, _)| (*blast, *coupled, *pushed)),
        });
        if let Some(last) = &p.log.last {
            info!(
                "wall prototype: {}: {:.0} J into {} chunks, {} dust, {} local cracks, \
                 {} structural cracks in {} sections (first push {:?} N s m and first weight {:?} \
                 Pa demand, capacity), weight check {} steps with {} touched chunks unassessed, \
                 {} groups fell ({} chunks), exit cones {:.0} J, blast {:?} (coupled J, N s)",
                last.what,
                last.deposited,
                last.chunks,
                last.dust,
                last.local,
                last.structural,
                last.sections,
                last.worst,
                last.weight,
                last.weight_steps,
                last.unassessed,
                last.groups,
                last.fallen,
                last.spall,
                last.blast,
            );
            info!(
                "wall prototype: values:\n{}",
                detail(wall, &p.controls, &p.charge, &p.log, last).join("\n")
            );
        }
    }
}

/// Cracked joints between standing matter (local in cyan, structural in magenta), the wall's exit
/// cones in orange, and the last blast's charge and a sample of its rays.
fn draw_wall(wall: Option<Res<Wall>>, log: Res<WallLog>, mut gizmos: Gizmos<WallGizmos>) {
    let Some(wall) = wall else {
        return;
    };
    let mid = layout().min.z + layout().size.z / 2.0;
    let out = |p: Vec3| p + Vec3::Z * (0.003 * (p.z - mid).signum());
    for (index, joint) in wall.cut.structure.joints.iter().enumerate() {
        if !wall.state.cracked[index] || !wall.state.live(joint) {
            continue;
        }
        let colour = if wall.structural[index] {
            Color::srgb(1.0, 0.2, 1.0)
        } else {
            Color::srgb(0.2, 1.0, 1.0)
        };
        let face = &wall.cut.faces[index];
        for k in 0..face.len() {
            gizmos.line(out(face[k]), out(face[(k + 1) % face.len()]), colour);
        }
    }
    for cone in &log.cones {
        let strongest = cone.rays.iter().map(|(_, s)| *s).fold(0.0_f32, f32::max);
        for &(end, strength) in &cone.rays {
            let alpha = (strength / strongest.max(f32::EPSILON)).clamp(0.02, 1.0);
            gizmos.line(cone.origin, end, Color::srgba(1.0, 0.55, 0.1, alpha));
        }
    }
    let marker = Color::srgb(1.0, 0.95, 0.5);
    if let Some((origin, radius, rays)) = &log.blast {
        gizmos.sphere(Isometry3d::from_translation(*origin), *radius, marker);
        for &end in rays {
            gizmos.line(*origin, end, Color::srgba(1.0, 0.95, 0.5, 0.15));
        }
    }
}

/// Each witness tile's material from its dose: none, or one of eight steps of a decade each from
/// 1 J.
fn paint_witness(
    log: Res<WallLog>,
    assets: Option<Res<WallAssets>>,
    mut tiles: Query<(&WitnessTile, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (tile, mut material) in &mut tiles {
        let dose = log.witness.get(tile.0).copied().unwrap_or(0.0);
        let level = if dose < 1.0 {
            0
        } else {
            (dose.log10().floor() as usize + 1).min(8)
        };
        if material.0 != assets.witness[level] {
            material.0 = assets.witness[level].clone();
        }
    }
}

/// Grow and fade each dust puff over [`PUFF_LIFE`], then remove it and its material.
fn animate_puffs(
    time: Res<Time>,
    mut puffs: Query<(Entity, &DustPuff, &mut Transform)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for (entity, puff, mut transform) in &mut puffs {
        let age = (time.elapsed_secs() - puff.born) / PUFF_LIFE;
        if age >= 1.0 {
            materials.remove(&puff.material);
            commands.entity(entity).despawn();
            continue;
        }
        transform.scale = Vec3::splat(0.12 + 0.45 * (1.0 - (1.0 - age).powi(3)));
        if let Some(mut material) = materials.get_mut(&puff.material) {
            material.base_color.set_alpha(0.75 * (1.0 - age));
        }
    }
}

fn spawn_readout(mut commands: Commands, script: Option<Res<WallScript>>) {
    let left = Val::Px(if script.is_some() { 6.0 } else { UI_LEFT });
    for (top, help) in [(6.0, false), (30.0, true)] {
        let mut text = commands.spawn((
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(Color::srgb(0.92, 0.94, 1.0)),
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(top),
                left,
                padding: UiRect::all(Val::Px(4.0)),
                ..default()
            },
        ));
        if help {
            text.insert(WallHelp);
        } else {
            text.insert(WallReadout);
        }
    }
}

/// `e` J in J, kJ or MJ.
fn joules(e: f32) -> String {
    if e >= 1.0e6 {
        format!("{:.2} MJ", e / 1.0e6)
    } else if e >= 1.0e3 {
        format!("{:.1} kJ", e / 1.0e3)
    } else {
        format!("{e:.1} J")
    }
}

/// The values behind the readout, for the log: the material, the controls, the last deposit's
/// outcome, the blast's energy and coupling, the first structural failure, and the exit cones'
/// strength and what the witness panel took. ASCII.
fn detail(
    wall: &Wall,
    c: &WallControls,
    charge_src: &Charge,
    log: &WallLog,
    last: &Outcome,
) -> Vec<String> {
    let m = &wall.material;
    let charge = charge_src.blast(MASSES[c.mass], c.cased);
    let mut lines = vec![
        format!(
            "brick  rho {:.0} kg/m3   crush w {:.1} MJ/m3   seam ft {:.2} MPa   Gf {:.0} J/m2   {} chunks {} joints",
            m.density,
            m.crush / 1.0e6,
            m.seam.tensile / 1.0e6,
            m.seam.fracture_energy,
            wall.cut.structure.chunks.len(),
            wall.cut.structure.joints.len()
        ),
        format!(
            "blast [O] {:.1} kg [, .] at {:.1} m [; '] {} [V]: {} chemical - {} casing (M/C {:.1}) = {} = {:.2} kg TNT",
            charge.mass,
            STANDOFFS[c.standoff],
            if c.cased { "cased" } else { "bare" },
            joules(charge.chemical),
            joules(charge.casing),
            if c.cased {
                charge_src.casing_ratio
            } else {
                0.0
            },
            joules(charge.energy),
            charge.charge,
        ),
        format!(
            "last: {}: {} into {} chunks -> {} dust, {} local cracks, {} structural cracks in {} sections, {} groups fell ({} chunks)",
            last.what,
            joules(last.deposited),
            last.chunks,
            last.dust,
            last.local,
            last.structural,
            last.sections,
            last.groups,
            last.fallen
        ),
    ];
    if let Some((blast, coupled, pushed)) = last.blast {
        lines.push(format!(
            "      blast coupled {} of {} ({:.0} %), pushed {:.0} N s",
            joules(coupled),
            joules(blast.energy),
            100.0 * coupled / blast.energy.max(f32::EPSILON),
            pushed
        ));
    }
    if let Some((demand, capacity)) = last.worst {
        lines.push(format!(
            "      first structural failure: angular impulse {demand:.0} N s m against sqrt(2 I Gf A) = {capacity:.0} N s m"
        ));
    }
    if let Some((demand, capacity)) = last.weight {
        lines.push(format!(
            "      first weight failure: peak tension {:.0} kPa against the seam's {:.0} kPa",
            demand / 1e3,
            capacity / 1e3
        ));
    }
    lines.push(format!(
        "      weight check: {} steps, {} touched chunks unassessed (held through several sections)",
        last.weight_steps, last.unassessed
    ));
    // `+ 0.0`: an empty sum is -0.0, which would print a sign.
    let witness = log.witness.iter().sum::<f32>() + 0.0;
    lines.push(format!(
        "spall: exit cone S {}   witness panel total {}",
        joules(last.spall),
        joules(witness)
    ));
    lines
}

/// The last shot in a few words: what it was and the energy it put into the wall.
fn last_shot(last: &Outcome) -> String {
    format!("{}, {}", last.what, joules(last.deposited))
}

#[derive(SystemParam)]
struct ReadoutParam<'w, 's> {
    wall: Option<Res<'w, Wall>>,
    log: Res<'w, WallLog>,
    controls: Res<'w, WallControls>,
    camera: Single<'w, 's, &'static Transform, With<FreeFlyCam>>,
    text: Single<
        'w,
        's,
        (&'static mut Text, &'static mut Visibility),
        (With<WallReadout>, Without<WallHelp>, Without<super::Legend>),
    >,
    help: Single<
        'w,
        's,
        (&'static mut Text, &'static mut Visibility),
        (With<WallHelp>, Without<WallReadout>, Without<super::Legend>),
    >,
    range_legend: Single<
        'w,
        's,
        &'static mut Visibility,
        (With<super::Legend>, Without<WallReadout>, Without<WallHelp>),
    >,
}

/// The readout: one line (the last shot), and the help panel
/// while H has it open. Both show only near the wall, where the range's own H legend stays shut.
fn update_readout(p: ReadoutParam) {
    if p.wall.is_none() {
        return;
    }
    let c = *p.controls;
    let line = format!(
        "{}  |  H help",
        p.log
            .last
            .as_ref()
            .map_or_else(|| "no shot yet".to_string(), last_shot),
    );
    let near = p.camera.translation.distance(layout().centre()) < READOUT_RANGE;
    let shown_if = |on: bool| {
        if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    let (mut text, mut visibility) = p.text.into_inner();
    visibility.set_if_neq(shown_if(near));
    if text.0 != line {
        text.0 = line;
    }
    let (mut help, mut help_visibility) = p.help.into_inner();
    help_visibility.set_if_neq(shown_if(near && c.help));
    if help.0 != HELP {
        help.0 = HELP.to_string();
    }
    if near {
        p.range_legend.into_inner().set_if_neq(Visibility::Hidden);
    }
}

/// The help panel (H), ASCII: the keys by group, and what each colour on the wall means.
const HELP: &str = "\
FIRE     B next round   F fire it
BLAST    O blast   , . charge mass   ; ' standoff   V cased / bare
VIEW     G camera: front, back, witness   N new wall   H this panel
COLOURS  brick red to glowing yellow chunk = crush capacity spent
         cyan seam outline = cracked by the local push
         magenta seam outline = cracked by the structural check
         grey-blue piece = loose   purple piece = toppled section
         orange rays = spall cone
         yellow sphere and rays = blast";

/// The capture script (module doc): stand the camera by the timeline and fire the case once.
#[derive(SystemParam)]
struct ScriptParam<'w, 's> {
    script: ResMut<'w, WallScript>,
    wall: Option<Res<'w, Wall>>,
    time: Res<'w, Time>,
    camera: Single<'w, 's, &'static mut Transform, With<FreeFlyCam>>,
    burst: ResMut<'w, WallBurst>,
    rounds: Res<'w, WallRounds>,
    queue: ResMut<'w, WallQueue>,
    label: Query<'w, 's, &'static mut Visibility, With<super::RoundLabel>>,
}

fn drive_script(mut p: ScriptParam) {
    // The range gun's round label names the range's selection, not the wall's case.
    for mut visibility in &mut p.label {
        visibility.set_if_neq(Visibility::Hidden);
    }
    let Some(wall) = p.wall.as_deref() else {
        return;
    };
    let t = p.time.elapsed_secs();
    let aim = aim_point(wall);
    let back = if p.script.witness && t >= SCRIPT_BACK_AGAIN {
        View::Witness
    } else {
        View::Back
    };
    let view = if (SCRIPT_BACK..SCRIPT_FRONT).contains(&t) || t >= SCRIPT_BACK_AGAIN {
        back
    } else {
        View::Front
    };
    **p.camera = view_pose(view, aim);
    if p.script.done || t < SCRIPT_FIRE {
        return;
    }
    p.script.done = true;
    match p.script.case {
        Case::Round(round) => start_burst(&mut p.burst, round, &p.rounds, t),
        Case::Blast { mass, standoff } => p.queue.0.push(Deposit::Blast {
            mass,
            standoff,
            at: aim,
            into: Vec3::NEG_Z,
        }),
    }
}
