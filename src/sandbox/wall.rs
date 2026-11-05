//! The wall prototype, about the game's environment destruction (game state only): a brick wall on
//! the range whose chunks the range's rounds and a blast turn to dust or crack loose. A prototype
//! isolated to the sandbox: nothing here is wired into the game's sim or its net.
//!
//! * **The wall.** One wall of the baked house (`assets/constructions/house_1/house_1.construction.ron`,
//!   [`WALL`]'s brick elements), set down at [`PLACE`] by [`baked::wall_cut`]: the bake's convex
//!   chunks, each a static collider with the `Brick` substance, so the walk crosses them as it
//!   crosses any hard volume, and the bake's joints and ground anchors.
//! * **Deposits.** A round's crossing deposits, in each chunk, the energy the walk says it lost
//!   there ([`ShellMarched`]), and pushes it with the momentum the round lost. The blast
//!   tool (O) bursts a charge of 0.1–5 kg at 0–5 m standoff: a sphere of rays ([`blast_field`])
//!   each stopping on the first chunk it meets with its share of the energy, pushing the face it
//!   meets with the blast's reflected impulse ([`model::Blast::push`]).
//! * **Responses.** The game's fracture laws ([`crate::construction::fracture`], through [`model`])
//!   turn each deposit into dust (the pools), local cracks (the push rule) and structural cracks
//!   (the structural check, then the weight check on what changed), then let every group with no
//!   path to the ground fall with the momentum pushed into it.
//! * **The view.** Each chunk's pool as heat, local cracks in cyan and structural ones in
//!   magenta, a dust puff where a chunk went, loose groups as falling bodies on the default layer
//!   (out of ballistics; grey-blue when a local crack freed them, purple when a structural one
//!   did), a one-line readout (the last shot and its energy) and, on H, a help panel of the keys
//!   and the colour legend. Every value of a resolved shot goes to the log. The readout shows only
//!   while the camera stands within [`WallTool::readout_range_m`] of the wall, so the range's own view carries
//!   no more text than its round's name.
//! * **Spall.** Every exit cone the wall throws is read off the shell's record and drawn, and its
//!   rays' doses are summed on a witness panel behind the wall: the game's own dose.
//!
//! Keys: B round, F fire it, O blast and `,`/`.` its filler mass, `;`/`'` its standoff, V its
//! casing, G camera (front, back, witness), N new wall, H help.
//!
//! A capture plan's `scene.range_wall` (`dev_capture::plan::RangeWall`) opens the wall for a
//! capture: its `case` (`Round("88mm_AP")`, `Blast(kg: 1.0, standoff_m: 3.0)`) runs one case
//! ([`drive_script`]): the camera stands at the front, then the back, fires (or bursts) at
//! [`SCRIPT_FIRE`] s from the front, and stands front then back again for the after frames, at the
//! witness pose for the last ones under `witness`. `bare` and `help` set the charge's casing and the
//! help panel.

mod baked;
mod model;
#[cfg(test)]
mod tests;

use avian3d::prelude::{
    AngularVelocity, Collider, CollisionLayers, LayerMask, LinearVelocity, RigidBody,
};
use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::SystemParam;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use self::baked::Placement;
use self::model::{Block, Cut, Load, Material};
use super::{FreeFlyCam, SEED_SPEC, UI_LEFT, panel_capturing};
use crate::ballistics::burst::products_momentum;
use crate::ballistics::{BallisticVolume, DeliveredLine, FireShell, ShellMarched, SpallMarks};
use crate::construction::schema::ConstructionAsset;
use crate::dev_capture::plan::RangeWallCase;
use crate::substances::SubstanceRegistry;
use crate::{Layer, spec};

/// The baked construction the wall comes from, and which of its walls (a declared component): the
/// brick elements of that wall.
const CONSTRUCTION: &str =
    include_str!("../../assets/constructions/house_1/house_1.construction.ron");
const WALL: &str = "east";
/// Fixture layout, world metres: where the wall stands, clear of the range's plates and target (x within ±7.5)
/// and of the test course (z ≤ −30). Its front face looks down +Z.
const PLACE: Placement = Placement {
    centre_x: 16.0,
    back_z: 3.875,
};
/// The substance every chunk wears; its values are its row's ([`Material::of`]).
const SUBSTANCE: &str = "Brick";
/// Fixture inputs: the blast tool's filler masses, kg, and standoffs, m (the owner's 0.1–5 kg and 0–5 m).
const MASSES: [f32; 6] = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0];
const STANDOFFS: [f32; 6] = [0.0, 0.5, 1.0, 2.0, 3.0, 5.0];
/// Fixture input: rounds an automatic gun fires per F.
const BURST: usize = 50;
/// Fixture layout: the witness panel's distance behind the back face (past the back view's camera), its tile
/// size and count (m, m, x × y).
const WITNESS_GAP: f32 = 4.4;
const TILE: f32 = 0.25;
const TILES: (usize, usize) = (24, 14);
/// Resolution: draw every sixth blast hit to bound diagnostic gizmo work; field sampling is unchanged.
const RAY_DRAW_STRIDE: usize = 6;
/// Resolution: these palette steps bound diagnostic material allocation.
const HEAT_LEVELS: usize = 10;
const WITNESS_LEVELS: usize = 8;
/// Fixture capture timeline, app seconds: back view from, front view from, the shot, and the
/// after back view from. Frames read at 3, 4, 11.5 and 12.5 s fall inside each stand.
const SCRIPT_BACK: f32 = 3.5;
const SCRIPT_FRONT: f32 = 4.5;
const SCRIPT_FIRE: f32 = 5.0;
const SCRIPT_BACK_AGAIN: f32 = 12.0;

pub(super) fn plugin(app: &mut App) {
    let rounds = wall_rounds();
    let planned = crate::dev_capture::plan()
        .and_then(|plan| plan.scene.range_wall.clone())
        .unwrap_or_default();
    let controls = WallControls {
        mass: 3,
        standoff: 0,
        cased: !planned.bare,
        round: 0,
        view: View::Front,
        help: planned.help,
    };
    if let Some(case) = &planned.case {
        let case = resolve_case(case, &rounds);
        info!("wall prototype: capture case {case:?}");
        app.insert_resource(WallScript {
            case,
            witness: planned.witness,
            done: false,
        })
        .add_systems(Update, drive_script.before(fire_burst));
    }
    wall_tool();
    let charge = Charge::of(&rounds);
    app.insert_resource(WallRounds(rounds))
        .insert_resource(charge)
        .insert_resource(BlastField(blast_field()))
        .insert_resource(controls)
        .init_resource::<WallQueue>()
        .init_resource::<WallLog>()
        .init_resource::<WallBurst>()
        .add_observer(read_hits)
        .init_gizmo_group::<WallGizmos>()
        .add_systems(Startup, (setup_wall, spawn_readout))
        .add_systems(FixedUpdate, apply_deposits)
        .add_systems(
            Update,
            (
                wall_keys.run_if(not(panel_capturing)),
                fire_burst,
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
    live: crate::construction::breakage::Live,
    structural: Vec<bool>,
    entities: Vec<Option<Entity>>,
    meshes: Vec<Handle<Mesh>>,
    material: Material,
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

/// The wall's layout, read once from [`CONSTRUCTION`]. A construction the bake wrote and the build
/// shipped: one that does not parse is a broken build.
fn layout() -> &'static Layout {
    static LAYOUT: std::sync::LazyLock<Layout> = std::sync::LazyLock::new(|| {
        let asset =
            ConstructionAsset::from_ron(CONSTRUCTION).expect("the shipped construction parses");
        let cut = baked::wall_cut(&asset, WALL, SUBSTANCE, brick(), PLACE)
            .expect("the shipped construction has the wall");
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
    round: crate::rounds::Round,
    mechanism: spec::FireMechanism,
    count: usize,
    interval: f32,
}

/// Every round each of the seed tank's guns chambers, the primary's first.
#[derive(Resource)]
struct WallRounds(Vec<WallRound>);

/// The blast tool's charge, from data: the filler's density (kg/m³), chemical energy (J/kg) and
/// speed constant `K` (m/s), and the casing of the tool's cased round (`assets/dev/wall-tool.ron`) as the burst law
/// derives it — its mass per kg of filler `M/C` and the speed it is driven to, `v_s = K (M/C +
/// ½)^−½` (m/s). A charge of `C` kg carries `M/C × C` kg of it.
#[derive(Resource, Clone, Copy, Debug)]
struct Charge {
    density: f32,
    chemical: f32,
    speed_constant: f32,
    casing_ratio: f32,
    casing_speed: f32,
}

/// The game's damage-model inspector inputs, parsed once before the tool admits play.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WallTool {
    cased_round: String,
    spread_rad: crate::provenance::Sourced<f32>,
    seam_m: crate::provenance::Sourced<f32>,
    puff_life_s: crate::provenance::Sourced<f32>,
    view_fill: crate::provenance::Sourced<f32>,
    readout_range_m: crate::provenance::Sourced<f32>,
    puff_start_m: crate::provenance::Sourced<f32>,
    puff_growth_m: crate::provenance::Sourced<f32>,
    heat_emissive: crate::provenance::Sourced<f32>,
    roughness: crate::provenance::Sourced<f32>,
    ray_alpha_floor: crate::provenance::Sourced<f32>,
    blast_ray_alpha: crate::provenance::Sourced<f32>,
    gizmo_width_px: crate::provenance::Sourced<f32>,
    readout_font_px: crate::provenance::Sourced<f32>,
    readout_padding_px: crate::provenance::Sourced<f32>,
    brick_rgba: crate::provenance::Sourced<[f32; 4]>,
    hot_rgba: crate::provenance::Sourced<[f32; 4]>,
    witness_clear_rgba: crate::provenance::Sourced<[f32; 4]>,
    witness_low_rgba: crate::provenance::Sourced<[f32; 4]>,
    witness_high_rgba: crate::provenance::Sourced<[f32; 4]>,
    loose_rgba: crate::provenance::Sourced<[f32; 4]>,
    toppled_rgba: crate::provenance::Sourced<[f32; 4]>,
    dust_rgba: crate::provenance::Sourced<[f32; 4]>,
    local_crack_rgba: crate::provenance::Sourced<[f32; 4]>,
    structural_crack_rgba: crate::provenance::Sourced<[f32; 4]>,
    cone_rgba: crate::provenance::Sourced<[f32; 4]>,
    blast_rgba: crate::provenance::Sourced<[f32; 4]>,
    readout_rgba: crate::provenance::Sourced<[f32; 4]>,
    readout_background_rgba: crate::provenance::Sourced<[f32; 4]>,
}

fn tool_from_ron(text: &str) -> Result<WallTool, String> {
    let tool: WallTool = ron::from_str(text).map_err(|err| err.to_string())?;
    if !tool.spread_rad.0.is_finite() || tool.spread_rad.0 < 0.0 {
        return Err(
            "spread_rad must be finite and nonnegative; author the inspection spread".to_string(),
        );
    }
    for (field, value) in [
        ("seam_m", tool.seam_m.0),
        ("puff_life_s", tool.puff_life_s.0),
        ("view_fill", tool.view_fill.0),
        ("readout_range_m", tool.readout_range_m.0),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!(
                "{field} must be finite and positive; author the inspection appearance"
            ));
        }
    }
    for (field, value) in [
        ("puff_start_m", tool.puff_start_m.0),
        ("puff_growth_m", tool.puff_growth_m.0),
        ("heat_emissive", tool.heat_emissive.0),
        ("roughness", tool.roughness.0),
        ("ray_alpha_floor", tool.ray_alpha_floor.0),
        ("blast_ray_alpha", tool.blast_ray_alpha.0),
        ("gizmo_width_px", tool.gizmo_width_px.0),
        ("readout_font_px", tool.readout_font_px.0),
        ("readout_padding_px", tool.readout_padding_px.0),
    ] {
        if !value.is_finite()
            || value < 0.0
            || (value == 0.0
                && matches!(field, "puff_start_m" | "gizmo_width_px" | "readout_font_px"))
        {
            return Err(format!(
                "{field}: author a finite admissible inspection magnitude"
            ));
        }
    }
    for (field, value) in [
        ("brick_rgba", tool.brick_rgba.0),
        ("hot_rgba", tool.hot_rgba.0),
        ("witness_clear_rgba", tool.witness_clear_rgba.0),
        ("witness_low_rgba", tool.witness_low_rgba.0),
        ("witness_high_rgba", tool.witness_high_rgba.0),
        ("loose_rgba", tool.loose_rgba.0),
        ("toppled_rgba", tool.toppled_rgba.0),
        ("dust_rgba", tool.dust_rgba.0),
        ("local_crack_rgba", tool.local_crack_rgba.0),
        ("structural_crack_rgba", tool.structural_crack_rgba.0),
        ("cone_rgba", tool.cone_rgba.0),
        ("blast_rgba", tool.blast_rgba.0),
        ("readout_rgba", tool.readout_rgba.0),
        ("readout_background_rgba", tool.readout_background_rgba.0),
    ] {
        if value
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(format!("{field}: author finite sRGBA channels in [0, 1]"));
        }
    }
    for (field, value) in [
        ("roughness", tool.roughness.0),
        ("ray_alpha_floor", tool.ray_alpha_floor.0),
        ("blast_ray_alpha", tool.blast_ray_alpha.0),
    ] {
        if value > 1.0 {
            return Err(format!("{field}: author a value in [0, 1]"));
        }
    }
    Ok(tool)
}

/// The inspector palette is authored in sRGBA.
fn rgba([red, green, blue, alpha]: [f32; 4]) -> Color {
    Color::srgba(red, green, blue, alpha)
}

fn wall_tool() -> &'static WallTool {
    static TOOL: std::sync::LazyLock<WallTool> = std::sync::LazyLock::new(|| {
        crate::provenance::shipped("assets/dev/wall-tool.ron", || {
            tool_from_ron(include_str!("../../assets/dev/wall-tool.ron"))
        })
        .unwrap_or_else(|err| panic!("assets/dev/wall-tool.ron: {err}"))
    });
    &TOOL
}

impl Charge {
    /// The game's environment blast tool requires a sourced core and a burst round whose
    /// casing supplies its cased setting. Missing construction is refused at initialization.
    fn of(rounds: &[WallRound]) -> Self {
        let key = &wall_tool().cased_round;
        let burst = rounds.iter().find(|round| &round.key == key)
            .and_then(|round| round.round.burst).unwrap_or_else(|| panic!(
                "wall blast tool: assets/dev/wall-tool.ron field cased_round {key:?}: rounds need a burst block for this key; author a chambered burst-round key"));
        // DERIVED from the selected burst's compiled construction: no second core lookup can
        // pair one filler's chemistry with another round's casing.
        let speed2 = burst.sideways_speed * burst.sideways_speed;
        let casing_per_kg = burst.energy / burst.core_mass;
        Self {
            density: 3.0 * burst.core_mass
                / (4.0 * std::f32::consts::PI * burst.charge_radius.powi(3)),
            chemical: (burst.blast + burst.energy) / burst.core_mass,
            speed_constant: (2.0 * casing_per_kg + 0.5 * speed2).sqrt(),
            casing_ratio: 2.0 * casing_per_kg / speed2,
            casing_speed: burst.sideways_speed,
        }
    }

    /// A charge of `mass` kg: its chemical energy, its casing's driven energy (none when bare) and
    /// the blast's energy (the difference), all J, the TNT charge that carries it (kg,
    /// [`model::Blast::of`]), its radius (m) and its detonation products' momentum (N·s, at the
    /// casing's `v_s`, or the bare core's `K √2`, [`products_momentum`]).
    fn blast(&self, mass: f32, cased: bool) -> Blast {
        let chemical = self.chemical * mass;
        let (casing, speed) = if cased {
            (
                0.5 * self.casing_ratio * mass * self.casing_speed * self.casing_speed,
                self.casing_speed,
            )
        } else {
            (0.0, self.speed_constant * std::f32::consts::SQRT_2)
        };
        let energy = chemical - casing;
        assert!(
            energy.is_finite() && energy > 0.0,
            "a positive tool charge scales the load-certified burst's positive blast energy"
        );
        let radius = (3.0 * mass / (4.0 * std::f32::consts::PI * self.density)).cbrt();
        let products = products_momentum(mass, speed);
        Blast {
            mass,
            chemical,
            casing,
            energy,
            charge: model::Blast::of(energy, radius, products).charge,
            radius,
            products,
        }
    }
}

/// One charge's energies (J), TNT charge (kg), radius (m) and products' momentum (N·s), filler
/// `mass` kg.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Blast {
    mass: f32,
    chemical: f32,
    casing: f32,
    energy: f32,
    charge: f32,
    radius: f32,
    products: f32,
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
    /// Continue unfinished equilibrium or redistribution without another source.
    Continue,
    /// One round's crossing of the wall: per chunk the line it cut (entry, end) and the energy it
    /// lost there (J); its travel and the momentum it lost across the wall (N·s).
    Line {
        what: String,
        lines: Vec<(usize, DeliveredLine)>,
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
    /// The game's damage model can deliver a wall exit cone before that shell's line.
    pending_spall: std::collections::HashMap<Entity, f32>,
}

/// One numerical source operation's outcome, for the readout.
#[derive(Clone, Debug, Default)]
struct Outcome {
    what: String,
    deposited: f32,
    crushed: f32,
    scabs: usize,
    chunks: usize,
    dust: usize,
    local: usize,
    structural: usize,
    /// The first shear failure's and the first tensile failure's demand and capacity.
    worst: Option<(f32, f32)>,
    opening: Option<(f32, f32)>,
    solve_rounds: usize,
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
        crate::provenance::from_shipped_str(crate::tank::TANK_SPEC_PATH, SEED_SPEC)
            .expect("the shipped spec sheet parses");
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

/// The plan's case among the wall's `rounds`.
///
/// # Panics
///
/// When the case names a key none of the wall's rounds has (ADR-0011).
fn resolve_case(case: &RangeWallCase, rounds: &[WallRound]) -> Case {
    match case {
        RangeWallCase::Round(key) => Case::Round(
            rounds
                .iter()
                .position(|round| round.key == *key)
                .unwrap_or_else(|| {
                    panic!(
                        "{}: scene.range_wall.case: no wall round has the key {key:?}",
                        crate::dev_capture::plan::VARIABLE
                    )
                }),
        ),
        &RangeWallCase::Blast { kg, standoff_m } => Case::Blast {
            mass: kg,
            standoff: standoff_m,
        },
    }
}

/// The brick as the wall reads it: the shipped registry's `Brick` row ([`Material::of`]).
fn brick() -> Material {
    Material::of(SubstanceRegistry::shipped_static(), SUBSTANCE)
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
    let distance = wall_tool().view_fill.0 * layout().size.y;
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
    config.line.width = wall_tool().gizmo_width_px.0;
    // Resolution: pull diagnostic lines ahead of the face to prevent depth fighting.
    config.depth_bias = -0.05;
    let brick_colour = LinearRgba::from(rgba(wall_tool().brick_rgba.0));
    let hot = LinearRgba::from(rgba(wall_tool().hot_rgba.0));
    let heat = (0..=HEAT_LEVELS)
        .map(|level| {
            let t = level as f32 / HEAT_LEVELS as f32;
            materials.add(StandardMaterial {
                base_color: Color::LinearRgba(brick_colour.mix(&hot, t)),
                emissive: hot * (wall_tool().heat_emissive.0 * t),
                perceptual_roughness: wall_tool().roughness.0,
                ..default()
            })
        })
        .collect();
    let witness = (0..=WITNESS_LEVELS)
        .map(|level| {
            let colour = if level == 0 {
                rgba(wall_tool().witness_clear_rgba.0)
            } else {
                let t = (level - 1) as f32 / (WITNESS_LEVELS - 1) as f32;
                {
                    let low = wall_tool().witness_low_rgba.0;
                    let high = wall_tool().witness_high_rgba.0;
                    rgba(std::array::from_fn(|i| low[i] + (high[i] - low[i]) * t))
                }
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
            base_color: rgba(wall_tool().loose_rgba.0),
            perceptual_roughness: wall_tool().roughness.0,
            ..default()
        }),
        toppled: materials.add(StandardMaterial {
            base_color: rgba(wall_tool().toppled_rgba.0),
            perceptual_roughness: wall_tool().roughness.0,
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

/// A chunk's drawn mesh, vertices about its bounding box's centre: its faces inset by [`WallTool::seam_m`].
fn chunk_mesh(block: &Block) -> Mesh {
    let inset = crate::construction::breakage::inset_point(block, 1.0, wall_tool().seam_m.0);
    let (mut positions, mut normals, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for (normal, face) in block.faces() {
        let base = positions.len() as u32;
        for corner in &face {
            positions.push((inset(*corner) - block.center()).to_array());
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

/// A convex collider of `block`'s whole hull, about its bounding box's centre: its occupied
/// volume, so its mass and inertia at the brick's density are the cut's.
fn chunk_collider(block: &Block) -> Collider {
    let centre = block.center();
    Collider::convex_hull(block.points.iter().map(|p| *p - centre).collect())
        .expect("a bake chunk has volume")
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
        let collider = chunk_collider(block);
        entities.push(Some(
            commands
                .spawn((
                    Name::new("wall chunk"),
                    WallChunk(index),
                    Mesh3d(mesh),
                    MeshMaterial3d(assets.heat[0].clone()),
                    Transform::from_translation(block.center()),
                    RigidBody::Static,
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
        live: crate::construction::breakage::Live::from_structure(
            cut.structure.clone(),
            cut.elements,
        ),
        structural: vec![false; cut.structure.joints.len()],
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
    camera: Query<'w, 's, &'static mut Transform, With<FreeFlyCam>>,
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
    let mut camera = p
        .camera
        .single_mut()
        .expect("the wall inspector has exactly one FreeFlyCam");
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
            *camera = view_pose(controls.view, aim_point(wall));
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
        let eye = camera.translation;
        let dir = camera.forward().as_vec3();
        let state = &wall.live.state;
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
/// over a sunflower of the authored inspection spread.
fn fire_burst(
    mut burst: ResMut<WallBurst>,
    rounds: Res<WallRounds>,
    time: Res<Time>,
    camera: Query<&Transform, With<FreeFlyCam>>,
    mut commands: Commands,
) {
    if burst.left == 0 {
        return; // No automatic burst is active.
    }
    let entry = rounds
        .0
        .get(burst.round)
        .expect("an active burst names a chambered wall round");
    let camera = camera
        .single()
        .expect("the wall inspector has exactly one FreeFlyCam");
    while burst.left > 0 && time.elapsed_secs() >= burst.next {
        let k = burst.fired;
        let forward = camera.forward().as_vec3();
        let direction = if entry.count > 1 {
            let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
            let r = wall_tool().spread_rad.0 * ((k as f32 + 0.5) / entry.count as f32).sqrt();
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
            camera.translation,
            Dir3::new(direction)
                .expect("the camera basis and forward-centred spread give a nonzero direction"),
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

/// The game's damage-model wall inspector reads resolved work and momentum, never inferred
/// round construction or immutable crossing metadata. Each tick's delivered lines are read once.
fn read_hits(
    marched: On<ShellMarched>,
    shells: Query<Option<&SpallMarks>>,
    chunks: Query<&WallChunk>,
    mut queue: ResMut<WallQueue>,
    mut log: ResMut<WallLog>,
) {
    let lines: Vec<_> = marched
        .lines
        .iter()
        .filter_map(|line| {
            match chunks.get(line.crossing.volume) {
                Ok(chunk) => Some((chunk.0, line.clone())),
                Err(_) => None, // Another body's volume belongs to its own reader.
            }
        })
        .collect();
    let spall = shells
        .get(marched.shell)
        .expect("ShellMarched observers run before their shell despawns");
    log.pending_spall.retain(|shell, _| shells.contains(*shell));
    let mut strength = 0.0;
    if let Some(spall) = spall {
        log.witness.resize(TILES.0 * TILES.1, 0.0);
        for cone in spall
            .cones
            .get(marched.cones_from..)
            .expect("ShellMarched's cone cursor precedes this tick's new cones")
        {
            if !cone.exited.iter().any(|&source| chunks.contains(source)) {
                continue; // Another body's exit field belongs to its own inspector.
            }
            let weight = cone.ray_share();
            for ray in &cone.rays {
                if let Some(tile) = witness_at(ray.end) {
                    log.witness[tile] += ray.strength * weight;
                }
            }
            log.cones.push(WallCone {
                origin: cone.origin,
                rays: cone
                    .rays
                    .iter()
                    .map(|ray| (ray.end, ray.strength))
                    .collect(),
            });
            strength += cone.strength;
        }
    }
    if strength > 0.0 {
        *log.pending_spall.entry(marched.shell).or_default() += strength;
    }
    if !lines.is_empty() {
        queue.0.push(Deposit::Line {
            what: "resolved round".to_string(),
            lines,
            // A line can emit no exit cone.
            spall: log.pending_spall.remove(&marched.shell).unwrap_or_default(),
        });
    }
}

/// What one deposit did to the wall.
#[derive(Debug, Default)]
struct Applied {
    deposited: f32,
    crushed: f32,
    scabs: usize,
    chunks: usize,
    worn: Vec<usize>,
    dusted: Vec<usize>,
    local: usize,
    failures: Vec<model::Failure>,
    /// Sparse equilibrium rounds performed in this deposit.
    solve_rounds: usize,
    /// Each loose group, its centroid, linear and angular velocity, and whether a structural crack
    /// bounds it.
    groups: Vec<(Vec<usize>, Vec3, Vec3, Vec3, bool)>,
    /// A blast's charge, origin, intercepted energy (audit only), its rays' summed unsigned
    /// impulse (N·s), and its drawn ray ends.
    blast: Option<(Blast, Vec3, f32, f32, Vec<Vec3>)>,
}

/// The holes a deposit cuts in the wall, the game's own sizing ([`crate::construction::breakage::cut_holes`]):
/// the wall is its own frame at scale 1, each hole about its chunk's centroid.
fn cut_holes(
    cut: &model::Cut,
    lines: &[(usize, Vec3, Vec3, f32)],
    blast: &[Load],
) -> Vec<(usize, crate::construction::breakage::Hole)> {
    crate::construction::breakage::cut_holes(
        &cut.blocks,
        |c| cut.structure.chunks[c].centroid,
        |c| cut.structure.chunks[c].material.crush,
        1.0,
        lines,
        blast,
    )
}

/// Run one deposit through the game's laws: the wear, holes and dust, the local cracks, the
/// structural check, and the loose groups with the velocities the blows on them give.
fn apply_deposit(
    wall: &mut Wall,
    controls: &WallControls,
    charge: &Charge,
    field: &[(Vec3, f32)],
    deposit: &Deposit,
    step: f32,
) -> Applied {
    let mut applied = Applied::default();
    let (loads, holes) = match deposit {
        Deposit::Continue => (Vec::new(), Vec::new()),
        Deposit::Line { lines, .. } => (
            lines
                .iter()
                .flat_map(|(chunk, line)| {
                    crate::construction::breakage::resolved_line_loads(
                        *chunk,
                        &wall.cut.structure.chunks[*chunk].material,
                        line,
                    )
                })
                .collect(),
            cut_holes(
                &wall.cut,
                &lines
                    .iter()
                    .map(|(chunk, line)| {
                        let crossing = line.crossing;
                        (*chunk, crossing.entry, crossing.end, crossing.energy)
                    })
                    .collect::<Vec<_>>(),
                &[],
            ),
        ),
        &Deposit::Blast {
            mass,
            standoff,
            at,
            into,
        } => {
            let blast = charge.blast(mass, controls.cased);
            let origin = at - into * (standoff + blast.radius);
            let state = &wall.live.state;
            let structure = &wall.cut.structure;
            let hits = model::sample_blocks(
                &wall.cut.blocks,
                |i| state.standing(i),
                |i| structure.chunks[i].material,
                origin,
                model::Blast::of(blast.energy, blast.radius, blast.products),
                field,
            );
            let coupled: f32 = hits.iter().map(|(_, hit)| hit.energy).sum();
            let pushed: f32 = hits.iter().map(|(_, hit)| hit.impulse.length()).sum();
            let drawn = hits
                .iter()
                .step_by(RAY_DRAW_STRIDE)
                .map(|(_, hit)| hit.at)
                .collect();
            applied.blast = Some((blast, origin, coupled, pushed, drawn));
            let contacts: Vec<_> = state.contacts(structure).collect();
            let (loads, scabs) = Load::blast(&hits, |i| model::Backing {
                block: &wall.cut.blocks[i],
                scale: 1.0,
                material: structure.chunks[i].material,
                contacts: &contacts,
            });
            applied.scabs = scabs.len();
            let holes = cut_holes(&wall.cut, &[], &loads);
            (loads, holes)
        }
    };
    applied.deposited = loads.iter().map(|load| load.energy).sum();
    applied.chunks = loads
        .iter()
        .map(|load| load.chunk)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    applied.crushed = loads.iter().map(|load| load.crushed).sum();
    let pools = wall.live.state.pool.clone();
    let changes = crate::construction::breakage::run_laws(
        &wall.cut.element_of,
        &mut wall.live,
        &loads,
        holes,
        Vec3::ZERO,
        step,
    );
    applied.worn = pools
        .iter()
        .zip(&wall.live.state.pool)
        .enumerate()
        .filter_map(|(c, (before, after))| (after < before).then_some(c))
        .collect();
    applied.dusted = changes
        .events
        .iter()
        .filter_map(|event| match event {
            crate::construction::breakage::Event::Dust(c) => Some(usize::from(*c)),
            _ => None,
        })
        .collect();
    let cracks = changes
        .events
        .iter()
        .filter(|event| matches!(event, crate::construction::breakage::Event::Crack(_)))
        .count();
    applied.failures = changes.failures;
    applied.local = cracks - applied.failures.len();
    applied.solve_rounds = changes.solve_rounds;
    for failure in &applied.failures {
        wall.structural[failure.joint] = true;
    }
    for (group, loads) in changes.groups {
        let chunks = &wall.cut.structure.chunks;
        let mass: f32 = group.iter().map(|&i| chunks[i].mass).sum();
        let centroid = group
            .iter()
            .map(|&i| chunks[i].centroid * chunks[i].mass)
            .sum::<Vec3>()
            / mass;
        let colliders = group
            .iter()
            .map(|&c| {
                let block = &wall.cut.blocks[c];
                (
                    c,
                    Collider::compound(vec![(
                        block.center() - centroid,
                        Quat::IDENTITY,
                        chunk_collider(block),
                    )]),
                )
            })
            .collect();
        let launch = crate::debris::launch::launch_colliders(
            centroid,
            colliders,
            |c| chunks[c].material.density,
            &loads,
        );
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
            .push((group, centroid, launch.velocity, launch.spin, structural));
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
    fixed: Res<'w, Time<Fixed>>,
    commands: Commands<'w, 's>,
}

fn apply_deposits(mut p: ApplyParam) {
    let (Some(wall), Some(assets)) = (p.wall.as_deref_mut(), p.assets.as_deref()) else {
        return;
    };
    if p.queue.0.is_empty() && wall.live.assessment.needs_tick() {
        p.queue.0.push(Deposit::Continue);
    }
    // This manual instrument delivers one queued source operation per numerical tick.
    // Further source operations retain input order; an idle tick continues the pending solve.
    if !p.queue.0.is_empty() {
        let deposit = p.queue.0.remove(0);
        let applied = apply_deposit(
            wall,
            &p.controls,
            &p.charge,
            &p.field.0,
            &deposit,
            p.fixed.timestep().as_secs_f32(),
        );
        info!(
            "wall prototype: shared laws: crushed {:.6e} m3, {} rear scabs",
            applied.crushed, applied.scabs
        );
        for &chunk in &applied.worn {
            if let Some(entity) = wall.entities[chunk] {
                let left = wall.live.state.pool[chunk]
                    / model::full_pool(&wall.cut.structure.chunks[chunk]);
                let level = ((1.0 - left) * HEAT_LEVELS as f32)
                    .round()
                    .clamp(0.0, HEAT_LEVELS as f32) as usize;
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
                base_color: rgba(wall_tool().dust_rgba.0),
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
                    .with_scale(Vec3::splat(wall_tool().puff_start_m.0)),
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
                    Name::new("construction piece"),
                    WallPiece,
                    Transform::from_translation(*centroid),
                    Visibility::default(),
                    crate::eased_dynamic_body(),
                    crate::substances::contact_friction(wall.material.friction),
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
                            crate::construction::breakage::loose_collider(
                                block,
                                1.0,
                                block.center(),
                            ),
                            crate::debris::launch::occupied_mass(
                                &chunk_collider(block),
                                wall.material.density,
                            ),
                            crate::substances::contact_friction(wall.material.friction),
                            CollisionLayers::new(
                                [Layer::Default],
                                [Layer::Default, Layer::Terrain, Layer::Armor],
                            ),
                        ));
                    }
                });
        }
        let what = match &deposit {
            Deposit::Continue => "structure update".to_owned(),
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
        let first = |cause: model::Cause| {
            applied
                .failures
                .iter()
                .find(|failure| failure.cause == cause)
                .map(|failure| (failure.demand, failure.capacity))
        };
        p.log.last = Some(Outcome {
            what,
            deposited: applied.deposited,
            crushed: applied.crushed,
            scabs: applied.scabs,
            chunks: applied.chunks,
            dust: applied.dusted.len(),
            local: applied.local,
            structural: applied.failures.len(),
            worst: first(model::Cause::Shear),
            opening: first(model::Cause::Tension),
            solve_rounds: applied.solve_rounds,
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
                "wall prototype: {}: {:.0} kinetic J into {} chunks, {} dust, {} local cracks, \
                 {} joint cracks (first shear {:?} Pa and first tension {:?} \
                 Pa demand, capacity), force solve {} rounds, \
                 {} groups fell ({} chunks), exit cones {:.0} J, blast {:?} (audit J, N s)",
                last.what,
                last.deposited,
                last.chunks,
                last.dust,
                last.local,
                last.structural,
                last.worst,
                last.opening,
                last.solve_rounds,
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
    // Resolution: separate face lines from the surface to prevent depth fighting.
    let out = |p: Vec3| p + Vec3::Z * (0.003 * (p.z - mid).signum());
    for (index, joint) in wall.cut.structure.joints.iter().enumerate() {
        if !wall.live.state.cracked[index] || !wall.live.state.live(joint) {
            continue;
        }
        let colour = if wall.structural[index] {
            rgba(wall_tool().structural_crack_rgba.0)
        } else {
            rgba(wall_tool().local_crack_rgba.0)
        };
        let face = &wall.cut.faces[index];
        for k in 0..face.len() {
            gizmos.line(out(face[k]), out(face[(k + 1) % face.len()]), colour);
        }
    }
    for cone in &log.cones {
        let strongest = cone.rays.iter().map(|(_, s)| *s).fold(0.0_f32, f32::max);
        for &(end, strength) in &cone.rays {
            let alpha =
                (strength / strongest.max(f32::EPSILON)).clamp(wall_tool().ray_alpha_floor.0, 1.0);
            gizmos.line(
                cone.origin,
                end,
                rgba(wall_tool().cone_rgba.0).with_alpha(alpha),
            );
        }
    }
    let marker = rgba(wall_tool().blast_rgba.0);
    if let Some((origin, radius, rays)) = &log.blast {
        gizmos.sphere(Isometry3d::from_translation(*origin), *radius, marker);
        for &end in rays {
            gizmos.line(
                *origin,
                end,
                marker.with_alpha(wall_tool().blast_ray_alpha.0),
            );
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
            (dose.log10().floor() as usize + 1).min(WITNESS_LEVELS)
        };
        if material.0 != assets.witness[level] {
            material.0 = assets.witness[level].clone();
        }
    }
}

/// Grow and fade each dust puff over [`WallTool::puff_life_s`], then remove it and its material.
fn animate_puffs(
    time: Res<Time>,
    mut puffs: Query<(Entity, &DustPuff, &mut Transform)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for (entity, puff, mut transform) in &mut puffs {
        let age = (time.elapsed_secs() - puff.born) / wall_tool().puff_life_s.0;
        if age >= 1.0 {
            materials.remove(&puff.material);
            commands.entity(entity).despawn();
            continue;
        }
        transform.scale = Vec3::splat(
            wall_tool().puff_start_m.0 + wall_tool().puff_growth_m.0 * (1.0 - (1.0 - age).powi(3)),
        );
        let mut material = materials
            .get_mut(&puff.material)
            .expect("a live dust puff retains its material until it expires");
        material
            .base_color
            .set_alpha(wall_tool().dust_rgba.0[3] * (1.0 - age));
    }
}

fn spawn_readout(mut commands: Commands, script: Option<Res<WallScript>>) {
    // Fixture layout: the capture caption and its help row use the screen's upper-left corner.
    let left = Val::Px(if script.is_some() { 6.0 } else { UI_LEFT });
    for (top, help) in [(6.0, false), (30.0, true)] {
        let mut text = commands.spawn((
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(wall_tool().readout_font_px.0),
                ..default()
            },
            TextColor(rgba(wall_tool().readout_rgba.0)),
            BackgroundColor(rgba(wall_tool().readout_background_rgba.0)),
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(top),
                left,
                padding: UiRect::all(Val::Px(wall_tool().readout_padding_px.0)),
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
            "last: {}: {} kinetic into {} chunks -> {} dust, {} local cracks, {} structural joint cracks, {} groups fell ({} chunks)",
            last.what,
            joules(last.deposited),
            last.chunks,
            last.dust,
            last.local,
            last.structural,
            last.groups,
            last.fallen
        ),
    ];
    lines.push(format!(
        "      crushed {:.6} m3, {} rear scabs",
        last.crushed, last.scabs
    ));
    if let Some((blast, coupled, pushed)) = last.blast {
        lines.push(format!(
            "      blast intercepted {} of {} ({:.0} %, audit), pushed {:.0} N s",
            joules(coupled),
            joules(blast.energy),
            100.0 * coupled / blast.energy.max(f32::EPSILON),
            pushed
        ));
    }
    if let Some((demand, capacity)) = last.worst {
        lines.push(format!(
            "      first shear failure: traction {demand:.0} Pa against {capacity:.0} Pa"
        ));
    }
    if let Some((demand, capacity)) = last.opening {
        lines.push(format!(
            "      first tensile failure: peak tension {:.0} kPa against the seam's {:.0} kPa",
            demand / 1e3,
            capacity / 1e3
        ));
    }
    lines.push(format!(
        "      force solve: {} conjugate-gradient rounds",
        last.solve_rounds
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

/// The last shot in a few words: its blast impulse or its kinetic cavity energy.
fn last_shot(last: &Outcome) -> String {
    if let Some((_, _, pushed)) = last.blast {
        format!("{}, {pushed:.0} N s", last.what)
    } else {
        format!("{}, {} kinetic", last.what, joules(last.deposited))
    }
}

#[derive(SystemParam)]
struct ReadoutParam<'w, 's> {
    wall: Option<Res<'w, Wall>>,
    log: Res<'w, WallLog>,
    controls: Res<'w, WallControls>,
    camera: Query<'w, 's, &'static Transform, With<FreeFlyCam>>,
    text: Query<
        'w,
        's,
        (&'static mut Text, &'static mut Visibility),
        (With<WallReadout>, Without<WallHelp>, Without<super::Legend>),
    >,
    help: Query<
        'w,
        's,
        (&'static mut Text, &'static mut Visibility),
        (With<WallHelp>, Without<WallReadout>, Without<super::Legend>),
    >,
    range_legend: Query<
        'w,
        's,
        &'static mut Visibility,
        (With<super::Legend>, Without<WallReadout>, Without<WallHelp>),
    >,
}

/// The readout: one line (the last shot), and the help panel
/// while H has it open. Both show only near the wall, where the range's own H legend stays shut.
fn update_readout(mut p: ReadoutParam) {
    let camera = p
        .camera
        .single()
        .expect("the wall inspector has exactly one FreeFlyCam");
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
    let near = camera.translation.distance(layout().centre()) < wall_tool().readout_range_m.0;
    let shown_if = |on: bool| {
        if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    let (mut text, mut visibility) = p
        .text
        .single_mut()
        .expect("the wall inspector spawns exactly one readout");
    visibility.set_if_neq(shown_if(near));
    if text.0 != line {
        text.0 = line;
    }
    let (mut help, mut help_visibility) = p
        .help
        .single_mut()
        .expect("the wall inspector spawns exactly one help panel");
    help_visibility.set_if_neq(shown_if(near && c.help));
    if help.0 != HELP {
        help.0 = HELP.to_string();
    }
    if near {
        p.range_legend
            .single_mut()
            .expect("the range spawns exactly one legend")
            .set_if_neq(Visibility::Hidden);
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
    camera: Query<'w, 's, &'static mut Transform, With<FreeFlyCam>>,
    burst: ResMut<'w, WallBurst>,
    rounds: Res<'w, WallRounds>,
    queue: ResMut<'w, WallQueue>,
    label: Query<'w, 's, &'static mut Visibility, With<super::RoundLabel>>,
}

fn drive_script(mut p: ScriptParam) {
    let mut camera = p
        .camera
        .single_mut()
        .expect("the wall inspector has exactly one FreeFlyCam");
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
    *camera = view_pose(view, aim);
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
