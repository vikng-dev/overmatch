//! Per-variant spec sheets as RON data assets (ADR-0010). The Blender model owns geometry and
//! spatial anchors; this owns the tuning numbers — mass + inertia, track powertrain/suspension, servo
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
use crate::track::transmission::{ShiftAddressing, TransmissionAuthoring, TransmissionParams};

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
    /// [`crate::tank::WeaponGateState::belt_remaining`]) over an INFINITE reserve (no stowed-ammo
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
    /// Vertical field of view, in RADIANS — the one spec angle not authored in degrees (contrast
    /// [`ServoSpec`], whose `_deg`-style authoring converts at this seam). That is deliberate and
    /// temporary: the field is expected to stop being an angle. The intent is ZOOM — a continuous
    /// fov slider for the third-person view, and DISCRETE magnification steps for the gunner sight
    /// (e.g. 4x / 8x), the way real optics work, so the sight's magnification is a property of the
    /// instrument rather than a free-floating number. Converting it to degrees first would be churn
    /// on a field that wants to become a different quantity; revisit with the sight/zoom work.
    pub fov: f32,
    #[serde(default)]
    pub requires: Requirement,
}

/// The continuous-track material loop + drivetrain (track architecture §7). Every field here is
/// authored vehicle DATA — link counts, link physicals, articulation limits, tooth count, and the
/// ride/powertrain models. It carries NO geometry: pitch, plate thickness, shoe width, the track
/// plane, and every running-gear circle (sprocket/idler/wheel) are MEASURED off the glb markers
/// (see [`crate::track::rig_geom`] / [`crate::track::marker_model`]) — one source of truth, in the
/// model. Solver-quality policy lives as constants in `track::view`; a new tracked vehicle is
/// authored here, never tuned there.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TrackSpec {
    /// Links per side. With the MEASURED pitch this is the immutable material loop
    /// (`pitch × link_count`, exact — tooth lock); the pitch itself is read off the glb markers.
    pub link_count: usize,
    /// One link assembly's mass (kg) — real inverse masses in the chain constraints.
    pub link_mass: f32,
    /// Pin dry-friction torque (N·m) — the rope-vs-track differentiator, scaled to link mass.
    pub hinge_torque: f32,
    /// Hard articulation stops between consecutive links (rad) — see [`LinkAngleSpec`].
    pub link_angle: LinkAngleSpec,
    /// Drive sprocket: tooth count ONLY. The pitch radius is DERIVED from the MEASURED pitch —
    /// `pitch × teeth / τ` — never authored: one link advance ≡ one tooth advance by
    /// construction, and two numbers that must agree are one number. Its centre (and every other
    /// running-gear circle) is measured off the glb markers (see [`crate::track::rig_geom`]).
    pub sprocket: SprocketSpec,
    /// The drivetrain spinning this track (phase B — the locomotion sim IS the track model).
    pub powertrain: PowertrainSpec,
    /// The ride model — the contact-envelope law derives its spring rate and damping from
    /// these three numbers, and its contact engage-ramp from `engage`.
    pub suspension: SuspensionSpec,
}

impl TrackSpec {
    /// Build the declared joint transmission through the same validated authoring seam used by
    /// spawn-time state construction and the runtime track gear. Geometry is no longer authored,
    /// so the CALLER supplies the measured sprocket pin-line radius and the half-tread (`plane_x`):
    /// [`crate::track::sim`]'s `init_track_gear` passes the
    /// [`RigGeom`](crate::track::rig_geom::RigGeom) values; the geometry-independent validators
    /// (asset-load `validate`, spawn-time state) pass nominal placeholders, because only
    /// `engine.idle_rpm` reaches replicated state — the geometry only SCALES the derived ladder,
    /// which is finite for any finite positive radius, so the validation verdict is identical.
    ///
    /// A MISSING transmission block is an error, not a silent Governor: every vehicle declares
    /// its architecture explicitly (`Governor` included — `transmission: (architecture:
    /// Governor)`). `Ok(None)` means the spec explicitly selected the tableless governor.
    pub(crate) fn transmission_params(
        &self,
        sprocket_radius: f32,
        half_tread: f32,
    ) -> Result<Option<TransmissionParams>, BevyError> {
        let Some(transmission) = self.powertrain.transmission.as_ref() else {
            return Err(
                "track.powertrain.transmission block is missing — every vehicle must declare \
                 its drivetrain architecture explicitly: `transmission: (architecture: \
                 Governor)` for the plain per-side governor, or architecture: Hybrid / \
                 FixedRadii with the full declared tables (a missing block used to silently \
                 select the governor; that fallback is retired)"
                    .into(),
            );
        };
        transmission.params(sprocket_radius, half_tread, self.powertrain.inertia)
    }
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
    /// gear ladders, steering table, brakes, architecture. REV 14 makes this architecture
    /// selection authoritative on MP paths. REQUIRED: `validate()` rejects a spec without it
    /// (the old absent-block-runs-the-governor fallback is retired — the governor is selected
    /// explicitly with `transmission: (architecture: Governor)`). Kept `Option` at the serde
    /// layer only so validation owns the error message and tests can strip it.
    #[serde(default)]
    pub transmission: Option<TransmissionSpec>,
}

/// Which drivetrain adapter the vehicle's declared transmission selects. The selection is
/// ALWAYS explicit: `validate()` rejects a spec without a transmission block, and the plain
/// governor is chosen by NAMING it (`transmission: (architecture: Governor)`) — never by
/// omission. (The old contract — omit the block, silently run the governor — is retired: a
/// forgotten block used to sim a different drivetrain than the author intended, silently.)
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransmissionArchitecture {
    /// The legacy symmetric per-side governor. It has NO declared tables — the block is just
    /// `(architecture: Governor)`; the `powertrain` scalars (power/force/gain/inertia) are the
    /// whole model. The honest WIP placeholder for a vehicle whose real transmission data has
    /// not been researched yet.
    Governor,
    /// Continuous regenerative hybrid (design menu C/D) — the arcade-honest default.
    Hybrid,
    /// Fixed-radius geared regenerative steering (the Tiger's L600, design menu B).
    FixedRadii,
}

impl TransmissionArchitecture {
    /// The runtime adapter this declared architecture selects — the ONE mapping between the
    /// authored spec value and [`TransmissionMode`], shared by the game (`track::sim`) and the
    /// sandbox so the two can never disagree on what a spec runs.
    pub(crate) fn mode(self) -> crate::track::transmission::TransmissionMode {
        use crate::track::transmission::TransmissionMode;
        match self {
            Self::Governor => TransmissionMode::Governor,
            Self::Hybrid => TransmissionMode::Hybrid,
            Self::FixedRadii => TransmissionMode::FixedRadii,
        }
    }
}

/// The declared drivetrain block. Authoring rule (tiger-transmission-data.md): per-gear
/// SPEEDS are the anchors; total reductions derive at build time against the spec's own
/// sprocket radius, so the ladder survives the open 19-vs-20-tooth sprocket discrepancy.
///
/// The table fields are `Option` ONLY so `architecture: Governor` (tableless by design) can be
/// authored without inventing fake engine data; [`Self::params`] enforces the per-architecture
/// contract loudly — regenerative architectures REQUIRE every table, Governor REJECTS any.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TransmissionSpec {
    pub architecture: TransmissionArchitecture,
    #[serde(default)]
    pub engine: Option<EngineSpec>,
    #[serde(default)]
    pub gearbox: Option<GearboxSpec>,
    #[serde(default)]
    pub steering: Option<SteeringSpec>,
    /// Per-side service/parking brake capacity at the sprocket (N).
    #[serde(default)]
    pub brake_force: Option<f32>,
    /// Static breakaway multiplier for an at-rest parking or hill-hold latch.
    #[serde(default)]
    pub brake_static_factor: Option<f32>,
}

impl TransmissionSpec {
    /// Adapt this RON shape into the shared validated authoring seam. Both asset validation and
    /// synchronous sim construction call this mapping, so the accepted domain cannot drift.
    ///
    /// Returns `Ok(None)` for `architecture: Governor` — the governor has no joint params by
    /// design. The per-architecture table contract is enforced HERE (the one seam every caller
    /// funnels through): a regenerative architecture missing a table fails by name, and a
    /// Governor block carrying tables fails too (authored-but-never-run data is exactly the
    /// silent-selection disease this contract exists to kill).
    pub(crate) fn params(
        &self,
        sprocket_radius_m: f32,
        half_tread_m: f32,
        belt_inertia: f32,
    ) -> Result<Option<TransmissionParams>, BevyError> {
        let arch = self.architecture;
        if arch == TransmissionArchitecture::Governor {
            let stray = [
                ("engine", self.engine.is_some()),
                ("gearbox", self.gearbox.is_some()),
                ("steering", self.steering.is_some()),
                ("brake_force", self.brake_force.is_some()),
                ("brake_static_factor", self.brake_static_factor.is_some()),
            ];
            if let Some((name, _)) = stray.iter().find(|(_, authored)| *authored) {
                return Err(format!(
                    "transmission.{name} is authored, but architecture: Governor has no tables \
                     — it would never run. Delete it, or declare Hybrid/FixedRadii"
                )
                .into());
            }
            return Ok(None);
        }
        fn required<'a, T>(
            field: &'a Option<T>,
            name: &str,
            arch: TransmissionArchitecture,
        ) -> Result<&'a T, BevyError> {
            field.as_ref().ok_or_else(|| {
                format!("transmission.{name} is required for architecture: {arch:?}").into()
            })
        }
        let engine = required(&self.engine, "engine", arch)?;
        let gearbox = required(&self.gearbox, "gearbox", arch)?;
        let steering = required(&self.steering, "steering", arch)?;
        let brake_force = *required(&self.brake_force, "brake_force", arch)?;
        let brake_static_factor =
            *required(&self.brake_static_factor, "brake_static_factor", arch)?;
        TransmissionParams::from_authoring(&TransmissionAuthoring {
            idle_rpm: engine.idle_rpm,
            governed_rpm: engine.governed_rpm,
            rated_rpm: engine.rated_rpm,
            torque_nm: &engine.torque_curve,
            forward_speeds_kmh: &gearbox.forward_speeds_kmh,
            reverse_speeds_kmh: &gearbox.reverse_speeds_kmh,
            shift_up_rpm: gearbox.shift_up_rpm,
            shift_down_rpm: gearbox.shift_down_rpm,
            steer_radii_m: &steering.radii,
            steer_capacity_n: steering.capacity,
            recirculation: steering.recirculation,
            brake_capacity_n: brake_force,
            brake_static_factor,
            drag_fraction: engine.drag_fraction,
            engine_inertia_kgm2: engine.inertia_kgm2,
            clutch_capacity_nm: engine.clutch_capacity_nm,
            belt_inertia,
            shift_secs: gearbox.shift_secs,
            shift_addressing: gearbox.shift_addressing,
            sprocket_radius_m,
            half_tread_m,
        })
        .map(Some)
    }
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
    /// Crank + flywheel + main-clutch rotational inertia J (kg·m²) — the stage-B engine
    /// crank state integrates against this. A declared transmission must declare its crank
    /// (no default): the coupling law divides by it every tick.
    pub inertia_kgm2: f32,
    /// Main clutch torque capacity (N·m) — the largest torque the engaged coupling
    /// transmits before slipping (≈ 1.3 × peak engine torque by the usual sizing rule).
    /// The stage-B coupling clamp; a torque-converter characteristic replaces the clamp
    /// for modern automatics later.
    pub clutch_capacity_nm: f32,
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
    /// Selection capability. Missing sheets default to [`ShiftAddressing::Sequential`]: paying
    /// one window per adjacent gear is the conservative behavior, while arbitrary selection is a
    /// vehicle capability that must be declared.
    #[serde(default)]
    pub shift_addressing: ShiftAddressing,
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

/// The ride model: three numbers, everything else derives. Spring rate `k = m·(2πf)²`,
/// static deflection `g/(2πf)²` (the droop/"green" envelope depth, chain-clamped by the link
/// window), damper `2ζ√(k·m)` — see `track::envelope::calibrate`.
#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct SuspensionSpec {
    /// Ride (bounce) natural frequency, Hz. Real tracked vehicles sit around 1–1.5 Hz.
    pub ride_frequency: f32,
    /// Suspension damping ratio ζ (dimensionless; 0.3–0.5 typical).
    pub damping_ratio: f32,
    /// Bump-stop travel above rest (m) — full compression, the red hard-stop pose.
    pub bump_stop: f32,
    /// Contact engage-ramp depth (m): the penetration over which a contact ramps from zero to
    /// full support — the anti-pop smoothing of the contact law (a contact-boundary policy, not
    /// a spring rate).
    pub engage: f32,
}

impl SuspensionSpec {
    /// The runtime parameter block (`track::derive::SuspensionParams`) — the ONE conversion
    /// seam, like the degree→radian rule: nothing downstream re-reads the spec.
    pub(crate) fn params(&self) -> crate::track::derive::SuspensionParams {
        crate::track::derive::SuspensionParams {
            ride_frequency: self.ride_frequency,
            damping_ratio: self.damping_ratio,
            bump_stop: self.bump_stop,
        }
    }
}

/// See [`TrackSpec::sprocket`]. Tooth count only — the sprocket's centre and pitch radius are
/// measured/derived off the glb markers, never authored.
#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct SprocketSpec {
    pub teeth: u32,
}

/// How far one pin joint may fold, per DIRECTION — a fact about the shoe, not a feel knob.
///
/// The limit is asymmetric because the shoe is: fold toward the wheels and the guide horns meet,
/// fold away and the ground-side structure does. Both are authored as POSITIVE MAGNITUDES named
/// for their direction rather than as a signed range, so no reader has to know (or guess) which
/// way a joint angle counts; a reader applies the sign its own belt orientation defines.
///
/// **Units.** DEGREES, which is why every field carries `_deg`: a hinge limit is measured in
/// Blender, in degrees, so degrees is what the RON authors. Any consumer converts at its own
/// boundary — there is none today (the stops were the deleted chain solver's; the measurement is
/// kept because it is a fact about the vehicle, not a solver knob), so nothing downstream holds
/// either form.
///
/// Hand-measured per vehicle, deliberately: the shoe MESH is only an upper bound (real
/// articulation also spends clearance in the pin/bushing and the end connectors), and an
/// automatic mesh sweep on the Tiger proved threshold-dependent on a shoe modelled with
/// near-zero clearance. See the RON's comment for the numbers and the rejected experiment.
#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct LinkAngleSpec {
    /// Folding TOWARD the wheels (DEGREES, positive): the wrap direction — every running-gear
    /// circle the belt bends around spends one fold of `2·asin(pitch / 2r)` here.
    pub inward_deg: f32,
    /// Folding AWAY from the wheels (DEGREES, positive): a sagging return run, a shoe cresting a
    /// rock.
    pub outward_deg: f32,
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
        if !t.link_mass.is_finite() || t.link_mass <= 0.0 {
            return Err(format!(
                "track.link_mass must be finite and > 0 (got {})",
                t.link_mass
            )
            .into());
        }
        if t.link_count < 3 {
            return Err(format!("track.link_count must be >= 3 (got {})", t.link_count).into());
        }
        if t.sprocket.teeth == 0 {
            return Err("track.sprocket.teeth must be > 0".into());
        }
        // Both hinge stops are MAGNITUDES, so both must be positive; 90° is the ceiling because a
        // joint that folds further has already buried the next shoe in this one on any real track.
        // Checked in the AUTHORED unit (degrees) so the error quotes what the author typed.
        for (field, value) in [
            ("link_angle.inward_deg", t.link_angle.inward_deg),
            ("link_angle.outward_deg", t.link_angle.outward_deg),
        ] {
            if !value.is_finite() || value <= 0.0 || value > 90.0 {
                return Err(format!(
                    "track.{field} must be a magnitude in (0, 90] degrees (got {value})"
                )
                .into());
            }
        }
        // The wrap direction is the roomier one on every track ever built — the belt has to fold
        // inward around its own sprocket, and nothing demands the outward fold at all. A spec
        // with these swapped is the signature of the sign confusion the named pair exists to
        // prevent, so it fails loudly here rather than kinking the chain in the view.
        if t.link_angle.outward_deg > t.link_angle.inward_deg {
            return Err(format!(
                "track.link_angle.outward_deg ({}) exceeds .inward_deg ({}) — inward is the WRAP \
                 direction and must be the roomier one; the pair is probably swapped",
                t.link_angle.outward_deg, t.link_angle.inward_deg
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
            ("suspension.ride_frequency", t.suspension.ride_frequency),
            ("suspension.damping_ratio", t.suspension.damping_ratio),
            ("suspension.bump_stop", t.suspension.bump_stop),
            ("suspension.engage", t.suspension.engage),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("track.{field} must be finite and > 0 (got {value})").into());
            }
        }
        // The declared transmission is validated through the same fallible constructor used by
        // synchronous sim construction, the sandbox, and its arithmetic tests. Geometry is measured
        // off the glb at RigGeom build, not authored here, so this domain-only pass feeds nominal
        // positive placeholders: every `from_authoring` rejection is independent of the sprocket
        // radius / half-tread (they only scale the derived ladder, finite for any finite positive
        // radius), and the geometry-coupled construction is re-run with the real measured values at
        // TrackGear build. This same call is the load-time gate that REQUIRES the block: a spec
        // without a transmission architecture fails here, loudly, instead of silently running
        // the governor.
        t.transmission_params(1.0, 1.0).map(|_| ())?;
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
        // The ride model the envelope law derives from (spring/damper are NOT authored).
        assert_eq!(spec.track.suspension.ride_frequency, 1.2);
        assert_eq!(spec.track.suspension.damping_ratio, 0.35);
        assert_eq!(spec.track.suspension.bump_stop, 0.200);
        assert_eq!(spec.track.suspension.engage, 0.02);
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
        // The regenerative tables are Option only for `architecture: Governor`'s sake — a
        // FixedRadii sheet must author all of them (params() enforces it; see
        // `governor_architecture_is_explicit_and_tableless`).
        let engine = tr
            .engine
            .as_ref()
            .expect("FixedRadii requires engine tables");
        let gearbox = tr.gearbox.as_ref().expect("FixedRadii requires a gearbox");
        let steering = tr.steering.as_ref().expect("FixedRadii requires steering");
        assert_eq!(engine.governed_rpm, 2500.0);
        assert_eq!(engine.rated_rpm, 3000.0);
        // DELIBERATE pin update (stage B, engine crank state): the crank block is now
        // required authoring. J = 4.0 kg·m² (INFERRED, 2.5–6 class band, flywheel-
        // dominant); clutch capacity 2400 N·m ≈ 1.3 × the 1850 N·m peak.
        assert_eq!(engine.inertia_kgm2, 4.0);
        assert_eq!(engine.clutch_capacity_nm, 2400.0);
        assert_eq!(gearbox.forward_speeds_kmh.len(), 8);
        assert_eq!(gearbox.reverse_speeds_kmh.len(), 4);
        assert_eq!(gearbox.shift_addressing, ShiftAddressing::Direct);
        assert_eq!(*gearbox.forward_speeds_kmh.last().unwrap(), 45.4);
        assert_eq!(steering.radii[0].0, 3.44);
        assert_eq!(steering.radii[7].1, 165.0);
        // DELIBERATE pin update (transmission fix 4 + review round, 2026-07-19):
        // brake_force re-anchored from the circular grip-limit sizing (250 kN — sized
        // against the very μ it was meant to test, and energy-impossible for two 1940s
        // discs) to the DUAL anchor: the settled 20° park hold (W·sin 20°/2 ≈ 95.6
        // kN/side) and 0.343 g total service decel (inside the 0.2–0.35 g WWII heavy-tank
        // band) → 96 kN/side.
        assert_eq!(tr.brake_force, Some(96_000.0));
        // Static breakaway is distinct from dynamic dissipation: the 1.5 INFERRED multiplier makes
        // 96 kN/side × 1.5 = 144 kN/side DERIVED, enough for the DERIVED 139.7925 kN/side demand
        // at 30°.
        assert_eq!(tr.brake_static_factor, Some(1.5));
        // Track: links per side (the material loop closes at MEASURED pitch × this count); the
        // sprocket's tooth count locks link advance to tooth advance. Geometry (pitch, plane_x,
        // sprocket/idler/wheel circles) is no longer authored — it is measured off the glb markers.
        assert_eq!(spec.track.link_count, 97);
        // 20 = the model-verified tooth count. Feel-neutral for the transmission: reductions
        // derive against the measured sprocket radius, so the radius cancels out of both
        // thrust (τ·gear/R) and per-gear speed (ω·R/gear) — the anchored speeds are the truth.
        assert_eq!(spec.track.sprocket.teeth, 20);
        // DELIBERATE pin (2026-07-23): the symmetric 0.6109 rad stop became a HAND-MEASURED
        // asymmetric pair authored in degrees. The measurement is a fact about the shoe, so it
        // stays authored (and validated) even though the solver that consumed it is gone.
        assert_eq!(spec.track.link_angle.inward_deg, 40.0);
        assert_eq!(spec.track.link_angle.outward_deg, 18.0);
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

    /// Older/unspecified vehicle sheets get the mechanically conservative crash-box behavior:
    /// one adjacent gear per paid interruption window. Arbitrary direct selection must be an
    /// explicit capability declaration.
    #[test]
    fn gearbox_shift_addressing_defaults_to_sequential() {
        let gearbox: GearboxSpec = ron::de::from_str(
            "(forward_speeds_kmh:[8.0,12.0],reverse_speeds_kmh:[8.0],\
             shift_up_rpm:1700.0,shift_down_rpm:900.0)",
        )
        .expect("a gearbox may omit the backward-compatible addressing field");
        assert_eq!(
            gearbox.shift_addressing,
            crate::track::transmission::ShiftAddressing::Sequential
        );

        let invalid = ron::de::from_str::<GearboxSpec>(
            "(forward_speeds_kmh:[8.0,12.0],reverse_speeds_kmh:[8.0],\
             shift_up_rpm:1700.0,shift_down_rpm:900.0,shift_addressing:Warp)",
        );
        assert!(
            invalid.is_err(),
            "the closed addressing enum must reject unknown vehicle capabilities"
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

        // Track: a non-positive link_mass parses but yields zero-inverse-mass chain constraints —
        // validate() rejects it and names the field (the one surviving track dimension check now
        // that all geometry is measured off the glb, not authored).
        let mut spec: TankSpec =
            ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron")).unwrap();
        spec.track.link_mass = 0.0;
        let err = spec.validate().unwrap_err().to_string();
        assert!(err.contains("track.link_mass"), "{err}");

        // Force-law scalars: each mutation must be rejected BY NAME — a NaN or zero here
        // reaches a division in `track::forces` and dissolves the belt state in one tick.
        let fresh = || -> TankSpec {
            ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron")).unwrap()
        };
        let cases: [(&str, fn(&mut TankSpec)); 6] = [
            ("powertrain.max_speed", |s| {
                s.track.powertrain.max_speed = f32::NAN;
            }),
            ("powertrain.power", |s| s.track.powertrain.power = 0.0),
            ("powertrain.force", |s| s.track.powertrain.force = -1.0),
            ("powertrain.governor_gain", |s| {
                s.track.powertrain.governor_gain = 0.0;
            }),
            ("powertrain.inertia", |s| s.track.powertrain.inertia = 0.0),
            ("suspension.engage", |s| s.track.suspension.engage = 0.0),
        ];
        for (field, mutate) in cases {
            let mut spec = fresh();
            mutate(&mut spec);
            let err = spec.validate().unwrap_err().to_string();
            assert!(err.contains(field), "expected `{field}` in: {err}");
        }

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
        // The table fields are Option (for `architecture: Governor`); the Tiger authors all of
        // them, so the mutation closures unwrap through these helpers.
        fn engine(tr: &mut TransmissionSpec) -> &mut EngineSpec {
            tr.engine.as_mut().expect("the Tiger authors engine tables")
        }
        fn gearbox(tr: &mut TransmissionSpec) -> &mut GearboxSpec {
            tr.gearbox.as_mut().expect("the Tiger authors a gearbox")
        }
        let cases: [(&str, fn(&mut TransmissionSpec)); 16] = [
            // Stage-B crank block: absurd-but-finite values must be refused outright, in
            // BOTH directions (review round FIX 4 added the lower bounds — the coupling
            // divides by J and the capacity gates every transmitted torque).
            ("inertia_kgm2", |tr| engine(tr).inertia_kgm2 = 0.0),
            ("inertia_kgm2", |tr| engine(tr).inertia_kgm2 = 0.05),
            ("inertia_kgm2", |tr| engine(tr).inertia_kgm2 = 250.0),
            ("clutch_capacity_nm", |tr| {
                engine(tr).clutch_capacity_nm = f32::NAN;
            }),
            ("clutch_capacity_nm", |tr| {
                engine(tr).clutch_capacity_nm = 50.0;
            }),
            ("clutch_capacity_nm", |tr| {
                engine(tr).clutch_capacity_nm = 80_000.0;
            }),
            // FIX 2/4: an idle under 300 rpm would put the sim's hard stall floor
            // (idle − 100) inside the spawn sentinel's territory.
            ("engine.idle_rpm floor", |tr| engine(tr).idle_rpm = 200.0),
            ("steering capacity", |tr| {
                tr.steering
                    .as_mut()
                    .expect("the Tiger authors steering")
                    .capacity = f32::INFINITY;
            }),
            ("brake_force", |tr| tr.brake_force = Some(f32::INFINITY)),
            ("brake_static_factor", |tr| {
                tr.brake_static_factor = Some(f32::NAN);
            }),
            ("brake_static_factor", |tr| {
                tr.brake_static_factor = Some(0.99)
            }),
            ("brake_static_factor", |tr| {
                tr.brake_static_factor = Some(2.51)
            }),
            // Post-upshift rpm = 2300 × v_g/v_g+1 ≈ 1494 at the Tiger's widest step — a
            // 2200 down band re-downshifts immediately: hunting on a boundary speed.
            ("hysteresis", |tr| gearbox(tr).shift_down_rpm = 2200.0),
            ("ladder shape", |tr| {
                gearbox(tr).forward_speeds_kmh.swap(2, 3);
            }),
            // 300 ascending reverse gears: passes ordering and hysteresis, but cannot be
            // addressed by the runtime's u8 gear index.
            ("ladder shape", |tr| {
                gearbox(tr).reverse_speeds_kmh = (1..=300).map(|i| i as f32).collect();
            }),
            ("drag_fraction", |tr| engine(tr).drag_fraction = 1.5),
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
        // A regenerative block missing one of its (serde-optional, Governor-only-optional)
        // tables must fail VALIDATION by name — the field parses as absent now, so the
        // per-architecture contract in `TransmissionSpec::params` is the gate.
        let missing_static_factor = include_str!("../assets/tiger_1/tiger_1.tank.ron")
            .replace("                brake_static_factor: 1.5,\n", "");
        let spec = ron::de::from_str::<TankSpec>(&missing_static_factor)
            .expect("a Governor-optional field may be absent at parse time");
        let err = spec.validate().unwrap_err().to_string();
        assert!(
            err.contains("brake_static_factor") && err.contains("FixedRadii"),
            "missing brake_static_factor should be named with its architecture: {err}"
        );
        // FIX 4, belt-inertia floor: with a transmission declared the coupling divides by
        // 2 × powertrain.inertia — a tiny-but-positive value passes the generic finite/> 0
        // check and must be caught by the transmission-block floor.
        let mut spec = fresh();
        spec.track.powertrain.inertia = 0.5;
        let err = spec.validate().unwrap_err().to_string();
        assert!(
            err.contains("powertrain.inertia floor"),
            "expected the belt-inertia floor in: {err}"
        );
    }

    /// A spec WITHOUT a transmission block must fail validation loudly — the old silent
    /// Governor fallback is retired. The error names the block and the fix.
    #[test]
    fn validate_requires_an_explicit_transmission_selection() {
        let mut spec: TankSpec =
            ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron")).unwrap();
        spec.track.powertrain.transmission = None;
        let err = spec.validate().unwrap_err().to_string();
        assert!(
            err.contains("track.powertrain.transmission")
                && err.contains("architecture")
                && err.contains("Governor"),
            "a missing transmission block must name the block and the explicit fix: {err}"
        );
    }

    /// The governor stays reachable ON PURPOSE: `transmission: (architecture: Governor)` is a
    /// legal, tableless block (Ok(None) params — no joint drivetrain), while a Governor block
    /// smuggling regenerative tables, or a regenerative block missing one, is rejected by name.
    #[test]
    fn governor_architecture_is_explicit_and_tableless() {
        let fresh = || -> TankSpec {
            ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron")).unwrap()
        };

        // The bare explicit-Governor block: parses, validates, selects no joint params.
        let block: TransmissionSpec = ron::de::from_str("(architecture: Governor)")
            .expect("a tableless Governor block must parse");
        let mut spec = fresh();
        spec.track.powertrain.transmission = Some(block);
        spec.validate()
            .expect("an explicit Governor selection is valid without tables");
        assert!(
            spec.track.transmission_params(1.0, 1.0).unwrap().is_none(),
            "Governor selects NO joint transmission params"
        );

        // Governor + authored tables = dead data hiding a selection bug — rejected by name.
        let mut spec = fresh();
        spec.track
            .powertrain
            .transmission
            .as_mut()
            .unwrap()
            .architecture = TransmissionArchitecture::Governor;
        let err = spec.validate().unwrap_err().to_string();
        assert!(
            err.contains("Governor") && err.contains("engine"),
            "authored tables under Governor must be rejected by name: {err}"
        );

        // A regenerative architecture missing a table is named with its architecture.
        let mut spec = fresh();
        spec.track.powertrain.transmission.as_mut().unwrap().engine = None;
        let err = spec.validate().unwrap_err().to_string();
        assert!(
            err.contains("transmission.engine") && err.contains("FixedRadii"),
            "a missing regenerative table must be named: {err}"
        );
    }

    /// Each declared architecture selects ITS OWN runtime adapter — the single shared mapping
    /// (`TransmissionArchitecture::mode`) the game and the sandbox both ride.
    #[test]
    fn each_architecture_selects_its_own_mode() {
        use crate::track::transmission::TransmissionMode;
        assert_eq!(
            TransmissionArchitecture::Governor.mode(),
            TransmissionMode::Governor
        );
        assert_eq!(
            TransmissionArchitecture::Hybrid.mode(),
            TransmissionMode::Hybrid
        );
        assert_eq!(
            TransmissionArchitecture::FixedRadii.mode(),
            TransmissionMode::FixedRadii
        );
    }

    /// The spec↔model **bind contract** — the CI-time twin of the runtime contract in
    /// the private tank assembler, but without launching Bevy: it reads glTF node names directly and
    /// checks both directions. Every node the spec references must exist in the `.glb`; the fixed
    /// structural nodes must be present; and every authored node the capture convention treats as a
    /// ballistic solid — `*_Ballistic`, plus the roadwheel stations, whose unified mesh makes each
    /// station its own armour volume — must be a declared volume (no orphans). This catches name
    /// drift — a rename, a typo, a forgotten declaration — before it ever reaches a runtime panic.
    /// Add a tank variant → add a case here.
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
        // `bake::roadwheel_side`'s rule, restated — this test reads the glTF directly (no Bevy, no
        // extractor), so the convention is mirrored rather than imported.
        let roadwheel_side = |name: &str| {
            ["Wheel_L_", "Wheel_R_"].into_iter().find(|side| {
                name.strip_prefix(side).is_some_and(|rest| {
                    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                })
            })
        };
        let has_roadwheel = |side: &str| nodes.iter().any(|n| roadwheel_side(n) == Some(side));
        assert!(has_roadwheel("Wheel_L_"), "model has no left roadwheel");
        assert!(has_roadwheel("Wheel_R_"), "model has no right roadwheel");

        // Reverse: no orphan volumes. A node is a ballistic solid if it is named `*_Ballistic` OR is
        // a roadwheel station — the wheels ship as ONE unified mesh per station (station and armour
        // in the same node), so their bare names carry geometry the march must see. Either way, an
        // authored solid with no spec entry is armour the game would never resolve.
        for node in &nodes {
            if node.ends_with("_Ballistic") || roadwheel_side(node).is_some() {
                assert!(
                    spec.volumes.contains_key(node),
                    "model node `{node}` is a ballistic solid but has no spec volume entry"
                );
            }
        }
    }
}
