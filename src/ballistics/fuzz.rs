//! The §13.6 ray fuzzer — the union-field contract enforced mechanically, not by eyeball.
//!
//! §13.6 lists six invariants and then names exactly one instrument for them: "10⁵–10⁶ random rays
//! at the bound tank, asserting the invariants, and reporting every corridor/opening that reaches
//! crew/ammo with its admitting caliber and per-caliber η". This module is that instrument, at both
//! scales — the same code runs 10⁴ rays inside `cargo test` and 10⁶ from
//! `cargo run --bin ballistic_fuzzer`.
//!
//! # It fires at the REAL tank
//!
//! The target is the bound Tiger: `bake`'s glb extraction, the material-keyed volumes, the
//! per-primitive trimesh colliders, at spawn pose. Not synthetic plates. A gate built on slabs
//! proves things about slabs, and every defect §13.1 catalogues lives at a JOINT of authored
//! geometry.
//!
//! # It does not own any law
//!
//! Every number the fuzzer reports comes back out of the shipped resolution path: the corridor is
//! built by `resolve::build_corridor` (the march's own kernel), the field integral and the events by
//! [`walk::walk_ray`]/[`walk::walk_disc`], and ADMISSION by [`drive`] — the runtime's own crossing
//! state machine ([`resolve::decide`] and [`resolve::Flight::follow`]: entrance rules, bend,
//! transit, stop, bounce, continuation), observed with no consequence applied. A gate that
//! re-derives the law it is gating cannot fail. The one survey number computed here is the
//! straight-axis prefix integral ([`Report::min_reach`]), which is reported and never admits.
//!
//! # What a FINDING is
//!
//! Not "a ray reached the crew" — a shot that pays for 100 mm of front plate and kills the driver is
//! the game working. A finding is a ray along which the FLOOR PROBE — the probe round of least
//! capability, at the speed where its capability is the gate floor — puts a positive chord into a
//! crew or ammunition volume before it stops: an effectively unarmoured route. Each one is measured
//! against the whole gun list, each round driven through the same mechanism (§13.5's caliber gate: a
//! big disc engages the rim of a small opening and pays for it, a small one flies through), reported
//! with its admitting caliber, and then it FAILS THE GATE BY NAME. There is no exemption channel: a
//! corridor is a defect in the model, and the fix is in the model. A walk error met on a driven
//! flight is a gate failure too, never a non-admission.
//!
//! # Determinism
//!
//! No wall clock, no unseeded RNG. Ray `i` is generated from `splitmix64(seed, i)` alone, so a
//! reported `(seed, ray)` pair replays that exact ray with no state in between — which is what makes
//! a 10⁶-ray bake finding reproducible in a one-ray debug run.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use avian3d::prelude::{Collider, ColliderAabb, Position, RigidBody, Rotation, SpatialQueryFilter};
use bevy::prelude::*;

use super::resolve::{self, Decision, Flight, ResolveContext};
use super::walk::{self, DiscFrame, DiscWalk, FaceHit, RayCorridor, Span, VolumeTable, WalkError};
use super::{
    Ahead, MARCH_EPS, ProjectileMarchWorld, RoundBody, capability, next_surface, speed_for,
};
use crate::Layer;
use crate::damage::{Ammo, CrewStation, hit_ancestor};
use crate::tank::{TankSimSource, spawn_headless_tank};

// ---------------------------------------------------------------------------------------------
// The gun list
// ---------------------------------------------------------------------------------------------

/// One probe round: a gun the fuzzer measures every opening against.
///
/// The point of a LIST rather than a single reference round is §13.5's caliber gate — "an 88 cannot
/// thread the turret-ring slit while an MG round centred in it flies free". An opening's identity is
/// the caliber at which it closes, so the report has to sweep the guns the game can meet.
#[derive(Clone, Copy, Debug)]
pub struct ProbeRound {
    pub name: &'static str,
    /// Metres.
    pub caliber: f32,
    /// Kilograms.
    pub mass: f32,
    /// Nose factor (dimensionless, `0 < N ≤ 1`).
    pub nose_factor: f32,
    /// Reference striking speed (m/s). Muzzle velocity, so every number in the report reads "at
    /// point blank" — the most permissive case, which is the right bias for a hole hunt.
    pub speed: f32,
}

impl ProbeRound {
    /// What the capability law and the crossing mechanism read off this round.
    pub(super) fn body(&self) -> RoundBody {
        RoundBody {
            mass: self.mass,
            caliber: self.caliber,
            nose_factor: self.nose_factor,
        }
    }

    /// Reference-mm this round can defeat at its reference speed — the shipped [`super::capability`]
    /// law, never a table.
    pub fn capability(&self) -> f32 {
        capability(self.body(), self.speed)
    }
}

/// The guns every opening is measured against, ASCENDING by caliber (the sweep takes the first that
/// admits, so order is load-bearing).
///
/// Masses and speeds are period AP loadings for each bore; the 7.92 and the 88 are the Tiger's own
/// spec values (`assets/tiger_1/tiger_1.tank.ron`), so the two rounds the game actually fires are
/// measured as they are actually fired, nose factors included (the spec's nose geometry, derived).
/// The other rounds' nose factor 0.3 is a labelled fixture value.
pub const PROBE_ROUNDS: &[ProbeRound] = &[
    ProbeRound {
        name: "7.92mm",
        caliber: 0.0079,
        mass: 0.0118,
        nose_factor: 0.5,
        speed: 755.0,
    },
    ProbeRound {
        name: "20mm",
        caliber: 0.020,
        mass: 0.148,
        nose_factor: 0.3,
        speed: 830.0,
    },
    ProbeRound {
        name: "37mm",
        caliber: 0.037,
        mass: 0.685,
        nose_factor: 0.3,
        speed: 762.0,
    },
    ProbeRound {
        name: "57mm",
        caliber: 0.057,
        mass: 2.87,
        nose_factor: 0.3,
        speed: 815.0,
    },
    ProbeRound {
        name: "75mm",
        caliber: 0.075,
        mass: 6.8,
        nose_factor: 0.3,
        speed: 790.0,
    },
    ProbeRound {
        name: "88mm",
        caliber: 0.088,
        mass: 10.2,
        nose_factor: 7.0 / 24.0,
        speed: 773.0,
    },
    ProbeRound {
        name: "122mm",
        caliber: 0.122,
        mass: 25.0,
        nose_factor: 0.3,
        speed: 795.0,
    },
];

// ---------------------------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------------------------

/// Tank-local metres per clustering cell. Findings landing in one cell are ONE opening in the
/// report, which is what makes a 10⁶-ray run readable.
///
/// 0.15 m: coarse enough that a vision slit is one region rather than forty, fine enough that the
/// hull MG port and the driver's visor — 0.5 m apart on the real tank — never merge.
const REGION_CELL: f32 = 0.15;

/// Every knob of one fuzzer run.
#[derive(Clone, Debug)]
pub struct FuzzConfig {
    pub seed: u64,
    pub rays: u64,
    /// Share of rays deliberately aimed WIDE of the bounding sphere. Their job is the
    /// no-fabricated-events half of the contract: a ray that meets nothing must report nothing, and
    /// a fuzzer that only fires hits can never observe that.
    pub miss_fraction: f32,
    /// Every n-th material-crossing ray also runs the duplication gate (§13.6 idempotence +
    /// monotonicity). Not every ray, because it costs two extra walks.
    pub duplication_stride: u64,
    /// Ceiling on distinct regions that get the full per-caliber disc sweep. A runaway (a genuinely
    /// open tank) must produce a truncated report, not a run that never ends.
    pub max_region_sweeps: usize,
    /// The capability (reference-mm) of the floor probe — the probe round of least capability,
    /// driven at the speed where its capability is this ([`floor_probe`]). `None` is the gate's
    /// definition: that round at its own reference speed, i.e. "a route even a machine-gun bullet
    /// takes". Raise it to survey the graded weakspots above that line — the number that reads as a
    /// hole rather than as armour is the thinnest authored plate (25 mm on this tank), and
    /// everything between the two is a real, deliberate, thin spot rather than a defect.
    pub finding_floor: Option<f32>,
}

impl Default for FuzzConfig {
    fn default() -> Self {
        Self {
            seed: 0x0713_2026_0806,
            rays: 10_000,
            miss_fraction: 0.15,
            duplication_stride: 97,
            max_region_sweeps: 256,
            finding_floor: None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------------------------

/// What a corridor reached — the two things §13.6 says to report on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetKind {
    Crew,
    Ammo,
}

impl TargetKind {
    pub fn label(self) -> &'static str {
        match self {
            TargetKind::Crew => "crew",
            TargetKind::Ammo => "ammo",
        }
    }
}

/// One probe round's measurement of one opening.
#[derive(Clone, Copy, Debug)]
pub struct RoundMeasurement {
    /// η — the ENTRANCE COVERAGE of the first armour the driven round met (§13.5's engagement
    /// fraction). 1.0 is a fully engaged face; 0.0 is a round that met no armour at all.
    pub eta: f32,
    /// The driven round put a positive chord into the target before it stopped.
    pub reached: bool,
    /// What this round can spend, from [`ProbeRound::capability`].
    pub capability: f32,
}

impl RoundMeasurement {
    /// This round gets through the opening to the target.
    pub fn admits(&self) -> bool {
        self.reached
    }
}

/// One clustered opening reaching crew or ammunition.
#[derive(Clone, Debug)]
pub struct Region {
    /// Tank-local clustering cell (`entry_local / REGION_CELL`, floored).
    pub cell: [i32; 3],
    /// Where the witness corridor pierced the tank's local bounding box — the "where on the tank
    /// does this come in" descriptor the finding is reported with.
    pub entry_local: Vec3,
    /// Tank-local travel direction of the witness ray.
    pub axis_local: Vec3,
    pub targets: BTreeSet<String>,
    pub kind: TargetKind,
    /// How many fuzz rays landed in this cell.
    pub rays: u64,
    /// The cheapest straight-axis cost seen to a target here (reference-mm) — a survey number;
    /// infinite when the floor probe reached the target off the straight axis (after a bounce).
    pub min_axis_cost: f32,
    /// One entry per [`PROBE_ROUNDS`] element, same order.
    pub measurements: Vec<RoundMeasurement>,
    /// Index into [`PROBE_ROUNDS`] of the smallest round that gets through. `None` means the sweep
    /// found the opening closed to every gun in the game (the axis ray got through, the disc did
    /// not — a sub-caliber crack that self-heals, §13.5).
    pub admitting: Option<usize>,
    /// The ray index that produced the measurements — replay with `(seed, witness_ray)`.
    pub witness_ray: u64,
}

impl Region {
    /// The caliber (metres) of the smallest round that gets through, if any.
    pub fn admitting_caliber(&self) -> Option<f32> {
        self.admitting.map(|index| PROBE_ROUNDS[index].caliber)
    }
}

/// A machine-checked invariant that did not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViolationKind {
    /// Material was crossed and the walk reported no boundary event — §13.6's free-penetration
    /// class, the defect the whole union model exists to kill.
    SilentPenetration,
    /// No material anywhere on the ray and the walk reported an armour event anyway (§13.6 "no
    /// fabricated events" — what makes a gap DETECTABLE rather than event noise).
    FabricatedEvent,
    /// Duplicating a volume changed the cost. `max` is idempotent, so it must not (§13.6).
    DuplicationChangedCost,
    /// Duplicating a volume at a HIGHER factor LOWERED the cost (§13.6 monotonicity).
    DuplicationLoweredCost,
}

#[derive(Clone, Debug)]
pub struct Violation {
    pub kind: ViolationKind,
    pub ray: u64,
    pub detail: String,
}

/// A [`WalkError`] the fuzzer met. Empty on a gated asset — the per-primitive manifold gate
/// (`bake`) is what makes that claim, and this is what tests it against real shot lines.
#[derive(Clone, Debug)]
pub struct WalkFailure {
    pub ray: u64,
    pub origin: Vec3,
    pub direction: Vec3,
    /// The volume the error named, by its authored node name. A rate is not actionable; a name is.
    pub volume: String,
    pub error: String,
    /// Every crossing of the BLAMED primitive along this ray, as `(t, axis · n)`. Sign says entry
    /// (negative) or exit (positive); spacing says which defect it is. Two entries a few centimetres
    /// apart are two closed shells of one primitive that overlap in space (legal authoring by §13.7,
    /// unrepresentable by the walk's per-primitive parity pairing); two entries a micron apart are a
    /// tolerance problem instead. The distinction is the whole diagnosis, so the fuzzer collects it
    /// rather than leaving it to a debugger.
    pub crossings: Vec<(f64, f32)>,
}

/// The volume a [`WalkError`] blames, when it blames one.
fn blamed_volume(error: &WalkError) -> Option<Entity> {
    match error {
        WalkError::BadFactor { volume, .. }
        | WalkError::UnknownVolume { volume }
        | WalkError::CollectorFailed { volume, .. }
        | WalkError::CorridorOverflow { volume, .. } => Some(*volume),
        WalkError::UnexpectedExit { key, .. }
        | WalkError::UnexpectedEntry { key, .. }
        | WalkError::UnrepresentableChord { key, .. } => Some(key.volume),
        WalkError::IncompleteCorridor { open, .. } => open.first().map(|key| key.volume),
        WalkError::BadCorridor { .. }
        | WalkError::CorridorMismatch { .. }
        | WalkError::UnattributedRun { .. }
        | WalkError::DegenerateEntryNormal { .. } => None,
    }
}

/// The SHELL a [`WalkError`] blames, when the error is about one.
fn blamed_shell(error: &WalkError) -> Option<walk::ShellKey> {
    match error {
        WalkError::UnexpectedExit { key, .. }
        | WalkError::UnexpectedEntry { key, .. }
        | WalkError::UnrepresentableChord { key, .. } => Some(*key),
        WalkError::IncompleteCorridor { open, .. } => open.first().copied(),
        _ => None,
    }
}

/// Everything one run learned.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub seed: u64,
    pub rays: u64,
    /// Rays that crossed material at all.
    pub rays_crossing: u64,
    pub duplication_checks: u64,
    pub violations: Vec<Violation>,
    pub walk_errors: Vec<WalkFailure>,
    /// Openings, sorted cheapest-first.
    pub regions: Vec<Region>,
    /// The CHEAPEST axis-ray cost (reference-mm) seen reaching each crew/ammo volume, whether or not
    /// it was cheap enough to be a finding.
    ///
    /// The survey half of the report, and the thing that makes a clean run readable: "no findings"
    /// is only reassuring next to "and the thinnest route to the driver was 96 reference-mm". A
    /// target missing from this map was never reached at all.
    pub min_reach: BTreeMap<String, f32>,
    /// How many walk errors each volume was blamed for.
    pub walk_error_volumes: BTreeMap<String, u64>,
    /// How many rays crossed each volume. The anti-vacuity evidence: a gate that never reached the
    /// ammunition racks has not cleared them, it has ignored them.
    pub presence_census: BTreeMap<String, u64>,
    /// [`FuzzConfig::max_region_sweeps`] was reached — the region list is not exhaustive.
    pub sweeps_truncated: bool,
    pub elapsed: Duration,
    /// Tank bounding sphere the rays were generated against (world).
    pub center: Vec3,
    pub radius: f32,
}

impl Report {
    /// Nothing the gate refuses: no violated invariant, no walk error, and no corridor to crew or
    /// ammunition.
    ///
    /// HARD ZERO ON ALL THREE, and there is no exemption channel. A `WalkError` is a round that
    /// stops dead in mid-armour, so one of them is a gameplay defect and a hundred are the same
    /// defect spreading — there is no rate at which the answer is "expected". A corridor is an
    /// effectively unarmoured route to a crew member or a magazine, which is the defect §13.6
    /// exists to find; it is named, and it is fixed in the model. A run that meets any of the three
    /// names the ray, and `replay_ray` re-fires it.
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty() && self.walk_errors.is_empty() && self.regions.is_empty()
    }
}

// ---------------------------------------------------------------------------------------------
// Determinism: the ray generator
// ---------------------------------------------------------------------------------------------

/// splitmix64, seeded per RAY rather than streamed.
///
/// Ray `i` depends on `(seed, i)` and on nothing else, so a report that names a ray index is a
/// complete reproduction recipe — no "run the first 843 991 rays to get back here".
#[derive(Clone, Copy)]
struct Rng(u64);

impl Rng {
    const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

    fn for_ray(seed: u64, ray: u64) -> Self {
        Self(seed ^ ray.wrapping_add(1).wrapping_mul(Self::GAMMA))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(Self::GAMMA);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)` from the top 24 bits (every one of which is a full-quality mix bit).
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / 16_777_216.0
    }

    /// Uniform on the sphere (Archimedes: `z` uniform, azimuth uniform).
    fn direction(&mut self) -> Vec3 {
        let z = 2.0 * self.unit() - 1.0;
        let azimuth = std::f32::consts::TAU * self.unit();
        let planar = (1.0 - z * z).max(0.0).sqrt();
        // Renormalized: the walk refuses a corridor whose axis is not unit, and the sampled `z`
        // makes the analytic identity only true to f32 rounding.
        Vec3::new(planar * azimuth.cos(), planar * azimuth.sin(), z).normalize_or(Vec3::Z)
    }
}

/// One generated shot line.
#[derive(Clone, Copy, Debug)]
struct FuzzRay {
    origin: Vec3,
    direction: Vec3,
    length: f32,
}

/// Aim ray `index` at the tank's bounding sphere.
///
/// The offset is sampled on the disc perpendicular to the travel direction, `sqrt`-warped so it is
/// uniform by AREA rather than by radius — an untransformed radius crowds every ray at the centre of
/// the tank and never probes a skirt or a turret ring. `miss_fraction` of rays get a disc 1.6× the
/// sphere so a real share of the run meets nothing at all.
fn ray_for(index: u64, config: &FuzzConfig, center: Vec3, radius: f32) -> FuzzRay {
    let mut rng = Rng::for_ray(config.seed, index);
    let direction = rng.direction();
    // An arbitrary but deterministic basis perpendicular to the travel direction. `any_orthonormal_pair`
    // is a pure function of the direction, so the same ray index always lays out the same disc.
    let (u, v) = direction.any_orthonormal_pair();
    let miss = rng.unit() < config.miss_fraction;
    let span = if miss { radius * 1.6 } else { radius };
    let offset_radius = span * rng.unit().sqrt();
    let angle = std::f32::consts::TAU * rng.unit();
    let offset = (u * angle.cos() + v * angle.sin()) * offset_radius;
    FuzzRay {
        // 1.5 radii back from the centre, so the corridor opens in clear air whatever the sphere
        // encloses, and closes 1.5 radii past it.
        origin: center + offset - direction * (radius * 1.5),
        direction,
        length: radius * 3.0,
    }
}

// ---------------------------------------------------------------------------------------------
// The headless target
// ---------------------------------------------------------------------------------------------

/// Where the probe tank stands. Off the world origin on purpose — a target at `(0,0,0)` cannot catch
/// a frame-mixup, since world and tank-local coincide there.
pub(super) const PROBE_TANK_AT: Vec3 = Vec3::new(12.0, 3.0, -7.0);

/// The bound Tiger, once spawned.
#[derive(Resource, Clone, Copy)]
pub(super) struct ProbeTank(pub Entity);

/// Two bare entities the duplication gate uses as the identity of the DUPLICATED volume. Real
/// entities rather than synthesized ids: `PrimitiveKey` is entity-keyed, and a fabricated id is
/// exactly the sort of thing that would make the gate pass for the wrong reason.
#[derive(Resource, Clone, Copy)]
struct Spares {
    volume: Entity,
    primitive: Entity,
}

fn spawn_probe_tank(
    mut commands: Commands,
    source: TankSimSource,
    existing: Option<Res<ProbeTank>>,
) {
    if existing.is_some() {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    let tank = spawn_headless_tank(
        &mut commands,
        content,
        (
            Transform::from_translation(PROBE_TANK_AT),
            Name::new("ballistic fuzzer Tiger I"),
            RigidBody::Static,
        ),
    );
    let volume = commands.spawn(Name::new("fuzz duplicate volume")).id();
    let primitive = commands.spawn(Name::new("fuzz duplicate primitive")).id();
    commands.insert_resource(ProbeTank(tank));
    commands.insert_resource(Spares { volume, primitive });
}

/// An Avian world holding nothing but the bound Tiger — the target both the fuzzer and the golden
/// shots fire at.
///
/// Deliberately NOT `SimPlugin`: the fuzzer wants the tank's GEOMETRY at spawn pose, and every
/// system that could move it (tracks, servos, gravity) is a source of nondeterminism the gate has no
/// use for. `RigidBody::Static` pins the pose; `bake::plugin` supplies the same blueprint the game
/// binds from.
pub(super) fn probe_world() -> Result<App, String> {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        // Avian's collider hierarchy reads `GlobalTransform` to place a child collider against its
        // body, so propagation must run — without it every trimesh sits at the world origin.
        bevy::transform::TransformPlugin,
        // Avian's collider cache reads `AssetEvent<Mesh>`; the asset system must exist even though
        // nothing here carries a mesh handle.
        AssetPlugin::default(),
        avian3d::prelude::PhysicsPlugins::default(),
    ))
    .init_asset::<Mesh>()
    .add_plugins(crate::bake::plugin)
    .add_systems(Update, spawn_probe_tank);

    // Avian registers diagnostics resources in `Plugin::finish`, and its spatial-query systems
    // require them — a bare `update()` loop skips both hooks.
    let deadline = Instant::now() + Duration::from_secs(60);
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        if Instant::now() > deadline {
            return Err("physics plugins never finished initializing".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();

    // Settle: extraction (Startup), the spawn, the collider hierarchy, and the broad phase all have
    // to land before a single ray is cast.
    for _ in 0..16 {
        app.update();
        if app.world().get_resource::<ProbeTank>().is_some() {
            break;
        }
    }
    if app.world().get_resource::<ProbeTank>().is_none() {
        return Err("the probe tank never spawned from the blueprint".into());
    }
    for _ in 0..8 {
        app.update();
    }
    Ok(app)
}

// ---------------------------------------------------------------------------------------------
// The fuzz pass
// ---------------------------------------------------------------------------------------------

#[derive(Resource, Clone)]
struct FuzzJob(FuzzConfig);

#[derive(Resource, Default)]
struct FuzzOutput(Report);

/// A crew or ammunition volume — the things a corridor is a finding for.
struct Target {
    name: String,
    kind: TargetKind,
}

/// Everything one fuzz pass borrows from the world, plus the frames it measures in.
struct Pass<'a, 'w, 's> {
    context: ResolveContext<'a, 'w, 's>,
    spares: Spares,
    targets: BTreeMap<Entity, Target>,
    /// Authored node name per entity, so every diagnostic names geometry rather than an entity id.
    names: BTreeMap<Entity, String>,
    /// World → tank-local.
    to_local: bevy::math::Affine3A,
    local_min: Vec3,
    local_max: Vec3,
}

/// Prefix of the union-field integral: `∫₀ᵗ max(factor) dt` over canonical spans, in reference-mm.
///
/// f64 accumulation, matching [`walk::RayWalk`]'s own cost, so a prefix taken at the corridor end
/// equals the walk's reported total rather than drifting from it.
fn prefix_cost(spans: &[Span], t: f64) -> f64 {
    spans
        .iter()
        .map(|span| {
            let end = span.end.min(t);
            if end <= span.start {
                0.0
            } else {
                (end - span.start) * span.factor as f64
            }
        })
        .sum()
}

/// One round's flight along one shot line, driven through the runtime's crossing state machine.
#[derive(Clone, Debug, Default)]
pub(super) struct Drive {
    /// Every crossing decided, in order.
    pub decisions: Vec<Decision>,
}

impl Drive {
    /// Entrance coverage η of the first armour the round met, `0` when it met none.
    pub(super) fn eta(&self) -> f32 {
        self.decisions
            .iter()
            .find_map(|decision| match decision {
                Decision::Clear { .. } => None,
                Decision::Bounce(bounce) => Some(bounce.coverage),
                Decision::Bite(bite) => Some(bite.coverage),
            })
            .unwrap_or(0.0)
    }

    /// THE ADMISSION RULE: some crossing put a positive chord into `target` before the round
    /// stopped. Deposits are clipped at the embed progress, so a round that dies short of the target
    /// never reaches it, however close.
    pub(super) fn reaches(&self, target: Entity) -> bool {
        self.entered().any(|entity| entity == target)
    }

    /// Every volume a crossing put a positive chord into, in decision order (with repeats).
    pub(super) fn entered(&self) -> impl Iterator<Item = Entity> + '_ {
        self.decisions.iter().flat_map(|decision| match decision {
            Decision::Bite(bite) => bite
                .deposits
                .iter()
                .filter(|deposit| deposit.chord > 0.0)
                .map(|deposit| deposit.entity)
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
    }
}

/// A walk error met on a driven flight, with the leg it was met on.
#[derive(Clone, Debug)]
pub(super) struct DriveFailure {
    pub error: WalkError,
    /// WORLD cast origin and direction of the failing leg.
    pub origin: Vec3,
    pub direction: Vec3,
}

/// Fly `body` at `speed` from `origin` along `direction` for at most `length` metres of travel,
/// deciding every crossing exactly as the runtime march does — the same surface cuts
/// ([`super::next_surface`]), the same [`resolve::decide`], the same [`Flight::follow`], the same
/// continuation drain — and applying nothing. Straight flight between crossings: the gate fires
/// geometric lines, without gravity or drag.
///
/// The probe round fires from the tank's own frame of reference, so nothing is excluded as its own.
pub(super) fn drive(
    context: &ResolveContext<'_, '_, '_>,
    body: RoundBody,
    speed: f32,
    origin: Vec3,
    direction: Vec3,
    length: f32,
) -> Result<Drive, DriveFailure> {
    let fail = |error: WalkError, flight: &Flight| DriveFailure {
        error,
        origin: flight.cast_origin(),
        direction: Vec3::from(flight.direction),
    };
    let bad = |reason| DriveFailure {
        error: WalkError::BadCorridor { sample: 0, reason },
        origin,
        direction,
    };
    let direction = Dir3::new(direction).map_err(|_| bad("the shot line has no direction"))?;
    let frame = DiscFrame::anchored(Vec3::from(direction))
        .ok_or_else(|| bad("the shot line has no usable sampling basis"))?;
    let mut flight = Flight {
        position: origin,
        direction,
        speed,
        frame,
        continuation: None,
    };
    let radius = resolve::disc_radius(body.caliber);
    let mut remaining = length;
    let mut out = Drive::default();
    while remaining > MARCH_EPS || flight.holds_material() {
        let contact = match next_surface(context.world, &flight, radius, remaining, &|_| true) {
            Ahead::Open | Ahead::Terrain { .. } => break,
            Ahead::Held => 0.0,
            Ahead::Armor(hit) => {
                if hit_ancestor(hit.entity, &context.world.volumes, &context.world.parents)
                    .is_none()
                {
                    break;
                }
                hit.distance
            }
        };
        let decision = resolve::decide(context, &|_| true, body, &flight, contact)
            .map_err(|error| fail(error, &flight))?;
        out.decisions.push(decision.clone());
        let followed = flight.follow(decision);
        remaining -= followed.travel;
        if followed.stopped {
            break;
        }
    }
    Ok(out)
}

/// The round the gate floor IS: the probe round of least capability, at the speed where its
/// capability equals the configured floor, or at its own reference speed when none is configured.
pub(super) fn floor_probe(config: &FuzzConfig) -> (RoundBody, f32) {
    let round = PROBE_ROUNDS
        .iter()
        .min_by(|a, b| a.capability().total_cmp(&b.capability()))
        .expect("the gun list is not empty");
    let speed = config
        .finding_floor
        .map_or(round.speed, |floor| speed_for(round.body(), floor));
    (round.body(), speed)
}

impl Pass<'_, '_, '_> {
    /// The authored name of a volume, or a legible stand-in. A split (mixed-substance) volume lives
    /// on an unnamed per-primitive collider, so the fallback names its PARENT node — §13.7's
    /// one-object-many-shells authoring means "the volume" a report has to talk about is the object.
    fn volume_name(&self, entity: Entity) -> String {
        if let Some(name) = self.names.get(&entity) {
            return name.clone();
        }
        let mut probe = entity;
        while let Ok(parent) = self.context.world.parents.get(probe) {
            probe = parent.parent();
            if let Some(name) = self.names.get(&probe) {
                return format!("{name} (shard)");
            }
        }
        format!("{entity:?}")
    }

    /// Collect and walk one corridor at the disc radius given. `radius == 0` degenerates to the pure
    /// axis ray (§13.5: a fragment IS a shell with `r → 0`).
    fn probe(&self, ray: &FuzzRay, radius: f32) -> Result<DiscWalk, WalkError> {
        let frame = DiscFrame::anchored(ray.direction).ok_or(WalkError::BadCorridor {
            sample: 0,
            reason: "the generated ray has no usable sampling basis",
        })?;
        let corridor = resolve::build_corridor(
            &self.context,
            &|_| true,
            ray.origin,
            Vec3::ZERO,
            ray.direction,
            ray.length,
            frame,
            radius,
            &[],
        )?;
        let volumes = resolve::volume_table(&self.context, &corridor)?;
        walk::walk_disc(&corridor, &volumes, &self.context.laws)
    }

    /// The same collection, kept as its parts, for the duplication gate.
    fn probe_parts(&self, ray: &FuzzRay) -> Result<(RayCorridor, VolumeTable), WalkError> {
        let frame = DiscFrame::anchored(ray.direction).ok_or(WalkError::BadCorridor {
            sample: 0,
            reason: "the generated ray has no usable sampling basis",
        })?;
        let corridor = resolve::build_corridor(
            &self.context,
            &|_| true,
            ray.origin,
            Vec3::ZERO,
            ray.direction,
            ray.length,
            frame,
            0.0,
            &[],
        )?;
        let volumes = resolve::volume_table(&self.context, &corridor)?;
        Ok((
            RayCorridor {
                anchor: ray.origin,
                origin: Vec3::ZERO,
                axis: ray.direction,
                length: ray.length,
                initial_presence: Vec::new(),
                hits: corridor.samples[0].hits.clone(),
            },
            volumes,
        ))
    }

    /// Every crossing of the primitive a [`WalkError`] blamed, as `(t, axis · n)` — see
    /// [`WalkFailure::crossings`] for what the shape of that list diagnoses.
    fn blamed_crossings(&self, ray: &FuzzRay, error: &WalkError) -> Vec<(f64, f32)> {
        let Some(key) = blamed_shell(error) else {
            return Vec::new();
        };
        let Ok((corridor, _)) = self.probe_parts(ray) else {
            return Vec::new();
        };
        let mut out: Vec<(f64, f32)> = corridor
            .hits
            .iter()
            .filter(|hit| hit.volume == key.volume && hit.primitive == key.primitive)
            .map(|hit| (hit.t, ray.direction.dot(hit.true_normal)))
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }

    /// §13.6 idempotence + monotonicity, on a REAL corridor through the bound tank.
    ///
    /// One primitive's crossings are copied onto a spare entity pair and the same ray re-walked. At
    /// the SAME factor the union `max` is idempotent, so the cost must come back bit-identical; at a
    /// HIGHER factor adding the volume must never lower it.
    fn duplication_gate(&self, ray_index: u64, ray: &FuzzRay, out: &mut Report) {
        let Ok((corridor, volumes)) = self.probe_parts(ray) else {
            return;
        };
        let Some(seed) = corridor.hits.first().copied() else {
            return;
        };
        let Ok(base) = walk::walk_ray(0, &corridor, &volumes, &self.context.laws) else {
            return;
        };
        let Ok(factor) = volumes
            .entries()
            .find(|(volume, _)| *volume == seed.volume)
            .map(|(_, factor)| factor)
            .ok_or(())
        else {
            return;
        };

        let copies: Vec<FaceHit> = corridor
            .hits
            .iter()
            .filter(|hit| hit.primitive == seed.primitive)
            .map(|hit| FaceHit {
                volume: self.spares.volume,
                primitive: self.spares.primitive,
                ..*hit
            })
            .collect();
        let mut doubled = corridor.clone();
        doubled.hits.extend(copies);

        out.duplication_checks += 1;
        for (scale, kind) in [
            (1.0_f32, ViolationKind::DuplicationChangedCost),
            (2.0, ViolationKind::DuplicationLoweredCost),
        ] {
            let mut entries: Vec<(Entity, f32)> = volumes.entries().collect();
            entries.push((self.spares.volume, factor * scale));
            let Ok(table) = VolumeTable::new(entries) else {
                continue;
            };
            let Ok(walked) = walk::walk_ray(0, &doubled, &table, &self.context.laws) else {
                continue;
            };
            let bad = match kind {
                ViolationKind::DuplicationChangedCost => walked.cost != base.cost,
                _ => walked.cost < base.cost,
            };
            if bad {
                out.violations.push(Violation {
                    kind,
                    ray: ray_index,
                    detail: format!(
                        "duplicating volume {:?} at {scale}× its factor moved the union cost \
                         {} → {} reference-mm",
                        seed.volume, base.cost, walked.cost
                    ),
                });
            }
        }
    }

    /// Where the ray pierces the tank's local bounding box, in tank-local metres — the regional
    /// descriptor a finding is reported with. `None` when the ray misses the box.
    fn pierce_local(&self, ray: &FuzzRay) -> Option<Vec3> {
        let origin = self.to_local.transform_point3(ray.origin);
        let direction = self.to_local.transform_vector3(ray.direction);
        let mut near = f32::NEG_INFINITY;
        let mut far = f32::INFINITY;
        for axis in 0..3 {
            let d = direction[axis];
            let (lo, hi) = (self.local_min[axis], self.local_max[axis]);
            if d.abs() < 1.0e-9 {
                if origin[axis] < lo || origin[axis] > hi {
                    return None;
                }
                continue;
            }
            let (mut t0, mut t1) = ((lo - origin[axis]) / d, (hi - origin[axis]) / d);
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            near = near.max(t0);
            far = far.min(t1);
        }
        (near <= far).then(|| origin + direction * near.max(0.0))
    }

    /// Measure one opening against every gun in the game (§13.5's caliber gate): each round driven
    /// along the witness ray through the shared mechanism, admitted iff it reaches the target.
    fn sweep(&self, ray: &FuzzRay, target: Entity) -> Result<Vec<RoundMeasurement>, DriveFailure> {
        PROBE_ROUNDS
            .iter()
            .map(|round| {
                let flown = drive(
                    &self.context,
                    round.body(),
                    round.speed,
                    ray.origin,
                    ray.direction,
                    ray.length,
                )?;
                Ok(RoundMeasurement {
                    eta: flown.eta(),
                    reached: flown.reaches(target),
                    capability: round.capability(),
                })
            })
            .collect()
    }

    /// A driven flight's walk error, as the report records it.
    fn drive_failure(&self, ray: u64, failure: DriveFailure) -> WalkFailure {
        let leg = FuzzRay {
            origin: failure.origin,
            direction: failure.direction,
            length: resolve::MAX_CORRIDOR,
        };
        WalkFailure {
            ray,
            origin: failure.origin,
            direction: failure.direction,
            volume: blamed_volume(&failure.error).map_or_else(
                || "(no volume)".to_owned(),
                |entity| self.volume_name(entity),
            ),
            crossings: self.blamed_crossings(&leg, &failure.error),
            error: format!("{:?}", failure.error),
        }
    }
}

/// The tank's world bounding sphere and its local box, from the bound colliders themselves.
fn tank_bounds(
    world: &ProjectileMarchWorld,
    aabbs: &Query<&ColliderAabb>,
    to_local: bevy::math::Affine3A,
) -> Option<(Vec3, f32, Vec3, Vec3)> {
    let (mut wmin, mut wmax) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    let (mut lmin, mut lmax) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    let mut any = false;
    world.spatial.aabb_intersections_with_aabb_callback(
        ColliderAabb::from_min_max(Vec3::splat(-1.0e5), Vec3::splat(1.0e5)),
        |entity| {
            if hit_ancestor(entity, &world.volumes, &world.parents).is_none() {
                return true;
            }
            let Ok(aabb) = aabbs.get(entity) else {
                return true;
            };
            any = true;
            wmin = wmin.min(aabb.min);
            wmax = wmax.max(aabb.max);
            for corner in 0..8 {
                let pick =
                    |bit: usize, lo: f32, hi: f32| if corner & (1 << bit) == 0 { lo } else { hi };
                let world_corner = Vec3::new(
                    pick(0, aabb.min.x, aabb.max.x),
                    pick(1, aabb.min.y, aabb.max.y),
                    pick(2, aabb.min.z, aabb.max.z),
                );
                let local = to_local.transform_point3(world_corner);
                lmin = lmin.min(local);
                lmax = lmax.max(local);
            }
            true
        },
    );
    any.then(|| {
        (
            (wmin + wmax) * 0.5,
            (wmax - wmin).length() * 0.5,
            lmin,
            lmax,
        )
    })
}

/// The pass itself: generate, probe, assert, cluster.
fn run_fuzz(
    job: Res<FuzzJob>,
    mut output: ResMut<FuzzOutput>,
    tank: Res<ProbeTank>,
    spares: Res<Spares>,
    world: ProjectileMarchWorld,
    // `'static` spelled out because `ResolveContext` (the march's own borrow group) names the
    // query that way: `Query`'s data parameter is invariant, so an elided lifetime here will not
    // coerce into it.
    colliders: Query<(
        &'static Position,
        &'static Rotation,
        &'static Collider,
        Option<&'static super::BallisticSurfaces>,
    )>,
    aabbs: Query<&ColliderAabb>,
    facets: Query<(Entity, &Name, Option<&CrewStation>, Has<Ammo>)>,
    names: Query<(Entity, &Name)>,
    roots: Query<&GlobalTransform>,
    mut commands: Commands,
) {
    let config = job.0.clone();
    let started = Instant::now();
    let root = roots
        .get(tank.0)
        .expect("the probe tank has a global transform");
    let to_local = root.affine().inverse();

    let armor = SpatialQueryFilter::from_mask(Layer::Armor);
    let pass_context = ResolveContext {
        world: &world,
        colliders: &colliders,
        armor: &armor,
        deposit: false,
        laws: walk::WalkLaws::default(),
    };
    let Some((center, radius, local_min, local_max)) = tank_bounds(&world, &aabbs, to_local) else {
        panic!("the probe tank presented no ballistic collider to fuzz");
    };

    let targets: BTreeMap<Entity, Target> = facets
        .iter()
        .filter_map(|(entity, name, crew, ammo)| {
            let kind = match (crew.is_some(), ammo) {
                (_, true) => TargetKind::Ammo,
                (true, false) => TargetKind::Crew,
                _ => return None,
            };
            Some((
                entity,
                Target {
                    name: name.as_str().to_owned(),
                    kind,
                },
            ))
        })
        .collect();
    assert!(
        !targets.is_empty(),
        "the bound tank declared no crew or ammunition volume — the fuzzer would be vacuous"
    );

    let pass = Pass {
        context: pass_context,
        spares: *spares,
        targets,
        names: names
            .iter()
            .map(|name| (name.0, name.1.as_str().to_owned()))
            .collect(),
        to_local,
        local_min,
        local_max,
    };

    let mut report = Report {
        seed: config.seed,
        rays: config.rays,
        center,
        radius,
        ..default()
    };
    let (floor_body, floor_speed) = floor_probe(&config);
    let mut regions: BTreeMap<([i32; 3], TargetKind), Region> = BTreeMap::new();

    for index in 0..config.rays {
        let ray = ray_for(index, &config, center, radius);
        let walked = match pass.probe(&ray, 0.0) {
            Ok(walked) => walked,
            Err(error) => {
                let volume = blamed_volume(&error).map_or_else(
                    || "(no volume)".to_owned(),
                    |entity| pass.volume_name(entity),
                );
                *report.walk_error_volumes.entry(volume.clone()).or_default() += 1;
                report.walk_errors.push(WalkFailure {
                    ray: index,
                    origin: ray.origin,
                    direction: ray.direction,
                    volume,
                    crossings: pass.blamed_crossings(&ray, &error),
                    error: format!("{error:?}"),
                });
                continue;
            }
        };
        let axis = &walked.walks[0];
        let crossed = axis.presence.iter().any(|presence| presence.chord > 0.0);
        if crossed {
            report.rays_crossing += 1;
        }
        check_ray_invariants(index, crossed, axis, &mut report);
        if crossed && config.duplication_stride > 0 && index % config.duplication_stride == 0 {
            pass.duplication_gate(index, &ray, &mut report);
        }
        survey_reach(axis, &pass, &mut report);
        // ADMISSION: the floor probe, flown through the runtime's own mechanism along this line.
        match drive(
            &pass.context,
            floor_body,
            floor_speed,
            ray.origin,
            ray.direction,
            ray.length,
        ) {
            Ok(flown) => cluster_findings(
                index,
                &ray,
                axis,
                &flown,
                &pass,
                &config,
                &mut regions,
                &mut report,
            ),
            Err(failure) => record_failure(pass.drive_failure(index, failure), &mut report),
        }
    }

    report.regions = regions.into_values().collect();
    report
        .regions
        .sort_by(|a, b| a.min_axis_cost.total_cmp(&b.min_axis_cost));
    report.elapsed = started.elapsed();
    output.0 = report;
    commands.remove_resource::<FuzzJob>();
}

/// A walk failure joins the report, counted against the volume it blames.
fn record_failure(failure: WalkFailure, report: &mut Report) {
    *report
        .walk_error_volumes
        .entry(failure.volume.clone())
        .or_default() += 1;
    report.walk_errors.push(failure);
}

/// The two per-ray invariants §13.6 states about EVENTS, machine-checked.
fn check_ray_invariants(index: u64, crossed: bool, axis: &walk::RayWalk, report: &mut Report) {
    if crossed && axis.events.is_empty() {
        report.violations.push(Violation {
            kind: ViolationKind::SilentPenetration,
            ray: index,
            detail: format!(
                "the ray crossed {} reference-mm of material and the walk reported no boundary event",
                axis.cost
            ),
        });
    }
    if !crossed && !axis.events.is_empty() {
        report.violations.push(Violation {
            kind: ViolationKind::FabricatedEvent,
            ray: index,
            detail: format!(
                "the ray crossed no material and the walk reported {} boundary event(s)",
                axis.events.len()
            ),
        });
    }
}

/// The straight axis's survey: the census of volumes it crossed and the cheapest straight-line cost
/// to each crew/ammo volume. Reported, never admitting.
fn survey_reach(axis: &walk::RayWalk, pass: &Pass<'_, '_, '_>, report: &mut Report) {
    for presence in &axis.presence {
        if presence.chord > 0.0 {
            *report
                .presence_census
                .entry(pass.volume_name(presence.entity))
                .or_default() += 1;
        }
        let Some(target) = pass.targets.get(&presence.entity) else {
            continue;
        };
        let Some((reach, _)) = presence.spans.first() else {
            continue;
        };
        let cost = prefix_cost(&axis.spans, *reach) as f32;
        let thinnest = report
            .min_reach
            .entry(target.name.clone())
            .or_insert(f32::INFINITY);
        *thinnest = thinnest.min(cost);
    }
}

/// The straight-axis cost to `target`, or infinity when the axis never crosses it.
fn axis_cost(axis: &walk::RayWalk, target: Entity) -> f32 {
    axis.presence
        .iter()
        .find(|presence| presence.entity == target)
        .and_then(|presence| presence.spans.first())
        .map_or(f32::INFINITY, |(reach, _)| {
            prefix_cost(&axis.spans, *reach) as f32
        })
}

/// Fold the crew/ammo volumes the floor probe reached into the region table, sweeping the guns the
/// first time a cell is seen.
#[expect(
    clippy::too_many_arguments,
    reason = "the fold's whole state, split out of `run_fuzz` only to keep that function readable"
)]
fn cluster_findings(
    index: u64,
    ray: &FuzzRay,
    axis: &walk::RayWalk,
    flown: &Drive,
    pass: &Pass<'_, '_, '_>,
    config: &FuzzConfig,
    regions: &mut BTreeMap<([i32; 3], TargetKind), Region>,
    report: &mut Report,
) {
    let reached: BTreeSet<Entity> = flown
        .entered()
        .filter(|entity| pass.targets.contains_key(entity))
        .collect();
    for entity in reached {
        let target = &pass.targets[&entity];
        let cost = axis_cost(axis, entity);
        let Some(entry_local) = pass.pierce_local(ray) else {
            continue;
        };
        let cell = [
            (entry_local.x / REGION_CELL).floor() as i32,
            (entry_local.y / REGION_CELL).floor() as i32,
            (entry_local.z / REGION_CELL).floor() as i32,
        ];
        match regions.get_mut(&(cell, target.kind)) {
            Some(region) => {
                region.rays += 1;
                region.targets.insert(target.name.clone());
                region.min_axis_cost = region.min_axis_cost.min(cost);
            }
            None => {
                if regions.len() >= config.max_region_sweeps {
                    report.sweeps_truncated = true;
                    continue;
                }
                let measurements = match pass.sweep(ray, entity) {
                    Ok(measurements) => measurements,
                    Err(failure) => {
                        record_failure(pass.drive_failure(index, failure), report);
                        continue;
                    }
                };
                let admitting = measurements.iter().position(RoundMeasurement::admits);
                regions.insert(
                    (cell, target.kind),
                    Region {
                        cell,
                        entry_local,
                        axis_local: pass.to_local.transform_vector3(ray.direction),
                        targets: BTreeSet::from([target.name.clone()]),
                        kind: target.kind,
                        rays: 1,
                        min_axis_cost: cost,
                        measurements,
                        admitting,
                        witness_ray: index,
                    },
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------------------------

/// Build the world, fire `config.rays` rays at the bound Tiger, and report.
pub fn fuzz(config: &FuzzConfig) -> Result<Report, String> {
    let mut app = probe_world()?;
    app.init_resource::<FuzzOutput>()
        .insert_resource(FuzzJob(config.clone()))
        .add_systems(Update, run_fuzz.run_if(resource_exists::<FuzzJob>));
    app.update();
    Ok(app.world().resource::<FuzzOutput>().0.clone())
}

/// One ray's corridor, dumped.
///
/// The report names a failing ray by `(seed, index)` and calls that a complete reproduction recipe.
/// This is what consumes it: the ray generator is re-run for that index alone and every crossing
/// along the corridor is printed in order, with the volume that owns it, the triangle that produced
/// it and the sign that makes it an entry or an exit. Which is what a `WalkError` has to be read
/// against — the error names one primitive, and the defect is almost always in what its neighbours
/// did or did not report.
#[derive(Resource, Clone)]
struct ReplayJob {
    config: FuzzConfig,
    ray: u64,
}

#[derive(Resource, Default)]
struct ReplayOutput(String);

fn run_replay(
    job: Res<ReplayJob>,
    mut output: ResMut<ReplayOutput>,
    tank: Res<ProbeTank>,
    spares: Res<Spares>,
    world: ProjectileMarchWorld,
    colliders: Query<(
        &'static Position,
        &'static Rotation,
        &'static Collider,
        Option<&'static super::BallisticSurfaces>,
    )>,
    aabbs: Query<&ColliderAabb>,
    names: Query<(Entity, &Name)>,
    roots: Query<&GlobalTransform>,
    mut commands: Commands,
) {
    use std::fmt::Write as _;
    let root = roots
        .get(tank.0)
        .expect("the probe tank has a global transform");
    let to_local = root.affine().inverse();
    let armor = SpatialQueryFilter::from_mask(Layer::Armor);
    let context = ResolveContext {
        world: &world,
        colliders: &colliders,
        armor: &armor,
        deposit: false,
        laws: walk::WalkLaws::default(),
    };
    let Some((center, radius, local_min, local_max)) = tank_bounds(&world, &aabbs, to_local) else {
        panic!("the probe tank presented no ballistic collider to fuzz");
    };
    let pass = Pass {
        context,
        spares: *spares,
        targets: BTreeMap::new(),
        names: names
            .iter()
            .map(|name| (name.0, name.1.as_str().to_owned()))
            .collect(),
        to_local,
        local_min,
        local_max,
    };

    let ray = ray_for(job.ray, &job.config, center, radius);
    let mut text = String::new();
    let _ = writeln!(
        text,
        "seed {} ray {}: origin {:?} direction {:?} length {}",
        job.config.seed, job.ray, ray.origin, ray.direction, ray.length
    );
    match pass.probe_parts(&ray) {
        Ok((corridor, _)) => {
            let mut hits = corridor.hits;
            hits.sort_by(|a, b| a.t.total_cmp(&b.t));
            for hit in &hits {
                let along = ray.direction.dot(hit.true_normal);
                let _ = writeln!(
                    text,
                    "  t={:.7} {:>3} {:<28} volume={:?} primitive={:?} triangle={} axis·n={:+.6}",
                    hit.t,
                    if along < 0.0 { "IN" } else { "OUT" },
                    pass.volume_name(hit.volume),
                    hit.volume,
                    hit.primitive,
                    hit.triangle,
                    along,
                );
            }
        }
        Err(error) => {
            let _ = writeln!(text, "  the corridor would not collect: {error:?}");
        }
    }
    let _ = writeln!(
        text,
        "walk: {}",
        match pass.probe(&ray, 0.0) {
            Ok(_) => "resolved".to_owned(),
            Err(error) => format!("{error:?}"),
        }
    );
    output.0 = text;
    commands.remove_resource::<ReplayJob>();
}

/// Re-fire one ray of a run and dump its corridor — see [`ReplayJob`].
pub fn replay_ray(config: &FuzzConfig, ray: u64) -> Result<String, String> {
    let mut app = probe_world()?;
    app.init_resource::<ReplayOutput>()
        .insert_resource(ReplayJob {
            config: config.clone(),
            ray,
        })
        .add_systems(Update, run_replay.run_if(resource_exists::<ReplayJob>));
    app.update();
    Ok(app.world().resource::<ReplayOutput>().0.clone())
}

/// `cargo run --bin ballistic_fuzzer [-- --rays N --seed S --out PATH]` — the bake-scale sweep.
///
/// Exit code 1 means the gate FAILED: a violated invariant, a walk error, or a corridor to crew or
/// ammunition. The report file is written either way, because a failing run is exactly the one
/// whose report somebody has to read.
pub fn run_ballistic_fuzzer() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = FuzzConfig {
        rays: 200_000,
        ..default()
    };
    let mut out = std::path::PathBuf::from("target/ballistic-fuzzer-report.md");
    let mut replay: Vec<u64> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--rays" => config.rays = value()?.parse()?,
            "--seed" => config.seed = value()?.parse()?,
            "--out" => out = value()?.into(),
            "--replay" => replay.push(value()?.parse()?),
            other => return Err(format!("unknown flag {other}").into()),
        }
    }
    if !replay.is_empty() {
        for ray in replay {
            println!("{}", replay_ray(&config, ray)?);
        }
        return Ok(());
    }

    let report = fuzz(&config)?;
    let text = render(&report);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, &text)?;
    println!("{text}");
    println!("report written to {}", out.display());
    if report.is_clean() {
        Ok(())
    } else {
        Err("the §13.6 gate did not pass — see the report".into())
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// The report, as markdown.
pub fn render(report: &Report) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "# Ballistic ray fuzzer — Tiger I\n");
    let _ = writeln!(
        out,
        "seed `{}` · {} rays · {} crossed material · {} duplication gates · {:.1}s",
        report.seed,
        report.rays,
        report.rays_crossing,
        report.duplication_checks,
        report.elapsed.as_secs_f32(),
    );
    let _ = writeln!(
        out,
        "bounding sphere: centre {:?}, radius {:.3} m\n",
        report.center, report.radius
    );

    let _ = writeln!(out, "## Invariants\n");
    if report.violations.is_empty() {
        let _ = writeln!(out, "- no violation");
    }
    for violation in &report.violations {
        let _ = writeln!(
            out,
            "- **{:?}** ray {} — {}",
            violation.kind, violation.ray, violation.detail
        );
    }
    let _ = writeln!(
        out,
        "\n`WalkError` rate: {} / {} rays",
        report.walk_errors.len(),
        report.rays
    );
    for (volume, count) in &report.walk_error_volumes {
        let _ = writeln!(out, "- `{volume}` — {count}");
    }
    for failure in report.walk_errors.iter().take(10) {
        let _ = writeln!(
            out,
            "  - ray {} `{}` from {:?} along {:?}: {}",
            failure.ray, failure.volume, failure.origin, failure.direction, failure.error
        );
        if !failure.crossings.is_empty() {
            let crossings: Vec<String> = failure
                .crossings
                .iter()
                .map(|(t, dot)| {
                    let side = if *dot < 0.0 { "in" } else { "out" };
                    format!("{t:.4}{side}")
                })
                .collect();
            let _ = writeln!(out, "    blamed primitive: {}", crossings.join(" "));
        }
    }

    let _ = writeln!(out, "\n## Volume census (rays crossing each volume)\n");
    for (volume, count) in &report.presence_census {
        let _ = writeln!(out, "- `{volume}` — {count}");
    }

    let _ = writeln!(
        out,
        "\n## Thinnest route to each crew / ammunition volume\n"
    );
    let _ = writeln!(out, "| volume | cheapest axis cost (ref-mm) |");
    let _ = writeln!(out, "|---|---|");
    for (volume, cost) in &report.min_reach {
        let _ = writeln!(out, "| {volume} | {cost:.1} |");
    }

    let _ = writeln!(out, "\n## Corridors reaching crew or ammunition\n");
    if report.sweeps_truncated {
        let _ = writeln!(
            out,
            "> **TRUNCATED** — the region cap was reached; the list below is not exhaustive.\n"
        );
    }
    if report.regions.is_empty() {
        let _ = writeln!(out, "None.");
    }
    for region in &report.regions {
        render_region(&mut out, region);
    }

    let _ = writeln!(out, "\n## Verdict\n");
    if report.regions.is_empty() {
        let _ = writeln!(out, "- no corridor reaches crew or ammunition");
    }
    for region in &report.regions {
        let _ = writeln!(
            out,
            "- **CORRIDOR** {:?} → {:?} at local {:?} (admits {})",
            region.kind.label(),
            region.targets,
            region.entry_local,
            region
                .admitting
                .map_or("nothing".to_owned(), |i| PROBE_ROUNDS[i].name.to_owned()),
        );
    }
    out
}

fn render_region(out: &mut String, region: &Region) {
    use std::fmt::Write as _;
    let _ = writeln!(
        out,
        "### {} · {}",
        region.kind.label(),
        region
            .targets
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
    );
    if let Some(caliber) = region.admitting_caliber() {
        let _ = writeln!(out, "- admitting caliber: {:.1} mm", caliber * 1000.0);
    }
    let _ = writeln!(
        out,
        "- entry (tank-local): `{:.3?}` · axis `{:.3?}` · cell {:?}",
        region.entry_local, region.axis_local, region.cell
    );
    let _ = writeln!(
        out,
        "- rays {} · cheapest axis cost {:.2} reference-mm · witness ray {} · admitting {}",
        region.rays,
        region.min_axis_cost,
        region.witness_ray,
        region
            .admitting
            .map_or("(closed to every gun)".to_owned(), |i| PROBE_ROUNDS[i]
                .name
                .to_owned()),
    );
    let _ = writeln!(out, "\n| round | η | capability | admits |");
    let _ = writeln!(out, "|---|---|---|---|");
    for (round, measurement) in PROBE_ROUNDS.iter().zip(&region.measurements) {
        let _ = writeln!(
            out,
            "| {} | {:.3} | {:.0} | {} |",
            round.name,
            measurement.eta,
            measurement.capability,
            if measurement.admits() { "yes" } else { "no" },
        );
    }
    let _ = writeln!(out);
}

#[cfg(test)]
mod tests;
