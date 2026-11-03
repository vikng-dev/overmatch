//! Per-variant spec sheets as RON data assets (ADR-0010). The Blender model owns geometry and
//! spatial anchors; this owns the tuning numbers — mass + inertia, track powertrain/suspension, servo
//! configs — that differ per tank variant. A `.tank.ron` file deserializes (via serde) straight
//! into the same values the sim reads (`Mass`, `ForceParams`, `ServoSpec`), so
//! values stay plain-text, git-diffable, and separate from Blender. There are **no code defaults**
//! (ADR-0011): a competitive sim never runs on guessed stats. The shipped RON is embedded into the
//! eager `TankBlueprint` for simulation construction and also loaded as a Bevy asset for validation
//! and presentation diagnostics; simulation never reads the asset handle.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoadFailedEvent, AssetLoader, LoadContext, LoadState};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::damage::{
    Capability, ControlStaffing, CrewStation, FunctionRole, GradedGroup, Group, Member, Part,
    Requirement,
};
use crate::guns::{GunError, GunRegistry, Mechanism};
use crate::provenance::Sourced;
use crate::sim_tick::TickSpan;
use crate::tank::{ServoSpec, Tank};
use crate::track::transmission::{TransmissionAuthoring, TransmissionParams};

/// One declared armour node, keyed by model node name in [`TankSpec::volumes`]: main armour, external
/// armour, or a component with its facets.
///
/// **Membership is the declaration, behaviour is RON facets** (design
/// `armor-penetration-and-damage.md` §12). A node marches iff it is a key of that map; the material
/// a node wears never decides membership, it supplies the resistance of a declared one. The map is
/// the one home of the fact, so a node cannot be declared twice or in two roles.
///
/// Authored as one record: `(main: true)` is main armour, `(external: true)` is external armour,
/// and `(hp: …, crew: …, ammo: …, function: …)` is a component. An entry that is more than one of
/// these, or none, fails to parse ([`VolumeEntry`]).
#[derive(Deserialize, Clone)]
#[serde(try_from = "VolumeEntry")]
pub enum VolumeSpec {
    /// Main armour: resists a round and carries no facet. The pen cam's shot verdict counts
    /// perforating it.
    MainArmour,
    /// A component: the HP pool that makes the volume damageable, and its consequence facets.
    Component(ComponentSpec),
    /// Armour outside the main armour — skirts, screens, stand-off plates, appliqué, road wheels,
    /// tracks. It resists like any armour; the pen cam's shot verdict does not count perforating
    /// it. Spawned as `ballistics::ExternalArmour`.
    External,
}

impl VolumeSpec {
    /// The component facets, or `None` for main and external armour.
    pub fn component(&self) -> Option<&ComponentSpec> {
        match self {
            Self::Component(component) => Some(component),
            Self::MainArmour | Self::External => None,
        }
    }
}

/// The record a [`VolumeSpec`] is authored as.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VolumeEntry {
    #[serde(default)]
    main: bool,
    #[serde(default)]
    external: bool,
    #[serde(default, deserialize_with = "present")]
    hp: Option<u32>,
    #[serde(default)]
    crew: Option<CrewStation>,
    #[serde(default)]
    ammo: bool,
    #[serde(default)]
    function: Option<FunctionRole>,
}

/// `volumes` as a map that refuses a repeated key: a plain `HashMap` keeps the last of two
/// declarations of one node, silently dropping the first.
fn unique_volumes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<String, VolumeSpec>, D::Error> {
    struct Unique;
    impl<'de> serde::de::Visitor<'de> for Unique {
        type Value = HashMap<String, VolumeSpec>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a map of node name to volume entry")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut volumes = HashMap::new();
            while let Some((node, volume)) = map.next_entry::<String, VolumeSpec>()? {
                if volumes.insert(node.clone(), volume).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "volumes declares `{node}` twice"
                    )));
                }
            }
            Ok(volumes)
        }
    }
    deserializer.deserialize_map(Unique)
}

/// A field authored as a bare value, absent when not authored.
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    u32::deserialize(deserializer).map(Some)
}

const ARMOUR_WITH_FACETS: &str = "a main or external armour volume carries no component facet (hp, \
     crew, ammo, function) and is not both: a volume is main armour, external armour or a component";
const COMPONENT_WITHOUT_HP: &str = "a component volume needs an hp pool; main armour is \
     `(main: true)` and external armour is `(external: true)`";
const CREW_WITH_HP: &str = "a crew station authors no hp: every crew station of every tank has \
     the one shared crew pool (`crew_hp` in assets/ballistics/box_classes.ron)";

impl TryFrom<VolumeEntry> for VolumeSpec {
    type Error = String;

    /// A crew station's pool is the shared crew pool, filled in here; every other component's is
    /// the one its entry authors.
    fn try_from(entry: VolumeEntry) -> Result<Self, String> {
        let facets = entry.crew.is_some() || entry.ammo || entry.function.is_some();
        let hp = match (entry.main, entry.external, entry.hp, entry.crew) {
            (true, false, None, _) if !facets => return Ok(Self::MainArmour),
            (false, true, None, _) if !facets => return Ok(Self::External),
            (true, ..) | (_, true, ..) => return Err(ARMOUR_WITH_FACETS.into()),
            (false, false, Some(_), Some(_)) => return Err(CREW_WITH_HP.into()),
            (false, false, None, Some(_)) => {
                crate::ballistics::box_classes::BoxClasses::shipped().crew_hp()
            }
            (false, false, Some(hp), None) => hp,
            (false, false, None, None) => return Err(COMPONENT_WITHOUT_HP.into()),
        };
        Ok(Self::Component(ComponentSpec {
            hp,
            crew: entry.crew,
            ammo: entry.ammo,
            function: entry.function,
        }))
    }
}

/// One COMPONENT's facets. **Composition over a `kind` enum**: "is it crew?" means "does it have
/// the crew facet?", and a future consequence adds another optional field, never a central enum.
///
/// `material_factor` is GONE (slice 3): the numbers moved from per-node RON to the one substance
/// registry, keyed by the material datablock name the model already carries. "No numbers in the
/// model" survives — a plate's resistance is now a property of what it is made of, stated once.
#[derive(Clone)]
pub struct ComponentSpec {
    /// HP pool, in whole HP points. A hard box's line dose and a bounce's shock convert at the one
    /// energy-to-HP rate, `ballistics::energy_dose::joules_per_hp`; a field's deposit at its
    /// mechanism's unit, `ballistics::spall_cone::FieldUnit`; a crew station's chord dose (chord ×
    /// the round's crew line rate) at `ballistics::HP_PER_REFERENCE_MM`, 0.1 reference-mm a point. A
    /// crew station's pool is the shared crew pool (`ballistics::box_classes::BoxClasses::crew_hp`),
    /// never authored in a tank file; every other component's is authored. The consequences of the
    /// pool emptying come from the facets below.
    pub hp: u32,
    /// Crew station served by this volume.
    pub crew: Option<CrewStation>,
    /// Ammunition volume: HP depletion cooks off and kills all crew.
    pub ammo: bool,
    /// Repairable capability served by this module. Function loss is derived from HP.
    pub function: Option<FunctionRole>,
}

/// One roadwheel station: the model node, and which track it drives.
///
/// EXPLICIT, replacing the `Wheel_{L,R}_{n}` name pattern (§12 identity rule: names address, they
/// never classify). The declaration ORDER is load-bearing — it is the `WheelIndex` slot order both
/// wire ends derive — which is the second reason it is authored rather than derived-and-sorted: one
/// file states it, in order, and no reader has to reproduce a sort to agree.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RoadwheelSpec {
    pub node: String,
    pub side: crate::tank::TrackSide,
    /// The station's physical tread, independent of decorative view vertices.
    pub tread: WheelTread,
    /// Axial moment of the complete rotating station assembly (kg m^2), including every
    /// disc at this axle. DERIVED belt contribution: this moment / tread.radius^2.
    pub(crate) spin_inertia_kgm2: Sourced<f32>,
}

/// A wheel's contact circle and the lateral spans of its tread, relative to its authored axle.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WheelTread {
    pub radius: f32,
    pub spans: Vec<(f32, f32)>,
}

/// A scalar construction fact for each track side.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SideValues {
    pub left: f32,
    pub right: f32,
}

impl SideValues {
    pub(crate) fn get(self, side: crate::track::side::Side) -> f32 {
        match side {
            crate::track::side::Side::Left => self.left,
            crate::track::side::Side::Right => self.right,
        }
    }
}

/// One collision-proxy node and the substance (`assets/materials/materials.ron`) it is of: the row
/// whose friction its convex hulls carry from spawn, on the hull or on a launched turret alike.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ColliderSpec {
    pub node: String,
    pub substance: String,
}

/// Which fire input a weapon answers to (design: LMB = the main gun, Spacebar = the MGs). Pure fire
/// routing — it has *no* bearing on aiming or traverse (servos are weapon-agnostic). The `Primary`
/// weapon also supplies the rig's main-bore handles (its chain → `Rig.turret`/`gun`/`muzzle`).
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Primary,
    Secondary,
}

/// A projectile's nose geometry, parameters in calibres. [`Self::factor`] derives `N`, the
/// frictionless nose factor of the cavity-expansion form with the inertia constant taken as 1, in
/// `(0, 1]`. The round registry ([`crate::rounds`]) rejects a parameter outside its variant's range.
#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub enum NoseShape {
    /// Tangent ogive of `crh` calibre-radius-head, `crh ≥ 0.5` (0.5 is a hemisphere).
    Ogive { crh: f32 },
    /// Cone of `length_calibres` length, `> 0`.
    Cone { length_calibres: f32 },
    /// Spherical cap of `radius_calibres` radius, `≥ 0.5` (0.5 is a hemisphere).
    Cap { radius_calibres: f32 },
    /// Flat face.
    Flat,
}

impl NoseShape {
    /// The nose factor `N`: ogive `(8ψ − 1) / (24ψ²)`, cone `1 / (1 + 4φ²)`, cap `1 − 1 / (8φ²)`,
    /// flat `1`. Evaluated in f64 and cast once: the squares leave f32's range for a parameter
    /// past ~1e19, where the factor itself is still representable. The round registry rejects a factor
    /// the cast leaves subnormal or zero.
    pub fn factor(self) -> f32 {
        self.factor_f64() as f32
    }

    /// [`Self::factor`] before its cast, for a law that combines the factor with other terms in
    /// f64 and casts its own result once (the drag form, `ballistics::DragLaw::form`).
    pub(crate) const fn factor_f64(self) -> f64 {
        match self {
            Self::Ogive { crh } => {
                let crh = crh as f64;
                (8.0 * crh - 1.0) / (24.0 * crh * crh)
            }
            Self::Cone { length_calibres } => {
                let length = length_calibres as f64;
                1.0 / (1.0 + 4.0 * length * length)
            }
            Self::Cap { radius_calibres } => {
                let radius = radius_calibres as f64;
                1.0 - 1.0 / (8.0 * radius * radius)
            }
            Self::Flat => 1.0,
        }
    }

    /// The variant's parameter and whether it lies in the variant's range, or `None` for `Flat`.
    pub(crate) fn parameter(self) -> Option<(&'static str, f32, bool)> {
        match self {
            Self::Ogive { crh } => Some(("Ogive.crh (≥ 0.5)", crh, crh.is_finite() && crh >= 0.5)),
            Self::Cone { length_calibres } => Some((
                "Cone.length_calibres (> 0)",
                length_calibres,
                length_calibres.is_finite() && length_calibres > 0.0,
            )),
            Self::Cap { radius_calibres } => Some((
                "Cap.radius_calibres (≥ 0.5)",
                radius_calibres,
                radius_calibres.is_finite() && radius_calibres >= 0.5,
            )),
            Self::Flat => None,
        }
    }
}

/// A mount's half of its fire *mechanism* — the single-shot reload, or the belt. An enum, not
/// optional fields on `WeaponSpec`, so invalid combos are unrepresentable (ADR-0010/0011): the 88
/// cannot author a belt it never consults, an MG cannot omit its belt. Its family is the mount's
/// gun's [`Mechanism`], which carries the cyclic rate; [`WeaponSpec::fire_cycle`] joins the two.
/// Extensible by design — a future overheat model adds fields to `Automatic` (deferred).
#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub enum FireMode {
    /// One round per trigger *edge* (the click), then a crew-gated reload of `reload_secs` — the
    /// 88's mechanism.
    Single { reload_secs: f32 },
    /// Belt-fed fire on a held trigger *level*, at the gun's cyclic rate. The belt is finite
    /// (`belt_size` rounds, tracked as sim state in
    /// [`crate::tank::WeaponGateState::belt_remaining`]) over an INFINITE reserve (a stowage count
    /// is read by no reserve yet): running dry automatically starts a belt swap of
    /// `belt_swap_secs`, and the *swap* is what the weapon's `load` requirement gates, same as the
    /// 88's reload. The belt's tracer mix is the mount's [`WeaponSpecFile::tracer_every`].
    Automatic { belt_size: u32, belt_swap_secs: f32 },
}

/// A mount's fire mechanism joined with its gun's ([`WeaponSpec::fire_cycle`]): what the fire
/// system runs, parsed at the data edge and copied at spawn. The mount's fire mode and its
/// gun's mechanism are of one family by construction.
/// Its delays are fixed-tick spans, parsed once from the authored seconds
/// ([`crate::sim_tick::TickSpan::from_secs`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FireCycle {
    /// [`FireMode::Single`]: one round per trigger edge, then a crew-gated reload of
    /// `reload_secs`. Every round traces (its visual is the shell scene, not a streak).
    Single { reload: TickSpan },
    /// [`FireMode::Automatic`] at the gun's `cyclic_rpm`: one round every `60 / cyclic_rpm` s (pure
    /// mechanism, NEVER crew-gated: a knocked-out loader does not slow a working action),
    /// `belt_size` rounds a belt, a crew-gated swap of `belt_swap_secs` when it runs dry, and every
    /// `tracer_every`-th round down the belt a tracer (`0` = a tracerless belt, never traces).
    Automatic {
        cyclic: TickSpan,
        belt_size: u32,
        swap: TickSpan,
        tracer_every: u32,
    },
}

impl FireCycle {
    pub fn mechanism(self) -> FireMechanism {
        match self {
            Self::Single { .. } => FireMechanism::Single,
            Self::Automatic { .. } => FireMechanism::Automatic,
        }
    }
}

/// One round a mount stows: its key and how many the tank carries for the mount.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct StowageEntry {
    /// A key of the round registry (`assets/rounds/`) that the mount's gun chambers.
    pub round: String,
    /// Rounds carried, at least one. `validate()` reads it; nothing else does yet — the reserve is
    /// infinite.
    pub(crate) count: Sourced<u32>,
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

/// A mount whose fire mode and gun were joined at the data edge. Spawn copies `fire_cycle`;
/// authored seconds and the raw mode are held only by the deserializer's `WeaponSpecFile`.
#[derive(Clone)]
pub struct WeaponSpec {
    pub trigger: Trigger,
    pub muzzle: String,
    pub(crate) bore_length: Sourced<f32>,
    pub barrel: Option<String>,
    pub gun: String,
    pub stowage: Vec<StowageEntry>,
    pub fire_cycle: FireCycle,
    pub recoil: Option<RecoilSpec>,
    pub fire: Requirement,
    pub load: Requirement,
}

impl TryFrom<WeaponSpecFile> for WeaponSpec {
    type Error = String;
    fn try_from(file: WeaponSpecFile) -> Result<Self, Self::Error> {
        let fire_cycle = file.fire_cycle()?;
        Ok(Self {
            trigger: file.trigger,
            muzzle: file.muzzle,
            bore_length: file.bore_length,
            barrel: file.barrel,
            gun: file.gun,
            stowage: file.stowage,
            fire_cycle,
            recoil: file.recoil,
            fire: file.fire,
            load: file.load,
        })
    }
}

/// Keep the logical mount name on a refusal raised while joining its authored mode and gun.
fn deserialize_weapons<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<String, WeaponSpec>, D::Error> {
    crate::keyed_map::KeyedMap::<WeaponSpecFile>::deserialize(deserializer)?
        .0
        .into_iter()
        .map(|(name, file)| {
            WeaponSpec::try_from(file)
                .map(|weapon| (name.clone(), weapon))
                .map_err(|err| serde::de::Error::custom(format!("weapon `{name}`: {err}")))
        })
        .collect()
}

/// The authored mount at the deserialization edge; its durations leave this type as tick spans.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct WeaponSpecFile {
    /// Fire input this weapon answers to. The single `Primary` also marks the rig's main bore.
    /// Pure input *routing* (which command field the weapon reads) — the fire mechanism, and with
    /// it the edge-vs-level input semantics, comes from [`Self::fire_mode`].
    pub trigger: Trigger,
    /// Bore node — shot origin + direction.
    pub muzzle: String,
    /// The game's damage model: distance behind the muzzle to the bore's start (m), in the
    /// recoiling weapon's frame. Required construction data, finite and positive at load.
    pub(crate) bore_length: Sourced<f32>,
    /// Recoiling barrel node, if the weapon reciprocates; omitted → no recoil (e.g. a coax).
    #[serde(default)]
    pub barrel: Option<String>,
    /// The mount's gun: a key of the gun registry (`assets/guns/`), whose ammunition table gives
    /// every stowed round its muzzle speed, whose mechanism [`Self::fire_mode`] shares, and whose
    /// report the weapon plays.
    pub gun: String,
    /// The rounds the tank carries for this mount, in order, and how many of each, each fired from
    /// [`Self::gun`] ([`WeaponSpec::resolved_rounds`]). The first is the default — the round the weapon
    /// spawns loaded with and selected; the crew selects among them. `validate()` refuses an empty
    /// list, a round stowed twice, a count of zero, a round no registry carries or the gun does not
    /// chamber, and more than [`MAX_WEAPON_ROUNDS`] (a round is addressed by its `u8` index here,
    /// on the command and on the fire event).
    pub stowage: Vec<StowageEntry>,
    /// The mount's half of the fire mechanism: single-shot reload or belt. Required (no code
    /// default, ADR-0011) — a weapon with an unstated mechanism is an authoring omission.
    pub fire_mode: FireMode,
    /// A belt's composition, authored exactly when [`Self::fire_mode`] is `Automatic`: every
    /// `tracer_every`-th round down the belt traces (`5` = one in five, `0` = a tracerless belt).
    /// NOT a VFX knob — real belts are loaded e.g. four ball, one tracer.
    #[serde(default)]
    pub tracer_every: Option<u32>,
    /// Recoil spring, present iff `barrel` is. Authored alongside it.
    #[serde(default)]
    pub recoil: Option<RecoilSpec>,
    /// Fire gate (design §7b): what must be crewed/intact to fire — operator + ordnance (e.g. the
    /// main gun's `[Gunner, Breech, GunBarrel]`, a coax's `Backup(Gunner|Loader)`). Required: an
    /// authored `[]` is a weapon nothing gates, an omitted one is an authoring omission.
    pub fire: Requirement,
    /// Load gate: what must hold for the reload timer to tick (e.g. `[Loader, Breech]`). Required,
    /// as [`Self::fire`] is: an authored `[]` always loads.
    pub load: Requirement,
}

/// Every [`Part`] a requirement names, flattened through its graded groups.
fn requirement_parts(requirement: &Requirement) -> Vec<Part> {
    requirement
        .iter()
        .flat_map(|group| match group {
            Group::Single(part) => vec![*part],
            Group::Graded(GradedGroup::Pool(members) | GradedGroup::Backup(members)) => members
                .iter()
                .flat_map(|(_, parts)| parts.iter().copied())
                .collect(),
        })
        .collect()
}

/// Every graded member of a requirement, through its `Pool` and `Backup` groups.
fn requirement_members(requirement: &Requirement) -> impl Iterator<Item = &Member> {
    requirement.iter().flat_map(|group| match group {
        Group::Single(_) => [].as_slice(),
        Group::Graded(GradedGroup::Pool(members) | GradedGroup::Backup(members)) => {
            members.as_slice()
        }
    })
}

/// The most rounds one weapon can list: its round index travels as a `u8`.
pub(crate) const MAX_WEAPON_ROUNDS: usize = u8::MAX as usize + 1;

impl WeaponSpec {
    /// The key of the default round, the first stowed; `""` (no registry key) on an empty
    /// stowage, which `validate()` refuses.
    #[cfg(test)]
    pub(crate) fn default_round_key(&self) -> &str {
        self.stowage
            .first()
            .map_or("", |stowed| stowed.round.as_str())
    }

    /// The default round, fired from [`Self::gun`]; an unknown gun or round, or one the gun does not
    /// chamber, is an error naming it. On a validated spec this is `Ok`.
    #[cfg(test)]
    pub(crate) fn resolved_round(&self) -> Result<crate::rounds::Round, GunError> {
        GunRegistry::shipped().round(&self.gun, self.default_round_key())
    }

    /// Every stowed round, in order, fired from [`Self::gun`]; the first that resolves to no round
    /// is an error naming it. On a validated spec this is `Ok`.
    pub(crate) fn resolved_rounds(&self) -> Result<Vec<crate::rounds::Round>, GunError> {
        let guns = GunRegistry::shipped();
        self.stowage
            .iter()
            .map(|stowed| guns.round(&self.gun, &stowed.round))
            .collect()
    }

    /// Why the mount cannot spawn or fire, if it cannot: [`TankSpec::validate`]'s mount rules (the
    /// stowage, the recoil stop, the gun, the fire mode against the gun's mechanism, the tracer mix,
    /// the timers).
    fn checked(&self) -> Result<(), String> {
        if !self.bore_length.0.is_finite() || self.bore_length.0 <= 0.0 {
            return Err(format!(
                "bore_length must be finite and positive (got {}) — author the distance from the muzzle back to the bore start in metres",
                self.bore_length.0,
            ));
        }
        if self.stowage.is_empty() {
            return Err(
                "stowage is empty — a mount stows at least one round, the one it spawns \
                        loaded with"
                    .into(),
            );
        }
        if self.stowage.len() > MAX_WEAPON_ROUNDS {
            return Err(format!(
                "{} rounds stowed, at most {MAX_WEAPON_ROUNDS}",
                self.stowage.len()
            ));
        }
        if let Some((index, key)) = self
            .stowage
            .iter()
            .map(|stowed| &stowed.round)
            .enumerate()
            .find(|(index, key)| {
                self.stowage[..*index]
                    .iter()
                    .any(|earlier| &earlier.round == *key)
            })
        {
            return Err(format!(
                "round {key:?} is stowed twice (again at index {index}) — a selection steps through \
                 the stowage, so a duplicate is one round under two indices"
            ));
        }
        if let Some(stowed) = self.stowage.iter().find(|stowed| stowed.count.0 == 0) {
            return Err(format!(
                "round {:?} is stowed with a count of 0 — a stowed round is one the tank carries",
                stowed.round
            ));
        }
        if self.recoil.is_some() != self.barrel.is_some() {
            return Err(
                "recoil is authored exactly when barrel is: the spring drives the recoiling \
                 barrel, and a barrel with no spring never moves"
                    .into(),
            );
        }
        if let Some(recoil) = &self.recoil {
            let stop = recoil.max_length.0;
            if !stop.is_finite() || stop <= 0.0 {
                return Err(format!(
                    "recoil.max_length must be finite and > 0 m (got {stop}) — the stop bounds the \
                     barrel's stroke"
                ));
            }
            // The live spring integrates all three every tick (`shooting::apply_recoil`): a kick of
            // zero is a recoiling barrel that never moves (omit `barrel` instead), no stiffness
            // never returns it to battery, and a NaN in any of them is a NaN barrel pose.
            for (field, value, floor_is_legal) in [
                ("kick", recoil.kick, false),
                ("stiffness", recoil.stiffness, false),
                ("damping", recoil.damping, true),
            ] {
                if !value.is_finite() || value < 0.0 || (value == 0.0 && !floor_is_legal) {
                    let domain = if floor_is_legal { ">= 0" } else { "> 0" };
                    return Err(format!(
                        "recoil.{field} must be finite and {domain} (got {value})"
                    ));
                }
            }
        }
        let cycle = self.fire_cycle;
        self.resolved_rounds().map_err(|err| err.to_string())?;
        // A tracer_every of 0 is legal: a tracerless belt. Every delay was parsed by fire_cycle.
        if let FireCycle::Automatic { belt_size: 0, .. } = cycle {
            return Err(
                "Automatic.belt_size must be > 0 (a 0-round belt can never fire or swap)".into(),
            );
        }
        Ok(())
    }
}

impl WeaponSpecFile {
    /// The mount's fire mode joined with its gun's mechanism and its belt's tracer mix; an error
    /// naming what disagrees: a gun no registry carries, a fire mode of another family than the
    /// gun's mechanism, a `tracer_every` on a single-shot mount or none on a belt. On a validated
    /// spec this is `Ok`.
    fn fire_cycle(&self) -> Result<FireCycle, String> {
        let gun = GunRegistry::shipped()
            .gun(&self.gun)
            .map_err(|err| err.to_string())?;
        match (self.fire_mode, gun.mechanism(), self.tracer_every) {
            (FireMode::Single { reload_secs }, Mechanism::Single, None) => Ok(FireCycle::Single {
                reload: TickSpan::from_secs(reload_secs)
                    .map_err(|err| format!("Single.reload_secs: {err} (got {reload_secs})"))?,
            }),
            // The cyclic rate is the gun's, refused at the gun's load when it is not finite and
            // positive; its interval must still fit the deadline horizon.
            (
                FireMode::Automatic {
                    belt_size,
                    belt_swap_secs,
                },
                Mechanism::Automatic { cyclic_rpm },
                Some(tracer_every),
            ) => Ok(FireCycle::Automatic {
                cyclic: TickSpan::from_secs(60.0 / cyclic_rpm).map_err(|err| {
                    format!(
                        "assets/guns/{}.gun.ron: Automatic.cyclic_rpm {cyclic_rpm} gives an interval \
                         that {err}; increase cyclic_rpm",
                        self.gun
                    )
                })?,
                belt_size,
                swap: TickSpan::from_secs(belt_swap_secs).map_err(|err| {
                    format!("Automatic.belt_swap_secs: {err} (got {belt_swap_secs})")
                })?,
                tracer_every,
            }),
            (FireMode::Single { .. }, Mechanism::Single, Some(_)) => Err(
                "tracer_every is a belt's tracer mix, and a Single mount fires no belt".to_owned(),
            ),
            (FireMode::Automatic { .. }, Mechanism::Automatic { .. }, None) => Err(
                "tracer_every is missing — an Automatic mount's belt states its tracer mix (0 = a \
                 tracerless belt)"
                    .to_owned(),
            ),
            (mode, mechanism, _) => Err(format!(
                "fire_mode is {:?} and gun {:?} is {:?} — a mount's fire mode is of its gun's \
                 mechanism family",
                mode.mechanism(),
                self.gun,
                mechanism.family()
            )),
        }
    }
}

/// A weapon's procedural barrel recoil: a 1-DOF damped spring along the bore, between two stops.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RecoilSpec {
    /// Backward impulse on firing (m/s along −bore). Higher = harder, longer kick.
    pub kick: f32,
    /// Spring stiffness pulling the barrel back to battery. Lower = longer stroke + slower return.
    pub stiffness: f32,
    /// Damping; slightly underdamped lets the barrel lumber home with a small settle.
    pub damping: f32,
    /// The recoil stop: the farthest the barrel travels back from battery, in METRES, the buffer's
    /// mechanical limit. No run of kicks carries the barrel past it (`shooting::apply_recoil`), so
    /// `validate()` refuses a stop that is not finite and positive.
    pub max_length: Sourced<f32>,
}

/// A crew viewpoint — the camera/optic anchor. A closed set of kinds (each its own bespoke camera
/// behaviour in code), keyed in [`TankSpec::views`]; the *parameters* (which node, later FOV/zoom)
/// are data. The gunner's view node is also how the binder finds the gunner's chain for the rig.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ViewKind {
    Gunner,
    Commander,
}

impl ViewKind {
    pub const ALL: [Self; 2] = [Self::Gunner, Self::Commander];

    pub fn label(self) -> &'static str {
        match self {
            ViewKind::Gunner => "Gunner sight",
            ViewKind::Commander => "Commander view",
        }
    }
}

/// How a view's vertical field is authored — the two kinds of eye a crewman uses.
///
/// An OPTIC authors BOTH numbers off the instrument's data sheet: its magnification, which is the
/// instrument's identity and the seat a selectable ladder sits in, and the field it frames, which is
/// the camera's. Neither derives the other and nothing cross-checks them. A NAKED EYE has no
/// magnification to state, so it authors the field alone. No per-vehicle branch either way: which
/// variant a view uses is a property of the instrument the data names.
#[derive(Deserialize, Clone, Copy, Debug)]
pub enum Optics {
    /// A magnified instrument: its MAGNIFICATION (×) and the vertical field it frames in DEGREES.
    Magnified { magnification: f32, field_deg: f32 },
    /// The naked eye, by its vertical field in DEGREES.
    Unaided(f32),
}

impl Optics {
    /// The vertical field of view the camera frames (RADIANS) — where these authored degrees
    /// convert, once.
    pub const fn vertical_fov(self) -> f32 {
        match self {
            Optics::Magnified { field_deg, .. } | Optics::Unaided(field_deg) => {
                field_deg.to_radians()
            }
        }
    }

    /// Every authored magnitude with the unit it was typed in, so a rejection quotes the author's
    /// own number back at them rather than a derived one.
    fn authored(self) -> impl Iterator<Item = (f32, &'static str)> {
        match self {
            Optics::Magnified {
                magnification,
                field_deg,
            } => [
                Some((magnification, "magnification (×)")),
                Some((field_deg, "field (degrees)")),
            ],
            Optics::Unaided(field_deg) => [Some((field_deg, "field (degrees)")), None],
        }
        .into_iter()
        .flatten()
    }
}

/// One view's parameters: the model node the camera bolts to (which rides its servo's lay), the
/// [`Optics`] the crewman looks through, and the `requires` gate that decides whether the view is
/// usable (the slew/fire-gate grammar, evaluated against the controlled tank). `requires` is
/// required: an authored `[]` is a view always available, an omitted one an authoring omission.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ViewSpec {
    pub node: String,
    pub optics: Optics,
    pub requires: Requirement,
}

/// The track's declared construction: links, wheel moments, contact radii, pin planes,
/// articulation and tooth datums, plus ride and powertrain models. The sim artifact supplies
/// pin pitch, shoe dimensions and declared axle positions (see [`crate::track::rig_geom`]).
/// Solver resolution belongs to the implementation; physical facts belong to this sheet.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TrackSpec {
    /// Hull-local X placement of the common shoe template's pin line on each side (m).
    pub pin_plane_x: SideValues,
    /// The `assets/materials/materials.ron` row the links are of. Its friction is the track's side
    /// of every contact pair ([`Self::friction`]): the belt on the ground or a piece, and the bump
    /// stops and side plates. The bake refuses a row that states none (`L2.SPEC`).
    pub substance: String,
    /// Links per side. With the MEASURED pitch this is the immutable material loop
    /// (`pitch × link_count`, exact — tooth lock); the pitch itself is read off the glb markers.
    pub link_count: usize,
    /// One complete link assembly's mass (kg). DERIVED translating belt inertia per side:
    /// link_count * link_mass; the hull mass already includes the links' carried mass.
    pub(crate) link_mass: Sourced<f32>,
    /// Pin dry-friction torque (N·m) — the rope-vs-track differentiator, scaled to link mass.
    pub hinge_torque: f32,
    /// Hard articulation stops between consecutive links (rad) — see [`LinkAngleSpec`].
    pub link_angle: LinkAngleSpec,
    /// Drive sprocket construction. DERIVED pitch radius: pitch / (2 sin(pi / teeth)),
    /// from the common pin pitch; its centre is the declared axle node's position.
    pub sprocket: SprocketSpec,
    /// Idler: declared axle nodes, contact radii and complete rotating assembly moments.
    pub idler: IdlerSpec,
    /// The drivetrain spinning this track (phase B — the locomotion sim IS the track model).
    pub powertrain: PowertrainSpec,
    /// The ride model — the contact-envelope law derives its spring rate and damping from
    /// these three numbers, and its contact engage-ramp from `engage`.
    pub suspension: SuspensionSpec,
}

impl TrackSpec {
    /// The friction every surface of this track brings to a contact: its substance's row under the
    /// one pair rule. A failure is a bug: the bake refuses a sheet whose track substance the
    /// registry gives no friction for (`bake::collider_substances`).
    pub(crate) fn friction(&self) -> avian3d::prelude::Friction {
        crate::substances::SubstanceRegistry::shipped_static()
            .contact_friction(&self.substance)
            .unwrap_or_else(|err| panic!("the track's substance: {err}, yet the bake certified it"))
    }

    /// DERIVED per-side extra inertia at belt speed (kg): translating links plus each wheel's
    /// axial moment divided by the square of the radius the belt turns it at. The hull mass
    /// already carries these parts, so only their motion relative to the hull belongs here. The
    /// belt wraps the sprocket and the idler, which therefore turn with the pin line: the
    /// sprocket at its chord-exact pitch radius, the idler at its pin-line radius ([`WrapRadii`]).
    /// A road wheel rolls on the straight belly run's inner face, which moves at belt speed, so
    /// it uses its tread radius.
    ///
    /// The joint drive currently has one inertia for both sides. Unequal construction is refused
    /// before play until the drive can integrate distinct side inertias; it is never averaged
    /// into an unlabelled approximation. See the design record `reflected-track-inertia.md`.
    pub(crate) fn reflected_inertia(
        &self,
        roadwheels: &[RoadwheelSpec],
        wrap: WrapRadii,
    ) -> Result<f32, BevyError> {
        let sides = self.inertia_terms(roadwheels, wrap)?;
        // Resolution: allow accumulated f32 construction-rounding, including equivalent radius
        // and moment rescalings; this tolerance changes no physical term in the force law.
        let tolerance = 16.0 * f64::from(f32::EPSILON) * sides[0].max(sides[1]);
        if (sides[0] - sides[1]).abs() > tolerance {
            return Err(format!("track derived reflected inertia differs by side (left {}, right {} kg); correct the wheel moments/contact radii, or extend the joint drive to distinct side inertias", sides[0], sides[1]).into());
        }
        // DERIVED: average only the admitted sides' floating-point roundoff.
        let inertia = ((sides[0] + sides[1]) / 2.0) as f32;
        if !inertia.is_finite() || inertia <= 0.0 || !(2.0 * inertia).is_finite() {
            return Err("track derived reflected inertia and its pair must be finite and positive; reduce link mass/count or wheel moments, or correct contact radii".into());
        }
        Ok(inertia)
    }

    /// Admit each fact independently of the joint drive's symmetry and divisor constraints.
    /// Those constraints need the actual wrap radii, available at running-gear admission.
    fn inertia_terms(
        &self,
        roadwheels: &[RoadwheelSpec],
        wrap: WrapRadii,
    ) -> Result<[f64; 2], BevyError> {
        fn reflected(moment: f32, radius: f32, field: &str) -> Result<f64, BevyError> {
            if !moment.is_finite() || moment <= 0.0 {
                return Err(format!("{field} must be finite and positive; author the complete assembly's axial moment in kg m^2").into());
            }
            if !radius.is_finite() || radius <= 0.0 {
                return Err(format!("{field}: contact radius must be finite and positive; correct the declared tread or sprocket pin pitch").into());
            }
            Ok(f64::from(moment) / f64::from(radius).powi(2))
        }
        if !self.link_mass.0.is_finite() || self.link_mass.0 <= 0.0 {
            return Err("track.link_mass must be finite and positive; author the complete link assembly mass in kg".into());
        }
        let links = self.link_count as f64 * f64::from(self.link_mass.0);
        if !(2.0 * links as f32).is_finite() {
            return Err("track derived reflected inertia of the links must be finite; correct link_count and link_mass".into());
        }
        let mut sides = [links; 2];
        for side in crate::track::side::Side::ALL {
            sides[side.index()] += reflected(
                self.sprocket.spin_inertia_kgm2.0.get(side),
                wrap.sprocket,
                &format!("track.sprocket.spin_inertia_kgm2.{side:?}"),
            )? + reflected(
                self.idler.spin_inertia_kgm2.0.get(side),
                wrap.idler.get(side),
                &format!("track.idler.spin_inertia_kgm2.{side:?}"),
            )?;
        }
        for wheel in roadwheels {
            let side = match wheel.side {
                crate::tank::TrackSide::Left => 0,
                crate::tank::TrackSide::Right => 1,
            };
            sides[side] += reflected(
                wheel.spin_inertia_kgm2.0,
                wheel.tread.radius,
                &format!("roadwheels.{}.spin_inertia_kgm2", wheel.node),
            )?;
        }
        Ok(sides)
    }

    /// Structural admission and the spawn crank seed need no physical geometry. Unit placeholders
    /// exercise the transmission's non-geometric rules (the idler's declared contact radius
    /// stands for its pin-line radius, whose pin offset only the glb measures); no force step
    /// consumes these tables. Running-gear admission checks the side equality and divisor using
    /// the real wrap radii.
    pub(crate) fn nominal_transmission_params(
        &self,
        roadwheels: &[RoadwheelSpec],
    ) -> Result<TransmissionParams, BevyError> {
        let nominal = WrapRadii {
            sprocket: 1.0,
            idler: self.idler.radius,
        };
        self.inertia_terms(roadwheels, nominal)?;
        self.powertrain.transmission.params(1.0, 1.0, 1.0)
    }

    /// Build the joint drive from construction, the measured wrap radii and the measured
    /// half-tread. Only admitted running gear consumes these physical tables; structural
    /// validation and the crank seed use [`Self::nominal_transmission_params`].
    pub(crate) fn transmission_params(
        &self,
        roadwheels: &[RoadwheelSpec],
        wrap: WrapRadii,
        half_tread: f32,
    ) -> Result<TransmissionParams, BevyError> {
        self.powertrain.transmission.params(
            wrap.sprocket,
            half_tread,
            self.reflected_inertia(roadwheels, wrap)?,
        )
    }
}

/// The measured radii at which the belt turns the wheels it wraps (m). The sprocket turns at its
/// chord-exact pitch radius. The idler turns with the pin line wrapped around it, so at its
/// pin-line radius: its contact radius plus the shoe's measured `pin_to_inner`
/// (`track::derive::pin_line_radius`). Running-gear admission reads both off the rig's rest
/// circles (`track::rig_geom::RigGeom::wrap_radii`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct WrapRadii {
    pub(crate) sprocket: f32,
    pub(crate) idler: SideValues,
}

/// Per-track powertrain: the declared drivetrain that drives the constructed running gear.
/// Powered speed and force derive from its gear ladder, engine and clutch; contact grip bounds traction.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PowertrainSpec {
    /// The DECLARED transmission (transmission-design.md): engine torque curve, gear ladders,
    /// steering table, brakes.
    pub transmission: TransmissionSpec,
}

/// The declared drivetrain block: a fixed-radius geared regenerative box (the L600 family, the
/// one law `track::transmission` runs). Authoring rule (tiger-transmission-data.md): per-gear
/// SPEEDS are the anchors; total reductions derive at build time against the spec's own
/// sprocket radius, so the ladder survives the open 19-vs-20-tooth sprocket discrepancy.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TransmissionSpec {
    pub engine: EngineSpec,
    pub gearbox: GearboxSpec,
    pub steering: SteeringSpec,
    /// Per-side service/parking brake capacity at the sprocket (N).
    pub brake_force: f32,
    /// Static breakaway multiplier for an at-rest parking lever.
    pub brake_static_factor: f32,
}

impl TransmissionSpec {
    /// Adapt this RON shape into the shared validated authoring seam. Both asset validation and
    /// synchronous sim construction call this mapping, so the accepted domain cannot drift.
    pub(crate) fn params(
        &self,
        sprocket_radius_m: f32,
        half_tread_m: f32,
        belt_inertia: f32,
    ) -> Result<TransmissionParams, BevyError> {
        let Self {
            engine,
            gearbox,
            steering,
            brake_force,
            brake_static_factor,
        } = self;
        TransmissionParams::from_authoring(&TransmissionAuthoring {
            idle_rpm: engine.idle_rpm,
            governed_rpm: engine.governed_rpm,
            crank_limit_rpm: engine.crank_limit_rpm.0,
            rated_rpm: engine.rated_rpm,
            torque_nm: &engine.torque_curve,
            forward_speeds_kmh: &gearbox.forward_speeds_kmh,
            reverse_speeds_kmh: &gearbox.reverse_speeds_kmh,

            launch_gears: gearbox.launch_gears,
            max_step: gearbox.max_step,
            steer_radii_m: &steering.radii,
            reverse_steer_radii_m: &steering.reverse_radii,
            steer_capacity_n: steering.capacity,
            recirculation: steering.recirculation,
            brake_capacity_n: *brake_force,
            brake_static_factor: *brake_static_factor,
            drag_fraction: engine.drag_fraction,
            engine_inertia_kgm2: engine.inertia_kgm2,
            clutch_capacity_nm: engine.clutch_capacity_nm,
            belt_inertia,
            shift_secs: gearbox.shift_secs,

            sprocket_radius_m,
            half_tread_m,
        })
    }
}

/// The engine's declared envelope: a piecewise-linear torque curve under a fuel governor.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct EngineSpec {
    pub idle_rpm: f32,
    /// Fuel-governor rpm — the fleet operating condition (Tiger: 2500 from Nov 1943).
    pub governed_rpm: f32,
    /// Do-not-exceed crank speed, at or beyond completed fuel cut. The torque curve brackets
    /// governed speed but may end before or after this limit; its tail remains end-clamped.
    pub(crate) crank_limit_rpm: Sourced<f32>,
    /// Cylinder count (Tiger: the HL230 P45 is a V-12). Engine data like the rpm band beside it;
    /// the audio law reads it as the four-stroke firing rate `rpm × cylinders / 2 / 60`.
    pub cylinders: u32,
    /// The recording this engine runs on, and the measured rate that anchors it. Absent = a silent
    /// engine — the vehicle simply carries no loop.
    #[serde(default)]
    pub sound: Option<EngineSoundSpec>,
    /// The rpm the per-gear speed anchors are quoted at (Tiger: 3000).
    pub rated_rpm: f32,
    /// `(rpm, N·m)` authoring points, ascending rpm.
    pub torque_curve: Vec<(f32, f32)>,
    /// Zero-throttle engine drag (compression braking): the MID-BAND anchor of the sim's
    /// rising motoring-torque curve, as a fraction of peak torque — the motoring torque
    /// equals this fraction of peak at `(idle + governed)/2` rpm and grows linearly with
    /// crank speed (`track::transmission::engine_drag` — pumping/friction losses rise with
    /// speed). Engine data, required: each sheet states its value and its provenance.
    pub drag_fraction: f32,
    /// Crank + flywheel + main-clutch rotational inertia J (kg·m²) — the engine
    /// crank state integrates against this. A declared transmission must declare its crank
    /// (no default): the coupling law divides by it every tick.
    pub inertia_kgm2: f32,
    /// Main clutch torque capacity (N·m) — the largest torque the engaged coupling
    /// transmits before slipping (≈ 1.3 × peak engine torque by the usual sizing rule).
    pub clutch_capacity_nm: f32,
}

/// One engine's loop recording: the clip, and the cylinder-pop rate MEASURED off it. Both are facts
/// about the file — the playback-speed law that consumes them lives in `sfx`, and holds no tank
/// numbers of its own.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct EngineSoundSpec {
    /// Asset-relative path to the seamless loop.
    pub clip: String,
    /// Cylinder pops per second in the recording as it sits on disk (playback speed 1.0). MEASURED
    /// off the audio, never read off the filename.
    pub clip_pop_hz: f32,
}

/// The gear ladders as authored per-gear top belt speeds (km/h) at `rated_rpm`, plus what the
/// construction allows a Driver: the highest launch gear and the widest single shift.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct GearboxSpec {
    pub forward_speeds_kmh: Vec<f32>,
    pub reverse_speeds_kmh: Vec<f32>,
    pub launch_gears: u8,
    pub max_step: u8,
    /// Gear-shift torque-interruption time (s) — how long the drive is uncoupled through a
    /// shift. Vehicle data, required: a preselector and a crash box differ.
    pub shift_secs: f32,
}

/// The steering member: per-gear fixed radii (the L600's two detents), its force capacity, and the
/// regenerative power path.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SteeringSpec {
    /// Per forward gear `(R_tight, R_wide)` turn radii (m).
    pub radii: Vec<(f32, f32)>,
    /// Per reverse gear `(R_tight, R_wide)` turn radii (m).
    pub reverse_radii: Vec<(f32, f32)>,
    /// Steering-member force capacity PER OUTPUT (N): the member drives the two outputs
    /// differentially, so the belt-difference axis `F_s` carries up to 2× this (each side
    /// sees `F_s/2`, bounded by this datum — the gearing/grip-scale per-track cap).
    pub capacity: f32,
    /// Inner→outer recirculation efficiency η.
    pub recirculation: f32,
    // Neutral differential speed derives from first-gear construction:
    // `neutral_d_full = κ_tight(F1) × v1_governed`. An authored share would replace
    // the radii/speed relationship with an unprovenanced feel scalar.
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

/// See [`TrackSpec::sprocket`]. Tooth construction, axial moments and declared axle nodes;
/// the chord-exact pitch radius derives from pin markers and the tooth count.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SprocketSpec {
    /// Complete rotating assembly's axial moment per side (kg m^2). DERIVED belt contribution:
    /// I / pitch_radius^2, with the chord-exact pitch radius from the declared teeth and pin pitch.
    pub(crate) spin_inertia_kgm2: Sourced<SideValues>,
    pub teeth: u32,
    /// Hull side-plane angle of a reference tooth tip at the authored rest pose (rad).
    pub tooth_tip_rad: SideValues,
    /// The model nodes of the two sprocket meshes, one per side: the view nodes the belt's tooth
    /// lock turns.
    pub nodes: SideNodes,
}

/// See [`TrackSpec::idler`].
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct IdlerSpec {
    /// Complete rotating assembly's axial moment per side (kg m^2). DERIVED belt contribution:
    /// I / r_pin^2, at the pin-line radius the wrapped belt turns it at ([`WrapRadii`]).
    pub(crate) spin_inertia_kgm2: Sourced<SideValues>,
    pub nodes: SideNodes,
    /// Physical rolling radius of each idler (m).
    pub radius: SideValues,
}

/// The game's damage model (game state only): turret assembly and inertia data, shared by
/// powered traverse and the detached rigid body. Dimensions follow the servo role axes.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TurretSpec {
    /// Structural assembly root; None declares no detachable assembly, independently of optics.
    #[serde(deserialize_with = "deserialize_assembly")]
    pub node: Option<String>,
    /// Mass (kg) of the turret with everything it carries.
    pub mass: f32,
    /// Full box dimensions (x, y, z metres) approximating its angular-inertia distribution.
    pub inertia_extents: (f32, f32, f32),
}

// A required declaration may contain None; omitting the field must not invent that construction.
fn deserialize_assembly<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

/// One model node name per track side.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SideNodes {
    pub left: String,
    pub right: String,
}

impl SideNodes {
    /// The side index the track view uses, `0` left and `1` right; `None` for any other name.
    pub fn side_of(&self, name: &str) -> Option<usize> {
        if name == self.left {
            Some(0)
        } else if name == self.right {
            Some(1)
        } else {
            None
        }
    }
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
    /// The name a player reads for this vehicle: the kill feed's, the pen cam's.
    pub display_name: String,
    /// Total mass (kg) — authored balance data; the collision proxy contributes none (ADR-0011).
    pub mass: f32,
    /// Hull box full dimensions (x, y, z metres) approximating the angular-inertia distribution.
    pub inertia_extents: (f32, f32, f32),
    /// The turret as a free body once a cook-off blows it off: its own mass and inertia box, which
    /// the hull's `mass` / `inertia_extents` do not describe.
    pub turret: TurretSpec,
    /// Continuous-track running gear, material loop, powertrain, and contact law — the
    /// locomotion spec (phase B: the track model IS the driving sim) and the track view's
    /// per-vehicle data.
    pub track: TrackSpec,
    /// Servos (actuator mounts) keyed by model node name — the **source of truth** for which nodes
    /// rotate and how. Each carries its aim `role` (which also derives the rotation axis: Yaw→Y,
    /// Pitch→X) and slew tuning; tank construction resolves each name and binds the servo.
    /// Replaces the old fixed `turret`/`gun` fields, so a variant can declare any number of mounts.
    pub servos: HashMap<String, ServoSpec>,
    /// **Declared armour** keyed by model node name — the only nodes that are ballistic volumes,
    /// each as main armour, external armour or a component with its facets (design §12). A node
    /// wearing a registry substance and not keyed here is plain view geometry; a node keyed here
    /// wears registry substances only, and bind refuses a key naming no ballistic volume, since an
    /// entry the march never meets is an authoring error.
    #[serde(deserialize_with = "unique_volumes")]
    pub volumes: HashMap<String, VolumeSpec>,
    /// Collision-proxy nodes (convex hull, Vehicle layer), by node name, each with its substance.
    /// Explicit, replacing the `*_Collider` suffix scan. A proxy is a physics stand-in: its node is
    /// never also a key of `volumes`, since declared armour is charged by the march and a hull
    /// would charge it twice.
    pub colliders: Vec<ColliderSpec>,
    /// Shadow proxies, by node name. Beyond the tank's caster switch range each one casts the
    /// shadow of the rigid node it is parented under, in place of that node's drawn geometry
    /// (`tank::shadow_proxy`). Never drawn and never armour: a collider listed here keeps its
    /// collision role, and a node listed here alone is shadow-only and collides with nothing.
    /// Required; `[]` is a tank whose drawn geometry casts its own shadow at every range.
    pub shadow_proxies: Vec<String>,
    /// Roadwheel stations in wire-slot order. Explicit, replacing the `Wheel_{L,R}_{n}` pattern.
    pub roadwheels: Vec<RoadwheelSpec>,
    /// Weapons keyed by logical name — the **source of truth** for the tank's armament. Each names
    /// its bore (+ optional recoiling barrel) node and carries its ballistics/reload/recoil; the
    /// binder attaches a `Weapon` the shooting systems read. Replaces the hardcoded `shooting.rs`
    /// consts. Required; `{}` is an unarmed vehicle.
    #[serde(deserialize_with = "deserialize_weapons")]
    pub weapons: HashMap<String, WeaponSpec>,
    /// Crew viewpoints (camera/optic anchors) keyed by [`ViewKind`]. The gunner's also identifies
    /// the gunner's chain for the rig's main-bore handles. Every playable rig states both kinds;
    /// unarmed structural tooling fixtures may declare none.
    pub views: HashMap<ViewKind, ViewSpec>,
    /// Per-tank capability requirements (design §7b). Each capability maps to a list of requirement
    /// groups (AND'd): a bare `Part` is mandatory; `Pool(..)`/`Backup(..)` express graded redundancy.
    /// Read by the drive selector, `track::sim::drive_target`.
    /// Required; Drive must require Engine and Transmission on every productive path.
    pub capabilities: HashMap<Capability, Requirement>,
    /// The game's damage model: who may operate Drive controls, regardless of throughput.
    pub drive_controls: ControlStaffing,
}

/// What the sim does with a node the spec names — the role half of a typed node reference.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum NodeRole {
    /// An actuator mount.
    Servo,
    /// A declared armour node: main armour, external armour or a component.
    Volume,
    /// A collision proxy — a convex-hull source, never armour.
    Collider,
    /// A shadow proxy — a caster standing in for its parent's drawn geometry at range, never
    /// armour.
    ShadowProxy,
    /// A roadwheel station, which carries its own wheel mesh.
    Roadwheel,
    /// A weapon's bore or its recoiling barrel.
    Weapon,
    /// A crew viewpoint anchor.
    View,
    /// A structural assembly root, independent of aiming and optics.
    Assembly,
}

/// One typed node reference: what the sim does with the node, the node's name, and the RON path
/// the reference was AUTHORED at — the line a report sends a human to.
///
/// The field is carried per reference and not derived from the role, because a role does not
/// determine one: a weapon names its bore in `muzzle` and its recoiling barrel in `barrel`, and
/// both are the same role. Declaration order is sort order — role, then node, then the path inside
/// it — so a report groups by what a node is for and reads the same between runs.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct NodeReference<'a> {
    pub role: NodeRole,
    pub node: &'a str,
    pub field: String,
}

impl NodeRole {
    /// Whether `crate::tank::rig_world_pose` composes through a node in this role. That composition
    /// is rigid — position and rotation only — so such a node and every ancestor of it must be
    /// authored at unit scale or the sim and the view disagree about where the part is.
    pub(crate) fn rigid_pose(self) -> bool {
        match self {
            Self::Servo | Self::Roadwheel | Self::Weapon | Self::View | Self::Assembly => true,
            Self::Volume | Self::Collider | Self::ShadowProxy => false,
        }
    }
}

impl TankSpec {
    /// Every contact consumer receives authored finite dimensions and a finite tooth frame.
    fn check_running_gear_contacts(&self) -> Result<(), BevyError> {
        for (index, wheel) in self.roadwheels.iter().enumerate() {
            let tread = &wheel.tread;
            if !tread.radius.is_finite() || tread.radius <= 0.0 {
                return Err(format!("roadwheels[{index}].tread.radius must be finite and positive; author the contact radius in metres").into());
            }
            if tread.spans.is_empty()
                || tread
                    .spans
                    .iter()
                    .any(|&(lo, hi)| !lo.is_finite() || !hi.is_finite() || lo >= hi)
            {
                return Err(format!("roadwheels[{index}].tread.spans must contain finite increasing lateral intervals; author each tread's axle-relative bounds in metres").into());
            }
        }
        let planes = self.track.pin_plane_x;
        if !planes.left.is_finite()
            || !planes.right.is_finite()
            || planes.left >= planes.right
            || !(planes.right - planes.left).is_finite()
        {
            return Err("track.pin_plane_x must be finite with left < right; author the two hull-local pin planes in metres".into());
        }
        for side in crate::track::side::Side::ALL {
            let radius = self.track.idler.radius.get(side);
            if !radius.is_finite() || radius <= 0.0 {
                return Err(format!("track.idler.radius.{side:?} must be finite and positive; author the contact radius in metres").into());
            }
            if !self.track.sprocket.tooth_tip_rad.get(side).is_finite() {
                return Err(format!("track.sprocket.tooth_tip_rad.{side:?} must be finite; author the rest tooth datum in hull-side-plane radians").into());
            }
        }
        Ok(())
    }

    /// Why the sheet's weapons cannot be mounted, if they cannot: a weapon its own rules refuse
    /// ([`WeaponSpec::checked`]), or more than one `Primary`. Every consumer that resolves the main
    /// bore (`tank::spawn`'s rig, `command`, `crew_sweep`) takes the first `Primary` it finds, so a
    /// second one would be dropped unseen. A sheet with none is refused at spawn by the rig contract,
    /// which names it.
    fn check_armament(&self) -> Result<(), String> {
        for (name, weapon) in &self.weapons {
            weapon
                .checked()
                .map_err(|err| format!("weapon `{name}`: {err}"))?;
        }
        let mut primaries: Vec<&str> = self
            .weapons
            .iter()
            .filter(|(_, weapon)| weapon.trigger == Trigger::Primary)
            .map(|(name, _)| name.as_str())
            .collect();
        primaries.sort_unstable();
        if primaries.len() > 1 {
            return Err(format!(
                "weapons {primaries:?} are all `Primary`; one weapon supplies the main bore, so \
                 mark the others `Secondary`"
            ));
        }
        Ok(())
    }

    /// Why the sheet's volumes cannot serve its crew swaps and gates, if they cannot: two volumes
    /// seating one crew station, a requirement naming a part no volume provides, or a member
    /// coefficient outside (0, 1].
    fn check_seats_and_gates(&self) -> Result<(), String> {
        // A crew swap addresses a seat BY STATION on the wire (`command::CrewSwap::Start`), and both
        // the authority (`damage::apply_crew_swap_commands`) and the replica mirror
        // (`net::protocol::mirror_swap_from_net_crew`) resolve it to the FIRST seat wearing it: two
        // seats under one station are not separately addressable, and the two sides can resolve one
        // command to different seats. Duplicate FUNCTIONS carry no such vocabulary and stay legal —
        // `damage::part_facts` max-combines every provider of a role.
        let mut seats: std::collections::HashMap<CrewStation, &str> =
            std::collections::HashMap::new();
        for (node, volume) in &self.volumes {
            if let Some(seat) = volume.component().and_then(|component| component.crew)
                && let Some(first) = seats.insert(seat, node)
            {
                return Err(format!(
                    "volumes `{first}` and `{node}` both seat crew station {seat:?} — a swap \
                 addresses a seat by its station, so each station has one volume"
                ));
            }
        }
        // The game's damage model requires at least two crew in action to start a Battle.
        // Construction below that floor is an authoring fault, never a scored spawn knockout.
        if seats.len() < 2 {
            return Err(format!(
                "volumes.*.crew declares {} crew stations; a tank needs at least two — add \
                 volumes with distinct crew stations until at least two are declared",
                seats.len()
            ));
        }
        // Every gate closes over what the tank is built of: a Part a requirement names that no
        // declared volume provides (a crew facet or a function facet) is a quality nothing on this
        // tank can ever have, so the gate reads as permanently unmet.
        let provided: std::collections::HashSet<Part> = self
            .volumes
            .values()
            .filter_map(VolumeSpec::component)
            .flat_map(|volume| {
                volume
                    .crew
                    .map(Part::from)
                    .into_iter()
                    .chain(volume.function.map(Part::from))
            })
            .collect();
        if let ControlStaffing::Any(paths) = &self.drive_controls {
            if paths.is_empty() || paths.iter().any(Vec::is_empty) {
                return Err(
                    "drive_controls: author nonempty complete staffing paths, or use NotRequired"
                        .into(),
                );
            }
            for station in paths.iter().flatten() {
                if !seats.contains_key(station) {
                    return Err(format!(
                        "drive_controls names absent station {station:?}; declare its volume or remove the path"
                    ));
                }
            }
        }
        let mut requirements: Vec<(String, &Requirement)> = Vec::new();
        for (capability, requirement) in &self.capabilities {
            requirements.push((format!("capabilities[{capability:?}]"), requirement));
        }
        for (name, weapon) in &self.weapons {
            requirements.push((format!("weapons[\"{name}\"].fire"), &weapon.fire));
            requirements.push((format!("weapons[\"{name}\"].load"), &weapon.load));
        }
        for (node, servo) in &self.servos {
            requirements.push((format!("servos[\"{node}\"].requires"), &servo.requires));
        }
        for (kind, view) in &self.views {
            requirements.push((format!("views[{kind:?}].requires"), &view.requires));
        }
        for (field, requirement) in &requirements {
            for (index, group) in requirement.iter().enumerate() {
                if let Group::Graded(GradedGroup::Pool(members) | GradedGroup::Backup(members)) =
                    group
                    && members.is_empty()
                {
                    return Err(format!(
                        "{field}[{index}].members is empty — author at least one member, or remove the group for an ungated requirement"
                    ));
                }
            }
            if let Some(part) = requirement_parts(requirement)
                .into_iter()
                .find(|part| !provided.contains(part))
            {
                return Err(format!(
                    "{field} requires {part:?}, which no volume provides — declare the volume \
                 with that crew or function facet, or drop it from the requirement"
                ));
            }
            // A member's coefficient is its share (`Pool`) or ceiling (`Backup`) of full capability,
            // and every consumer reads a gate's effectiveness as a fraction of full.
            let is_share = |coefficient: f32| coefficient > 0.0 && coefficient <= 1.0;
            if let Some((coefficient, _)) =
                requirement_members(requirement).find(|(coefficient, _)| !is_share(*coefficient))
            {
                return Err(format!(
                    "{field} has a member coefficient of {coefficient}, but a member's share or \
                     ceiling is a fraction of full capability, in (0, 1] — author it there"
                ));
            }
        }
        // The game's driving and damage model reads both source conditions independently.
        // A productive path cannot bypass either source and conceal it from the task rate.
        let drive = &self.capabilities[&Capability::Drive];
        for source in [Part::Engine, Part::Transmission] {
            let mandatory = drive.iter().any(|group| match group {
                Group::Single(part) => *part == source,
                Group::Graded(GradedGroup::Pool(members) | GradedGroup::Backup(members)) => {
                    !members.is_empty() && members.iter().all(|(_, parts)| parts.contains(&source))
                }
            });
            if !mandatory {
                return Err(format!(
                    "capabilities[Drive] must require {source:?} on every productive path — add it as a mandatory group or to every member of a required Pool/Backup group"
                ));
            }
        }
        // The game's damage model: load deadlines must fit even at the smallest positive
        // module pool share or foreign-station competence. An emptied contributor may disappear,
        // so each graded group's floor uses its smallest member, not their full sum or maximum.
        for (name, weapon) in &self.weapons {
            let part_floor = |part: Part| {
                self.volumes
                    .values()
                    .filter_map(VolumeSpec::component)
                    .filter_map(|volume| {
                        if volume.crew.map(Part::from) == Some(part) {
                            let station = volume.crew.expect("the matching crew facet is present");
                            Some(
                                self.volumes
                                    .values()
                                    .filter_map(VolumeSpec::component)
                                    .filter_map(|volume| volume.crew)
                                    .map(|home| crate::damage::competence(home, station))
                                    .fold(1.0_f32, f32::min),
                            )
                        } else if volume.function.map(Part::from) == Some(part) {
                            Some(
                                1.0 / crate::ballistics::ComponentHealth::full(volume.hp).capacity()
                                    as f32,
                            )
                        } else {
                            None
                        }
                    })
                    .fold(1.0_f32, f32::min)
            };
            let floor = weapon
                .load
                .iter()
                .map(|group| match group {
                    Group::Single(part) => part_floor(*part),
                    Group::Graded(GradedGroup::Pool(members) | GradedGroup::Backup(members)) => {
                        members
                            .iter()
                            .map(|(coefficient, parts)| {
                                parts
                                    .iter()
                                    .fold(*coefficient, |rate, part| rate * part_floor(*part))
                            })
                            .fold(1.0_f32, f32::min)
                    }
                })
                .fold(1.0_f32, f32::min);
            let delay = match weapon.fire_cycle {
                FireCycle::Single { reload } => reload,
                FireCycle::Automatic { swap, .. } => swap,
            };
            if delay.raw() > 0
                && ((f64::from(delay.raw()) / f64::from(floor)).ceil() > f64::from(TickSpan::MAX)
                    || floor == 0.0)
            {
                return Err(format!(
                    "weapons[\"{name}\"].load: base work / minimum positive effectiveness exceeds the signed deadline horizon; shorten reload/swap, reduce module pools or raise requirement coefficients"
                ));
            }
        }
        Ok(())
    }

    /// Every model node this sheet names, with the role it names it in — the canonical reference
    /// list, so no consumer (the bake gate, the source lint, a future editor) maintains a second
    /// vocabulary of RON field names. Sorted by role then name: the maps are unordered, and a
    /// report that reorders between runs is one nobody can diff.
    pub fn node_references(&self) -> Vec<NodeReference<'_>> {
        let mut references: Vec<NodeReference<'_>> = Vec::new();
        fn at(role: NodeRole, node: &str, field: String) -> NodeReference<'_> {
            NodeReference { role, node, field }
        }
        // `servos` and `volumes` are keyed BY the node name and `colliders` and `shadow_proxies`
        // are lists of them, so the field alone names the entry; the rest hold the name inside a
        // keyed or indexed one.
        references.extend(
            self.servos
                .keys()
                .map(|node| at(NodeRole::Servo, node, "servos".to_owned())),
        );
        references.extend(
            self.volumes
                .keys()
                .map(|node| at(NodeRole::Volume, node, "volumes".to_owned())),
        );
        references.extend(
            self.colliders
                .iter()
                .map(|collider| at(NodeRole::Collider, &collider.node, "colliders".to_owned())),
        );
        references.extend(
            self.shadow_proxies
                .iter()
                .map(|node| at(NodeRole::ShadowProxy, node, "shadow_proxies".to_owned())),
        );
        references.extend(self.roadwheels.iter().enumerate().map(|(index, wheel)| {
            at(
                NodeRole::Roadwheel,
                &wheel.node,
                format!("roadwheels[{index}].node"),
            )
        }));
        for (name, weapon) in &self.weapons {
            references.push(at(
                NodeRole::Weapon,
                &weapon.muzzle,
                format!("weapons[\"{name}\"].muzzle"),
            ));
            if let Some(barrel) = weapon.barrel.as_deref() {
                references.push(at(
                    NodeRole::Weapon,
                    barrel,
                    format!("weapons[\"{name}\"].barrel"),
                ));
            }
        }
        references.extend(
            self.views.iter().map(|(kind, view)| {
                at(NodeRole::View, &view.node, format!("views[{kind:?}].node"))
            }),
        );
        if let Some(node) = &self.turret.node {
            references.push(at(NodeRole::Assembly, node, "turret.node".into()));
        }
        references.sort_unstable();
        references.dedup();
        references
    }

    /// Every node the sheet moves relative to its parent: servo frames, recoiling barrels and
    /// roadwheel stations. A rigid stand-in parented above one of them cannot follow it.
    pub fn articulated_nodes(&self) -> impl Iterator<Item = &str> {
        self.servos
            .keys()
            .map(String::as_str)
            .chain(self.weapons.values().filter_map(|w| w.barrel.as_deref()))
            .chain(self.roadwheels.iter().map(|wheel| wheel.node.as_str()))
    }

    /// Fail-fast semantic validation past what serde's shape check catches (ADR-0011: a competitive
    /// sim never runs on silently-bricked stats). serde proves the *fields* exist and typecheck; this
    /// proves the *values* yield a weapon that can actually fire and cycle. Each rejection names the
    /// offending weapon. Called at asset-load (so a bad hot-reload/authoring slip is a hard load
    /// error, surfaced by `report_failed_spec`), and re-run by the schema test on the shipped sheet.
    ///
    /// The rejections and their failure modes:
    /// - `Automatic { belt_size: 0 }` — a permanently dry belt: the swap timer is only armed *inside*
    ///   `fire()`, which a dry belt blocks, so the weapon can never fire *or* swap. Bricked.
    /// - Negative or unrepresentable authored delays are refused while deserializing `WeaponSpec`.
    /// - A mount whose fire mode and its gun's mechanism disagree (a belt on a single-shot gun, a
    ///   `tracer_every` on a single-shot mount, none on a belt): `fire_cycle` refuses the pair.
    /// - A recoil stop (`RecoilSpec::max_length`) that is not finite and positive: a NaN stop
    ///   compares false and never holds the barrel, and one at or ahead of battery leaves no stroke.
    /// - A recoil spring the live barrel cannot integrate: a kick or stiffness that is not finite
    ///   and positive, a damping that is not finite and non-negative.
    /// - A reload or belt swap reaches validation only after parsing into a checked tick span.
    /// - Hull or turret body data (mass, inertia extents) that is not finite and positive.
    ///
    /// The gun's own numbers are the gun registry's to refuse (`crate::guns`): a cyclic rate that is
    /// not finite and positive never loads, so `60 / cyclic_rpm` is always a real interval here.
    ///
    /// Deliberately NOT rejected (documented so a future editor does not "tighten" them into bugs):
    /// - `tracer_every: 0` on a belt mount — a legal tracerless "stealth belt" (`tracer_round`
    ///   short-circuits on `0`, so there is no divide/modulo-by-zero); never traces, by design.
    /// - `belt_swap_secs == 0.0` / `reload_secs == 0.0` — a degenerate instant reload, not bricked
    ///   (the belt refills / the gun readies immediately); left legal.
    pub fn validate(&self) -> Result<(), BevyError> {
        // DERIVED: the wire addresses weapon slots with a u8, including slot zero.
        if self.weapons.len() > u8::MAX as usize + 1 {
            return Err(
                "weapons: at most 256 slots fit the wire; remove excess weapon entries".into(),
            );
        }
        // Components: an HP pool of zero is a component that is dead on arrival, and one past
        // `MAX_POOL_POINTS` is one no pool holds in pool units.
        for (node, volume) in &self.volumes {
            let Some(volume) = volume.component() else {
                continue;
            };
            if volume.hp == 0 {
                return Err(format!("component `{node}`: hp must be > 0 (got 0)").into());
            }
            if volume.hp > crate::ballistics::MAX_POOL_POINTS {
                return Err(format!(
                    "component `{node}`: hp must be at most {} (got {})",
                    crate::ballistics::MAX_POOL_POINTS,
                    volume.hp
                )
                .into());
            }
        }
        for (node, servo) in &self.servos {
            if let Some(fault) = servo.admission_fault() {
                return Err(format!("servo `{node}`: {fault}").into());
            }
        }
        // The two explicit node-list declarations that replaced the name scans. Both are required
        // structure — a tank with no collision proxy falls through the world, one with no roadwheel
        // on a side has no belt to drive — and a duplicate is a node bound twice, which for
        // roadwheels silently shifts every later wire slot.
        if self.colliders.is_empty() {
            return Err("colliders must declare at least one collision-proxy node".into());
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for collider in &self.colliders {
            let name = &collider.node;
            if !seen.insert(name.as_str()) {
                return Err(format!("colliders declares `{name}` twice").into());
            }
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for wheel in &self.roadwheels {
            if !seen.insert(wheel.node.as_str()) {
                return Err(format!("roadwheels declares `{}` twice", wheel.node).into());
            }
        }
        for side in [crate::tank::TrackSide::Left, crate::tank::TrackSide::Right] {
            if !self.roadwheels.iter().any(|wheel| wheel.side == side) {
                return Err(format!("roadwheels declares no station on the {side:?} track").into());
            }
        }
        self.check_running_gear_contacts()?;
        // Track: values that parse but can never wrap a running gear. (The one check that needs
        // GEOMETRY — the material loop closing around the rest wheel circles — lives at rig
        // bind, where the baked wheel rests exist; these are the spec-local invariants.)
        let t = &self.track;
        if !(3..=crate::track::belt_draw::MAX_BELT_LINKS).contains(&t.link_count) {
            return Err(format!("track.link_count must be in 3..={} (got {}); reduce the count or widen the belt table's record address",
                crate::track::belt_draw::MAX_BELT_LINKS, t.link_count).into());
        }
        if t.sprocket.teeth < 3 {
            return Err("track.sprocket.teeth must be >= 3 to form a pin polygon".into());
        }
        let turret = &self.turret;
        // The hull's and the turret's body data: `Mass` and the cuboid `AngularInertia` are built
        // from these at spawn, and a zero or NaN one is a body the solver cannot integrate.
        for (what, value) in [
            ("mass", self.mass),
            ("inertia_extents.0", self.inertia_extents.0),
            ("inertia_extents.1", self.inertia_extents.1),
            ("inertia_extents.2", self.inertia_extents.2),
            ("turret.mass", turret.mass),
            ("turret.inertia_extents.0", turret.inertia_extents.0),
            ("turret.inertia_extents.1", turret.inertia_extents.1),
            ("turret.inertia_extents.2", turret.inertia_extents.2),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{what} must be finite and > 0 (got {value})").into());
            }
        }
        for role in [crate::tank::ServoRole::Yaw, crate::tank::ServoRole::Pitch] {
            let inertia = crate::tank::axis_inertia(turret, role);
            if !inertia.is_finite() || inertia <= 0.0 {
                return Err("turret.mass/inertia_extents yield an unrepresentable axis inertia; author dimensions and mass with finite positive cuboid moments".into());
            }
        }
        // Four view nodes the belt binds by name: a blank or repeated one binds two roles to one
        // node, or none.
        let gear = [
            &t.sprocket.nodes.left,
            &t.sprocket.nodes.right,
            &t.idler.nodes.left,
            &t.idler.nodes.right,
        ];
        for (i, name) in gear.iter().enumerate() {
            if name.is_empty() || gear[..i].contains(name) {
                return Err(format!(
                    "track.sprocket/idler nodes must be four distinct non-empty names (got `{name}`)"
                )
                .into());
            }
        }
        for wheel in &self.roadwheels {
            if gear.iter().any(|node| **node == wheel.node) {
                return Err(format!("roadwheels.node `{}` is also a sprocket/idler; author one running-gear role per node", wheel.node).into());
            }
        }
        // Both hinge stops are MAGNITUDES, so both must be positive; 90° is the ceiling because a
        // joint that folds further has already buried the next shoe in this one on any real track.
        // Checked in the AUTHORED unit (degrees) so the error quotes what the author typed.
        for (field, value) in [
            ("link_angle.inward_deg", t.link_angle.inward_deg),
            ("link_angle.outward_deg", t.link_angle.outward_deg),
        ] {
            if !value.is_finite() || value <= 0.0 || value > 180.0 {
                return Err(format!(
                    "track.{field} must be a magnitude in (0, 180] degrees (got {value})"
                )
                .into());
            }
        }
        let sprocket_demand_deg = 360.0 / t.sprocket.teeth as f32;
        if t.link_angle.inward_deg < sprocket_demand_deg {
            return Err(format!("track.link_angle.inward_deg must cover the sprocket's {sprocket_demand_deg} degree tooth pitch; correct the usable articulation or tooth count").into());
        }
        if !t.hinge_torque.is_finite() || t.hinge_torque < 0.0 {
            return Err(format!(
                "track.hinge_torque must be finite and >= 0 (got {})",
                t.hinge_torque
            )
            .into());
        }
        // Suspension divisions require finite positive authored support parameters.
        for (field, value) in [
            ("suspension.ride_frequency", t.suspension.ride_frequency),
            ("suspension.damping_ratio", t.suspension.damping_ratio),
            ("suspension.bump_stop", t.suspension.bump_stop),
            ("suspension.engage", t.suspension.engage),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("track.{field} must be finite and > 0 (got {value})").into());
            }
        }
        // Structural rules need no geometry. Running-gear admission checks the physical
        // tables, side inertia equality and coupling divisor against the actual pitch radius.
        t.nominal_transmission_params(&self.roadwheels)
            .map(|_| ())?;
        // Engine audio data: the firing rate divides by neither, but both are DIVISORS in the
        // playback law (`sfx`) — a 0-cylinder engine pops at 0 Hz and a 0 Hz recording makes the
        // anchor infinite.
        let engine = &t.powertrain.transmission.engine;
        if engine.cylinders < 1 {
            return Err("engine.cylinders must be >= 1 (got 0)".into());
        }
        if let Some(sound) = engine.sound.as_ref()
            && !(sound.clip_pop_hz.is_finite() && sound.clip_pop_hz > 0.0)
        {
            return Err(format!(
                "engine.sound.clip_pop_hz must be finite and > 0 (got {})",
                sound.clip_pop_hz
            )
            .into());
        }
        // A capability the sheet does not key reads as unavailable to every consumer, so each one is
        // stated before its requirement is inspected.
        if let Some(missing) = Capability::ALL
            .iter()
            .find(|capability| !self.capabilities.contains_key(capability))
        {
            return Err(format!(
                "capabilities states no `{}` — key every capability and author its required sources",
                missing.label()
            )
            .into());
        }
        self.check_seats_and_gates()?;
        self.check_armament()?;
        // A playable rig supplies both camera modes. Unarmed structural tooling fixtures have
        // no rig and may declare no views; the complete spawn already requires a primary weapon.
        if self
            .weapons
            .values()
            .any(|weapon| weapon.trigger == Trigger::Primary)
        {
            for kind in ViewKind::ALL {
                if !self.views.contains_key(&kind) {
                    return Err(format!("views lacks {kind:?} — author every player view with its node, optics and requires").into());
                }
            }
        }
        // Every view's FOV is a DIVISOR downstream, not merely a camera setting: both LOD ladders
        // derive every switch distance through `2·tan(fov/2)`
        // (`view::ViewFacts::sub_pixel_distance_m`) and the sight derives its cursor-travel margin
        // and sensitivity from it. A NaN authored here propagates into NaN range boundaries, which
        // compare false against every distance and silently delete the ground; a negative one
        // inverts the range chain. None is a picture bug that leads back to a spec sheet, so the
        // sheet refuses them.
        for (kind, view) in &self.views {
            // Every authored number, each in its own unit, so the error quotes what the author
            // typed. A magnification reaches nothing downstream, but a sheet that states an
            // impossible one is a sheet whose instrument is wrong.
            for (authored, unit) in view.optics.authored() {
                if !authored.is_finite() || authored <= 0.0 {
                    return Err(format!(
                        "view `{}`: {unit} must be finite and > 0 (got {authored})",
                        kind.label(),
                    )
                    .into());
                }
            }
            let fov = view.optics.vertical_fov();
            if fov <= 0.0 || fov >= core::f32::consts::PI {
                return Err(format!(
                    "view `{}`: the field must be in (0, π) radians — a perspective projection has \
                     no half-angle at or past 90°, and none at zero (got {fov} rad)",
                    kind.label(),
                )
                .into());
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
        load_context: &mut LoadContext<'_>,
    ) -> Result<TankSpec, BevyError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let spec: TankSpec = ron::de::from_bytes(&bytes)?;
        // Past serde's shape check: reject values that parse but yield an unfirable weapon (ADR-0011).
        spec.validate()
            .map_err(|error| BevyError::from(format!("{}: {error}", load_context.path())))?;
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
///
/// Checked on its two triggers: a tank's handle when it arrives, for a sheet that failed first,
/// and each failure as it arrives, for a tank already holding that sheet. A failure no tank holds
/// (the pen cam's stage view of a peer's vehicle) is not this check's.
fn report_failed_spec(
    asset_server: Res<AssetServer>,
    mut failures: MessageReader<AssetLoadFailedEvent<TankSpec>>,
    arrived: Query<&TankSpecHandle, (With<Tank>, Changed<TankSpecHandle>)>,
    tanks: Query<&TankSpecHandle, With<Tank>>,
) {
    for handle in &arrived {
        if let LoadState::Failed(err) = asset_server.load_state(&handle.0) {
            error!("required tank spec sheet failed to load: {err}");
            panic!("required tank spec sheet failed to load: {err}");
        }
    }
    for failure in failures.read() {
        if tanks.iter().any(|handle| handle.0.id() == failure.id) {
            let err = &failure.error;
            error!("required tank spec sheet failed to load: {err}");
            panic!("required tank spec sheet failed to load: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The certificate on the SHIPPED sheet: it deserializes (so schema drift — a renamed field, a
    /// changed type, a stray key under `deny_unknown_fields` — fails at `cargo test` time rather
    /// than in a player's hands), `validate()` accepts it, and the cross-field relations the
    /// schema's own meaning implies hold.
    ///
    /// It pins NO authored magnitude. The RON is the one home of every tuning number, and every
    /// invariant `validate()` already enforces is asserted here exactly once — by calling it.
    #[test]
    fn the_shipped_sheet_parses_validates_and_is_self_consistent() {
        let spec: TankSpec = crate::provenance::from_shipped_str(
            crate::tank::TANK_SPEC_PATH,
            include_str!("../assets/tiger_1/tiger_1.tank.ron"),
        )
        .expect("tiger_1.tank.ron must deserialize into TankSpec");
        spec.validate()
            .expect("the shipped sheet must be semantically valid");

        // Bulk: the mass divides every acceleration, and each extent is a moment-arm factor.
        assert!(spec.mass.is_finite() && spec.mass > 0.0, "{}", spec.mass);
        let (ex, ey, ez) = spec.inertia_extents;
        for extent in [ex, ey, ez] {
            assert!(
                extent.is_finite() && extent > 0.0,
                "inertia_extents must be positive full dimensions (got {extent})"
            );
        }

        // Every node the sheet names, in every role — a blank name addresses nothing in the model.
        for reference in spec.node_references() {
            assert!(
                !reference.node.trim().is_empty(),
                "{} names an empty node",
                reference.field
            );
        }

        // A collision proxy is a physics stand-in, never armour and never an actuator mount, so
        // its node cannot also carry a volume entry or a servo.
        for collider in spec.colliders.iter().map(|collider| &collider.node) {
            assert!(
                !spec.volumes.contains_key(collider),
                "collider `{collider}` is also a ballistic volume"
            );
            assert!(
                !spec.servos.contains_key(collider),
                "collider `{collider}` is also a servo mount"
            );
        }

        // The sheet supplies construction; the Driver selects within the admitted ladders.
        let transmission = &spec.track.powertrain.transmission;
        let (engine, steering) = (&transmission.engine, &transmission.steering);
        // The rpm band, in the order the three anchors mean: the engine idles below its
        // governor, and the governor cannot sit above the rpm the gear speeds are quoted at.
        assert!(
            engine.idle_rpm < engine.governed_rpm,
            "idle_rpm ({}) must sit below governed_rpm ({})",
            engine.idle_rpm,
            engine.governed_rpm
        );
        assert!(
            engine.governed_rpm <= engine.rated_rpm,
            "governed_rpm ({}) cannot exceed the rated_rpm ({}) the ladder is anchored at",
            engine.governed_rpm,
            engine.rated_rpm
        );
        // The curve is end-clamped, so authoring that stops short of the governor makes the
        // whole governed band one extrapolated flat — the operating point must be bracketed
        // by real data.
        let (first_rpm, _) = engine.torque_curve[0];
        let (last_rpm, _) = *engine
            .torque_curve
            .last()
            .expect("the torque curve is non-empty");
        assert!(
            first_rpm < engine.governed_rpm && engine.governed_rpm <= last_rpm,
            "the torque curve ({first_rpm}..{last_rpm} rpm) must bracket governed_rpm ({})",
            engine.governed_rpm
        );
        let params = spec
            .track
            .nominal_transmission_params(&spec.roadwheels)
            .expect("transmission admission precedes aggregate checks");
        let peak_torque = params.torque_at(params.peak_torque_rpm);
        assert!(
            peak_torque > 0.0,
            "engine.torque_curve must deliver positive torque under its governor"
        );
        assert!(
            engine.clutch_capacity_nm >= peak_torque,
            "clutch_capacity_nm ({}) must carry the delivered peak torque ({peak_torque})",
            engine.clutch_capacity_nm
        );
        // The recording is a file reference, and the loader resolves it verbatim.
        if let Some(sound) = engine.sound.as_ref() {
            assert!(
                sound.clip.ends_with(".ogg"),
                "engine.sound.clip must name an .ogg (got `{}`)",
                sound.clip
            );
        }
        // Each detent pair is named for its geometry: the tight radius is the tighter one. A
        // pair the other way round is the swap this naming exists to prevent.
        for (gear, &(tight, wide)) in steering.radii.iter().enumerate() {
            assert!(
                tight <= wide,
                "steering.radii[{gear}]: tight ({tight} m) is wider than wide ({wide} m)"
            );
        }

        // Armament. The single `Primary` is also the rig's main-bore handle, so exactly one weapon
        // wears it; the recoil spring is authored iff the barrel that reciprocates is; and the
        // ballistics scalars all divide or scale the penetration march.
        assert!(!spec.weapons.is_empty(), "a tank declares its armament");
        let primaries = spec
            .weapons
            .iter()
            .filter(|(_, weapon)| weapon.trigger == Trigger::Primary)
            .count();
        assert_eq!(
            primaries, 1,
            "exactly one weapon is Primary — it supplies the rig's main-bore handles"
        );

        // The gunner's view node is how the binder finds the gunner's chain for the rig.
        assert!(
            spec.views.contains_key(&ViewKind::Gunner),
            "the sheet must declare a Gunner view"
        );

        // The seat, provider and recoil rules are `validate`'s, checked above for every sheet.
    }

    /// Each declared contact datum is refused at the sheet edge; the running gear and view
    /// rely on finite radii, nonempty tread surfaces and a finite reference-tooth frame.
    #[test]
    fn running_gear_contact_data_has_a_complete_domain() {
        let mut spec = fixture();
        spec.roadwheels[0].tread.radius = f32::NAN;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("tread.radius")
        );
        let mut spec = fixture();
        spec.roadwheels[0].tread.spans.clear();
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("tread.spans")
        );
        let mut spec = fixture();
        spec.roadwheels[0].tread.spans[0] = (1.0, -1.0);
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("tread.spans")
        );
        let mut spec = fixture();
        spec.track.pin_plane_x.left = spec.track.pin_plane_x.right;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("pin_plane_x")
        );
        let mut spec = fixture();
        spec.track.idler.radius.left = 0.0;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("idler.radius")
        );
        let mut spec = fixture();
        spec.track.sprocket.tooth_tip_rad.right = f32::INFINITY;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("tooth_tip_rad")
        );
    }

    /// The MUTATION BATTERY's subject: a fictional vehicle that authors every block a rejection
    /// case flips, and nothing else. The shipped sheet is deliberately NOT the subject — a
    /// mutation battery riding on it turns every deliberate tuning or content edit (a pulled
    /// sound block, a re-anchored ladder) into a test break, which is friction with no bug behind
    /// it. Its numbers are round and meaningless; the only thing asserted about them is that
    /// `validate()` accepts the sheet unmutated (`the_fixture_sheet_is_valid_unmutated`), which is
    /// what makes each mutation's rejection attributable to the mutation.
    const FIXTURE_RON: &str = r#"#![enable(implicit_some)]
TankSpec(
    display_name: "Fixture",
    mass: 30_000.0,
    inertia_extents: (3.0, 2.0, 6.0),
    turret: (node: Some("Turret_Yaw"), mass: 5_000.0, inertia_extents: (2.0, 1.0, 2.0)),
    track: (
        pin_plane_x: (left: -1.3, right: 1.3),
        substance: "TrackSteel",
        link_count: 80,
        link_mass: (20.0, Fixture("fixture link assembly")),
        hinge_torque: 30.0,
        link_angle: (inward_deg: 40.0, outward_deg: 18.0),
        sprocket: (spin_inertia_kgm2: ((left: 4.0, right: 4.0), Fixture("fixture sprockets")), teeth: 16, tooth_tip_rad: (left: 0.0, right: 0.0), nodes: (left: "Sprocket_L", right: "Sprocket_R")),
        idler: (spin_inertia_kgm2: ((left: 4.0, right: 4.0), Fixture("fixture idlers")), radius: (left: 0.4, right: 0.4), nodes: (left: "Idler_L", right: "Idler_R")),
        powertrain: (
            transmission: (
                engine: (
                    idle_rpm: 600.0,
                    governed_rpm: 2000.0,
                    crank_limit_rpm: (2400.0, Fixture("fixture crank limit")),
                    rated_rpm: 2400.0,
                    cylinders: 6,
                    sound: (clip: "sfx/engine/fixture_loop.ogg", clip_pop_hz: 40.0),
                    torque_curve: [(700.0, 1000.0), (2000.0, 1200.0), (2400.0, 1100.0)],
                    drag_fraction: 0.25,
                    inertia_kgm2: 3.0,
                    clutch_capacity_nm: 1600.0,
                ),
                gearbox: (
                    forward_speeds_kmh: [5.0, 8.0, 12.0, 18.0],
                    reverse_speeds_kmh: [5.0, 8.0],

                    launch_gears: 4,
                    max_step: 3,
                    shift_secs: 0.3,

                ),
                steering: (
                    radii: [(4.0, 12.0), (6.0, 18.0), (9.0, 27.0), (14.0, 42.0)],
                    reverse_radii: [(4.0, 12.0), (6.0, 18.0)],
                    capacity: 100_000.0,
                    recirculation: 0.9,
                ),
                brake_force: 50_000.0,
                brake_static_factor: 1.5,
            ),
        ),
        suspension: (
            ride_frequency: 1.2,
            damping_ratio: 0.35,
            bump_stop: 0.2,
            engage: 0.02,
        ),
    ),
    servos: {
        "Turret_Yaw": (role: Yaw, drive: Hand(hand_rate: (30.0, Fixture("fixture hand rate"))), travel: Continuous, requires: [Gunner]),
    },
    volumes: {
        "Gunner": (crew: Gunner),
        "Driver": (crew: Driver),
        "Breech": (hp: 80, function: Breech),
        "Engine": (hp: 80, function: Engine),
        "Transmission": (hp: 80, function: Transmission),
        "Hull_Plate": (main: true),
        "Wheel_L_0": (external: true),
        "Wheel_R_0": (external: true),
    },
    colliders: [(node: "Hull_Collider", substance: "RHA")],
    shadow_proxies: [],
    roadwheels: [
        (node: "Wheel_L_0", side: Left, spin_inertia_kgm2: (8.0, Fixture("fixture station")), tread: (radius: 0.4, spans: [(-0.1, 0.1)])),
        (node: "Wheel_R_0", side: Right, spin_inertia_kgm2: (8.0, Fixture("fixture station")), tread: (radius: 0.4, spans: [(-0.1, 0.1)])),
    ],
    weapons: {
        "Cannon": (
            trigger: Primary,
            muzzle: "Cannon_Muzzle",
            bore_length: (1.0, Fixture("fixture construction")),
            barrel: "Cannon_Recoil",
            gun: "kwk36_l56",
            stowage: [
                (round: "88mm_AP", count: (40, Fixture("fixture"))),
                (round: "88mm_APCR", count: (10, Fixture("fixture"))),
            ],
            fire_mode: Single(reload_secs: 3.0),
            recoil: (kick: 10.0, stiffness: 90.0, damping: 14.0, max_length: (0.5, Fixture("fixture"))),
            fire: [Gunner, Breech],
            load: [Gunner],
        ),
        "MG": (
            trigger: Secondary,
            muzzle: "MG_Muzzle",
            bore_length: (1.0, Fixture("fixture construction")),
            gun: "mg34",
            stowage: [(round: "7.9mm_AP", count: (1000, Fixture("fixture")))],
            fire_mode: Automatic(belt_size: 100, belt_swap_secs: 3.0),
            tracer_every: 5,
            fire: [],
            load: [],
        ),
    },
    views: {
        Gunner: (node: "Cannon_Sight", optics: Magnified(magnification: 2.5, field_deg: 25.0), requires: [Gunner]),
        Commander: (node: "Cannon_Sight", optics: Unaided(60.0), requires: []),
    },
    capabilities: { Drive: [Engine, Transmission] },
    drive_controls: NotRequired,
)
"#;

    /// Spawned rigs and both camera modes rely on complete authored views; a renamed or omitted
    /// kind must refuse at load, before a camera could invent its field or availability.
    #[test]
    fn a_playable_rig_requires_every_player_view() {
        for kind in ViewKind::ALL {
            let mut spec = fixture();
            spec.views.remove(&kind);
            let error = spec
                .validate()
                .expect_err("a playable view was omitted")
                .to_string();
            assert!(
                error.contains("views") && error.contains(&format!("{kind:?}")),
                "{error}"
            );
        }
    }

    /// The game's damage model: construction must admit the two-crew floor before spawn;
    /// scoring and device ownership rely on a valid sheet starting in action.
    #[test]
    fn validate_refuses_fewer_than_two_crew_stations() {
        for count in [1, 0] {
            let mut spec = fixture();
            spec.volumes.retain(|_, volume| {
                volume.component().is_none_or(|part| {
                    part.crew
                        .is_none_or(|seat| count == 1 && seat == CrewStation::Gunner)
                })
            });
            let error = spec
                .validate()
                .expect_err("a tank below the crew floor must refuse before play")
                .to_string();
            assert!(
                error.contains("volumes")
                    && error.contains("crew")
                    && error.contains("at least two"),
                "{error}"
            );
        }
    }

    /// A fresh, unmutated [`FIXTURE_RON`] sheet.
    fn fixture() -> TankSpec {
        ron::de::from_str(FIXTURE_RON).expect("the fixture sheet must parse")
    }

    /// The fixture's declared transmission block, for the mutation closures to flip a field in.
    fn fixture_transmission(spec: &mut TankSpec) -> &mut TransmissionSpec {
        &mut spec.track.powertrain.transmission
    }

    /// A volume entry is exactly one of main armour, external armour or a component: the
    /// armour tags carry no component facet, a component other than a crew station authors an
    /// hp pool, and a crew station authors none and carries the shared crew pool, so no tank file
    /// can give its crew a pool of its own.
    #[test]
    fn a_volume_entry_is_main_external_or_a_component() {
        // The extension the shipped sheets declare.
        let parse = |text: &str| {
            ron::de::from_str::<VolumeSpec>(&format!("#![enable(implicit_some)]\n{text}"))
        };
        assert!(matches!(
            parse("(external: true)"),
            Ok(VolumeSpec::External)
        ));
        assert!(matches!(parse("(main: true)"), Ok(VolumeSpec::MainArmour)));
        let crew = parse("(crew: Driver)").expect("a crew station parses");
        let crew = crew.component().expect("it is a component");
        assert_eq!(
            crew.hp,
            crate::ballistics::box_classes::BoxClasses::shipped().crew_hp()
        );
        assert_eq!(crew.crew, Some(CrewStation::Driver));
        let module = parse("(hp: 20, external: false)").expect("a module parses");
        assert_eq!(module.component().expect("it is a component").hp, 20);
        for refused in [
            "(main: true, hp: 20)",
            "(main: true, external: true)",
            "(main: false)",
            "(external: true, hp: 20)",
            "(external: true, ammo: true)",
            "(external: true, crew: Driver)",
            "(external: true, function: Engine)",
            "()",
            "(external: false)",
            "(hp: 30, crew: Driver)",
        ] {
            assert!(parse(refused).is_err(), "`{refused}` must not parse");
        }
    }

    /// A node keyed twice in `volumes` is refused at parse. Invariant: a plain map keeps the last
    /// of two declarations, so the first would silently vanish; a node has exactly one role.
    #[test]
    fn a_repeated_volume_key_is_refused_at_parse() {
        let doubled = FIXTURE_RON.replacen(
            "\"Hull_Plate\": (main: true),",
            "\"Hull_Plate\": (main: true),\n        \"Hull_Plate\": (external: true),",
            1,
        );
        assert_ne!(doubled, FIXTURE_RON, "the fixture keys Hull_Plate");
        let err = ron::de::from_str::<TankSpec>(&doubled)
            .err()
            .expect("a repeated key must not parse");
        assert!(err.to_string().contains("twice"), "{err}");
    }

    /// Link counts must fit the renderer's record format before any rig/table allocation.
    /// The asset gate owns this bound; a conversion failure during draw must be unreachable.
    #[test]
    fn link_counts_beyond_the_table_address_refuse_at_admission() {
        let mut spec = fixture();
        spec.track.link_count = crate::track::belt_draw::MAX_BELT_LINKS + 1;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("track.link_count")
        );
    }

    /// The mutation battery's one baseline: unmutated, the fixture passes. Without this, a
    /// rejection proves nothing — it could be the fixture rather than the mutation.
    #[test]
    fn the_fixture_sheet_is_valid_unmutated() {
        fixture()
            .validate()
            .expect("the mutation fixture must be valid before any mutation");
    }

    /// The belt binds its sprocket and idler view nodes by the sheet's names, so two roles on one
    /// node (or a blank) must be refused, and `side_of` must map each declared name to its side.
    #[test]
    fn running_gear_nodes_are_four_distinct_names() {
        let mut spec = fixture();
        spec.track.idler.nodes.left = spec.track.sprocket.nodes.right.clone();
        assert!(spec.validate().is_err());
        let mut spec = fixture();
        spec.track.idler.nodes.right = String::new();
        assert!(spec.validate().is_err());
        let spec = fixture();
        assert_eq!(spec.track.sprocket.nodes.side_of("Sprocket_L"), Some(0));
        assert_eq!(spec.track.sprocket.nodes.side_of("Sprocket_R"), Some(1));
        assert_eq!(spec.track.sprocket.nodes.side_of("Idler_L"), None);
        assert_eq!(spec.track.idler.nodes.side_of("Idler_R"), Some(1));
    }

    /// The game and sandbox bind one running-gear role to each authored node. A node declared
    /// both sprung and hull-fixed would take contradictory poses; the spec gate must refuse it.
    #[test]
    fn running_gear_roles_cannot_share_a_node() {
        let mut spec = fixture();
        spec.roadwheels[0].node = spec.track.sprocket.nodes.left.clone();
        assert!(spec.validate().is_err());
    }

    /// **A view frames the field it authors, and the magnification is not a term in it.** Both
    /// variants pass their own degrees through the one conversion; two instruments of different
    /// power over the same field frame the same angle.
    #[test]
    fn the_authored_field_is_the_framed_field() {
        for field_deg in [6.25_f32, 12.5, 25.0, 45.0] {
            let framed = Optics::Magnified {
                magnification: 2.5,
                field_deg,
            }
            .vertical_fov()
            .to_degrees();
            assert!(
                (framed - field_deg).abs() < 1e-3,
                "an optic authoring {field_deg}° frames {framed}°",
            );
        }
        assert_eq!(
            Optics::Magnified {
                magnification: 12.0,
                field_deg: 25.0,
            }
            .vertical_fov(),
            Optics::Magnified {
                magnification: 2.5,
                field_deg: 25.0,
            }
            .vertical_fov(),
        );
        // A naked eye passes its own field through, converted from the authored degrees.
        assert!((Optics::Unaided(45.0).vertical_fov() - core::f32::consts::FRAC_PI_4).abs() < 1e-6);
    }

    /// `validate()` rejects optics that would silently delete the picture instead of showing a wrong
    /// one. The field is a DIVISOR downstream — the terrain LOD ladder derives every switch distance
    /// from `dev / fov` — so `NaN` propagates into `NaN` range boundaries, which compare false
    /// against every distance and simply stop drawing the ground, with nothing anywhere pointing
    /// back at a spec sheet. Negative inverts the chain; a field at or past 180° has no perspective
    /// half-angle. **Both of a magnified optic's numbers are checked**, the magnification included:
    /// it reaches nothing downstream, but an impossible one means the sheet's instrument is wrong.
    #[test]
    fn validate_rejects_optics_that_are_not_a_view() {
        let with_optics = |optics: Optics| {
            let mut spec = fixture();
            spec.views.get_mut(&ViewKind::Gunner).unwrap().optics = optics;
            spec
        };
        let magnified = |magnification, field_deg| Optics::Magnified {
            magnification,
            field_deg,
        };
        for optics in [
            magnified(0.0, 25.0),
            magnified(-2.5, 25.0),
            magnified(f32::NAN, 25.0),
            magnified(2.5, 0.0),
            magnified(2.5, -25.0),
            magnified(2.5, f32::INFINITY),
            magnified(2.5, 180.0),
            Optics::Unaided(0.0),
            Optics::Unaided(f32::INFINITY),
            Optics::Unaided(180.0),
        ] {
            let err = with_optics(optics).validate().unwrap_err().to_string();
            assert!(err.contains("Gunner"), "{optics:?} was refused with: {err}");
        }
        // A magnified optic, a barely-magnified one, and a wide naked eye all pass.
        for optics in [
            magnified(2.5, 25.0),
            magnified(1.0, 60.0),
            Optics::Unaided(45.0),
        ] {
            with_optics(optics)
                .validate()
                .unwrap_or_else(|err| panic!("{optics:?} must be accepted: {err}"));
        }
    }

    /// The game's damage model: weapon eligibility. Spawn relies on the parsed cycle being
    /// retained, and asset authors need the mount and field on a refusal at that same edge.
    #[test]
    fn the_weapon_edge_retains_its_cycle_and_names_invalid_mounts() {
        let spec = fixture();
        for (name, weapon) in &spec.weapons {
            let spawned =
                crate::tank::Weapon::of_spec(name, weapon, None).expect("validated fixture weapon");
            assert_eq!(spawned.fire_cycle, weapon.fire_cycle);
        }
        // DERIVED fixture duration: beyond the default timestep's signed deadline horizon.
        let invalid = FIXTURE_RON.replace(
            "Single(reload_secs: 3.0)",
            "Single(reload_secs: 100000000.0)",
        );
        let err = ron::de::from_str::<TankSpec>(&invalid)
            .err()
            .expect("invalid duration is refused while parsing")
            .to_string();
        assert!(
            err.contains("Cannon") && err.contains("reload_secs"),
            "{err}"
        );
    }

    /// Fixture input reconstructed from the fixture sheet solely for malformed-data tests.
    fn raw_weapon(base: &str) -> WeaponSpecFile {
        let sheet = fixture();
        let weapon = &sheet.weapons[base];
        let (fire_mode, tracer_every) = match weapon.fire_cycle {
            FireCycle::Single { reload } => (
                FireMode::Single {
                    reload_secs: reload.secs(),
                },
                None,
            ),
            FireCycle::Automatic {
                belt_size,
                swap,
                tracer_every,
                ..
            } => (
                FireMode::Automatic {
                    belt_size,
                    belt_swap_secs: swap.secs(),
                },
                Some(tracer_every),
            ),
        };
        WeaponSpecFile {
            trigger: weapon.trigger,
            muzzle: weapon.muzzle.clone(),
            bore_length: weapon.bore_length,
            barrel: weapon.barrel.clone(),
            gun: weapon.gun.clone(),
            stowage: weapon.stowage.clone(),
            fire_mode,
            tracer_every,
            recoil: weapon.recoil.clone(),
            fire: weapon.fire.clone(),
            load: weapon.load.clone(),
        }
    }

    /// The fixture sheet with its `base` mount cloned as a `Secondary` "Phantom" (the fixture's
    /// cannon stays its one `Primary`) and edited by `edit`, validated; the error as text.
    fn with_phantom(base: &str, edit: impl FnOnce(&mut WeaponSpec)) -> Result<(), String> {
        let mut spec = fixture();
        let mut weapon = spec.weapons[base].clone();
        weapon.trigger = Trigger::Secondary;
        edit(&mut weapon);
        spec.weapons.insert("Phantom".to_string(), weapon);
        spec.validate().map_err(|err| err.to_string())
    }

    /// `rounds` as stowage, `count` of each, with a fixture provenance.
    fn stowage(rounds: &[&str], count: u32) -> Vec<StowageEntry> {
        rounds
            .iter()
            .map(|round| StowageEntry {
                round: (*round).to_owned(),
                count: Sourced(count),
            })
            .collect()
    }

    /// A mount's stowage loads only whole: none twice, none of count zero, at most
    /// [`MAX_WEAPON_ROUNDS`]; the error names the mount and what is wrong. Spawn binds every
    /// stowed round and the selector and the fire event address them by `u8` index, so a list that
    /// slips through panics mid-spawn or puts one round under two indices. A round's own validity is
    /// the registry's ([`crate::rounds`]), its speed the gun's ([`crate::guns`]).
    #[test]
    fn a_mount_s_stowage_is_validated_whole() {
        let with_stowage = |rounds: &[&str], count: u32| {
            with_phantom("Cannon", |weapon| weapon.stowage = stowage(rounds, count))
        };
        with_stowage(&["88mm_AP", "88mm_APHE_ring"], 1).expect("known, distinct keys load");
        for (rounds, count, named) in [
            (vec!["88mm_AP", "88mm_APCR", "88mm_AP"], 1, "88mm_AP"),
            (vec!["88mm_AP"], 0, "count of 0"),
            (vec!["88mm_AP"; MAX_WEAPON_ROUNDS + 1], 1, "at most"),
        ] {
            let shown = format!("{:?} x{count}", &rounds[..rounds.len().min(3)]);
            let err = with_stowage(&rounds, count).expect_err(&shown);
            assert!(
                err.contains("Phantom") && err.contains(named),
                "{shown}: {err}"
            );
        }
    }

    /// A mount stows at least one round: spawn loads the first ([`WeaponSpec::default_round_key`]), and
    /// the selector and the range table read it, so an empty stowage leaves a weapon with nothing
    /// to load.
    #[test]
    fn a_mount_with_empty_stowage_is_refused() {
        let err = with_phantom("Cannon", |weapon| weapon.stowage.clear()).unwrap_err();
        assert!(err.contains("Phantom") && err.contains("empty"), "{err}");
    }

    /// A mount names a gun the gun registry carries: spawn fires every round through it
    /// (`Weapon::of_spec`) and the fire system runs its mechanism, so a dangling key would panic
    /// mid-spawn instead of failing the load.
    #[test]
    fn a_mount_naming_no_gun_is_refused() {
        let err = with_phantom("Cannon", |weapon| weapon.gun = "Unobtainium".into()).unwrap_err();
        assert!(
            err.contains("Phantom") && err.contains("Unobtainium") && err.contains("gun"),
            "{err}"
        );
    }

    /// Every stowed round is a key of the round registry: spawn names each on the selector and
    /// fires it. The unknown key sits second, so a check of the default round alone does not pass.
    #[test]
    fn a_stowed_round_no_registry_carries_is_refused() {
        let err = with_phantom("Cannon", |weapon| {
            weapon.stowage = stowage(&["88mm_AP", "Unobtainium"], 1);
        })
        .unwrap_err();
        assert!(
            err.contains("Phantom") && err.contains("Unobtainium"),
            "{err}"
        );
    }

    /// A mount stows only rounds its gun's ammunition table lists: the table gives a stowed round
    /// its muzzle speed, so a round the gun does not chamber has none to fire at. The round sits
    /// second, so a check of the default round alone does not pass.
    #[test]
    fn a_stowed_round_its_gun_does_not_chamber_is_refused() {
        let err = with_phantom("Cannon", |weapon| {
            weapon.stowage = stowage(&["88mm_AP", "7.9mm_AP"], 1);
        })
        .unwrap_err();
        assert!(
            err.contains("Phantom") && err.contains("7.9mm_AP") && err.contains("kwk36_l56"),
            "{err}"
        );
    }

    /// A mount's fire mode is of its gun's mechanism family: the fire system runs the joined cycle
    /// ([`WeaponSpec::fire_cycle`]), and a belt on a single-shot gun has no cyclic rate to run at,
    /// a reload on a belt-fed one no belt. Both crossings are refused.
    #[test]
    fn a_mount_whose_fire_mode_is_not_its_gun_s_family_is_refused() {
        for (base, fire_mode, tracer_every) in [
            (
                "Cannon",
                FireMode::Automatic {
                    belt_size: 10,
                    belt_swap_secs: 3.0,
                },
                Some(5),
            ),
            ("MG", FireMode::Single { reload_secs: 1.0 }, None),
        ] {
            let mut weapon = raw_weapon(base);
            weapon.fire_mode = fire_mode;
            weapon.tracer_every = tracer_every;
            let err = WeaponSpec::try_from(weapon)
                .err()
                .expect("crossed mechanism is refused");
            assert!(
                err.contains("fire_mode") && err.contains("family"),
                "{base}: {err}"
            );
        }
    }

    /// A belt's tracer mix is authored exactly on a belt mount: the fire system decides each
    /// belt round's tracer from it, and a single-shot mount has no belt for one to describe.
    #[test]
    fn a_tracer_mix_is_authored_exactly_on_a_belt_mount() {
        for (base, tracer_every) in [("Cannon", Some(5)), ("MG", None)] {
            let mut weapon = raw_weapon(base);
            weapon.tracer_every = tracer_every;
            let err = WeaponSpec::try_from(weapon)
                .err()
                .expect("tracer mismatch is refused");
            assert!(err.contains("tracer_every"), "{base}: {err}");
        }
    }

    /// `validate()` rejects each silently-bricked `FireMode` value, and its error names the weapon.
    /// The legal edge cases (tracerless belt, instant reloads) must still pass. Guards ADR-0011's
    /// fail-fast: a weapon that parses but can never fire/cycle must be a hard load error, not a
    /// dead gun discovered mid-match.
    #[test]
    fn validate_rejects_bricked_fire_modes() {
        // Swap one bad weapon at a time into an otherwise-valid sheet: a single-shot mode on the
        // single-shot mount's gun, a belt on the belt-fed one's.
        let with_weapon = |name: &str, mode: FireMode| {
            let mut spec = fixture();
            let base = match mode {
                FireMode::Single { .. } => "Cannon",
                FireMode::Automatic { .. } => "MG",
            };
            let mut weapon = raw_weapon(base);
            weapon.fire_mode = mode;
            weapon.trigger = Trigger::Secondary;
            spec.weapons.insert(
                name.to_string(),
                WeaponSpec::try_from(weapon).expect("valid fixture durations"),
            );
            spec
        };

        // A 0-round belt: never fires or swaps.
        let bad = with_weapon(
            "Bricked",
            FireMode::Automatic {
                belt_size: 0,
                belt_swap_secs: 3.5,
            },
        );
        let err = bad.validate().unwrap_err().to_string();
        assert!(
            err.contains("Bricked") && err.contains("belt_size"),
            "{err}"
        );

        // A single-shot reload or a belt swap that is no fixed-tick span: the sheet refuses each
        // by name where `TickSpan::from_secs` parses it.
        for secs in [
            -0.5,
            f32::NAN,
            f32::INFINITY,
            f32::MAX,
            100_000_000.0,
            67_108_860.0,
            33_554_432.0,
        ] {
            for (base, mode, field) in [
                (
                    "Cannon",
                    FireMode::Single { reload_secs: secs },
                    "reload_secs",
                ),
                (
                    "MG",
                    FireMode::Automatic {
                        belt_size: 150,
                        belt_swap_secs: secs,
                    },
                    "belt_swap_secs",
                ),
            ] {
                let mut weapon = raw_weapon(base);
                weapon.fire_mode = mode;
                let err = WeaponSpec::try_from(weapon)
                    .err()
                    .expect("invalid span refused at parsing");
                assert!(err.contains(field), "{secs}: {err}");
            }
        }

        // The drive relies on a positive mass for every declared link assembly.
        let mut spec = fixture();
        spec.track.link_mass.0 = 0.0;
        let err = spec.validate().unwrap_err().to_string();
        assert!(err.contains("track.link_mass"), "{err}");

        // Force-law scalars: each mutation must be rejected BY NAME — a NaN or zero here
        // reaches a division in `track::forces` and dissolves the belt state in one tick.
        let cases: [(&str, fn(&mut TankSpec)); 1] =
            [("suspension.engage", |s| s.track.suspension.engage = 0.0)];
        for (field, mutate) in cases {
            let mut spec = fixture();
            mutate(&mut spec);
            let err = spec.validate().unwrap_err().to_string();
            assert!(err.contains(field), "expected `{field}` in: {err}");
        }

        // Legal edges: a tracerless stealth belt (tracer_every: 0) and instant reloads pass.
        let mut stealth = with_weapon(
            "Stealth",
            FireMode::Automatic {
                belt_size: 150,
                belt_swap_secs: 0.0,
            },
        );
        if let FireCycle::Automatic { tracer_every, .. } = &mut stealth
            .weapons
            .get_mut("Stealth")
            .expect("the stealth mount")
            .fire_cycle
        {
            *tracer_every = 0;
        }
        assert!(
            stealth.validate().is_ok(),
            "tracer_every: 0 is a legal tracerless belt; belt_swap_secs: 0 is a legal instant refill"
        );
        assert!(
            with_weapon("InstantReload", FireMode::Single { reload_secs: 0.0 })
                .validate()
                .is_ok(),
            "reload_secs: 0 is a legal instant reload"
        );
    }

    /// The game's damage model: fire consumes the authored bore start without inferring a
    /// pivot from the tank root. Load admission must refuse an invalid or missing distance.
    #[test]
    fn validate_requires_a_finite_positive_bore_length() {
        for length in [0.0, -0.1, f32::NAN, f32::INFINITY] {
            let mut spec = fixture();
            spec.weapons.get_mut("Cannon").unwrap().bore_length = Sourced(length);
            let error = spec.validate().unwrap_err().to_string();
            assert!(
                error.contains("Cannon") && error.contains("bore_length"),
                "{error}"
            );
        }
        let absent = FIXTURE_RON.replace(
            "            bore_length: (1.0, Fixture(\"fixture construction\")),\n",
            "",
        );
        assert!(
            ron::de::from_str::<TankSpec>(&absent).is_err(),
            "the bore start is required data"
        );
    }

    /// `validate()` refuses a recoil stop that is not a finite positive length, naming the weapon.
    /// `shooting::apply_recoil` holds the barrel at the stop: a NaN stop compares false and never holds, an infinite one bounds nothing, and one at
    /// or ahead of battery leaves no stroke. A mutant that drops any arm of the check loads these.
    #[test]
    fn validate_rejects_a_recoil_stop_that_is_not_a_positive_length() {
        for stop in [0.0, -0.1, f32::NAN, f32::INFINITY] {
            let mut spec = fixture();
            spec.weapons
                .get_mut("Cannon")
                .and_then(|weapon| weapon.recoil.as_mut())
                .expect("the fixture's cannon recoils")
                .max_length = Sourced(stop);
            let err = spec.validate().unwrap_err().to_string();
            assert!(
                err.contains("Cannon") && err.contains("max_length"),
                "stop {stop}: {err}"
            );
        }
        assert!(fixture().validate().is_ok(), "the fixture's own stop loads");
    }

    /// `validate()` refuses a recoil spring the live barrel cannot integrate, naming the weapon and
    /// the field: `shooting::apply_recoil` steps kick, stiffness and damping every tick, so a NaN is
    /// a NaN barrel pose, a zero kick or stiffness a barrel that never moves or never returns. A
    /// zero damping is an undamped spring and loads.
    /// The main bore is resolved by finding THE `Primary` weapon (`tank::spawn`'s rig, `command`,
    /// `crew_sweep`); a sheet with two would bind whichever the map yields first. A mutant that
    /// drops the count lets the second one load and be ignored.
    /// The game's damage model: every authored weapon must have a wire address. The authority
    /// and receivers rely on this gate instead of dropping fire from an unencodable slot.
    #[test]
    fn validate_refuses_more_weapons_than_the_wire_can_address() {
        let mut spec = fixture();
        let weapon = spec.weapons.values().next().unwrap().clone();
        spec.weapons = (0..=u8::MAX as usize + 1)
            .map(|slot| (format!("fixture_weapon_{slot}"), weapon.clone()))
            .collect();
        let error = spec.validate().unwrap_err().to_string();
        assert!(
            error.contains("weapons: at most 256 slots fit the wire"),
            "{error}"
        );
    }

    #[test]
    fn validate_refuses_a_sheet_with_two_primary_weapons() {
        let mut spec = fixture();
        spec.weapons
            .get_mut("MG")
            .expect("the fixture mounts an MG")
            .trigger = Trigger::Primary;
        let err = spec.validate().unwrap_err().to_string();
        assert!(
            err.contains("[\"Cannon\", \"MG\"]") && err.contains("Primary"),
            "{err}"
        );
    }

    #[test]
    fn validate_rejects_a_recoil_spring_it_cannot_integrate() {
        type Mutate = fn(&mut RecoilSpec, f32);
        let fields: [(&str, Mutate); 3] = [
            ("kick", |recoil, value| recoil.kick = value),
            ("stiffness", |recoil, value| recoil.stiffness = value),
            ("damping", |recoil, value| recoil.damping = value),
        ];
        for (field, mutate) in fields {
            let bad: &[f32] = if field == "damping" {
                &[-1.0, f32::NAN, f32::INFINITY]
            } else {
                &[0.0, -1.0, f32::NAN, f32::INFINITY]
            };
            for &value in bad {
                let mut spec = fixture();
                mutate(
                    spec.weapons
                        .get_mut("Cannon")
                        .and_then(|weapon| weapon.recoil.as_mut())
                        .expect("the fixture's cannon recoils"),
                    value,
                );
                let err = spec.validate().unwrap_err().to_string();
                assert!(
                    err.contains("Cannon") && err.contains(&format!("recoil.{field}")),
                    "{field} {value}: {err}"
                );
            }
        }
        let mut undamped = fixture();
        undamped
            .weapons
            .get_mut("Cannon")
            .and_then(|weapon| weapon.recoil.as_mut())
            .expect("the fixture's cannon recoils")
            .damping = 0.0;
        assert!(undamped.validate().is_ok(), "an undamped spring loads");
    }

    /// `validate()` refuses hull body data the solver cannot integrate, naming the field: `Mass`
    /// and the cuboid `AngularInertia` are built from `mass` and `inertia_extents` at spawn.
    #[test]
    fn validate_rejects_hull_body_data_that_is_not_positive() {
        type Mutate = fn(&mut TankSpec, f32);
        let fields: [(&str, Mutate); 4] = [
            ("mass", |spec, value| spec.mass = value),
            ("inertia_extents.0", |spec, value| {
                spec.inertia_extents.0 = value
            }),
            ("inertia_extents.1", |spec, value| {
                spec.inertia_extents.1 = value
            }),
            ("inertia_extents.2", |spec, value| {
                spec.inertia_extents.2 = value
            }),
        ];
        for (field, mutate) in fields {
            for value in [0.0, -1.0, f32::NAN, f32::INFINITY] {
                let mut spec = fixture();
                mutate(&mut spec, value);
                let err = spec.validate().unwrap_err().to_string();
                assert!(err.starts_with(field), "{field} {value}: {err}");
            }
        }
    }

    /// Clutch admission is about delivered combustion torque, not a curve knot beyond fuel cut.
    #[test]
    fn a_post_cut_torque_knot_does_not_size_the_clutch() {
        let mut spec = fixture();
        // Fixture: 800 N m is delivered; the larger knot is past completed fuel cut.
        let engine = &mut spec.track.powertrain.transmission.engine;
        engine.torque_curve = vec![(600.0, 800.0), (2000.0, 800.0), (2600.0, 900.0)];
        engine.clutch_capacity_nm = 850.0;
        assert!(spec.validate().is_ok());
    }

    /// The servo window contract (`ServoSpec::admission_fault`): the sim clamps to the window
    /// so one that is inverted or excludes the spawn angle 0 is refused
    /// at load, the same refusal `certify_asset` reports as `L2.SPEC`.
    #[test]
    fn validate_rejects_a_servo_window_that_excludes_its_spawn_angle() {
        use crate::tank::{ServoRole, ServoSpec};
        for (min, max) in [(5.0, -5.0), (5.0, 20.0), (-20.0, -5.0), (f32::NAN, 1.0)] {
            let mut spec = fixture();
            spec.servos.insert(
                "Bad_Pitch".to_owned(),
                ServoSpec::test_limited(ServoRole::Pitch, 10.0, min, max),
            );
            let err = spec.validate().unwrap_err().to_string();
            assert!(err.contains("Bad_Pitch"), "window ({min}, {max}): {err}");
        }
        let mut spec = fixture();
        spec.servos.insert(
            "Good_Pitch".to_owned(),
            ServoSpec::test_limited(ServoRole::Pitch, 10.0, -8.0, 15.0),
        );
        assert!(spec.validate().is_ok());
    }

    /// Transmission-block runtime invariants: non-finite capacities NaN out the
    /// brake engagement scaling, hunting shift bands and unordered ladders break the shift
    /// logic's assumptions, and the u8 gear index must be able to address every gear. Each
    /// rejection is named.
    #[test]
    fn validate_rejects_broken_transmission_blocks() {
        fn engine(tr: &mut TransmissionSpec) -> &mut EngineSpec {
            &mut tr.engine
        }
        fn gearbox(tr: &mut TransmissionSpec) -> &mut GearboxSpec {
            &mut tr.gearbox
        }
        let cases: [(&str, fn(&mut TransmissionSpec)); 15] = [
            // Engine audio data: both are DIVISORS in the playback law — a 0-cylinder engine pops
            // at 0 Hz (silence played at speed 0) and a 0 Hz recording makes the anchor infinite.
            ("cylinders", |tr| engine(tr).cylinders = 0),
            ("clip_pop_hz", |tr| {
                engine(tr)
                    .sound
                    .as_mut()
                    .expect("the fixture authors a sound block")
                    .clip_pop_hz = 0.0;
            }),
            ("clip_pop_hz", |tr| {
                engine(tr)
                    .sound
                    .as_mut()
                    .expect("the fixture authors a sound block")
                    .clip_pop_hz = f32::NAN;
            }),
            // Coupling requires positive crank inertia and finite clutch capacity.
            ("inertia_kgm2", |tr| engine(tr).inertia_kgm2 = 0.0),
            ("clutch_capacity_nm", |tr| {
                engine(tr).clutch_capacity_nm = f32::NAN;
            }),
            // The game's driving model requires idle above its authored stall-guard band;
            // the drag regularisation independently reaches rest.
            ("engine.idle_rpm floor", |tr| {
                engine(tr).idle_rpm = f32::from_bits(1)
            }),
            ("steering capacity", |tr| {
                tr.steering.capacity = f32::INFINITY
            }),
            ("brake_force", |tr| tr.brake_force = f32::INFINITY),
            ("brake_static_factor", |tr| {
                tr.brake_static_factor = f32::NAN
            }),
            ("brake_static_factor", |tr| tr.brake_static_factor = 0.99),
            ("launch_gears", |tr| gearbox(tr).launch_gears = 0),
            ("max_step", |tr| gearbox(tr).max_step = 0),
            ("ladder shape", |tr| {
                gearbox(tr).forward_speeds_kmh.swap(2, 3);
            }),
            // 300 ascending reverse gears: passes ordering and hysteresis, but cannot be
            // addressed by the runtime's u8 gear index.
            ("ladder shape", |tr| {
                gearbox(tr).reverse_speeds_kmh = (1..=300).map(|i| i as f32).collect();
                tr.steering.reverse_radii = vec![(4.0, 12.0); 300]; // Fixture rows isolate index admission.
            }),
            ("drag_fraction", |tr| engine(tr).drag_fraction = 1.5),
        ];
        for (needle, mutate) in cases {
            let mut spec = fixture();
            mutate(fixture_transmission(&mut spec));
            let err = spec.validate().unwrap_err().to_string();
            assert!(err.contains(needle), "expected `{needle}` in: {err}");
        }
        // A block missing one of its tables is refused at parse, by name: every table is
        // required data, so no law runs on an absent one.
        let missing = FIXTURE_RON.replace("brake_static_factor: 1.5,", "");
        assert_ne!(
            missing, FIXTURE_RON,
            "the fixture authors brake_static_factor"
        );
        let err = ron::de::from_str::<TankSpec>(&missing)
            .err()
            .expect("a block missing a table must not parse")
            .to_string();
        assert!(
            err.contains("brake_static_factor"),
            "missing brake_static_factor should be named: {err}"
        );
    }

    /// The asset door relies on its own declared drivetrain remaining valid. Read that
    /// declaration from its owner, including its small finite crank inertia.
    #[test]
    fn blender_fixture_drivetrain_meets_the_generic_admission() {
        let source = include_str!("../.agents/blender/fixture_tank.py");
        let (_, declaration) = source
            .split_once("SPEC = ")
            .expect("the Blender fixture owns a literal SPEC declaration");
        let declaration = declaration.trim_start_matches('"').trim_start_matches('\\');
        let (declaration, _) = declaration
            .split_once("\"\"\"")
            .expect("the Blender fixture SPEC declaration must close");
        let spec: TankSpec = ron::de::from_str(declaration).unwrap();
        spec.validate().unwrap();
    }

    /// FIXTURE: wrap radii for [`fixture`]'s running gear: a sprocket pitch radius and the
    /// fixture idler's 0.4 m contact radius plus a 0.1 m pin offset.
    fn lab_wrap(sprocket: f32) -> WrapRadii {
        WrapRadii {
            sprocket,
            idler: SideValues {
                left: 0.5,
                right: 0.5,
            },
        }
    }

    /// The belt integrator and brake predictor rely on every construction term, each at the
    /// radius the belt turns it at. FIXTURE DATA: a link loop, one roadwheel per side, and
    /// distinct sprocket/idler moments and radii. A mutant omitting any wheel family, dividing
    /// a road wheel by a pin-line radius or the wrapped idler by its contact radius, breaks the
    /// exact sum.
    #[test]
    fn reflected_inertia_is_the_sum_of_links_and_each_wheels_contact_term() {
        let mut spec = fixture();
        let inertia = |s: &TankSpec| {
            s.track
                .reflected_inertia(&s.roadwheels, lab_wrap(0.5))
                .unwrap()
        };
        // DERIVED from FIXTURE DATA: 80 * 20 + 4 / 0.5^2 + 4 / 0.5^2 + 8 / 0.4^2.
        let base = inertia(&spec);
        assert!((base - 1682.0).abs() < 1e-3);
        spec.track.link_mass.0 *= 2.0;
        assert!((inertia(&spec) - base - 1600.0).abs() < 1e-3);
        spec.track.sprocket.spin_inertia_kgm2.0 = SideValues {
            left: 8.0,
            right: 8.0,
        };
        assert!((inertia(&spec) - base - 1616.0).abs() < 1e-3);
        spec.track.idler.spin_inertia_kgm2.0 = SideValues {
            left: 8.0,
            right: 8.0,
        };
        assert!((inertia(&spec) - base - 1632.0).abs() < 1e-3);
        for wheel in &mut spec.roadwheels {
            wheel.spin_inertia_kgm2.0 *= 2.0;
        }
        assert!((inertia(&spec) - base - 1682.0).abs() < 1e-3);
    }

    /// Load admission protects both drive divisions and the shared-side inertia premise.
    /// FIXTURE DATA: each invalid moment and an intentionally asymmetric station assembly.
    #[test]
    fn malformed_or_unrepresentable_rotating_construction_is_refused_before_play() {
        for value in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            for family in 0..3 {
                let mut spec = fixture();
                let field = match family {
                    0 => {
                        spec.track.sprocket.spin_inertia_kgm2.0.left = value;
                        "track.sprocket.spin_inertia_kgm2"
                    }
                    1 => {
                        spec.track.idler.spin_inertia_kgm2.0.right = value;
                        "track.idler.spin_inertia_kgm2"
                    }
                    _ => {
                        spec.roadwheels[0].spin_inertia_kgm2.0 = value;
                        "roadwheels.Wheel_L_0.spin_inertia_kgm2"
                    }
                };
                let err = spec.validate().unwrap_err().to_string();
                assert!(err.contains(field), "{field}: {err}");
            }
        }
        let mut spec = fixture();
        spec.roadwheels[0].spin_inertia_kgm2.0 *= 2.0;
        assert!(
            spec.validate().is_ok(),
            "the spec gate has no physical sprocket radius"
        );
        let err = spec
            .track
            .reflected_inertia(&spec.roadwheels, lab_wrap(0.5))
            .unwrap_err()
            .to_string();
        assert!(err.contains("differs by side"), "{err}");
        let mut spec = fixture();
        spec.track.link_mass.0 = f32::MAX;
        let err = spec.validate().unwrap_err().to_string();
        assert!(err.contains("derived reflected inertia"), "{err}");
        for declaration in [
            "spin_inertia_kgm2: ((left: 4.0, right: 4.0), Fixture(\"fixture sprockets\")), ",
            "spin_inertia_kgm2: ((left: 4.0, right: 4.0), Fixture(\"fixture idlers\")), ",
            "spin_inertia_kgm2: (8.0, Fixture(\"fixture station\")), ",
        ] {
            let missing = FIXTURE_RON.replace(declaration, "");
            assert_ne!(missing, FIXTURE_RON);
            let err = ron::from_str::<TankSpec>(&missing)
                .err()
                .expect("required wheel fact")
                .to_string();
            assert!(err.contains("spin_inertia_kgm2"), "{err}");
        }
    }

    /// Spawn's crank seed and structural admission must not reject construction whose side
    /// totals balance at the real pitch radius. FIXTURE DATA: unequal sprocket moments balanced
    /// by a roadwheel term at the declared lab radius, but not at nominal unit geometry.
    #[test]
    fn side_inertia_balance_is_checked_at_the_actual_sprocket_radius() {
        let mut spec = fixture();
        spec.track.sprocket.spin_inertia_kgm2.0.left = 8.0;
        spec.roadwheels[0].spin_inertia_kgm2.0 = 5.44;
        assert!(spec.validate().is_ok());
        assert!(
            spec.track
                .nominal_transmission_params(&spec.roadwheels)
                .is_ok()
        );
        assert!(
            spec.track
                .transmission_params(&spec.roadwheels, lab_wrap(0.5), 1.0)
                .is_ok()
        );
        assert!(
            spec.track
                .transmission_params(&spec.roadwheels, lab_wrap(1.0), 1.0)
                .is_err()
        );
    }

    /// Every field a law reads is the sheet's to state, so omitting one is refused at parse, naming
    /// it, never filled from code (ADR-0011). A mutant restoring a `#[serde(default)]` on any of
    /// these parses the cut sheet and fails here; the gates, the drivetrain and the shadow pass all
    /// read the sheet's own value.
    #[test]
    fn a_sheet_omitting_a_required_field_does_not_parse() {
        for (field, authored, kept) in [
            ("drag_fraction", "drag_fraction: 0.25,", ""),
            ("shift_secs", "shift_secs: 0.3,", ""),
            ("launch_gears", "launch_gears: 4,", ""),
            ("max_step", "max_step: 3,", ""),
            ("shadow_proxies", "shadow_proxies: [],", ""),
            (
                "capabilities",
                "capabilities: { Drive: [Engine, Transmission] },",
                "",
            ),
            ("drive_controls", "drive_controls: NotRequired,", ""),
            ("fire", "fire: [Gunner, Breech],", ""),
            ("load", "load: [Gunner],", ""),
            (
                "requires",
                "travel: Continuous, requires: [Gunner]",
                "travel: Continuous",
            ),
            (
                "requires",
                "field_deg: 25.0), requires: [Gunner]",
                "field_deg: 25.0)",
            ),
        ] {
            let cut = FIXTURE_RON.replacen(authored, kept, 1);
            assert_ne!(cut, FIXTURE_RON, "the fixture authors `{authored}`");
            let err = ron::de::from_str::<TankSpec>(&cut)
                .err()
                .unwrap_or_else(|| panic!("a sheet without `{field}` must not parse"))
                .to_string();
            assert!(err.contains(field), "the refusal names `{field}`: {err}");
        }
    }

    /// `validate()` refuses a sheet whose construction cannot serve its own gates, naming the field:
    /// a requirement naming a part no declared volume provides (the gate could never open), two
    /// volumes seating one crew station (a swap addresses a seat by station, so the authority and
    /// the replica could pick different seats), and a recoil spring without its barrel. Mutant: a
    /// `validate` that drops any of the three loads its case.
    #[test]
    fn validate_refuses_a_sheet_whose_construction_cannot_serve_its_gates() {
        let mut unprovided = fixture();
        unprovided
            .weapons
            .get_mut("Cannon")
            .expect("the fixture's cannon")
            .load = vec![Group::Single(Part::Loader)];
        let err = unprovided.validate().unwrap_err().to_string();
        assert!(
            err.contains("weapons[\"Cannon\"].load") && err.contains("Loader"),
            "{err}"
        );

        let mut doubled = fixture();
        let gunner = doubled.volumes["Gunner"].clone();
        doubled.volumes.insert("Gunner_Spare".to_owned(), gunner);
        let err = doubled.validate().unwrap_err().to_string();
        assert!(err.contains("Gunner") && err.contains("station"), "{err}");

        let mut unsprung = fixture();
        unsprung
            .weapons
            .get_mut("Cannon")
            .expect("the fixture's cannon")
            .barrel = None;
        let err = unsprung.validate().unwrap_err().to_string();
        assert!(err.contains("Cannon") && err.contains("recoil"), "{err}");
    }

    /// The game's damage model: empty cooperative or backup groups are asset faults,
    /// never a panic first discovered by a live requirement consumer.
    #[test]
    fn validate_refuses_empty_graded_member_lists() {
        for group in [GradedGroup::Backup(vec![]), GradedGroup::Pool(vec![])] {
            let mut spec = fixture();
            spec.capabilities
                .insert(Capability::Drive, vec![Group::Graded(group)]);
            let error = spec.validate().unwrap_err().to_string();
            assert!(
                error.contains("capabilities[Drive]") && error.contains("member"),
                "{error}"
            );
        }
    }

    /// A member's coefficient is its share (`Pool`) or ceiling (`Backup`) of full capability, so
    /// `validate()` refuses one outside (0, 1], NaN included, naming the field: a ceiling above 1
    /// would serve a gate past full, and a zero or NaN one is a member that serves nothing. Mutant:
    /// a check that drops either bound, or lets NaN through, loads its case; the 1.0 control
    /// proves the refusal is the coefficient's.
    #[test]
    fn validate_refuses_a_member_coefficient_outside_a_share() {
        let with_load_coefficient = |coefficient: f32| {
            let mut spec = fixture();
            spec.weapons
                .get_mut("Cannon")
                .expect("the fixture's cannon")
                .load = vec![Group::Graded(GradedGroup::Backup(vec![(
                coefficient,
                vec![Part::Gunner],
            )]))];
            spec.validate()
        };
        for coefficient in [0.0, 1.5, f32::NAN] {
            let err = with_load_coefficient(coefficient).unwrap_err().to_string();
            assert!(
                err.contains("weapons[\"Cannon\"].load") && err.contains("(0, 1]"),
                "{coefficient}: {err}"
            );
        }
        with_load_coefficient(1.0).expect("a full ceiling is a share");
    }

    /// The game's damage model: rate consumers must never discover an unrepresentable
    /// reload during play. Fixture coefficients exercise the smallest positive-rate horizon.
    #[test]
    fn load_work_outside_the_reduced_rate_horizon_is_refused_before_play() {
        for coefficient in [1e-12, f32::MIN_POSITIVE] {
            let mut spec = fixture();
            spec.weapons.get_mut("Cannon").unwrap().load = vec![Group::Graded(
                GradedGroup::Backup(vec![(coefficient, vec![Part::Gunner])]),
            )];
            let error = spec.validate().unwrap_err().to_string();
            assert!(
                error.contains("weapons[\"Cannon\"].load") && error.contains("deadline horizon"),
                "{error}"
            );
        }
    }

    /// The game's damage model: the load gate independently needs both contributor floors.
    /// Fixture work fits at full rate but crosses the horizon at either reduced floor.
    #[test]
    fn load_horizon_requires_both_module_and_crew_floors() {
        let mut admitted = Vec::new();
        for part in [Part::Breech, Part::Gunner] {
            let mut spec = fixture();
            let weapon = spec.weapons.get_mut("Cannon").unwrap();
            weapon.load = vec![Group::Single(part)];
            weapon.fire_cycle = FireCycle::Single {
                reload: TickSpan::of(TickSpan::MAX / 4 * 3),
            };
            match spec.validate() {
                Ok(()) => admitted.push(part),
                Err(error) => assert!(
                    error.to_string().contains("deadline horizon"),
                    "{part:?}: {error}"
                ),
            }
        }
        assert!(
            admitted.is_empty(),
            "missing contributor floors admitted: {admitted:?}"
        );
    }

    /// The game's damage model: zero authored work needs no reduced-rate horizon budget.
    /// Fixture tiny positive competence would stretch a one-tick guard beyond that horizon.
    #[test]
    fn zero_load_work_does_not_budget_the_emission_guard_as_work() {
        let mut spec = fixture();
        for weapon in spec.weapons.values_mut() {
            weapon.load = vec![Group::Graded(GradedGroup::Backup(vec![(
                1e-12,
                vec![Part::Gunner],
            )]))];
            match &mut weapon.fire_cycle {
                FireCycle::Single { reload } => *reload = TickSpan::of(0),
                FireCycle::Automatic { swap, .. } => *swap = TickSpan::of(0),
            }
        }
        spec.validate()
            .expect("zero load work stays zero at every rate");
    }

    /// A capability the sheet does not key would read as unavailable to every consumer, so
    /// `validate` refuses the sheet that omits one.
    #[test]
    fn validate_refuses_a_sheet_missing_a_capability() {
        let mut spec = fixture();
        spec.capabilities.clear();
        let err = spec.validate().unwrap_err().to_string();
        assert!(
            err.contains("Drive"),
            "the refusal names the capability: {err}"
        );
    }

    /// Weapon mounts are keyed construction; a repeated key must never conceal a declaration.
    #[test]
    fn duplicate_weapon_keys_are_refused() {
        let text = FIXTURE_RON.replace("weapons: {", "weapons: {\n        \"MG\": (trigger: Secondary, muzzle: \"Duplicate\", bore_length: (1.0, Fixture(\"fixture\")), gun: \"mg34\", stowage: [(round: \"7.9mm_AP\", count: (10, Fixture(\"fixture\")))], fire_mode: Automatic(belt_size: 10, belt_swap_secs: 1.0), fire: [], load: []),");
        assert!(
            ron::from_str::<TankSpec>(&text).is_err(),
            "repeated mounts must refuse during parsing"
        );
    }

    /// The game's driving and damage model: Drive must name both physical sources it reads;
    /// otherwise a valid task path could conceal an absent or emptied source from the HUD.
    #[test]
    fn drive_requires_each_physical_source() {
        for missing in [Part::Engine, Part::Transmission] {
            let mut spec = fixture();
            spec.capabilities.insert(
                Capability::Drive,
                [Part::Engine, Part::Transmission]
                    .into_iter()
                    .filter(|part| *part != missing)
                    .map(Group::Single)
                    .collect(),
            );
            let error = spec
                .validate()
                .expect_err("an omitted drive source must refuse before play")
                .to_string();
            assert!(
                error.contains("capabilities[Drive]") && error.contains(&format!("{missing:?}")),
                "{error}"
            );
        }
    }

    /// The game's driving and damage model: naming a source only on one alternative must
    /// not conceal a bypass; additional dependencies remain generic and legal.
    #[test]
    fn drive_source_dependencies_cover_alternatives_and_allow_extra_parts() {
        let mut spec = fixture();
        for make in [GradedGroup::Pool, GradedGroup::Backup] {
            spec.capabilities.insert(
                Capability::Drive,
                vec![
                    Group::Single(Part::Transmission),
                    Group::Graded(make(vec![
                        (0.5, vec![Part::Engine]),
                        (0.5, vec![Part::Breech]),
                    ])),
                ],
            );
            assert!(spec.validate().unwrap_err().to_string().contains("Engine"));
            spec.capabilities
                .get_mut(&Capability::Drive)
                .unwrap()
                .push(Group::Single(Part::Engine));
            spec.validate()
                .expect("mandatory sources may accompany any declared dependency");
        }
    }

    /// The game's damage model: control staffing must name complete, declared station paths
    /// before play, so the Drive adapter never discovers an impossible authority rule.
    #[test]
    fn validate_refuses_incomplete_or_absent_control_staffing() {
        // Fixture: empty alternatives and a station absent from the fixture sheet.
        let mut spec = fixture();
        let absent = [
            CrewStation::Commander,
            CrewStation::Gunner,
            CrewStation::Loader,
            CrewStation::Driver,
            CrewStation::BowGunner,
        ]
        .into_iter()
        .find(|station| {
            !spec
                .volumes
                .values()
                .filter_map(VolumeSpec::component)
                .any(|volume| volume.crew == Some(*station))
        })
        .expect("the fixture leaves a station undeclared");
        for paths in [vec![], vec![vec![]], vec![vec![absent]]] {
            spec.drive_controls = ControlStaffing::Any(paths);
            let err = spec.validate().unwrap_err().to_string();
            assert!(err.contains("drive_controls"), "{err}");
        }
    }

    /// Silence is authorable: an engine with no `sound` block is a vehicle that runs no loop, not
    /// an authoring omission — the field is optional and validation passes without it.
    #[test]
    fn an_engine_without_a_sound_block_is_legal_silence() {
        let mut spec = fixture();
        fixture_transmission(&mut spec).engine.sound = None;
        spec.validate()
            .expect("a vehicle may declare no engine recording");
    }
}
