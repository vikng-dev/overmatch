//! Per-variant spec sheets as RON data assets (ADR-0010). The Blender model owns geometry and
//! spatial anchors; this owns the tuning numbers — mass + inertia, track powertrain/support, servo
//! configs — that differ per tank variant. A `.tank.ron` file deserializes (via serde) straight
//! into the same values the sim reads (`Mass`, `ForceParams`, `ServoSpec`), so
//! values stay plain-text, git-diffable, and separate from Blender. There are **no code defaults**
//! (ADR-0011): a competitive sim never runs on guessed stats. The shipped RON is embedded into the
//! eager `TankBlueprint` for simulation construction and also loaded as a Bevy asset for validation
//! and presentation diagnostics; simulation never reads the asset handle.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext, LoadState};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::damage::{Capability, CrewStation, FunctionRole, Requirement};
use crate::tank::{ServoSpec, Tank};

/// One tank variant's spec sheet — the typed contents of a `.tank.ron` file. Its fields *are* the
/// components the sim consumes; `tank::apply_tank_spec` copies them onto the rig once ready.
/// One ballistic volume's data, keyed by model node name in [`TankSpec::volumes`]. **Composition
/// over a `kind` enum** (design `armor-penetration-and-damage.md` §2/§12): `material_factor` is the
/// base every volume has (shell-resistance per metre), and optional facets layer roles on top:
/// `hp` makes it damageable, `crew` makes it a crewman, `ammo` makes depletion cook off, and
/// `function` marks a repairable capability. Never add a central `kind` enum; "is it crew?" means
/// "does it have the crew facet?"
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct VolumeSpec {
    /// Reference-mm of armour per metre of material — the shell-resistance cost, decoupled from role
    /// (a steel barrel module carries the same factor as a steel plate).
    pub material_factor: f32,
    /// HP pool if damageable (module/crew/ammo); absent → pure armour, nothing to lose. The RON
    /// enables `implicit_some`, so this is authored bare (`hp: 8.0`, not `hp: Some(8.0)`); omitting
    /// it yields `None`. Future facets follow the same optional-field-per-facet shape.
    #[serde(default)]
    pub hp: Option<f32>,
    /// Crew station served by this volume. Requires `hp`.
    #[serde(default)]
    pub crew: Option<CrewStation>,
    /// Ammunition volume: HP depletion cooks off and kills all crew. Requires `hp`.
    #[serde(default)]
    pub ammo: bool,
    /// Repairable capability served by this module. Function loss is derived from HP.
    #[serde(default)]
    pub function: Option<FunctionRole>,
}

/// Which fire input a weapon answers to (design: LMB = the main gun, Spacebar = the MGs). Pure fire
/// routing — it has *no* bearing on aiming or traverse (servos are weapon-agnostic). The `Primary`
/// weapon also supplies the rig's main-bore handles (its chain → `Rig.turret`/`gun`/`muzzle`).
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Primary,
    Secondary,
}

/// A weapon's fire *mechanism* — single-shot with a per-round reload, or belt-fed automatic. An
/// enum, not optional fields on `WeaponSpec`, so invalid combos are unrepresentable (ADR-0010/0011):
/// the 88 cannot author a `tracer_every` it never consults, an MG cannot omit its belt. Extensible
/// by design — a future overheat model adds fields to `Automatic` (deferred, owner call 2026-07-12).
#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub enum FireMode {
    /// One round per trigger *edge* (the click), then a crew-gated reload of `reload_secs` — the
    /// 88's mechanism. Every round is its own "belt": the shot always traces (its visual is the
    /// shell scene, not a streak).
    Single { reload_secs: f32 },
    /// Belt-fed cyclic fire on a held trigger *level*. `rpm` sets the cyclic interval (60/rpm s
    /// between rounds — pure mechanism, NEVER crew-gated: a dead loader does not slow a working
    /// action). The belt is finite (`belt_size` rounds, tracked as sim state in
    /// [`crate::tank::WeaponState::belt_remaining`]) over an INFINITE reserve (no stowed-ammo
    /// inventory — owner call 2026-07-12): running dry automatically starts a belt swap of
    /// `belt_swap_secs`, and the *swap* is what the weapon's `load` requirement gates, same as the
    /// 88's reload. `tracer_every` is the belt's composition (real belts are loaded e.g.
    /// 4-ball-1-tracer), NOT a VFX knob: every `tracer_every`-th round down the belt traces
    /// (`5` = one-in-five, `0` = a tracerless stealth belt — never traces). The seed of the
    /// belt-customization feature; a future load-out UI edits these same fields.
    Automatic {
        rpm: f32,
        belt_size: u32,
        belt_swap_secs: f32,
        tracer_every: u32,
    },
}

/// The mechanism category a fired round carries across the simulation and network seams.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FireMechanism {
    Single,
    Automatic,
}

impl FireMode {
    pub fn mechanism(self) -> FireMechanism {
        match self {
            Self::Single { .. } => FireMechanism::Single,
            Self::Automatic { .. } => FireMechanism::Automatic,
        }
    }
}

/// One weapon's data, keyed by logical name in [`TankSpec::weapons`]. `muzzle` (the bore the shot
/// leaves from) and the optional recoiling `barrel` are model node names; the weapon's aiming chain
/// is *not* declared here — it's the muzzle's servo ancestors, derived from the model hierarchy.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WeaponSpec {
    /// Fire input this weapon answers to. The single `Primary` also marks the rig's main bore.
    /// Pure input *routing* (which command field the weapon reads) — the fire mechanism, and with
    /// it the edge-vs-level input semantics, comes from [`Self::fire_mode`].
    pub trigger: Trigger,
    /// Bore node — shot origin + direction.
    pub muzzle: String,
    /// Recoiling barrel node, if the weapon reciprocates; omitted → no recoil (e.g. a coax).
    #[serde(default)]
    pub barrel: Option<String>,
    /// Muzzle velocity (m/s).
    pub speed: f32,
    /// Shell calibre (m) — drives overmatch in the penetration march.
    pub caliber: f32,
    /// Projectile mass (kg) — primary driver of penetration capability.
    pub mass: f32,
    /// The fire mechanism: single-shot reload or belt-fed automatic. Required (no code default,
    /// ADR-0011) — a weapon with an unstated mechanism is an authoring omission.
    pub fire_mode: FireMode,
    /// Recoil spring, present iff `barrel` is. Authored alongside it.
    #[serde(default)]
    pub recoil: Option<RecoilSpec>,
    /// Fire gate (design §7b): what must be crewed/intact to fire — operator + ordnance (e.g. the
    /// main gun's `[Gunner, Breech, GunBarrel]`, a coax's `Backup(Gunner|Loader)`). The per-weapon
    /// successor to the old global `Fire` capability. Empty = always firable.
    #[serde(default)]
    pub fire: Requirement,
    /// Load gate: what must hold for the reload timer to tick (e.g. `[Loader, Breech]`). The
    /// per-weapon successor to the old global `Load`. Empty = always loading.
    #[serde(default)]
    pub load: Requirement,
}

/// A weapon's procedural barrel-recoil spring (a 1-DOF damped spring along the bore).
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RecoilSpec {
    /// Backward impulse on firing (m/s along −bore). Higher = harder, longer kick.
    pub kick: f32,
    /// Spring stiffness pulling the barrel back to battery. Lower = longer stroke + slower return.
    pub stiffness: f32,
    /// Damping; slightly underdamped lets the barrel lumber home with a small settle.
    pub damping: f32,
}

/// A crew viewpoint — the camera/optic anchor. A closed set of kinds (each its own bespoke camera
/// behaviour in code), keyed in [`TankSpec::views`]; the *parameters* (which node, later FOV/zoom)
/// are data. The gunner's view node is also how the binder finds the gunner's chain for the rig.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewKind {
    Gunner,
    Commander,
}

impl ViewKind {
    pub fn label(self) -> &'static str {
        match self {
            ViewKind::Gunner => "Gunner sight",
            ViewKind::Commander => "Commander view",
        }
    }
}

/// One view's parameters: the model node the camera bolts to (which rides its servo's lay), the
/// camera vertical FOV (radians — narrow = magnified optic, wide = third-person), and the `requires`
/// gate that decides whether the view is usable. `requires` is the per-view successor to the old
/// global `GunnerSight`/`CommanderView` capabilities (same slew/fire-gate grammar, evaluated against
/// the controlled tank); empty = always available.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ViewSpec {
    pub node: String,
    pub fov: f32,
    #[serde(default)]
    pub requires: Requirement,
}

/// The continuous-track running gear + material loop (track architecture §7, minimal phase-A
/// cut). Every field is vehicle DATA — solver quality policy lives as constants in
/// `track::view`; a new tracked vehicle is authored here, never tuned there. Geometry the model
/// cannot express yet (sprocket/idler circles — their GLB visuals carry identity transforms with
/// position baked into vertices) is authored in **side-plane coordinates**: hull-local `(z, y)`
/// on the track's centreline plane, mirrored across `±plane_x`. The Tiger authoring pass replaces
/// these with proper rig nodes + baked bounds (`tiger-authoring-agenda.md`).
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TrackSpec {
    /// Link pitch (m). With `link_count` this is the IMMUTABLE material loop: its length is
    /// `pitch × link_count`, exact — the solver never rounds or spreads residual (tooth lock).
    pub pitch: f32,
    /// Links per side.
    pub link_count: usize,
    /// Shoe width (m) — the link mesh and the lateral terrain-probe stations.
    pub width: f32,
    /// Plate thickness (m); the pin line runs through the middle of the plate.
    pub thickness: f32,
    /// One link assembly's mass (kg) — real inverse masses in the chain constraints.
    pub link_mass: f32,
    /// Pin dry-friction torque (N·m) — the rope-vs-track differentiator, scaled to link mass.
    pub hinge_torque: f32,
    /// Hard articulation stop between consecutive links (rad).
    pub max_link_angle: f32,
    /// Track centreline |x| (m): the side plane the chain solves in. Left −, right +.
    pub plane_x: f32,
    /// Drive sprocket: side-plane centre + tooth count. The pitch radius is DERIVED —
    /// `pitch × teeth / τ` — never authored: one link advance ≡ one tooth advance by
    /// construction, and two numbers that must agree are one number.
    pub sprocket: SprocketSpec,
    /// Idler: side-plane centre + pin-line radius (idler rim + half plate).
    pub idler: IdlerSpec,
    /// Road-wheel PIN-LINE radius (m): wheel rim + half plate — the chain's wheel circles.
    pub wheel_radius: f32,
    /// The drivetrain spinning this track (phase B — the locomotion sim IS the track model).
    pub powertrain: PowertrainSpec,
    /// The belt-support contact law (replaces the raycast suspension spec).
    pub support: SupportSpec,
}

/// Per-track powertrain: constant-power engine curve under a low-speed force cap, with a
/// governor chasing `command × max_speed` against the reflected belt+drivetrain inertia.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PowertrainSpec {
    /// Top belt speed (m/s).
    pub max_speed: f32,
    /// Engine power per track (W).
    pub power: f32,
    /// Low-speed tractive force cap per track (N).
    pub force: f32,
    /// Governor gain (N per m/s of belt-speed error) — the throttle response feel.
    pub governor_gain: f32,
    /// Reflected belt + drivetrain inertia (kg).
    pub inertia: f32,
    /// The DECLARED transmission (phase 2.5, transmission-design.md): engine torque curve,
    /// gear ladders, steering table, brakes, architecture. `default` (absent) means the
    /// vehicle only has the legacy symmetric governor — the RON stays valid without it and
    /// every MP composition runs `Governor` regardless (the regenerative adapters are gated
    /// to the offline composition and the sandbox under REV 13).
    #[serde(default)]
    pub transmission: Option<TransmissionSpec>,
}

/// Which regenerative adapter the vehicle's declared transmission is (the `Governor` parity
/// mode is expressed by OMITTING the whole block, never authored).
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransmissionArchitecture {
    /// Continuous regenerative hybrid (design menu C/D) — the arcade-honest default.
    Hybrid,
    /// Fixed-radius geared regenerative steering (the Tiger's L600, design menu B).
    FixedRadii,
}

/// The declared drivetrain block. Authoring rule (tiger-transmission-data.md): per-gear
/// SPEEDS are the anchors; total reductions derive at build time against the spec's own
/// sprocket radius, so the ladder survives the open 19-vs-20-tooth sprocket discrepancy.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TransmissionSpec {
    pub architecture: TransmissionArchitecture,
    pub engine: EngineSpec,
    pub gearbox: GearboxSpec,
    pub steering: SteeringSpec,
    /// Per-side service/parking brake capacity at the sprocket (N).
    pub brake_force: f32,
}

/// The engine's declared envelope: a piecewise-linear torque curve under a fuel governor.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct EngineSpec {
    pub idle_rpm: f32,
    /// Fuel-governor rpm — the fleet operating condition (Tiger: 2500 from Nov 1943).
    pub governed_rpm: f32,
    /// The rpm the per-gear speed anchors are quoted at (Tiger: 3000).
    pub rated_rpm: f32,
    /// `(rpm, N·m)` authoring points, ascending rpm.
    pub torque_curve: Vec<(f32, f32)>,
    /// Zero-throttle engine drag (compression braking) as a fraction of peak torque,
    /// reflected through the current gear. Diesel motoring torque runs ~20–30% of rated
    /// (INFERRED band — no per-engine motoring curve reached); defaults to 0.25 when the
    /// vehicle does not author one.
    #[serde(default = "default_drag_fraction")]
    pub drag_fraction: f32,
}

/// See [`EngineSpec::drag_fraction`] — the middle of the diesel compression-braking band.
fn default_drag_fraction() -> f32 {
    0.25
}

/// The gear ladders as authored per-gear top belt speeds (km/h) at `rated_rpm`, plus the
/// auto-shift rpm bands (hysteresis: the band gap must exceed one ratio step or the box hunts).
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct GearboxSpec {
    pub forward_speeds_kmh: Vec<f32>,
    pub reverse_speeds_kmh: Vec<f32>,
    pub shift_up_rpm: f32,
    pub shift_down_rpm: f32,
    /// Gear-shift torque-interruption time (s) — how long the drive is uncoupled through a
    /// shift. Vehicle data (a preselector and a crash box differ); defaults to 0.31 s when
    /// unauthored (INFERRED, no per-vehicle shift-time datum reached).
    #[serde(default = "default_shift_secs")]
    pub shift_secs: f32,
}

/// See [`GearboxSpec::shift_secs`].
fn default_shift_secs() -> f32 {
    0.31
}

/// The steering member: per-gear fixed radii (the L600's two detents; the hybrid interpolates
/// the tight column continuously), its force capacity, and the regenerative power path.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SteeringSpec {
    /// Per FORWARD gear `(R_tight, R_wide)` turn radii (m); reverse mirrors the low gears.
    pub radii: Vec<(f32, f32)>,
    /// Steering-member force capacity PER OUTPUT (N): the member drives the two outputs
    /// differentially, so the belt-difference axis `F_s` carries up to 2× this (each side
    /// sees `F_s/2`, bounded by this datum — the gearing/grip-scale per-track cap).
    pub capacity: f32,
    /// Inner→outer recirculation efficiency η.
    pub recirculation: f32,
    // `neutral_fraction` DELETED (transmission fix 3, 2026-07-19): it was an unprovenanced
    // authored feel scalar; the L600 neutral turn now uses the DERIVED
    // `neutral_d_full = κ_tight(F1) × v1_governed` directly (the radii table's own
    // gear-independent invariant makes that the correct emergent pivot scale).
}

/// The belt-support penalty law, per metre of contacting belt.
#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct SupportSpec {
    /// Spring (N/m per metre of belt) — sets static sink under weight.
    pub stiffness_per_m: f32,
    /// Normal-velocity damping (N·s/m per metre).
    pub damping_per_m: f32,
    /// Soft-engagement ramp depth (m).
    pub engage: f32,
}

/// See [`TrackSpec::sprocket`]. `center` is side-plane `(z, y)`.
#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct SprocketSpec {
    pub center: (f32, f32),
    pub teeth: u32,
}

/// See [`TrackSpec::idler`]. `center` is side-plane `(z, y)`.
#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct IdlerSpec {
    pub center: (f32, f32),
    pub radius: f32,
}

#[derive(Asset, TypePath, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TankSpec {
    /// Total mass (kg) — authored balance data; the collision proxy contributes none (ADR-0011).
    pub mass: f32,
    /// Hull box full dimensions (x, y, z metres) approximating the angular-inertia distribution.
    pub inertia_extents: (f32, f32, f32),
    /// Continuous-track running gear, material loop, powertrain, and contact law — the
    /// locomotion spec (phase B: the track model IS the driving sim) and the track view's
    /// per-vehicle data.
    pub track: TrackSpec,
    /// Servos (actuator mounts) keyed by model node name — the **source of truth** for which nodes
    /// rotate and how. Each carries its aim `role` (which also derives the rotation axis: Yaw→Y,
    /// Pitch→X) and slew tuning; tank construction resolves each name and binds the servo.
    /// Replaces the old fixed `turret`/`gun` fields, so a variant can declare any number of mounts.
    pub servos: HashMap<String, ServoSpec>,
    /// Ballistic volumes keyed by model node name — the **source of truth** for which nodes are
    /// volumes and what they are (design §12). The march reads `material_factor`; tank construction
    /// layers components from the facets. The `Armor_/Module_/...` name prefix is documentation only.
    pub volumes: HashMap<String, VolumeSpec>,
    /// Weapons keyed by logical name — the **source of truth** for the tank's armament. Each names
    /// its bore (+ optional recoiling barrel) node and carries its ballistics/reload/recoil; the
    /// binder attaches a `Weapon` the shooting systems read. Replaces the hardcoded `shooting.rs`
    /// consts. (Multi-weapon control — selecting/aiming the coax + hull MG — is a later increment.)
    #[serde(default)]
    pub weapons: HashMap<String, WeaponSpec>,
    /// Crew viewpoints (camera/optic anchors) keyed by [`ViewKind`]. The gunner's also identifies
    /// the gunner's chain for the rig's main-bore handles.
    #[serde(default)]
    pub views: HashMap<ViewKind, ViewSpec>,
    /// Per-tank capability requirements (design §7b). Each capability maps to a list of requirement
    /// groups (AND'd): a bare `Part` is mandatory; `Pool(..)`/`Backup(..)` express graded redundancy.
    /// Drives [`crate::damage::capability_effectiveness`] — the single gate consuming systems query.
    #[serde(default)]
    pub capabilities: HashMap<Capability, Requirement>,
}

impl TankSpec {
    /// Fail-fast semantic validation past what serde's shape check catches (ADR-0011: a competitive
    /// sim never runs on silently-bricked stats). serde proves the *fields* exist and typecheck; this
    /// proves the *values* yield a weapon that can actually fire and cycle. Each rejection names the
    /// offending weapon. Called at asset-load (so a bad hot-reload/authoring slip is a hard load
    /// error, surfaced by `report_failed_spec`), and re-run by the schema test on the shipped sheet.
    ///
    /// The rejections and their failure modes:
    /// - `Automatic { belt_size: 0 }` — a permanently dry belt: the swap timer is only armed *inside*
    ///   `fire()`, which a dry belt blocks, so the weapon can never fire *or* swap. Bricked.
    /// - `Automatic { rpm: <= 0.0 }` — the cyclic interval is `60.0 / rpm`: `0.0` arms an infinite
    ///   (never-elapsing) reload, negative arms a nonsense one.
    /// - `Automatic { belt_swap_secs: < 0.0 }` / `Single { reload_secs: < 0.0 }` — a negative timer.
    ///
    /// Deliberately NOT rejected (documented so a future editor does not "tighten" them into bugs):
    /// - `Automatic { tracer_every: 0 }` — a legal tracerless "stealth belt" (spec doc + `tracer_round`
    ///   short-circuits on `0`, so there is no divide/modulo-by-zero); never traces, by design.
    /// - `belt_swap_secs == 0.0` / `reload_secs == 0.0` — a degenerate instant reload, not bricked
    ///   (the belt refills / the gun readies immediately); left legal.
    pub fn validate(&self) -> Result<(), BevyError> {
        // Track: values that parse but can never wrap a running gear. (The one check that needs
        // GEOMETRY — the material loop closing around the rest wheel circles — lives at rig
        // bind, where the baked wheel rests exist; these are the spec-local invariants.)
        let t = &self.track;
        for (field, value) in [
            ("pitch", t.pitch),
            ("width", t.width),
            ("thickness", t.thickness),
            ("link_mass", t.link_mass),
            ("plane_x", t.plane_x),
            ("idler.radius", t.idler.radius),
            ("wheel_radius", t.wheel_radius),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("track.{field} must be finite and > 0 (got {value})").into());
            }
        }
        if t.link_count < 3 {
            return Err(format!("track.link_count must be >= 3 (got {})", t.link_count).into());
        }
        if t.sprocket.teeth == 0 {
            return Err("track.sprocket.teeth must be > 0".into());
        }
        if t.wheel_radius <= t.thickness / 2.0 || t.idler.radius <= t.thickness / 2.0 {
            return Err(
                "track wheel/idler pin-line radii must exceed half the plate \
                        thickness (the rolling radius would be <= 0)"
                    .into(),
            );
        }
        if !t.max_link_angle.is_finite()
            || t.max_link_angle <= 0.0
            || t.max_link_angle > std::f32::consts::FRAC_PI_2
        {
            return Err(format!(
                "track.max_link_angle must be in (0, π/2] (got {})",
                t.max_link_angle
            )
            .into());
        }
        if !t.hinge_torque.is_finite() || t.hinge_torque < 0.0 {
            return Err(format!(
                "track.hinge_torque must be finite and >= 0 (got {})",
                t.hinge_torque
            )
            .into());
        }
        // The force-law scalars: each reaches an integrator division or clamp bound in
        // `track::forces` (engage/inertia divide; power/force/max_speed bound the engine
        // curve; a NaN in any of them dissolves the belt state in one tick).
        for (field, value) in [
            ("powertrain.max_speed", t.powertrain.max_speed),
            ("powertrain.power", t.powertrain.power),
            ("powertrain.force", t.powertrain.force),
            ("powertrain.governor_gain", t.powertrain.governor_gain),
            ("powertrain.inertia", t.powertrain.inertia),
            ("support.stiffness_per_m", t.support.stiffness_per_m),
            ("support.engage", t.support.engage),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("track.{field} must be finite and > 0 (got {value})").into());
            }
        }
        if !t.support.damping_per_m.is_finite() || t.support.damping_per_m < 0.0 {
            return Err(format!(
                "track.support.damping_per_m must be finite and >= 0 (got {})",
                t.support.damping_per_m
            )
            .into());
        }
        for (field, value) in [
            ("sprocket.center.0", t.sprocket.center.0),
            ("sprocket.center.1", t.sprocket.center.1),
            ("idler.center.0", t.idler.center.0),
            ("idler.center.1", t.idler.center.1),
        ] {
            if !value.is_finite() {
                return Err(format!("track.{field} must be finite (got {value})").into());
            }
        }
        // The declared transmission (optional block): values that parse but can never spin a
        // gearbox — empty ladders index out, a radii/gear length mismatch mis-keys the
        // steering table, a non-positive speed or radius reaches a division, and inverted
        // shift bands hunt on every tick.
        if let Some(tr) = &t.powertrain.transmission {
            let gb = &tr.gearbox;
            if gb.forward_speeds_kmh.is_empty() || gb.reverse_speeds_kmh.is_empty() {
                return Err("transmission.gearbox ladders must be non-empty".into());
            }
            if tr.steering.radii.len() != gb.forward_speeds_kmh.len() {
                return Err(format!(
                    "transmission.steering.radii must have one (tight, wide) pair per forward \
                     gear ({} pairs for {} gears)",
                    tr.steering.radii.len(),
                    gb.forward_speeds_kmh.len()
                )
                .into());
            }
            for (field, ok) in [
                (
                    "gearbox speeds",
                    gb.forward_speeds_kmh
                        .iter()
                        .chain(&gb.reverse_speeds_kmh)
                        .all(|v| v.is_finite() && *v > 0.0),
                ),
                (
                    "steering.radii",
                    tr.steering
                        .radii
                        .iter()
                        .all(|(a, b)| a.is_finite() && b.is_finite() && *a > 0.0 && *b > 0.0),
                ),
                (
                    "engine.torque_curve",
                    tr.engine.torque_curve.len() >= 2
                        && tr.engine.torque_curve.windows(2).all(|w| w[0].0 < w[1].0)
                        && tr.engine.torque_curve.iter().all(|(r, tq)| {
                            r.is_finite() && tq.is_finite() && *r > 0.0 && *tq >= 0.0
                        }),
                ),
                (
                    "engine rpms",
                    [
                        tr.engine.idle_rpm,
                        tr.engine.governed_rpm,
                        tr.engine.rated_rpm,
                    ]
                    .iter()
                    .all(|v| v.is_finite() && *v > 0.0),
                ),
                (
                    "gearbox shift bands",
                    gb.shift_up_rpm.is_finite()
                        && gb.shift_down_rpm.is_finite()
                        && gb.shift_down_rpm > 0.0
                        && gb.shift_down_rpm < gb.shift_up_rpm,
                ),
                (
                    // Ladders ascend (the shift logic assumes gear n+1 is faster) and fit
                    // the runtime's u8 gear index.
                    "gearbox ladder shape (ascending, u8-indexable)",
                    gb.forward_speeds_kmh.len() <= u8::MAX as usize
                        && gb.reverse_speeds_kmh.len() <= u8::MAX as usize
                        && gb.forward_speeds_kmh.windows(2).all(|w| w[0] < w[1])
                        && gb.reverse_speeds_kmh.windows(2).all(|w| w[0] < w[1]),
                ),
                (
                    // The documented hysteresis-by-construction condition, ENFORCED (not
                    // just down < up): a post-upshift rpm lands at `shift_up × v_g/v_g+1`,
                    // which must stay above the down band for EVERY adjacent pair in both
                    // ladders — otherwise an accepted sheet hunts up-down on a boundary
                    // speed (codex-5).
                    "gearbox shift-band hysteresis vs ratio steps",
                    gb.forward_speeds_kmh
                        .windows(2)
                        .chain(gb.reverse_speeds_kmh.windows(2))
                        .all(|w| gb.shift_up_rpm * w[0] / w[1] > gb.shift_down_rpm),
                ),
                (
                    // `is_finite` matters beyond > 0: an infinite brake capacity meets
                    // `0 × ∞ = NaN` in the engagement scaling before the clamp (codex-5).
                    "steering capacity/efficiency + brake_force",
                    tr.steering.capacity.is_finite()
                        && tr.steering.capacity > 0.0
                        && (0.0..=1.0).contains(&tr.steering.recirculation)
                        && tr.brake_force.is_finite()
                        && tr.brake_force > 0.0,
                ),
                (
                    // The compression-braking datum is a fraction of peak torque; the range
                    // check also rejects NaN/∞.
                    "engine.drag_fraction",
                    (0.0..=1.0).contains(&tr.engine.drag_fraction),
                ),
                (
                    // Bounded so the u8 tick countdown can represent it (255 ticks ≈ 4 s —
                    // far past any honest shift); the range check also rejects NaN/∞.
                    "gearbox.shift_secs",
                    (0.0..=3.0).contains(&gb.shift_secs),
                ),
            ] {
                if !ok {
                    return Err(format!("track.powertrain.transmission: invalid {field}").into());
                }
            }
        }
        for (name, weapon) in &self.weapons {
            match weapon.fire_mode {
                FireMode::Single { reload_secs } => {
                    if reload_secs < 0.0 {
                        return Err(format!(
                            "weapon `{name}`: Single.reload_secs must be >= 0 (got {reload_secs})"
                        )
                        .into());
                    }
                }
                FireMode::Automatic {
                    rpm,
                    belt_size,
                    belt_swap_secs,
                    tracer_every: _, // 0 is legal (tracerless stealth belt) — see the doc above.
                } => {
                    if belt_size == 0 {
                        return Err(format!(
                            "weapon `{name}`: Automatic.belt_size must be > 0 (a 0-round belt can \
                             never fire or swap)"
                        )
                        .into());
                    }
                    if rpm <= 0.0 {
                        return Err(format!(
                            "weapon `{name}`: Automatic.rpm must be > 0 (the cyclic interval is \
                             60/rpm; got {rpm})"
                        )
                        .into());
                    }
                    if belt_swap_secs < 0.0 {
                        return Err(format!(
                            "weapon `{name}`: Automatic.belt_swap_secs must be >= 0 (got \
                             {belt_swap_secs})"
                        )
                        .into());
                    }
                }
            }
        }
        Ok(())
    }
}

/// The handle to a tank's spec sheet, carried on its root entity so each tank knows its variant
/// (multi-variant ready). `spawn_tank` loads it alongside the model.
#[derive(Component)]
pub struct TankSpecHandle(pub Handle<TankSpec>);

/// Parses a `.tank.ron` file into a [`TankSpec`]. Tiny by design — the work is serde + RON.
#[derive(TypePath)]
struct TankSpecLoader;

impl AssetLoader for TankSpecLoader {
    type Asset = TankSpec;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<TankSpec, BevyError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let spec: TankSpec = ron::de::from_bytes(&bytes)?;
        // Past serde's shape check: reject values that parse but yield an unfirable weapon (ADR-0011).
        spec.validate()?;
        Ok(spec)
    }

    fn extensions(&self) -> &[&str] {
        &["tank.ron"]
    }
}

pub fn plugin(app: &mut App) {
    app.init_asset::<TankSpec>()
        .register_asset_loader(TankSpecLoader)
        .add_systems(Update, report_failed_spec);
}

/// Surface a failed spec-sheet load instead of swallowing it. The `.tank.ron` is required, in-repo
/// config with **no fallback** (ADR-0011): a competitive sim must never run on guessed stats, so a
/// parse/schema/IO error is fatal — we log the carried `AssetLoadError` and **panic in every
/// build**. (The schema test catches this class pre-ship; this is the runtime backstop for a bad
/// hot-reload or a file that slipped through.)
fn report_failed_spec(asset_server: Res<AssetServer>, tank: Query<&TankSpecHandle, With<Tank>>) {
    for handle in &tank {
        if let LoadState::Failed(err) = asset_server.load_state(&handle.0) {
            error!("required tank spec sheet failed to load: {err}");
            panic!("required tank spec sheet failed to load: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped spec sheet must always deserialize into `TankSpec`. This catches schema drift —
    /// a renamed/removed field, a changed type, a bad enum variant — at `cargo test` time, before
    /// the bad file ever ships (where `report_failed_spec` would catch it at runtime instead, but
    /// only after a player already has it). With `deny_unknown_fields`, a stray/typo'd key fails
    /// here too instead of being silently ignored.
    #[test]
    fn tiger_1_spec_sheet_matches_schema() {
        let ron = include_str!("../assets/tiger_1/tiger_1.tank.ron");
        let spec: TankSpec =
            ron::de::from_str(ron).expect("tiger_1.tank.ron must deserialize into TankSpec");
        // Spot-check values across sections so the test exercises real field wiring, not just "it
        // parsed".
        assert_eq!(spec.mass, 57000.0);
        assert_eq!(spec.inertia_extents, (3.0, 2.0, 6.3));
        // Grip-limit gearing rule (≈ μ·W/2; see the RON comment — the 100 kN placeholder cap
        // could not break a neutral steer under the element grip law).
        assert_eq!(spec.track.powertrain.force, 250_000.0);
        assert_eq!(spec.track.support.stiffness_per_m, 1_460_000.0);
        // The declared transmission block (phase 2.5 — DELIBERATE pin update with the new
        // powertrain field): the Tiger authors the L600 fixed-radius regenerative box from
        // the anchored tables (tiger-transmission-data.md). Spot-check the anchors: the 8F/4R
        // speed ladder ends at 45.4 km/h @ 3000, the radii table is anchored at both corners
        // (3.44 m F1-tight, 165 m F8-wide), and the fleet governor sits at 2500 rpm.
        let tr = spec
            .track
            .powertrain
            .transmission
            .as_ref()
            .expect("the Tiger authors a transmission block");
        assert_eq!(tr.architecture, TransmissionArchitecture::FixedRadii);
        assert_eq!(tr.engine.governed_rpm, 2500.0);
        assert_eq!(tr.engine.rated_rpm, 3000.0);
        assert_eq!(tr.gearbox.forward_speeds_kmh.len(), 8);
        assert_eq!(tr.gearbox.reverse_speeds_kmh.len(), 4);
        assert_eq!(*tr.gearbox.forward_speeds_kmh.last().unwrap(), 45.4);
        assert_eq!(tr.steering.radii[0].0, 3.44);
        assert_eq!(tr.steering.radii[7].1, 165.0);
        // DELIBERATE pin update (transmission fix 4 + review round, 2026-07-19):
        // brake_force re-anchored from the circular grip-limit sizing (250 kN — sized
        // against the very μ it was meant to test, and energy-impossible for two 1940s
        // discs) to the DUAL anchor: the settled 20° park hold (W·sin 20°/2 ≈ 95.6
        // kN/side) and 0.343 g total service decel (inside the 0.2–0.35 g WWII heavy-tank
        // band) → 96 kN/side.
        assert_eq!(tr.brake_force, 96_000.0);
        // Track: the material loop is authored exact (pitch × count = the immutable belt
        // length); the sprocket's tooth count locks link advance to tooth advance.
        assert_eq!(spec.track.pitch, 0.130);
        assert_eq!(spec.track.link_count, 97);
        assert_eq!(spec.track.sprocket.teeth, 19);
        assert_eq!(spec.track.plane_x, 1.4904);
        // Servos are a node-keyed map now (not fixed turret/gun fields); the yaw + pitch mounts must
        // be declared for the rig to bind.
        assert!(spec.servos.contains_key("Turret_Yaw"));
        assert!(spec.servos.contains_key("Main_Gun_Pitch"));
        // Weapons: the main gun's ballistics live in data now, with its muzzle/barrel node refs.
        assert_eq!(spec.weapons["MainGun"].muzzle, "Main_Gun_Muzzle");
        assert_eq!(
            spec.weapons["MainGun"].barrel.as_deref(),
            Some("Main_Gun_Recoil")
        );
        assert_eq!(spec.weapons["MainGun"].speed, 773.0);
        // Fire mechanisms: the 88 is single-shot with a crew-gated reload (and authors NO belt
        // fields — the enum makes that combo unrepresentable); the MGs are belt-fed automatics
        // with a one-in-five tracer belt.
        assert_eq!(
            spec.weapons["MainGun"].fire_mode,
            FireMode::Single { reload_secs: 3.0 }
        );
        let mg_mode = FireMode::Automatic {
            rpm: 750.0,
            belt_size: 150,
            belt_swap_secs: 3.5,
            tracer_every: 5,
        };
        assert_eq!(spec.weapons["Coax"].fire_mode, mg_mode);
        assert_eq!(spec.weapons["HullMG"].fire_mode, mg_mode);
        // Volumes: a steel-grade *module* (barrel) and a pure-armour plate (no hp) exercise the
        // composition facet — material decoupled from role.
        assert_eq!(spec.volumes["Gun_Barrel_Ballistic"].material_factor, 1000.0);
        assert_eq!(spec.volumes["Gun_Barrel_Ballistic"].hp, Some(8.0));
        assert_eq!(
            spec.volumes["Gun_Barrel_Ballistic"].function,
            Some(FunctionRole::GunBarrel)
        );
        assert_eq!(
            spec.volumes["Commander_Ballistic"].crew,
            Some(CrewStation::Commander)
        );
        assert!(spec.volumes["Ammo_L_0_Ballistic"].ammo);
        assert_eq!(spec.volumes["Hull_UFP_Ballistic"].hp, None);
        // Capability requirements: the flat RON shape deserializes into requirement groups. Drive =
        // [Driver, Engine, Transmission] (all mandatory `Single`s); Traverse = [Gunner]. Exercises
        // the `#[serde(untagged)]` bare-`Part` parse.
        use crate::damage::{Group, Part};
        assert_eq!(
            spec.capabilities[&Capability::Drive],
            vec![
                Group::Single(Part::Driver),
                Group::Single(Part::Engine),
                Group::Single(Part::Transmission),
            ]
        );
        // Fire/Load are no longer global capabilities — they're each weapon's own gates.
        assert_eq!(
            spec.weapons["MainGun"].fire,
            vec![
                Group::Single(Part::Gunner),
                Group::Single(Part::Breech),
                Group::Single(Part::GunBarrel),
            ]
        );
        assert_eq!(
            spec.weapons["MainGun"].load,
            vec![Group::Single(Part::Loader), Group::Single(Part::Breech)]
        );
        // Traverse is no longer a global capability — it's each servo's `requires` (slew gate).
        assert_eq!(
            spec.servos["Turret_Yaw"].requires,
            vec![Group::Single(Part::Gunner)]
        );
        // Views carry the camera FOV + their own gate — the per-view successors to the old
        // GunnerSight/CommanderView capabilities, which no longer exist on the global map.
        assert_eq!(spec.views[&ViewKind::Gunner].fov, 0.12);
        assert_eq!(
            spec.views[&ViewKind::Gunner].requires,
            vec![Group::Single(Part::Gunner)]
        );
    }

    /// The shipped sheet must pass semantic validation, not just parse — the CI-time twin of the
    /// load-time `validate()` gate.
    #[test]
    fn tiger_1_spec_passes_validation() {
        let spec: TankSpec = ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron"))
            .expect("tiger_1.tank.ron must parse");
        spec.validate()
            .expect("the shipped sheet must be semantically valid");
    }

    /// `validate()` rejects each silently-bricked `FireMode` value, and its error names the weapon.
    /// The legal edge cases (tracerless belt, instant reloads) must still pass. Guards ADR-0011's
    /// fail-fast: a weapon that parses but can never fire/cycle must be a hard load error, not a
    /// dead gun discovered mid-match.
    #[test]
    fn validate_rejects_bricked_fire_modes() {
        // Start from a valid shipped sheet, then swap in one bad weapon at a time.
        let with_weapon = |name: &str, mode: FireMode| {
            let mut spec: TankSpec =
                ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron")).unwrap();
            let mut w = spec.weapons["Coax"].clone();
            w.fire_mode = mode;
            spec.weapons.insert(name.to_string(), w);
            spec
        };

        // A 0-round belt: never fires or swaps.
        let bad = with_weapon(
            "Bricked",
            FireMode::Automatic {
                rpm: 750.0,
                belt_size: 0,
                belt_swap_secs: 3.5,
                tracer_every: 5,
            },
        );
        let err = bad.validate().unwrap_err().to_string();
        assert!(
            err.contains("Bricked") && err.contains("belt_size"),
            "{err}"
        );

        // rpm == 0.0: infinite cyclic interval.
        let err = with_weapon(
            "ZeroRpm",
            FireMode::Automatic {
                rpm: 0.0,
                belt_size: 150,
                belt_swap_secs: 3.5,
                tracer_every: 5,
            },
        )
        .validate()
        .unwrap_err()
        .to_string();
        assert!(err.contains("ZeroRpm") && err.contains("rpm"), "{err}");

        // Negative belt-swap timer.
        let err = with_weapon(
            "NegSwap",
            FireMode::Automatic {
                rpm: 750.0,
                belt_size: 150,
                belt_swap_secs: -1.0,
                tracer_every: 5,
            },
        )
        .validate()
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("NegSwap") && err.contains("belt_swap_secs"),
            "{err}"
        );

        // Negative single-shot reload.
        let err = with_weapon("NegReload", FireMode::Single { reload_secs: -0.5 })
            .validate()
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("NegReload") && err.contains("reload_secs"),
            "{err}"
        );

        // Track: a zero pitch parses but can never wrap a gear — validate() rejects it and
        // names the field (one representative case; the loop covers the whole dimension list).
        let mut spec: TankSpec =
            ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron")).unwrap();
        spec.track.pitch = 0.0;
        let err = spec.validate().unwrap_err().to_string();
        assert!(err.contains("track.pitch"), "{err}");

        // Force-law scalars: each mutation must be rejected BY NAME — a NaN or zero here
        // reaches a division in `track::forces` and dissolves the belt state in one tick.
        let fresh = || -> TankSpec {
            ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron")).unwrap()
        };
        let cases: [(&str, fn(&mut TankSpec)); 9] = [
            ("powertrain.max_speed", |s| {
                s.track.powertrain.max_speed = f32::NAN;
            }),
            ("powertrain.power", |s| s.track.powertrain.power = 0.0),
            ("powertrain.force", |s| s.track.powertrain.force = -1.0),
            ("powertrain.governor_gain", |s| {
                s.track.powertrain.governor_gain = 0.0;
            }),
            ("powertrain.inertia", |s| s.track.powertrain.inertia = 0.0),
            ("support.stiffness_per_m", |s| {
                s.track.support.stiffness_per_m = f32::INFINITY;
            }),
            ("support.engage", |s| s.track.support.engage = 0.0),
            ("support.damping_per_m", |s| {
                s.track.support.damping_per_m = -1.0;
            }),
            ("sprocket.center.0", |s| {
                s.track.sprocket.center.0 = f32::NAN;
            }),
        ];
        for (field, mutate) in cases {
            let mut spec = fresh();
            mutate(&mut spec);
            let err = spec.validate().unwrap_err().to_string();
            assert!(err.contains(field), "expected `{field}` in: {err}");
        }
        // Legal edge: zero damping (undamped support) is odd but not bricked.
        let mut spec = fresh();
        spec.track.support.damping_per_m = 0.0;
        assert!(spec.validate().is_ok());

        // Legal edges: a tracerless stealth belt (tracer_every: 0) and instant reloads pass.
        assert!(
            with_weapon(
                "Stealth",
                FireMode::Automatic {
                    rpm: 750.0,
                    belt_size: 150,
                    belt_swap_secs: 0.0,
                    tracer_every: 0,
                },
            )
            .validate()
            .is_ok(),
            "tracer_every: 0 is a legal tracerless belt; belt_swap_secs: 0 is a legal instant refill"
        );
        assert!(
            with_weapon("InstantReload", FireMode::Single { reload_secs: 0.0 })
                .validate()
                .is_ok(),
            "reload_secs: 0 is a legal instant reload"
        );
    }

    /// Transmission-block runtime invariants (codex-5): non-finite capacities NaN out the
    /// brake engagement scaling, hunting shift bands and unordered ladders break the shift
    /// logic's assumptions, and the u8 gear index must be able to address every gear. Each
    /// rejection is named; the shipped Tiger sheet passes (see
    /// `tiger_1_spec_passes_validation`).
    #[test]
    fn validate_rejects_broken_transmission_blocks() {
        let fresh = || -> TankSpec {
            ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron")).unwrap()
        };
        let cases: [(&str, fn(&mut TransmissionSpec)); 6] = [
            ("steering capacity", |tr| {
                tr.steering.capacity = f32::INFINITY;
            }),
            ("brake_force", |tr| tr.brake_force = f32::INFINITY),
            // Post-upshift rpm = 2300 × v_g/v_g+1 ≈ 1494 at the Tiger's widest step — a
            // 2200 down band re-downshifts immediately: hunting on a boundary speed.
            ("hysteresis", |tr| tr.gearbox.shift_down_rpm = 2200.0),
            ("ladder shape", |tr| {
                tr.gearbox.forward_speeds_kmh.swap(2, 3);
            }),
            // 300 ascending reverse gears: passes ordering and hysteresis, but cannot be
            // addressed by the runtime's u8 gear index.
            ("ladder shape", |tr| {
                tr.gearbox.reverse_speeds_kmh = (1..=300).map(|i| i as f32).collect();
            }),
            ("drag_fraction", |tr| tr.engine.drag_fraction = 1.5),
        ];
        for (needle, mutate) in cases {
            let mut spec = fresh();
            mutate(
                spec.track
                    .powertrain
                    .transmission
                    .as_mut()
                    .expect("the Tiger authors a transmission block"),
            );
            let err = spec.validate().unwrap_err().to_string();
            assert!(err.contains(needle), "expected `{needle}` in: {err}");
        }
    }

    /// The spec↔model **bind contract** — the CI-time twin of the runtime contract in
    /// the private tank assembler, but without launching Bevy: it reads glTF node names directly and
    /// checks both directions. Every node the spec references must exist in the `.glb`; the fixed
    /// structural nodes must be present; and every authored `*_Ballistic` node must be a declared
    /// volume (no orphans). This catches name drift — a rename, a typo, a forgotten declaration —
    /// before it ever reaches a runtime panic. Add a tank variant → add a case here.
    #[test]
    fn tiger_1_spec_binds_to_model() {
        use std::collections::HashSet;

        let gltf = gltf::Gltf::open("assets/tiger_1/tiger_1.glb").expect("tiger_1.glb must open");
        let nodes: HashSet<String> = gltf
            .nodes()
            .filter_map(|n| n.name().map(str::to_string))
            .collect();
        let spec: TankSpec = ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron"))
            .expect("tiger_1.tank.ron must parse");

        let has = |name: &str| {
            assert!(
                nodes.contains(name),
                "spec references node `{name}`, which is absent from the model"
            );
        };

        // Forward: every spec-declared node resolves to a model node.
        for servo in spec.servos.keys() {
            has(servo);
        }
        for weapon in spec.weapons.values() {
            has(&weapon.muzzle);
            if let Some(barrel) = &weapon.barrel {
                has(barrel);
            }
        }
        for volume in spec.volumes.keys() {
            has(volume);
        }
        for view in spec.views.values() {
            has(&view.node);
        }

        // Fixed structural contract mirrored from complete tank assembly.
        has("Hull");
        has("Center_Of_Mass");
        assert!(
            nodes.iter().any(|n| n.ends_with("_Collider")),
            "model has no `*_Collider` proxy"
        );
        let has_roadwheel = |side: &str| {
            nodes.iter().any(|n| {
                n.strip_prefix(side).is_some_and(|rest| {
                    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                })
            })
        };
        assert!(has_roadwheel("Wheel_L_"), "model has no left roadwheel");
        assert!(has_roadwheel("Wheel_R_"), "model has no right roadwheel");

        // Reverse: no orphan volumes — every authored `*_Ballistic` node is a declared volume.
        for node in &nodes {
            if node.ends_with("_Ballistic") {
                assert!(
                    spec.volumes.contains_key(node),
                    "model node `{node}` is named like a ballistic volume but has no spec entry"
                );
            }
        }
    }
}
