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
//! [`walk::walk_ray`]/[`walk::walk_disc`], η by the disc walk's own entrance coverage, and the
//! caliber gate by [`super::capability`]. The only arithmetic here is the prefix integral of a walk
//! the core already produced. A gate that re-derives the law it is gating cannot fail.
//!
//! # What a FINDING is
//!
//! Not "a ray reached the crew" — a shot that pays for 100 mm of front plate and kills the driver is
//! the game working. A finding is a corridor whose factor-weighted cost to a crew or ammunition
//! volume is BELOW the smallest probe round's capability: an effectively unarmoured route. Each one
//! is then measured against the whole gun list (§13.5's caliber gate: a big disc engages the rim of
//! a small opening and pays for it, a small one flies through), and is either a real hole to fix or
//! a deliberate opening to BLESS — the turret ring, the MG ports, the vision slits. The bless list
//! (`assets/tiger_1/tiger_1.bless.ron`) is where weakspots are DECIDED; an unblessed corridor fails
//! the CI gate by name.
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
use serde::Deserialize;

use super::resolve::{self, ResolveContext};
use super::walk::{self, DiscFrame, DiscWalk, FaceHit, RayCorridor, Span, VolumeTable, WalkError};
use super::{ProjectileMarchWorld, capability};
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
    /// Reference striking speed (m/s). Muzzle velocity, so every number in the report reads "at
    /// point blank" — the most permissive case, which is the right bias for a hole hunt.
    pub speed: f32,
}

impl ProbeRound {
    /// Reference-mm this round can defeat at its reference speed — the shipped [`super::capability`]
    /// law, never a table.
    pub fn capability(&self) -> f32 {
        capability(self.mass, self.speed)
    }
}

/// The guns every opening is measured against, ASCENDING by caliber (the sweep takes the first that
/// admits, so order is load-bearing).
///
/// Masses and speeds are period AP loadings for each bore; the 7.92 and the 88 are the Tiger's own
/// spec values (`assets/tiger_1/tiger_1.tank.ron`), so the two rounds the game actually fires are
/// measured as they are actually fired.
pub const PROBE_ROUNDS: &[ProbeRound] = &[
    ProbeRound {
        name: "7.92mm",
        caliber: 0.0079,
        mass: 0.0118,
        speed: 755.0,
    },
    ProbeRound {
        name: "20mm",
        caliber: 0.020,
        mass: 0.148,
        speed: 830.0,
    },
    ProbeRound {
        name: "37mm",
        caliber: 0.037,
        mass: 0.685,
        speed: 762.0,
    },
    ProbeRound {
        name: "57mm",
        caliber: 0.057,
        mass: 2.87,
        speed: 815.0,
    },
    ProbeRound {
        name: "75mm",
        caliber: 0.075,
        mass: 6.8,
        speed: 790.0,
    },
    ProbeRound {
        name: "88mm",
        caliber: 0.088,
        mass: 10.2,
        speed: 773.0,
    },
    ProbeRound {
        name: "122mm",
        caliber: 0.122,
        mass: 25.0,
        speed: 795.0,
    },
];

// ---------------------------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------------------------

/// Tank-local metres per clustering cell. Findings landing in one cell are ONE opening in the
/// report, which is what makes a 10⁶-ray run readable and what a bless-list region has to match.
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
    /// Reference-mm a corridor to crew/ammo must come in UNDER to count as a finding. `None` is the
    /// gate's definition: the smallest probe round's capability, i.e. "a route even a machine-gun
    /// bullet takes for free". Raise it to survey the graded weakspots above that line — the number
    /// that reads as a hole rather than as armour is the thinnest authored plate (25 mm on this
    /// tank), and everything between the two is a real, deliberate, thin spot rather than a defect.
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
    /// η — the ENTRANCE COVERAGE the disc walk itself reported (§13.5's engagement fraction). 1.0 is
    /// a fully engaged face; 0.0 is a round that flew through the opening without touching it.
    pub eta: f32,
    /// Disc-mean `∫ max(factor) dt` from the corridor origin to the target's face, in reference-mm.
    pub cost: f32,
    /// What this round can spend, from [`ProbeRound::capability`].
    pub capability: f32,
}

impl RoundMeasurement {
    /// This round gets through the opening to the target.
    pub fn admits(&self) -> bool {
        self.cost <= self.capability
    }
}

/// One clustered opening reaching crew or ammunition.
#[derive(Clone, Debug)]
pub struct Region {
    /// Tank-local clustering cell (`entry_local / REGION_CELL`, floored).
    pub cell: [i32; 3],
    /// Where the witness corridor pierced the tank's local bounding box — the "where on the tank
    /// does this come in" descriptor a bless-list region is written against.
    pub entry_local: Vec3,
    /// Tank-local travel direction of the witness ray.
    pub axis_local: Vec3,
    pub targets: BTreeSet<String>,
    pub kind: TargetKind,
    /// How many fuzz rays landed in this cell.
    pub rays: u64,
    /// The cheapest axis-ray cost seen reaching a target here (reference-mm).
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

/// A [`WalkError`] the fuzzer met. Should be empty on a gated asset — the per-primitive manifold
/// gate (`bake`) is what makes that claim, and this is what tests it against real shot lines.
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
    pub crossings: Vec<(f32, f32)>,
}

/// The volume a [`WalkError`] blames, when it blames one.
fn blamed_volume(error: &WalkError) -> Option<Entity> {
    match error {
        WalkError::BadFactor { volume, .. }
        | WalkError::UnknownVolume { volume }
        | WalkError::CollectorFailed { volume, .. }
        | WalkError::CorridorOverflow { volume, .. } => Some(*volume),
        WalkError::UnexpectedExit { key, .. } | WalkError::UnexpectedEntry { key, .. } => {
            Some(key.volume)
        }
        WalkError::IncompleteCorridor { open, .. } => open.first().map(|key| key.volume),
        WalkError::BadCorridor { .. }
        | WalkError::CorridorMismatch { .. }
        | WalkError::DegenerateEntryNormal { .. } => None,
    }
}

/// The PRIMITIVE a [`WalkError`] blames, when the error is about one.
fn blamed_primitive(error: &WalkError) -> Option<walk::PrimitiveKey> {
    match error {
        WalkError::UnexpectedExit { key, .. } | WalkError::UnexpectedEntry { key, .. } => {
            Some(*key)
        }
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

/// Volumes whose glTF primitives are KNOWN to hold several closed shells that OVERLAP in space,
/// which the walk's per-primitive parity pairing cannot represent.
///
/// §13.7 makes this authoring legal and expects it — "multiple closed shells per object are legal
/// and expected", the road wheel being the named example (two steel bodies, two rubber rims, one
/// axle, in one object). The walk pairs entry/exit PER PRIMITIVE, so a ray that enters the axle
/// while still inside a wheel body is an entry for a primitive it is already in:
/// `WalkError::UnexpectedEntry`, and the round fails closed.
///
/// MEASURED 2026-08-07 by this fuzzer at 200 000 rays: 937 failures, every one of them
/// `UnexpectedEntry`, every one on a volume in this list, and the crossing dumps
/// ([`WalkFailure::crossings`]) show `enter, enter, exit, exit` with the two entries 3–92 mm apart —
/// two overlapping shells, not a tolerance hair. Prefix-matched, so all sixteen road wheels are one
/// row.
///
/// This is a KNOWN GAP between the authoring contract and the resolution core, recorded rather than
/// papered over: the gate below refuses any walk error blamed on ANYTHING ELSE, so a new defect
/// still fails loudly. Closing it means pairing per SHELL (or counting depth rather than presence),
/// which is a §13 core change and not this slice's to make.
pub const KNOWN_MULTI_SHELL_VOLUMES: &[&str] = &["Hull_Rear", "Turret_Cupola", "Wheel_"];

/// Whether a walk error on this volume is the known multi-shell gap rather than a new defect.
pub fn is_known_multi_shell(volume: &str) -> bool {
    KNOWN_MULTI_SHELL_VOLUMES
        .iter()
        .any(|known| volume.starts_with(known))
}

impl Report {
    /// Walk errors this run cannot explain — anything not blamed on
    /// [`KNOWN_MULTI_SHELL_VOLUMES`]. THE gate quantity: a rate is not a contract, but "no
    /// unexplained failure" is.
    pub fn unexplained_walk_errors(&self) -> Vec<&WalkFailure> {
        self.walk_errors
            .iter()
            .filter(|failure| !is_known_multi_shell(&failure.volume))
            .collect()
    }

    /// Nothing the gate refuses: no violated invariant, no unexplained walk error.
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty() && self.unexplained_walk_errors().is_empty()
    }
}

// ---------------------------------------------------------------------------------------------
// The bless list
// ---------------------------------------------------------------------------------------------

/// The checked-in list of DELIBERATE openings — §13.6's "the bless-list is where weakspots are
/// decided, not discovered by players as bugs".
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlessList {
    pub openings: Vec<Blessing>,
}

/// One blessed opening.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blessing {
    pub name: String,
    /// Tank-local centre (metres) of the region on the tank's bounding box the corridors come in
    /// through — the same descriptor [`Region::entry_local`] carries.
    pub center: [f32; 3],
    /// Tank-local radius (metres) around `center` this blessing covers.
    pub radius: f32,
    /// The smallest caliber (metres) MEASURED to get through. Documentation of what was accepted,
    /// not a threshold the gate enforces — the gate is regional.
    pub admits: f32,
    pub reason: String,
}

impl Blessing {
    fn covers(&self, point: Vec3) -> bool {
        point.distance(Vec3::from_array(self.center)) <= self.radius
    }
}

impl BlessList {
    /// The shipped Tiger bless list, embedded so the gate never depends on asset-server timing.
    pub fn shipped() -> Self {
        ron::de::from_str(include_str!("../../assets/tiger_1/tiger_1.bless.ron"))
            .expect("assets/tiger_1/tiger_1.bless.ron must parse")
    }
}

/// The gate's verdict on a report.
#[derive(Clone, Debug, Default)]
pub struct Verdict {
    /// Openings nobody has decided about. THE failure condition — each is either a hole to fix or a
    /// weakspot to bless, and the point of the gate is that somebody has to say which.
    pub unblessed: Vec<Region>,
    /// Blessings no corridor came in through. A warning, not a failure: geometry moved, or the run
    /// was too small to find it again.
    pub stale: Vec<String>,
}

/// Match every region against the bless list.
pub fn adjudicate(report: &Report, bless: &BlessList) -> Verdict {
    let mut matched = vec![false; bless.openings.len()];
    let mut unblessed = Vec::new();
    for region in &report.regions {
        let mut covered = false;
        for (index, opening) in bless.openings.iter().enumerate() {
            if opening.covers(region.entry_local) {
                matched[index] = true;
                covered = true;
            }
        }
        if !covered {
            unblessed.push(region.clone());
        }
    }
    Verdict {
        unblessed,
        stale: bless
            .openings
            .iter()
            .zip(&matched)
            .filter(|(_, hit)| !**hit)
            .map(|(opening, _)| opening.name.clone())
            .collect(),
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
fn prefix_cost(spans: &[Span], t: f32) -> f64 {
    spans
        .iter()
        .map(|span| {
            let end = span.end.min(t);
            if end <= span.start {
                0.0
            } else {
                (end as f64 - span.start as f64) * span.factor as f64
            }
        })
        .sum()
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
    fn blamed_crossings(&self, ray: &FuzzRay, error: &WalkError) -> Vec<(f32, f32)> {
        let Some(key) = blamed_primitive(error) else {
            return Vec::new();
        };
        let Ok((corridor, _)) = self.probe_parts(ray) else {
            return Vec::new();
        };
        let mut out: Vec<(f32, f32)> = corridor
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
    /// descriptor a bless-list entry is written against. `None` when the ray misses the box.
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

    /// Measure one opening against every gun in the game (§13.5's caliber gate).
    fn sweep(&self, ray: &FuzzRay, target: Entity) -> Vec<RoundMeasurement> {
        PROBE_ROUNDS
            .iter()
            .map(|round| {
                let radius = if round.caliber >= resolve::DISC_MIN_CALIBER {
                    round.caliber * 0.5
                } else {
                    0.0
                };
                let capability = round.capability();
                let Ok(walked) = self.probe(ray, radius) else {
                    // A corridor that will not resolve is not an admission — fail closed.
                    return RoundMeasurement {
                        eta: 1.0,
                        cost: f32::INFINITY,
                        capability,
                    };
                };
                // η is the disc walk's OWN entrance coverage — the §13.5 engagement fraction the
                // ricochet, bleed and normalization laws already consume. Nothing recomputed here.
                let eta = walked.events.first().map_or(0.0, |event| event.coverage);
                let Some(reach) = walked.walks[0]
                    .presence
                    .iter()
                    .find(|presence| presence.entity == target)
                    .and_then(|presence| presence.spans.first())
                    .map(|(start, _)| *start)
                else {
                    return RoundMeasurement {
                        eta,
                        cost: f32::INFINITY,
                        capability,
                    };
                };
                // §13.5's transit law over the target depth: the disc's cost is the MEAN over its k
                // samples, uncovered samples contributing their (zero) integral like any other.
                let mean = walked
                    .walks
                    .iter()
                    .map(|walk| prefix_cost(&walk.spans, reach))
                    .sum::<f64>()
                    / walked.walks.len() as f64;
                RoundMeasurement {
                    eta,
                    cost: mean as f32,
                    capability,
                }
            })
            .collect()
    }
}

/// The floor a corridor has to be under to count as a finding: the smallest probe round's
/// capability. Above it the corridor went through real armour and paid for it, which is the game
/// working rather than a hole.
fn finding_floor(config: &FuzzConfig) -> f32 {
    config.finding_floor.unwrap_or_else(|| {
        PROBE_ROUNDS
            .iter()
            .map(ProbeRound::capability)
            .fold(f32::INFINITY, f32::min)
    })
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
    colliders: Query<(&'static Position, &'static Rotation, &'static Collider)>,
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
    let floor = finding_floor(&config);
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
        cluster_findings(
            index,
            &ray,
            axis,
            &pass,
            floor,
            &config,
            &mut regions,
            &mut report,
        );
    }

    report.regions = regions.into_values().collect();
    report
        .regions
        .sort_by(|a, b| a.min_axis_cost.total_cmp(&b.min_axis_cost));
    report.elapsed = started.elapsed();
    output.0 = report;
    commands.remove_resource::<FuzzJob>();
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

/// Fold one ray's crew/ammo reaches into the region table, sweeping the guns the first time a cell
/// is seen.
#[expect(
    clippy::too_many_arguments,
    reason = "the fold's whole state, split out of `run_fuzz` only to keep that function readable"
)]
fn cluster_findings(
    index: u64,
    ray: &FuzzRay,
    axis: &walk::RayWalk,
    pass: &Pass<'_, '_, '_>,
    floor: f32,
    config: &FuzzConfig,
    regions: &mut BTreeMap<([i32; 3], TargetKind), Region>,
    report: &mut Report,
) {
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
        if cost >= floor {
            continue;
        }
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
                let measurements = pass.sweep(ray, presence.entity);
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

/// `cargo run --bin ballistic_fuzzer [-- --rays N --seed S --out PATH]` — the bake-scale sweep.
///
/// Exit code 1 means the gate FAILED: a violated invariant, a walk error, or an unblessed corridor
/// to crew or ammunition. The report file is written either way, because a failing run is exactly
/// the one whose report somebody has to read.
pub fn run_ballistic_fuzzer() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = FuzzConfig {
        rays: 200_000,
        ..default()
    };
    let mut out = std::path::PathBuf::from("target/ballistic-fuzzer-report.md");
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--rays" => config.rays = value()?.parse()?,
            "--seed" => config.seed = value()?.parse()?,
            "--out" => out = value()?.into(),
            other => return Err(format!("unknown flag {other}").into()),
        }
    }

    let report = fuzz(&config)?;
    let bless = BlessList::shipped();
    let verdict = adjudicate(&report, &bless);
    let text = render(&report, &bless, &verdict);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, &text)?;
    println!("{text}");
    println!("report written to {}", out.display());
    if report.is_clean() && verdict.unblessed.is_empty() {
        Ok(())
    } else {
        Err("the §13.6 gate did not pass — see the report".into())
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// The report, as markdown.
pub fn render(report: &Report, bless: &BlessList, verdict: &Verdict) -> String {
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
        let known = if is_known_multi_shell(volume) {
            "known multi-shell gap"
        } else {
            "**UNEXPLAINED**"
        };
        let _ = writeln!(out, "- `{volume}` — {count} ({known})");
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
        render_region(&mut out, region, bless);
    }

    let _ = writeln!(out, "\n## Verdict\n");
    if verdict.unblessed.is_empty() {
        let _ = writeln!(out, "- every corridor found is blessed");
    }
    for region in &verdict.unblessed {
        let _ = writeln!(
            out,
            "- **UNBLESSED** {:?} → {:?} at local {:?} (admits {})",
            region.kind.label(),
            region.targets,
            region.entry_local,
            region
                .admitting
                .map_or("nothing".to_owned(), |i| PROBE_ROUNDS[i].name.to_owned()),
        );
    }
    for stale in &verdict.stale {
        let _ = writeln!(out, "- stale blessing (no corridor found): `{stale}`");
    }
    out
}

fn render_region(out: &mut String, region: &Region, bless: &BlessList) {
    use std::fmt::Write as _;
    let opening = bless
        .openings
        .iter()
        .find(|opening| opening.covers(region.entry_local));
    let _ = writeln!(
        out,
        "### {} · {} · {}",
        opening.map_or("UNBLESSED", |opening| opening.name.as_str()),
        region.kind.label(),
        region
            .targets
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
    );
    if let Some(opening) = opening {
        let _ = writeln!(
            out,
            "- blessed: admits {:.0} mm — {}",
            opening.admits * 1000.0,
            opening.reason
        );
    }
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
    let _ = writeln!(out, "\n| round | η | cost (ref-mm) | capability | admits |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    for (round, measurement) in PROBE_ROUNDS.iter().zip(&region.measurements) {
        let _ = writeln!(
            out,
            "| {} | {:.3} | {} | {:.0} | {} |",
            round.name,
            measurement.eta,
            if measurement.cost.is_finite() {
                format!("{:.2}", measurement.cost)
            } else {
                "blocked".to_owned()
            },
            measurement.capability,
            if measurement.admits() { "yes" } else { "no" },
        );
    }
    let _ = writeln!(out);
}

#[cfg(test)]
mod tests;
