//! The wall prototype, about the game's environment destruction (game state only): a brick wall on
//! the range whose chunks the range's rounds, a point deposit and a blast turn to dust or crack
//! loose, under a candidate crack law switched live, with a structural check beside it. A
//! prototype isolated to the sandbox: nothing here is wired into the game's sim or its net.
//!
//! * **The wall.** 4 × 2 m of brick, 0.25 m thick, standing at [`WALL_MIN`], cut by a generator
//!   ([`model::running_bond`] or [`model::jittered`], K) into chunks that each carry a static
//!   collider and the `Brick` substance, so the walk crosses them as it crosses any hard volume.
//! * **Deposits.** A round's crossing deposits, in each chunk, the energy the walk says it lost
//!   there ([`ArmourMark::Crossing`]), and pushes it with the momentum the round lost. The point
//!   tool (P) deposits a chosen energy, 50 kJ to 5 MJ, at the aim point, with no push. The blast
//!   tool (O) bursts a charge of 0.1–5 kg at 0–5 m standoff: a sphere of rays ([`blast_field`])
//!   each stopping on the first chunk it meets with its share of the energy and the momentum.
//! * **Responses.** [`model`] turns each deposit into dust (the pools), local cracks (the active
//!   [`CrackLaw`]) and structural cracks ([`model::structural`], U toggles it), then lets every
//!   group with no path to the ground fall with the momentum pushed into it.
//! * **The view.** Each chunk's pool as heat, local cracks in cyan and structural ones in
//!   magenta, a dust puff where a chunk went, loose groups as falling bodies on the default layer
//!   (out of ballistics; grey-blue when a local crack freed them, purple when a structural one
//!   did), and a one-line readout (the crack rule, the structural check, the last shot); H opens a
//!   help panel of the keys and the colour legend. Every value of a resolved shot goes to the log.
//!   The readout shows only while the camera stands within [`READOUT_RANGE`] of the wall, so the
//!   range's own view carries no more text than its round's name.
//! * **Spall.** Every exit cone the wall throws is read off the shell's record: the game's today
//!   (its energy share of the energy lost) and the candidate (that, scaled by the exited
//!   substance's strength relative to armour steel, capped at 1), each one's dose summed on a
//!   witness panel behind the wall; J toggles which is drawn. The game's own dose is untouched.
//!
//! Keys: 1–5 law A–E, K cut, N new wall, B round, F fire it, P point deposit and `[`/`]` its
//! energy, H help, O blast and `,`/`.` its filler mass, `;`/`'` its standoff, V its casing, U the
//! structural check, J spall, G camera (front, back, witness).
//!
//! `OVERMATCH_WALL_CASE=<law letter>,<round key | p<energy> | x<kg>@<standoff m>>` (`B,88mm_AP`,
//! `D,p1m`, `E,x1@3`) runs one case for a hidden capture ([`drive_script`]): the camera stands at
//! the front, then the back, fires (or deposits, or bursts) at [`SCRIPT_FIRE`] s from the front,
//! and stands front then back again for the after frames. `OVERMATCH_WALL_SPALL=candidate`,
//! `OVERMATCH_WALL_CUT=jittered`, `OVERMATCH_WALL_CASING=bare`, `OVERMATCH_WALL_STRUCTURAL=off`
//! and `OVERMATCH_WALL_VIEW=witness` (the last frames from the witness pose) set the rest.

mod model;
#[cfg(test)]
mod tests;

use avian3d::prelude::{
    AngularVelocity, Collider, ColliderDensity, CollisionLayers, LayerMask, LinearVelocity,
    RigidBody,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use self::model::{CrackLaw, Cut, CutKind, Masonry, Push, Source, WallState};
use super::{Autopilot, FreeFlyCam, SEED_SPEC, UI_LEFT, panel_capturing};
use crate::ballistics::spall_cone::{AxisRule, ConeShape, FieldDescriptor, FieldLaw};
use crate::ballistics::{
    ArmourMark, BallisticVolume, CrossingEnd, FireShell, PenetrationMarks, SpallMarks,
};
use crate::substances::{REFERENCE_FACTOR, SubstanceRegistry};
use crate::{Layer, spec};

/// The wall's min corner, world metres: clear of the range's plates and target (x within ±7.5)
/// and of the test course (z ≤ −30). Its front face looks down +Z.
const WALL_MIN: Vec3 = Vec3::new(14.0, 0.0, 3.875);
/// Width, height and thickness, m (the brief's wall).
const WALL_SIZE: Vec3 = Vec3::new(4.0, 2.0, 0.25);
/// The cut's chunk size, m (the brief's ~20 cm along brick seams).
const CHUNK: f32 = 0.2;
/// The jittered cut's seed: any fixed value.
const JITTER_SEED: u64 = 0x5EED_0001;
/// The substance every chunk wears.
const SUBSTANCE: &str = "Brick";
/// Brick density, kg/m³: London stock, a molded soft-mud brick. CLAIMED (Engineering ToolBox,
/// "Bricks - Densities", read 2026-10-30); 1765–2165 across common clay bricks.
const DENSITY: f32 = 1845.0;
/// The joints' tensile strength, Pa: the modulus of rupture normal to bed joints of solid units in
/// portland cement–lime Type N mortar, 690 kPa. CLAIMED (BIA Technical Note 3A, 2024, Table 7,
/// read); 349–919 kPa across mortar types.
const JOINT_TENSILE: f32 = 0.69e6;
/// The brick–mortar interface's mode-I fracture energy, J/m²: the geometric mean of 5–20 J/m².
/// CLAIMED (van der Pluijm's couplet tests as Lourenço 1996 restates them; not read here).
const JOINT_TOUGHNESS: f32 = 10.0;
/// The masonry's Young's modulus, Pa: `E_m = 700 f'm` (the TMS 402 lower bound; prisms test at
/// 700–1200 f'm, BIA TN 3A eq. 1, read) at `f'm` 12.3 MPa, DERIVED by interpolating TN 3A Table 5
/// for 36.5 MPa units in Type N mortar: 8.6 GPa, DERIVED.
const MODULUS: f32 = 8.6e9;
/// A filler's chemical energy per kilogram, J/kg: the TNT-equivalent convention, 1 kg = 4.184 MJ.
/// CLAIMED (NIST SP 811, the ton of TNT as 4.184e9 J); ASSUMED to stand for the seed round's
/// filler, whose own heat of detonation no file states.
const FILLER_ENERGY: f32 = 4.184e6;
/// The point tool's energy ladder, J (the brief's 50 kJ to 5 MJ).
const ENERGIES: [f32; 7] = [5.0e4, 1.0e5, 2.0e5, 5.0e5, 1.0e6, 2.0e6, 5.0e6];
/// The blast tool's filler masses, kg, and standoffs, m (the owner's 0.1–5 kg and 0–5 m).
const MASSES: [f32; 6] = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0];
const STANDOFFS: [f32; 6] = [0.0, 0.5, 1.0, 2.0, 3.0, 5.0];
/// The blast field's ray count (Resolution): 0.0031 sr a ray, a chunk face at 3 m holds ~1.4.
const BLAST_RAYS: usize = 4096;
/// Rounds an automatic gun fires per F (the brief's burst of 50).
const BURST: usize = 50;
/// The burst's spread half-angle, rad. ASSUMED: no gun file states a dispersion; a tight group so
/// a burst's wear lands on a few chunks.
const BURST_SPREAD: f32 = 0.003;
/// Spacing of the point sources a line deposit is split into along its chord, m: a quarter of a
/// chunk (Resolution).
const LINE_STEP: f32 = 0.05;
/// How far the drawn chunk falls short of its collider on every side, m: the drawn seam.
const SEAM: f32 = 0.004;
/// How far a loose chunk's collider falls short of its box on every side, m, so a group freed in
/// place does not start inside its standing neighbours.
const LOOSE_GAP: f32 = 0.005;
/// The witness panel: its distance behind the back face (past the back view's camera), its tile
/// size and count (m, m, x × y).
const WITNESS_GAP: f32 = 4.4;
const TILE: f32 = 0.25;
const TILES: (usize, usize) = (12, 8);
/// The camera's distance from the face it frames, m: the 4 × 2 m wall fills two thirds of a 45°
/// 16:9 view's height, below the readout.
const VIEW_DISTANCE: f32 = 3.8;
/// A dust puff's life, s.
const PUFF_LIFE: f32 = 1.6;
/// How near the wall's centre the camera must stand for the readout to show, m: the wall's own
/// views stand 3.8–7.4 m off it, the range's boot pose 21 m.
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
    let mut controls = WallControls {
        law: CrackLaw::Stress,
        cut: CutKind::RunningBond,
        energy: 4,
        mass: 3,
        standoff: 0,
        cased: !flag("OVERMATCH_WALL_CASING", "bare"),
        structural: !flag("OVERMATCH_WALL_STRUCTURAL", "off"),
        spall: Spall::Today,
        round: 0,
        view: View::Front,
        help: flag("OVERMATCH_WALL_HELP", "1"),
    };
    if flag("OVERMATCH_WALL_SPALL", "candidate") {
        controls.spall = Spall::Candidate;
    }
    if flag("OVERMATCH_WALL_CUT", "jittered") {
        controls.cut = CutKind::Jittered;
    }
    if let Some((law, case)) = script {
        controls.law = law;
        info!("wall prototype: capture case {law:?}, {case:?}");
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

/// The wall as it stands: its cut, its state, each standing chunk's entity and drawn mesh, its
/// material and the range of its joints' Griffith strengths (Pa) for the readout.
#[derive(Resource)]
struct Wall {
    cut: Cut,
    state: WallState,
    entities: Vec<Option<Entity>>,
    meshes: Vec<Handle<Mesh>>,
    material: Masonry,
    griffith: (f32, f32),
}

/// Which spall the view draws.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Spall {
    Today,
    Candidate,
}

/// The camera stand G steps through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Front,
    Back,
    Witness,
}

/// The wall's settings: the crack law, the cut, the point tool's energy and the blast tool's mass
/// and standoff (indices into [`ENERGIES`], [`MASSES`], [`STANDOFFS`]), whether the charge is
/// cased and the structural check runs, the spall drawn, the round F fires (an index into
/// [`WallRounds`]) and the camera stand.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
struct WallControls {
    law: CrackLaw,
    cut: CutKind,
    energy: usize,
    mass: usize,
    standoff: usize,
    cased: bool,
    structural: bool,
    spall: Spall,
    round: usize,
    view: View,
    /// The help panel (H) is open.
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

/// The blast tool's charge, from data: the filler's density (kg/m³), and the casing of the seed
/// tank's burst round as the burst law derives it — its mass per kg of filler `M/C` and the speed
/// it is driven to, `v_s = K (M/C + ½)^−½` (m/s). A charge of `C` kg carries `M/C × C` kg of it.
#[derive(Resource, Clone, Copy, Debug)]
struct Charge {
    density: f32,
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
                    casing_ratio: ((core.speed_constant / speed).powi(2) - 0.5) as f32,
                    casing_speed: burst.sideways_speed,
                }
            }
            (Some(core), None) => Self {
                density: core.density as f32,
                casing_ratio: 0.0,
                casing_speed: 0.0,
            },
            _ => Self {
                density: 1650.0,
                casing_ratio: 0.0,
                casing_speed: 0.0,
            },
        }
    }

    /// A charge of `mass` kg: its chemical energy, its casing's driven energy (none when bare), the
    /// blast's energy (the difference) and momentum `√(2 C E)`, all J or N·s, and its radius (m).
    fn blast(&self, mass: f32, cased: bool) -> Blast {
        let chemical = FILLER_ENERGY * mass;
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
            momentum: (2.0 * mass * energy).sqrt(),
            radius: (3.0 * mass / (4.0 * std::f32::consts::PI * self.density)).cbrt(),
        }
    }
}

/// One charge's energies (J), momentum (N·s) and radius (m), filler `mass` kg.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Blast {
    mass: f32,
    chemical: f32,
    casing: f32,
    energy: f32,
    momentum: f32,
    radius: f32,
}

/// The blast field's directions with their shares (summing to 1): the field machinery's ball at
/// ratio 0, uniform over the sphere ([`crate::ballistics::spall_cone`]).
#[derive(Resource)]
struct BlastField(Vec<(Vec3, f32)>);

fn blast_field() -> Vec<(Vec3, f32)> {
    let spread = ConeShape::new(AxisRule::ExitDirection, BLAST_RAYS).and_then(|shape| {
        shape.spread(FieldDescriptor {
            law: FieldLaw::Ball,
            ratio: 0.0,
        })
    });
    let directions: Vec<(Vec3, f32)> = match spread {
        Ok(spread) => spread
            .directions(Dir3::Y)
            .map(|(dir, weight)| (Vec3::from(dir), weight))
            .collect(),
        Err(error) => {
            error!("wall prototype: the blast field is not built: {error}");
            Vec::new()
        }
    };
    let total: f32 = directions.iter().map(|(_, w)| w).sum();
    directions
        .into_iter()
        .map(|(dir, w)| (dir, w / total.max(f32::EPSILON)))
        .collect()
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
        /// The strength (J) of the exit cones it threw, under today's spall.
        spall: f32,
    },
    /// `energy` J at `at`, pushing along `push`; no momentum.
    Point { energy: f32, at: Vec3, push: Vec3 },
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

/// One exit cone the wall threw: its origin, and its rays' ends and strengths (J, today's law).
struct WallCone {
    origin: Vec3,
    rays: Vec<(Vec3, f32)>,
}

/// What the readout and the drawing keep: the last outcome, the cones thrown, each witness tile's
/// dose under today's spall and the candidate's (J), the last point deposit (where, crush
/// radius, push) and the last blast's origin, radius and drawn rays.
#[derive(Resource, Default)]
struct WallLog {
    last: Option<Outcome>,
    cones: Vec<WallCone>,
    witness_today: Vec<f32>,
    witness_candidate: Vec<f32>,
    point: Option<(Vec3, f32, Vec3)>,
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
    worst: Option<(f32, f32)>,
    groups: usize,
    fallen: usize,
    spall_today: f32,
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
    /// A point deposit of this many J.
    Point(f32),
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

/// `<law letter>,<round key | p<energy> | x<kg>@<standoff m>>`, the energy in J with an optional
/// `k` or `m` suffix.
fn parse_case(case: &str, rounds: &[WallRound]) -> Option<(CrackLaw, Case)> {
    let (law, what) = case.split_once(',')?;
    let law = CrackLaw::of_letter(law.trim().chars().next()?)?;
    let what = what.trim();
    if let Some(index) = rounds.iter().position(|round| round.key == what) {
        return Some((law, Case::Round(index)));
    }
    if let Some(blast) = what.strip_prefix('x') {
        let (mass, standoff) = blast.split_once('@')?;
        return Some((
            law,
            Case::Blast {
                mass: mass.parse().ok()?,
                standoff: standoff.parse().ok()?,
            },
        ));
    }
    let energy = what.strip_prefix('p')?.to_ascii_lowercase();
    let (number, scale) = if let Some(n) = energy.strip_suffix('k') {
        (n, 1.0e3)
    } else if let Some(n) = energy.strip_suffix('m') {
        (n, 1.0e6)
    } else {
        (energy.as_str(), 1.0)
    };
    Some((law, Case::Point(number.parse::<f32>().ok()? * scale)))
}

/// The brick as the wall reads it: the registry's `Brick` factor turned back into the unit's
/// compressive strength (`factor / 1000 × the reference flow strength`), its crush energy the
/// static term of the walk's own force law at that factor (`factor / 1000 × a`, the reference's
/// cavity strength), and the handbook constants above.
fn brick() -> Masonry {
    let registry = SubstanceRegistry::shipped_static();
    let factor = registry
        .hard(SUBSTANCE)
        .expect("the shipped registry names Brick")
        .factor();
    let reference = registry.reference_facts();
    let share = f64::from(factor / REFERENCE_FACTOR);
    Masonry {
        density: DENSITY,
        compressive: (share * reference.flow_strength) as f32,
        crush: (share * reference.cavity_strength()) as f32,
        joint_tensile: JOINT_TENSILE,
        joint_toughness: JOINT_TOUGHNESS,
        modulus: MODULUS,
    }
}

/// The candidate spall's scale: the exited substance's strength relative to armour steel, its
/// factor over the reference's, capped at 1.
fn spall_scale() -> f32 {
    let factor = SubstanceRegistry::shipped_static()
        .hard(SUBSTANCE)
        .map_or(REFERENCE_FACTOR, |hard| hard.factor());
    (factor / REFERENCE_FACTOR).min(1.0)
}

fn new_cut(kind: CutKind) -> Cut {
    match kind {
        CutKind::RunningBond => model::running_bond(WALL_MIN, WALL_SIZE, CHUNK, brick()),
        CutKind::Jittered => model::jittered(WALL_MIN, WALL_SIZE, CHUNK, JITTER_SEED, brick()),
    }
}

/// The front face's z and the back face's, world metres.
fn faces() -> (f32, f32) {
    (WALL_MIN.z + WALL_SIZE.z, WALL_MIN.z)
}

/// Witness tile `index`'s box, row-major from the panel's low corner.
fn witness_tile(index: usize) -> model::Block {
    let (_, back) = faces();
    let low = Vec3::new(
        WALL_MIN.x + WALL_SIZE.x / 2.0 - TILES.0 as f32 * TILE / 2.0,
        0.1,
        back - WITNESS_GAP - 0.01,
    );
    let (column, row) = (index % TILES.0, index / TILES.0);
    let min = low + Vec3::new(column as f32 * TILE, row as f32 * TILE, 0.0);
    model::Block {
        min,
        max: min + Vec3::new(TILE, TILE, 0.01),
    }
}

/// The tile a point lies on, within a millimetre.
fn witness_at(point: Vec3) -> Option<usize> {
    (0..TILES.0 * TILES.1).find(|&index| {
        let tile = witness_tile(index);
        (point.cmpge(tile.min - 1e-3) & point.cmple(tile.max + 1e-3)).all()
    })
}

/// The aim point: the front-face centre of the chunk whose centroid is nearest the wall's centre.
fn aim_point(wall: &Wall) -> Vec3 {
    let centre = WALL_MIN + WALL_SIZE / 2.0;
    let block = wall
        .cut
        .blocks
        .iter()
        .min_by(|a, b| {
            a.center()
                .distance(centre)
                .total_cmp(&b.center().distance(centre))
        })
        .copied()
        .unwrap_or(model::Block {
            min: centre,
            max: centre,
        });
    Vec3::new(block.center().x, block.center().y, faces().0)
}

/// The camera's pose at `view`, framing the wall about `aim`.
fn view_pose(view: View, aim: Vec3) -> Transform {
    let (front, back) = faces();
    let (eye, look) = match view {
        View::Front => (Vec3::new(aim.x, aim.y, front + VIEW_DISTANCE), aim),
        View::Back => (
            Vec3::new(aim.x, aim.y, back - VIEW_DISTANCE),
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
    controls: Res<WallControls>,
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
        commands.spawn((
            Name::new("witness tile"),
            WitnessTile(index),
            Mesh3d(tile_mesh.clone()),
            MeshMaterial3d(witness[0].clone()),
            Transform::from_translation(tile.center()),
            RigidBody::Static,
            Collider::cuboid(TILE, TILE, 0.01),
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
            WALL_MIN + WALL_SIZE * Vec3::new(0.5, 0.7, 0.0) - Vec3::Z * 3.5,
        ),
    ));
    let wall = spawn_wall(&mut commands, &mut meshes, &assets, controls.cut);
    commands.insert_resource(wall);
    commands.insert_resource(assets);
}

/// Spawn a new wall of `kind`'s cut: one static chunk per block, full pools, nothing cracked.
fn spawn_wall(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    assets: &WallAssets,
    kind: CutKind,
) -> Wall {
    let cut = new_cut(kind);
    let brick_material =
        crate::ballistics::VolumeMaterial::of(SubstanceRegistry::shipped_static(), SUBSTANCE)
            .expect("the shipped registry names Brick");
    let mut entities = Vec::with_capacity(cut.blocks.len());
    let mut chunk_meshes = Vec::with_capacity(cut.blocks.len());
    for (index, block) in cut.blocks.iter().enumerate() {
        let size = block.size();
        let mesh = meshes.add(Cuboid::from_size(size - 2.0 * SEAM));
        chunk_meshes.push(mesh.clone());
        entities.push(Some(
            commands
                .spawn((
                    Name::new("wall chunk"),
                    WallChunk(index),
                    Mesh3d(mesh),
                    MeshMaterial3d(assets.heat[0].clone()),
                    Transform::from_translation(block.center()),
                    RigidBody::Static,
                    Collider::cuboid(size.x, size.y, size.z),
                    CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                    BallisticVolume {
                        material: brick_material,
                        substance: SUBSTANCE.to_string(),
                    },
                ))
                .id(),
        ));
    }
    let material = brick();
    let griffith = cut
        .structure
        .joints
        .iter()
        .map(|joint| material.griffith_strength(joint.area))
        .fold((f32::INFINITY, 0.0_f32), |(lo, hi), s| {
            (lo.min(s), hi.max(s))
        });
    Wall {
        state: WallState::new(&cut.structure),
        cut,
        entities,
        meshes: chunk_meshes,
        material,
        griffith,
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
    for (key, law) in [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
    ]
    .into_iter()
    .zip(CrackLaw::ALL)
    {
        if keys.just_pressed(key) {
            controls.law = law;
        }
    }
    if keys.just_pressed(KeyCode::KeyJ) {
        controls.spall = match controls.spall {
            Spall::Today => Spall::Candidate,
            Spall::Candidate => Spall::Today,
        };
    }
    if keys.just_pressed(KeyCode::KeyH) {
        controls.help = !controls.help;
    }
    if keys.just_pressed(KeyCode::KeyV) {
        controls.cased = !controls.cased;
    }
    if keys.just_pressed(KeyCode::KeyU) {
        controls.structural = !controls.structural;
    }
    if keys.just_pressed(KeyCode::KeyB) && !p.rounds.0.is_empty() {
        controls.round = (controls.round + 1) % p.rounds.0.len();
    }
    step(
        keys,
        KeyCode::BracketLeft,
        KeyCode::BracketRight,
        &mut controls.energy,
        ENERGIES.len(),
    );
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
    let new_cut = keys.just_pressed(KeyCode::KeyK);
    if new_cut {
        controls.cut = match controls.cut {
            CutKind::RunningBond => CutKind::Jittered,
            CutKind::Jittered => CutKind::RunningBond,
        };
    }
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
    let point = keys.just_pressed(KeyCode::KeyP);
    let blast = keys.just_pressed(KeyCode::KeyO);
    if (point || blast)
        && let Some(wall) = p.wall.as_deref()
    {
        let eye = p.camera.translation;
        let dir = p.camera.forward().as_vec3();
        let state = &wall.state;
        if let Some((_, t)) = model::first_hit(&wall.cut.blocks, |i| state.standing(i), eye, dir) {
            let at = eye + dir * t;
            p.queue.0.push(if point {
                Deposit::Point {
                    energy: ENERGIES[controls.energy],
                    at,
                    push: dir,
                }
            } else {
                Deposit::Blast {
                    mass: MASSES[controls.mass],
                    standoff: STANDOFFS[controls.standoff],
                    at,
                    into: dir,
                }
            });
        }
    }
    if new_cut || keys.just_pressed(KeyCode::KeyN) {
        p.rebuild.rebuild(controls.cut);
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
    fn rebuild(&mut self, kind: CutKind) {
        let Some(assets) = self.assets.as_deref() else {
            return;
        };
        for entity in &self.old {
            self.commands.entity(entity).despawn();
        }
        let wall = spawn_wall(&mut self.commands, &mut self.meshes, assets, kind);
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
    let scale = spall_scale();
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
            p.log.witness_today.resize(tiles, 0.0);
            p.log.witness_candidate.resize(tiles, 0.0);
            let mut strength = 0.0;
            for cone in thrown {
                let weight = 1.0 / cone.rays.len().max(1) as f32;
                for ray in &cone.rays {
                    if let Some(tile) = witness_at(ray.end) {
                        p.log.witness_today[tile] += ray.strength * weight;
                        p.log.witness_candidate[tile] += ray.strength * weight * scale;
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
    /// Each loose group, its centroid, linear and angular velocity, and whether a structural crack
    /// bounds it.
    groups: Vec<(Vec<usize>, Vec3, Vec3, Vec3, bool)>,
    /// A point deposit's crush ball: centre, radius, push.
    point: Option<(Vec3, f32, Vec3)>,
    /// A blast's charge, origin, coupled energy and momentum, and its drawn ray ends.
    blast: Option<(Blast, Vec3, f32, f32, Vec<Vec3>)>,
}

/// The sources, chunk energies and pushes of one deposit, read before anything is removed (each
/// source's back face among them).
struct Loading {
    sources: Vec<Source>,
    energies: Vec<(usize, f32)>,
    pushes: Vec<Push>,
}

/// Split each chunk's line into point sources [`LINE_STEP`] apart, and share the round's lost
/// momentum among the chunks by the energy each took, at its chord's midpoint.
fn line_loading(
    wall: &Wall,
    lines: &[(usize, Vec3, Vec3, f32)],
    along: Vec3,
    momentum: f32,
) -> Loading {
    let state = &wall.state;
    let back = |at: Vec3, push: Vec3| {
        model::back_distance(&wall.cut.blocks, |i| state.standing(i), at, push)
    };
    let total: f32 = lines.iter().map(|line| line.3).sum();
    let mut sources = Vec::new();
    let mut pushes = Vec::new();
    for &(chunk, entry, end, energy) in lines {
        let chord = end - entry;
        let length = chord.length();
        let push = if length > 1e-6 { chord / length } else { along };
        let n = (length / LINE_STEP).ceil().max(1.0) as usize;
        for k in 0..n {
            let at = entry + chord * ((k as f32 + 0.5) / n as f32);
            sources.push(Source {
                energy: energy / n as f32,
                at,
                push,
                back: back(at, push),
            });
        }
        if total > 0.0 {
            pushes.push(Push {
                chunk,
                at: (entry + end) / 2.0,
                impulse: along * (momentum * energy / total),
            });
        }
    }
    Loading {
        sources,
        energies: lines.iter().map(|&(i, _, _, e)| (i, e)).collect(),
        pushes,
    }
}

/// A blast's rays summed per chunk: each chunk a source at the energy-weighted mean of its hits,
/// pushing along their mean direction, and a push of their summed impulse there.
fn blast_loading(wall: &Wall, hits: &[model::RayHit]) -> Loading {
    let state = &wall.state;
    let mut per: std::collections::BTreeMap<usize, (f32, Vec3, Vec3)> = Default::default();
    for hit in hits {
        let entry = per.entry(hit.chunk).or_default();
        entry.0 += hit.energy;
        entry.1 += hit.at * hit.energy;
        entry.2 += hit.impulse;
    }
    let mut loading = Loading {
        sources: Vec::new(),
        energies: Vec::new(),
        pushes: Vec::new(),
    };
    for (chunk, (energy, weighted, impulse)) in per {
        if energy <= 0.0 {
            continue;
        }
        let at = weighted / energy;
        let push = impulse.normalize_or(Vec3::NEG_Z);
        loading.sources.push(Source {
            energy,
            at,
            push,
            back: model::back_distance(&wall.cut.blocks, |i| state.standing(i), at, push),
        });
        loading.energies.push((chunk, energy));
        loading.pushes.push(Push { chunk, at, impulse });
    }
    loading
}

/// Run one deposit through the model under `controls`' law: the wear and dust, the local cracks,
/// the structural check when on, and the loose groups with the velocities the pushes on them give.
fn apply_deposit(
    wall: &mut Wall,
    controls: &WallControls,
    charge: &Charge,
    field: &[(Vec3, f32)],
    deposit: &Deposit,
) -> Applied {
    let mut applied = Applied::default();
    let law = controls.law;
    let loading = match deposit {
        Deposit::Line {
            lines,
            along,
            momentum,
            ..
        } => line_loading(wall, lines, *along, *momentum),
        &Deposit::Point { energy, at, push } => {
            let state = &wall.state;
            let crush = model::crush_point(&wall.cut.structure, state, at, energy);
            applied.point = Some((at, crush.radius, push));
            Loading {
                sources: vec![Source {
                    energy,
                    at,
                    push,
                    back: model::back_distance(&wall.cut.blocks, |i| state.standing(i), at, push),
                }],
                energies: crush.doses,
                pushes: Vec::new(),
            }
        }
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
                blast.energy,
                blast.momentum,
                field,
            );
            let coupled: f32 = hits.iter().map(|hit| hit.energy).sum();
            let momentum: f32 = hits.iter().map(|hit| hit.impulse.length()).sum();
            let drawn = hits
                .iter()
                .step_by(RAY_DRAW_STRIDE)
                .map(|hit| hit.at)
                .collect();
            applied.blast = Some((blast, origin, coupled, momentum, drawn));
            blast_loading(wall, &hits)
        }
    };
    for &(chunk, energy) in &loading.energies {
        applied.deposited += energy;
        if model::wear(&wall.cut.structure, &mut wall.state, chunk, energy) {
            applied.dusted.push(chunk);
        } else {
            applied.worn.push(chunk);
        }
    }
    applied.chunks = loading.energies.len();
    applied.local = model::crack(law, &wall.cut.structure, &mut wall.state, &loading.sources).len();
    // A push on a chunk turned to dust went into crushing it, and moves nothing.
    let pushes: Vec<Push> = loading
        .pushes
        .iter()
        .copied()
        .filter(|push| wall.state.standing(push.chunk))
        .collect();
    if controls.structural {
        applied.failures = model::structural(&wall.cut.structure, &mut wall.state, &pushes);
    }
    for group in model::loose_groups(&wall.cut.structure, &wall.state) {
        let chunks = &wall.cut.structure.chunks;
        let mass: f32 = group.iter().map(|&i| chunks[i].mass).sum();
        let centroid = group
            .iter()
            .map(|&i| chunks[i].centroid * chunks[i].mass)
            .sum::<Vec3>()
            / mass.max(f32::EPSILON);
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
            .max(mass * CHUNK * CHUNK / 6.0);
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
                wall.state.structural[j]
                    && (group.binary_search(&joint.a).is_ok()
                        || matches!(joint.b, model::Side::Chunk(b) if group.binary_search(&b).is_ok()))
            });
        for &i in &group {
            wall.state.fate[i] = model::Fate::Fallen;
        }
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
                        let block = wall.cut.blocks[chunk];
                        let size = (block.size() - 2.0 * LOOSE_GAP).max(Vec3::splat(0.01));
                        parent.spawn((
                            Mesh3d(wall.meshes[chunk].clone()),
                            MeshMaterial3d(tint.clone()),
                            Transform::from_translation(block.center() - *centroid),
                            Collider::cuboid(size.x, size.y, size.z),
                            ColliderDensity(wall.material.density),
                            CollisionLayers::new(
                                [Layer::Default],
                                [Layer::Default, Layer::Terrain, Layer::Armor],
                            ),
                        ));
                    }
                });
        }
        if applied.point.is_some() {
            p.log.point = applied.point;
        }
        let what = match &deposit {
            Deposit::Line { what, .. } => what.clone(),
            Deposit::Point { energy, .. } => format!("point deposit {}", joules(*energy)),
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
        p.log.last = Some(Outcome {
            what,
            deposited: applied.deposited,
            chunks: applied.chunks,
            dust: applied.dusted.len(),
            local: applied.local,
            structural: applied.failures.iter().map(|f| f.joints.len()).sum(),
            sections: applied.failures.len(),
            worst: applied
                .failures
                .first()
                .map(|failure| (failure.demand, failure.capacity)),
            groups: applied.groups.len(),
            fallen: applied.groups.iter().map(|(group, ..)| group.len()).sum(),
            spall_today: match &deposit {
                Deposit::Line { spall, .. } => *spall,
                _ => 0.0,
            },
            blast: applied
                .blast
                .as_ref()
                .map(|(blast, _, coupled, momentum, _)| (*blast, *coupled, *momentum)),
        });
        if let Some(last) = &p.log.last {
            info!(
                "wall prototype: law {}: {}: {:.0} J into {} chunks, {} dust, {} local cracks, \
                 {} structural cracks in {} sections (first {:?} N s m demand, capacity), {} groups \
                 fell ({} chunks), exit cones {:.0} J, blast {:?} (coupled J, N s)",
                p.controls.law.letter(),
                last.what,
                last.deposited,
                last.chunks,
                last.dust,
                last.local,
                last.structural,
                last.sections,
                last.worst,
                last.groups,
                last.fallen,
                last.spall_today,
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
/// cones (today's in orange, the candidate's as faint as its share), the last point deposit's
/// crush ball, and the last blast's charge and a sample of its rays.
fn draw_wall(
    wall: Option<Res<Wall>>,
    log: Res<WallLog>,
    controls: Res<WallControls>,
    mut gizmos: Gizmos<WallGizmos>,
) {
    let Some(wall) = wall else {
        return;
    };
    let mid = WALL_MIN.z + WALL_SIZE.z / 2.0;
    let out = |p: Vec3| p + Vec3::Z * (0.003 * (p.z - mid).signum());
    for (index, joint) in wall.cut.structure.joints.iter().enumerate() {
        if !wall.state.cracked[index] || !wall.state.live(joint) {
            continue;
        }
        let colour = if wall.state.structural[index] {
            Color::srgb(1.0, 0.2, 1.0)
        } else {
            Color::srgb(0.2, 1.0, 1.0)
        };
        let face = wall.cut.faces[index];
        for k in 0..4 {
            gizmos.line(out(face[k]), out(face[(k + 1) % 4]), colour);
        }
    }
    let scale = match controls.spall {
        Spall::Today => 1.0,
        Spall::Candidate => spall_scale(),
    };
    for cone in &log.cones {
        let strongest = cone.rays.iter().map(|(_, s)| *s).fold(0.0_f32, f32::max);
        for &(end, strength) in &cone.rays {
            let alpha = (strength * scale / strongest.max(f32::EPSILON)).clamp(0.02, 1.0);
            let colour = match controls.spall {
                Spall::Today => Color::srgba(1.0, 0.55, 0.1, alpha),
                Spall::Candidate => Color::srgba(0.4, 0.8, 1.0, alpha),
            };
            gizmos.line(cone.origin, end, colour);
        }
    }
    let marker = Color::srgb(1.0, 0.95, 0.5);
    if let Some((at, radius, push)) = log.point {
        gizmos.sphere(Isometry3d::from_translation(at), radius, marker);
        gizmos.arrow(at - push * 0.6, at, marker);
    }
    if let Some((origin, radius, rays)) = &log.blast {
        gizmos.sphere(Isometry3d::from_translation(*origin), *radius, marker);
        for &end in rays {
            gizmos.line(*origin, end, Color::srgba(1.0, 0.95, 0.5, 0.15));
        }
    }
}

/// Each witness tile's material from its dose under the spall drawn: none, or one of eight steps
/// of a decade each from 1 J.
fn paint_witness(
    log: Res<WallLog>,
    controls: Res<WallControls>,
    assets: Option<Res<WallAssets>>,
    mut tiles: Query<(&WitnessTile, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let Some(assets) = assets else {
        return;
    };
    let doses = match controls.spall {
        Spall::Today => &log.witness_today,
        Spall::Candidate => &log.witness_candidate,
    };
    for (tile, mut material) in &mut tiles {
        let dose = doses.get(tile.0).copied().unwrap_or(0.0);
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

/// `m` metres in m or km.
fn metres(m: f32) -> String {
    if m >= 1000.0 {
        format!("{:.1} km", m / 1000.0)
    } else if m >= 10.0 {
        format!("{m:.0} m")
    } else {
        format!("{m:.2} m")
    }
}

/// The values behind the readout, for the log: the active rule's formula, the material, the
/// controls, the last deposit's outcome and the lone-point reach each closed-form law gives its
/// energy, the blast's energy and coupling, the structural check, and the spall today against the
/// candidate. ASCII.
fn detail(
    wall: &Wall,
    c: &WallControls,
    charge_src: &Charge,
    log: &WallLog,
    last: &Outcome,
) -> Vec<String> {
    let m = &wall.material;
    let joints = &wall.cut.structure.joints;
    let charge = charge_src.blast(MASSES[c.mass], c.cased);
    let mut lines = vec![
        format!(
            "WALL PROTOTYPE  law [1-5] {} {}:  {}",
            c.law.letter(),
            c.law.name(),
            c.law.formula()
        ),
        format!(
            "brick  rho {:.0} kg/m3   fc {:.1} MPa   factor {:.1} ref-mm/m   crush w = {:.2} x fc = {:.1} MJ/m3",
            m.density,
            m.compressive / 1.0e6,
            m.compressive / 1.0e6,
            m.crush / m.compressive,
            m.crush / 1.0e6
        ),
        format!(
            "joint  ft {:.2} MPa   Gf {:.0} J/m2   Em {:.1} GPa   Griffith {:.2}-{:.2} MPa   eta {} (ASSUMED)   c {:.0} m/s",
            m.joint_tensile / 1.0e6,
            m.joint_toughness,
            m.modulus / 1.0e9,
            wall.griffith.0 / 1.0e6,
            wall.griffith.1 / 1.0e6,
            model::CRACK_SHARE,
            m.wave_speed()
        ),
        format!(
            "cut [K] {} {} chunks {} joints   point [P, [ ]] {}",
            c.cut.label(),
            wall.cut.structure.chunks.len(),
            joints.len(),
            joules(ENERGIES[c.energy])
        ),
        format!(
            "blast [O] {:.1} kg [, .] at {:.1} m [; '] {} [V]: {} chemical - {} casing (M/C {:.1}) = {}, p = sqrt(2 C E) = {:.0} N s   structural [U] {}",
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
            charge.momentum,
            if c.structural { "on" } else { "off" }
        ),
    ];
    {
        lines.push(format!(
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
        ));
        if let Some((blast, coupled, momentum)) = last.blast {
            lines.push(format!(
                "      blast coupled {} of {} ({:.0} %), {:.0} of {:.0} N s",
                joules(coupled),
                joules(blast.energy),
                100.0 * coupled / blast.energy.max(f32::EPSILON),
                momentum,
                blast.momentum
            ));
        }
        if let Some((demand, capacity)) = last.worst {
            lines.push(format!(
                "      first structural failure: angular impulse {demand:.0} N s m against sqrt(2 I Gf A) = {capacity:.0} N s m"
            ));
        }
        let area = joints
            .iter()
            .map(|joint| joint.area)
            .fold(0.0_f32, f32::max);
        let reach: Vec<String> = CrackLaw::ALL
            .iter()
            .filter_map(|law| {
                law.point_reach(last.deposited, m, area)
                    .map(|r| format!("{} {}", law.letter(), metres(r)))
            })
            .collect();
        lines.push(format!(
            "      lone-point crack reach of {}: {}",
            joules(last.deposited),
            reach.join("   ")
        ));
        let scale = spall_scale();
        // `+ 0.0`: an empty sum is −0.0, which would print a sign.
        let today = log.witness_today.iter().sum::<f32>() + 0.0;
        let candidate = log.witness_candidate.iter().sum::<f32>() + 0.0;
        lines.push(format!(
            "spall [J] showing {}: exit cone S today {} | candidate x{:.4} {}   witness panel total today {} | candidate {}",
            match c.spall {
                Spall::Today => "today",
                Spall::Candidate => "candidate",
            },
            joules(last.spall_today),
            scale,
            joules(last.spall_today * scale),
            joules(today),
            joules(candidate)
        ));
    }
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

/// The readout: one line (the crack rule, the structural check, the last shot), and the help panel
/// while H has it open. Both show only near the wall, where the range's own H legend stays shut.
fn update_readout(p: ReadoutParam) {
    if p.wall.is_none() {
        return;
    }
    let c = *p.controls;
    let line = format!(
        "{} rule ({})  |  structural check {}  |  last: {}  |  H help",
        {
            let name = c.law.name();
            format!("{}{}", name[..1].to_uppercase(), &name[1..])
        },
        c.law.letter(),
        if c.structural { "on" } else { "off" },
        p.log
            .last
            .as_ref()
            .map_or_else(|| "none".to_string(), last_shot),
    );
    let near = p.camera.translation.distance(WALL_MIN + WALL_SIZE / 2.0) < READOUT_RANGE;
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
FIRE     F fire the round   B next round   P point deposit   [ ] its energy
BLAST    O blast   , . charge mass   ; ' standoff   V cased / bare
RULES    1-5 crack rule A-E   U structural check   K cut   N new wall
VIEW     G camera: front, back, witness   J spall: today / candidate
COLOURS  brick red to glowing yellow chunk = crush capacity spent
         cyan seam outline = cracked by the local push
         magenta seam outline = cracked by the structural check
         grey-blue piece = loose   purple piece = toppled section
         orange / blue rays = spall cone (today's / candidate's)
         yellow sphere and arrow = point deposit or blast";

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
        Case::Point(energy) => p.queue.0.push(Deposit::Point {
            energy,
            at: aim,
            push: Vec3::NEG_Z,
        }),
        Case::Blast { mass, standoff } => p.queue.0.push(Deposit::Blast {
            mass,
            standoff,
            at: aim,
            into: Vec3::NEG_Z,
        }),
    }
}
