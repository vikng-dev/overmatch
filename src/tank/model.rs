use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::servo::ServoState;
use crate::command::TankCommand;
use crate::damage::Requirement;
use crate::sim_tick::{SimTick, TickSpan};
use crate::spec::{FireCycle, RecoilSpec, Trigger, ViewKind};
use crate::track::sim::{TrackContacts, TrackDrive, TrackGrip};

#[derive(Component)]
pub struct Turret;

/// The turret's rigid-body data, built at spawn from the sheet's `turret` block, so the launch
/// reads it from the entity rather than from the asset.
#[derive(Component, Clone, Copy)]
pub struct TurretBody {
    /// Mass (kg).
    pub mass: f32,
    /// Full box dimensions (m) standing in for the angular-inertia distribution.
    pub inertia_extents: Vec3,
}

#[derive(Component)]
pub struct Gun;

#[derive(Component)]
pub struct Hull;

/// Local simulation root. Command and drive state are required in the same construction flush.
#[derive(Component)]
#[require(TankCommand, TrackDrive, TrackGrip, TrackContacts)]
pub struct Tank;

/// Selects the tank that local input and player-facing systems address.
#[derive(Component)]
pub struct Controlled;

/// No controlled tank is a legitimate state before spawn and between possessions. Multiple
/// matches violate local input ownership and must never read as that absence.
pub(crate) fn controlled_one<T>(
    result: Result<T, bevy::ecs::query::QuerySingleError>,
) -> Option<T> {
    match result {
        Ok(one) => Some(one),
        Err(bevy::ecs::query::QuerySingleError::NoEntities(_)) => None,
        Err(bevy::ecs::query::QuerySingleError::MultipleEntities(_)) => {
            panic!("a player controls at most one tank; multiple Controlled entities matched")
        }
    }
}

/// Required rig handles, resolved and validated during complete construction.
#[derive(Component)]
pub struct Rig {
    pub hull: Entity,
    pub turret: Entity,
    pub gun: Entity,
    pub muzzle: Entity,
}

/// Runtime camera configuration and availability gate for one authored view.
pub struct ViewConfig {
    /// Vertical field of view (RADIANS), taken from the authored [`crate::spec::Optics`] at spawn —
    /// the authored degrees have already become radians by the time they reach a camera.
    pub fov: f32,
    pub requires: Requirement,
}

/// Both player views, complete at spawn. A missing authored kind is refused by the spec gate.
#[derive(Component)]
pub struct TankViews {
    pub gunner: ViewConfig,
    pub commander: ViewConfig,
}

impl TankViews {
    pub fn get(&self, kind: ViewKind) -> &ViewConfig {
        match kind {
            ViewKind::Gunner => &self.gunner,
            ViewKind::Commander => &self.commander,
        }
    }
}

#[derive(Component)]
pub struct Muzzle;

/// The recoiling barrel node (child of `Gun`, parent of `Muzzle`).
#[derive(Component)]
pub struct GunBarrel;

/// Runtime weapon configuration attached to its muzzle.
#[derive(Component)]
pub struct Weapon {
    /// Logical authored name, distinct from the muzzle node name.
    pub name: String,
    /// The game's damage model: validated distance from the muzzle back to the bore start (m).
    pub bore_length: f32,
    /// Every round the weapon can load, in authored order; index 0 is the default. A slot's
    /// [`WeaponLoad`] and a fired round's `ShotSource::round` index this list.
    pub rounds: Vec<WeaponRound>,
    /// The mount's reload or belt joined with its gun's mechanism, and with them the
    /// edge-versus-level input semantics.
    pub fire_cycle: FireCycle,
    pub recoil: Option<RecoilSpec>,
    pub barrel: Option<Entity>,
    /// Damage/crew gates for firing and loading.
    pub fire: Requirement,
    pub load: Requirement,
    /// Command channel for this weapon; independent of its mechanism.
    pub trigger: Trigger,
    /// The gun's report clips (asset-relative paths); empty = a silent weapon. `sfx` rolls one per
    /// round off this list.
    pub report_clips: Vec<String>,
}

/// One round a weapon can load, resolved at spawn: its name from the round registry
/// (`assets/rounds/`), the round itself fired from the gun that chambers it (`assets/guns/`).
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponRound {
    /// The registry key: the data's handle, never shown to a player.
    pub key: String,
    /// The registry's display name, drawn whole by the shipped font.
    pub name: String,
    /// The body the force law binds, the muzzle speed and the wound rate, its burst set to the
    /// fuze's default mode.
    pub(crate) round: crate::rounds::Round,
    /// The round's fuze as authored, whose modes the crew picks among; `None` for an inert round.
    pub(crate) fuze: Option<crate::rounds::fuze::Spec>,
}

impl WeaponRound {
    /// The round as fired in fuze mode `index`: its burst carries that mode's delay. `None` for an
    /// index its fuze does not list, and for any index but 0 on an inert round: the server's and
    /// the receiver's bound check on a mode index, never a fallback to the default.
    pub(crate) fn in_mode(&self, index: u8) -> Option<crate::rounds::Round> {
        let mut round = self.round;
        match (&self.fuze, round.burst.as_mut()) {
            (Some(fuze), Some(burst)) => burst.delay = fuze.delay_s(index)?,
            (None, None) if index == 0 => {}
            _ => return None,
        }
        round.fuze_mode = index;
        Some(round)
    }

    /// The modes the crew can pick among: the fuze's list, empty for an inert round.
    pub(crate) fn fuze_modes(&self) -> &[crate::rounds::fuze::Mode] {
        self.fuze.as_ref().map_or(&[], |fuze| &fuze.modes)
    }

    /// The shipped round keyed `key`, fired from the shipped gun `gun`, or the error naming either.
    pub(crate) fn fired_from(gun: &str, key: &str) -> Result<Self, crate::guns::GunError> {
        Self::named(key, |guns| guns.round(gun, key))
    }

    /// The shipped round keyed `key`, fired from the one shipped gun that chambers it, or the
    /// error naming it: the handle the tests and the dev capture's shot cue fire by.
    pub(crate) fn resolve(key: &str) -> Result<Self, crate::guns::GunError> {
        Self::named(key, |guns| guns.round_by_key(key))
    }

    /// The round keyed `key`, named and fuzed by the round registry, fired as `round` gives it.
    fn named(
        key: &str,
        round: impl FnOnce(
            &crate::guns::GunRegistry,
        ) -> Result<crate::rounds::Round, crate::guns::GunError>,
    ) -> Result<Self, crate::guns::GunError> {
        let registry = crate::rounds::RoundRegistry::shipped();
        let name = registry.name(key).map_err(crate::guns::GunError::Round)?;
        let fuze = registry
            .get(key)
            .map_err(crate::guns::GunError::Round)?
            .fuze;
        Ok(Self {
            key: key.to_owned(),
            name: name.to_owned(),
            round: round(crate::guns::GunRegistry::shipped())?,
            fuze,
        })
    }
}

impl Weapon {
    /// The weapon `name` as `spec` authors it: every stowed round fired from the mount's gun, the
    /// mount's fire mode joined with the gun's mechanism, the gun's report, its recoiling barrel
    /// `barrel`. An error names what does not resolve; on a validated spec this is `Ok`.
    pub(crate) fn of_spec(
        name: &str,
        spec: &crate::spec::WeaponSpec,
        barrel: Option<Entity>,
    ) -> Result<Self, String> {
        let rounds = spec
            .stowage
            .iter()
            .map(|stowed| WeaponRound::fired_from(&spec.gun, &stowed.round))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| err.to_string())?;
        let report_clips = crate::guns::GunRegistry::shipped()
            .gun(&spec.gun)
            .map_err(|err| err.to_string())?
            .report_clips()
            .to_vec();
        Ok(Self {
            name: name.to_owned(),
            bore_length: spec.bore_length.0,
            rounds,
            fire_cycle: spec.fire_cycle,
            recoil: spec.recoil.clone(),
            barrel,
            fire: spec.fire.clone(),
            load: spec.load.clone(),
            trigger: spec.trigger,
            report_clips,
        })
    }

    /// Whether `state` is a load in progress: a `Single` reloading, an `Automatic` with a dry belt
    /// awaiting its swap. The window in which [`WeaponLoad::loaded`] follows the selection.
    pub fn is_loading(&self, state: WeaponGateState) -> bool {
        match self.fire_cycle {
            FireCycle::Single { .. } => !state.is_ready(),
            FireCycle::Automatic { .. } => state.belt_remaining == 0,
        }
    }
}

/// The tank's engine recording and the facts its playback speed is derived from — spec data on the
/// root, carried by every tank with an authored `engine.sound` (local, headless, and replicated
/// alike). Its absence is silence. The rpm→playback-speed law lives in `sfx`.
#[derive(Component, Clone)]
pub struct EngineSound {
    /// Asset-relative path to the seamless loop.
    pub clip: String,
    /// Cylinder pops per second in the recording at playback speed 1.0 (MEASURED).
    pub clip_pop_hz: f32,
    /// Cylinders in the engine — with the four-stroke cycle, the crank's firing rate.
    pub cylinders: u32,
    /// The crank band the recording is mapped across.
    pub idle_rpm: f32,
    pub governed_rpm: f32,
}

/// Which track a roadwheel drives (for differential thrust). Left wheels sit at −X, right at +X.
/// `Deserialize` because each station's side is AUTHORED in `<tank>.tank.ron` now
/// ([`crate::spec::RoadwheelSpec`]) rather than parsed out of the node name.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize)]
pub enum TrackSide {
    Left,
    Right,
}

/// Load-bearing belt-contact station; sprockets and idlers are excluded.
#[derive(Component)]
pub struct Roadwheel {
    pub side: TrackSide,
}

/// Direct owner link for rig-part systems that need root state.
#[derive(Component)]
pub struct TankRoot(pub Entity);

/// Local per-weapon rollback state that does not decide whether a round may fire. The authoritative
/// fire gate lives in the separately replicated [`WeaponGate`] component.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct WeaponState {
    pub recoil_offset: f32,
    pub recoil_velocity: f32,
    /// Belt phase used to derive tracer cadence. It rolls back, but stays outside the determinism
    /// hash because a phase mismatch is cosmetic and does not gate simulation.
    pub rounds_fired: u32,
}

/// The game's damage model: a positive load rate, bounded by full capability.
/// NaN and zero cannot reach deadline division, including from a peer. The checked domain
/// contains no NaN, so equality is reflexive and the atomic gate can implement `Eq`.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(try_from = "f32")]
pub struct LoadRate(f32);

impl Eq for LoadRate {}

impl Default for LoadRate {
    fn default() -> Self {
        Self(1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InvalidLoadRate(f32);

impl std::fmt::Display for InvalidLoadRate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "load rate {} must be finite and in (0, 1]", self.0)
    }
}

impl TryFrom<f32> for LoadRate {
    type Error = InvalidLoadRate;
    fn try_from(rate: f32) -> Result<Self, Self::Error> {
        if rate > 0.0 && rate <= 1.0 {
            Ok(Self(rate))
        } else {
            Err(InvalidLoadRate(rate))
        }
    }
}

impl LoadRate {
    pub fn get(self) -> f32 {
        self.0
    }
}

/// The game's damage model: one weapon's fire eligibility and belt supply. Its timing enum
/// makes an unarmed-but-paused state unrepresentable, including at the peer deserialization edge.
/// Paused work is load work, a `Single`'s reload or a dry belt's swap, so a pause holds an empty
/// belt; decoding refuses a paused gate with rounds left ([`WeaponGateError`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(try_from = "WeaponGateWire")]
pub struct WeaponGateState {
    pub timing: WeaponTiming,
    pub belt_remaining: u32,
    /// Rate used to derive the active deadline, retained through a pause.
    pub load_rate: LoadRate,
}

/// [`WeaponGateState`]'s decoded fields before its phase rule is checked: the same fields in the
/// same order, so the encoding is the state's own.
#[derive(Deserialize)]
struct WeaponGateWire {
    timing: WeaponTiming,
    belt_remaining: u32,
    load_rate: LoadRate,
}

/// A decoded gate no authority can produce.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WeaponGateError {
    /// Frozen load work beside a belt with rounds left: an `Automatic` cycling its belt has no
    /// crew-gated work to freeze, and a `Single` holds no belt.
    PausedWithRounds(u32),
}

impl std::fmt::Display for WeaponGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PausedWithRounds(rounds) => write!(
                f,
                "a paused weapon gate holds {rounds} belt rounds; paused load work holds an empty belt"
            ),
        }
    }
}

impl TryFrom<WeaponGateWire> for WeaponGateState {
    type Error = WeaponGateError;
    fn try_from(wire: WeaponGateWire) -> Result<Self, Self::Error> {
        match wire.timing {
            WeaponTiming::Paused { .. } if wire.belt_remaining > 0 => {
                Err(WeaponGateError::PausedWithRounds(wire.belt_remaining))
            }
            timing => Ok(Self {
                timing,
                belt_remaining: wire.belt_remaining,
                load_rate: wire.load_rate,
            }),
        }
    }
}

/// The game's damage model: a weapon is ready, loading against a wrapping deadline, or frozen
/// with a checked work span. Serde parses this phase and refuses an invalid span at its edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum WeaponTiming {
    #[default]
    Ready,
    Loading {
        ready_tick: SimTick,
    },
    Paused {
        work: TickSpan,
        since: SimTick,
    },
}

impl WeaponGateState {
    /// Loaded spawn state; automatic weapons start with a full belt.
    pub fn for_cycle(cycle: &FireCycle) -> Self {
        Self {
            timing: WeaponTiming::Ready,
            load_rate: LoadRate::default(),
            belt_remaining: match cycle {
                FireCycle::Single { .. } => 0,
                FireCycle::Automatic { belt_size, .. } => *belt_size,
            },
        }
    }

    pub fn is_ready(self) -> bool {
        matches!(self.timing, WeaponTiming::Ready)
    }

    /// The active deadline, or a paused work clock for diagnostics. A pause has no active
    /// deadline ordering; its first unmet tick is retained in the displayed work count.
    pub fn ready_tick(self) -> Option<SimTick> {
        match self.timing {
            WeaponTiming::Ready => None,
            WeaponTiming::Loading { ready_tick } => Some(ready_tick),
            WeaponTiming::Paused { work, since } => Some(since.after(work).after(TickSpan::ONE)),
        }
    }

    pub fn paused_at_tick(self) -> Option<SimTick> {
        match self.timing {
            WeaponTiming::Paused { since, .. } => Some(since),
            WeaponTiming::Ready | WeaponTiming::Loading { .. } => None,
        }
    }

    pub fn finish(&mut self) {
        self.timing = WeaponTiming::Ready;
    }

    /// Every active comparison uses the shared signed wrapped relation, as the battle cycle
    /// does. Frozen work is not eligible until its load requirement allows resumption.
    pub fn deadline_reached(self, now: SimTick) -> bool {
        match self.timing {
            WeaponTiming::Ready => true,
            WeaponTiming::Loading { ready_tick } => now.has_reached(ready_tick),
            WeaponTiming::Paused { .. } => false,
        }
    }

    /// At least one tick prevents duplicate emission in a tick.
    pub fn arm(&mut self, now: SimTick, delay: TickSpan) {
        self.load_rate = LoadRate::default();
        self.timing = WeaponTiming::Loading {
            ready_tick: now.after(delay.at_least_one()),
        };
    }

    /// The game's damage model: arm base work at its current capability rate. No available
    /// worker freezes the base work immediately; a later positive rate resumes that same work.
    pub fn arm_load(&mut self, now: SimTick, base: TickSpan, effectiveness: f32) {
        self.arm(now, base);
        self.load_at_rate(now, effectiveness);
    }

    /// The game's damage model: at a rate change, preserve completed work and re-derive the
    /// remaining interval once (`old remaining × old rate / new rate`, ceiling to a tick).
    /// Unchanged rates leave the absolute deadline untouched; paused ticks consume no work.
    pub fn load_at_rate(&mut self, now: SimTick, effectiveness: f32) {
        if effectiveness == 0.0 {
            self.pause(now);
            return;
        }
        let rate = LoadRate::try_from(effectiveness)
            .expect("requirement evaluation serves effectiveness in [0, 1]");
        self.resume(now);
        if self.load_rate != rate {
            if let WeaponTiming::Loading { ready_tick } = self.timing {
                let remaining = f64::from(now.remaining_until(ready_tick))
                    * f64::from(self.load_rate.get())
                    / f64::from(rate.get());
                assert!(
                    remaining.ceil() <= f64::from(TickSpan::MAX),
                    "TankSpec admits load work only within the deadline horizon at every positive rate"
                );
                self.timing = WeaponTiming::Loading {
                    ready_tick: now.after(TickSpan::of(remaining.ceil() as u32)),
                };
            }
            self.load_rate = rate;
        }
    }

    /// Freeze load work on entry. Repeated unmet ticks leave the gate byte-identical, and a past
    /// arriving deadline holds no remaining work rather than almost a full counter's interval.
    pub fn pause(&mut self, now: SimTick) {
        if let WeaponTiming::Loading { ready_tick } = self.timing {
            assert_eq!(
                self.belt_remaining, 0,
                "only load work pauses: a Single holds no belt and an Automatic pauses a dry belt's swap"
            );
            let work = TickSpan::try_from(now.remaining_until(ready_tick)).expect(
                "remaining_until is a nonnegative signed difference, within TickSpan's horizon",
            );
            self.timing = WeaponTiming::Paused { work, since: now };
        }
    }

    /// A frozen interval survives any number of clock wraps; only the resume tick starts it.
    pub fn resume(&mut self, now: SimTick) {
        if let WeaponTiming::Paused { work, .. } = self.timing {
            self.timing = WeaponTiming::Loading {
                ready_tick: now.after(work),
            };
        }
    }

    pub fn remaining_ticks(self, now: SimTick) -> u32 {
        match self.timing {
            WeaponTiming::Ready => 0,
            WeaponTiming::Loading { ready_tick } => now.remaining_until(ready_tick),
            // The entry tick cannot consume work, including at the greatest admitted span.
            WeaponTiming::Paused { work, .. } => work.raw() + 1,
        }
    }
}

/// Tick-correlated authority state for every weapon slot, in deterministic name-sorted slot order.
/// The server owns it and replicates it privately to the owner. The owner's fire-presentation
/// ledger carries its local feedback beside the stream; no client predicts gameplay state.
#[derive(Component, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct WeaponGate {
    pub weapons: Vec<WeaponGateState>,
}

/// One weapon's round choice, as indices into its [`Weapon::rounds`].
///
/// Invariant (the next-load rule): `loaded` is the round the weapon's next shot fires, and it
/// changes only while the weapon [is loading](Weapon::is_loading), when it follows `selected`. A
/// chambered round — a ready `Single`, a belt with rounds left — is fired as it was loaded,
/// whatever is selected after. A selection made while a reload has not completed applies to that
/// reload; one made in the tick a reload completes waits for the next load.
///
/// The fuze mode is read when the round is fired, not when it is loaded
/// ([`WeaponLoad::fuze_mode`]).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct WeaponLoad {
    pub loaded: u8,
    pub selected: u8,
    /// The fuze mode the crew picked for each round type, by index into [`Weapon::rounds`], each an
    /// index into that round's fuze modes. Empty until the crew first toggles a mode: a type with
    /// no entry is on its fuze's default mode.
    pub fuze_modes: Vec<u8>,
}

impl WeaponLoad {
    /// The fuze mode of round `index` of `rounds`: the crew's pick, or the round's default mode
    /// while there is none. `None` for an index past the list.
    pub fn fuze_mode(&self, rounds: &[WeaponRound], index: u8) -> Option<u8> {
        let round = rounds.get(usize::from(index))?;
        Some(
            self.fuze_modes
                .get(usize::from(index))
                .copied()
                .unwrap_or(round.round.fuze_mode),
        )
    }

    /// Step round `index`'s fuze mode to the next in its fuze's list, wrapping. A round whose fuze
    /// lists one mode or none, and an index past the list, have nothing to toggle.
    pub(crate) fn toggle_fuze(&mut self, rounds: &[WeaponRound], index: u8) {
        let Some(round) = rounds.get(usize::from(index)) else {
            return;
        };
        let count = round.fuze_modes().len();
        let Some(next) = self
            .fuze_mode(rounds, index)
            .and_then(|mode| crate::command::RoundStep::Next.apply(mode, count))
            .filter(|_| count > 1)
        else {
            return;
        };
        if self.fuze_modes.len() != rounds.len() {
            // Every type's mode made explicit, each on what it reads as now.
            self.fuze_modes = (0..=u8::MAX)
                .take(rounds.len())
                .map(|i| self.fuze_mode(rounds, i).unwrap_or_default())
                .collect();
        }
        self.fuze_modes[usize::from(index)] = next;
    }
}

/// The round choice for every weapon slot, in the name-sorted slot order of [`WeaponGate`]. Built at
/// spawn from the spec with every slot on its default round (index 0). Only the authority writes it
/// (`shooting::apply_round_selection`, from the command's `select_round` edge); it replicates to the
/// owner beside `WeaponGate`, and a client reads it and never writes it, so a selection is in force
/// only once the authority's value arrives.
#[derive(Component, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WeaponLoads {
    pub weapons: Vec<WeaponLoad>,
}

/// Tick-correlated servo integrator state in deterministic name-sorted [`super::ServoIndex`] order.
/// This is the complete minimal state needed to replay the fixed-step mechanism: each slot carries
/// its current angle, previous angle, and angular velocity. The authority replicates the atomic
/// component to the owner; prediction restores it at the producing tick before replay.
#[derive(Component, Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TankServos {
    pub states: Vec<ServoState>,
}

impl TankServos {
    pub(crate) fn for_count(count: usize) -> Self {
        Self {
            states: vec![ServoState::default(); count],
        }
    }
}

/// Local weapon rollback state lives on the replicated root. Children carry deterministic,
/// name-sorted indices into this vector and derive their transforms from the restored root state.
/// Servo state is authoritative in the separately replicated [`TankServos`] component.
#[derive(Component, Clone, PartialEq, Debug, Default)]
pub struct TankSim {
    pub weapons: Vec<WeaponState>,
}

/// This weapon's slot in [`TankSim::weapons`] — on the muzzle AND the recoiling barrel (both
/// actuate from the same weapon state), assigned at spawn in sorted-name order.
#[derive(Component, Clone, Copy)]
pub struct WeaponIndex(pub usize);

/// Compose tick-truth local transforms from the physics root. Simulation must not read
/// `GlobalTransform`, which belongs to the interpolated render frame. Returns `None` when the node
/// is no longer under `root`.
pub(crate) fn rig_world_pose<F: bevy::ecs::query::QueryFilter>(
    node: Entity,
    root: Entity,
    root_position: Vec3,
    root_rotation: Quat,
    parents: &Query<&ChildOf>,
    locals: &Query<&Transform, F>,
) -> Option<(Vec3, Quat)> {
    let mut chain = Vec::new();
    let mut entity = node;
    while entity != root {
        chain.push(entity);
        entity = parents.get(entity).ok()?.parent();
    }
    let mut position = root_position;
    let mut rotation = root_rotation;
    for &link in chain.iter().rev() {
        let local = locals.get(link).ok()?;
        position += rotation * local.translation;
        rotation *= local.rotation;
    }
    Some((position, rotation))
}
