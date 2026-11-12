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

/// The turret's rigid-body data, built at spawn from the sheet's `turret` block and the tank's
/// geometry, so the launch reads it from the entity rather than from the asset.
#[derive(Component, Clone, Copy)]
pub struct TurretBody {
    /// Mass (kg).
    pub mass: f32,
    /// The sheet's turret box ([`crate::spec::TurretSpec::angular_inertia`]).
    pub angular_inertia: avian3d::prelude::AngularInertia,
    /// The assembly's mass centre in its node's frame (m), a StandIn for its substances' densities:
    /// the centroid of its hard volumes at rest, every volume one density and an overlap counted
    /// twice ([`crate::ballistics::armour_set::ArmourSet::mass_centre`]). The sheet states no
    /// centre and `materials.ron` no density for most of its substances; per-substance densities,
    /// or an authored centre node as the hull has, replace it.
    pub centre_of_mass: Vec3,
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
    /// Detachable structural assembly. It need not itself be a servo.
    pub turret: Entity,
    /// The Gunner's yaw servo, resolved from the sheet independently of the structural assembly.
    /// Sight margins and the public aim-angle stream address this servo.
    pub gunner_yaw: Entity,
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
    /// IEEE binary32 exactly: value = mantissa / 2^shift. The checked rate is positive,
    /// finite and at most one, so both normal and subnormal forms have a positive shift.
    fn ratio(self) -> (u32, u32) {
        let bits = self.0.to_bits();
        let exponent = (bits >> 23) & 0xff;
        let fraction = bits & 0x7f_ffff;
        if exponent == 0 {
            (fraction, 149)
        } else {
            (fraction | (1 << 23), 150 - exponent)
        }
    }
}

/// The game's damage model: a normalized share of one full load.
/// Resolution: Q63 gives a work quantum below 2^-32 ticks over the admitted signed horizon,
/// and permits exact u128 ceiling arithmetic for binary32 rates. It never invents work at a
/// pause boundary. The upper bound leaves invalid wire values for the checked constructor.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(try_from = "u64")]
pub struct LoadShare(u64);

const FULL_LOAD_WORK: u64 = 1_u64 << 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidLoadShare(u64);
impl std::fmt::Display for InvalidLoadShare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "load work units {} exceed one full load ({FULL_LOAD_WORK})",
            self.0
        )
    }
}
impl TryFrom<u64> for LoadShare {
    type Error = InvalidLoadShare;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value <= FULL_LOAD_WORK {
            Ok(Self(value))
        } else {
            Err(InvalidLoadShare(value))
        }
    }
}

/// The game's damage model: a bounded amount of work, its retained rate and its reference
/// base. The reference converts elapsed ticks into completed work; a new deadline always
/// uses the local weapon's admitted base, never a peer-authored duration.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(try_from = "LoadWorkWire")]
pub struct LoadWork {
    share: LoadShare,
    rate: LoadRate,
    base: TickSpan,
}
#[derive(Deserialize)]
struct LoadWorkWire {
    share: LoadShare,
    rate: LoadRate,
    base: TickSpan,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadWorkError {
    ZeroBase,
}
impl std::fmt::Display for LoadWorkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroBase => write!(
                f,
                "load work needs a positive reference base; zero work uses the duplicate-emission interval"
            ),
        }
    }
}
impl TryFrom<LoadWorkWire> for LoadWork {
    type Error = LoadWorkError;
    fn try_from(wire: LoadWorkWire) -> Result<Self, Self::Error> {
        if wire.base.raw() == 0 {
            return Err(LoadWorkError::ZeroBase);
        }
        Ok(Self {
            share: wire.share,
            rate: wire.rate,
            base: wire.base,
        })
    }
}
impl LoadWork {
    pub fn share(self) -> u64 {
        self.share.0
    }
    pub fn rate(self) -> f32 {
        self.rate.get()
    }
    pub fn base(self) -> TickSpan {
        self.base
    }
    fn advance(self, since: SimTick, now: SimTick) -> Self {
        // A peer's anchor can be ahead of the presentation clock; no work has elapsed yet.
        let elapsed = (now.raw().wrapping_sub(since.raw()) as i32).max(0) as u32;
        let (mantissa, shift) = self.rate.ratio();
        let product = u128::from(elapsed) * u128::from(mantissa);
        let (numerator, denominator) = if shift <= 63 {
            (product << (63 - shift), u128::from(self.base.raw()))
        } else {
            (product, u128::from(self.base.raw()) << (shift - 63))
        };
        // Complete partial work quanta rather than rounding them into new work. Saturation
        // is the law's max(remaining - completed, 0), including an already-due deadline.
        let consumed = numerator.div_ceil(denominator);
        let remaining = u128::from(self.share.0).saturating_sub(consumed) as u64;
        Self {
            share: LoadShare(remaining),
            ..self
        }
    }
    fn at_rate(self, base: TickSpan, rate: LoadRate) -> (Self, TickSpan) {
        let (mantissa, shift) = rate.ratio();
        // An admitted positive base fits at its minimum rate, so the rate is at least
        // 1 / TickSpan::MAX; its binary32 denominator shift is therefore at most 54.
        assert!(
            shift <= 63,
            "TankSpec admits the local base at every positive requirement rate"
        );
        let denominator = u128::from(mantissa) << (63 - shift);
        let ticks = (u128::from(self.share.0) * u128::from(base.raw())).div_ceil(denominator);
        assert!(
            ticks <= u128::from(TickSpan::MAX),
            "TankSpec admits the local base work at every positive requirement rate; a decoded work share is at most one"
        );
        (Self { base, rate, ..self }, TickSpan::of(ticks as u32))
    }
}

/// The game's damage model: one weapon's fire eligibility and belt supply. Only load states
/// carry work and its rate; ready gates and cyclic/duplicate-emission intervals have neither.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(try_from = "WeaponGateWire")]
pub struct WeaponGateState {
    pub timing: WeaponTiming,
    pub belt_remaining: u32,
}
#[derive(Deserialize)]
struct WeaponGateWire {
    timing: WeaponTiming,
    belt_remaining: u32,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WeaponGateError {
    LoadWithRounds(u32),
}
impl std::fmt::Display for WeaponGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoadWithRounds(rounds) => write!(
                f,
                "a weapon load holds {rounds} belt rounds; load work holds an empty belt"
            ),
        }
    }
}
impl TryFrom<WeaponGateWire> for WeaponGateState {
    type Error = WeaponGateError;
    fn try_from(wire: WeaponGateWire) -> Result<Self, Self::Error> {
        if matches!(
            wire.timing,
            WeaponTiming::Loading { .. } | WeaponTiming::Paused { .. }
        ) && wire.belt_remaining > 0
        {
            return Err(WeaponGateError::LoadWithRounds(wire.belt_remaining));
        }
        Ok(Self {
            timing: wire.timing,
            belt_remaining: wire.belt_remaining,
        })
    }
}

/// The game's damage model: an interval is mechanism cadence or a zero-work emission guard.
/// Load work is bounded at decoding, independently of the peer's clock or cached deadline.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum WeaponTiming {
    #[default]
    Ready,
    Interval {
        ready_tick: SimTick,
    },
    Loading {
        ready_tick: SimTick,
        since: SimTick,
        work: LoadWork,
    },
    Paused {
        remaining: TickSpan,
        since: SimTick,
        work: LoadWork,
    },
}
impl WeaponGateState {
    pub fn for_cycle(cycle: &FireCycle) -> Self {
        Self {
            timing: WeaponTiming::Ready,
            belt_remaining: match cycle {
                FireCycle::Single { .. } => 0,
                FireCycle::Automatic { belt_size, .. } => *belt_size,
            },
        }
    }
    pub fn is_ready(self) -> bool {
        matches!(self.timing, WeaponTiming::Ready)
    }
    pub fn ready_tick(self) -> Option<SimTick> {
        match self.timing {
            WeaponTiming::Ready => None,
            WeaponTiming::Interval { ready_tick } | WeaponTiming::Loading { ready_tick, .. } => {
                Some(ready_tick)
            }
            WeaponTiming::Paused {
                remaining, since, ..
            } => Some(since.after(remaining)),
        }
    }
    pub fn paused_at_tick(self) -> Option<SimTick> {
        match self.timing {
            WeaponTiming::Paused { since, .. } => Some(since),
            WeaponTiming::Ready | WeaponTiming::Interval { .. } | WeaponTiming::Loading { .. } => {
                None
            }
        }
    }
    pub fn load_work(self) -> Option<(LoadWork, SimTick)> {
        match self.timing {
            WeaponTiming::Loading { work, since, .. }
            | WeaponTiming::Paused { work, since, .. } => Some((work, since)),
            WeaponTiming::Ready | WeaponTiming::Interval { .. } => None,
        }
    }
    pub fn finish(&mut self) {
        self.timing = WeaponTiming::Ready;
    }
    pub fn deadline_reached(self, now: SimTick) -> bool {
        match self.timing {
            WeaponTiming::Ready => true,
            WeaponTiming::Interval { ready_tick } | WeaponTiming::Loading { ready_tick, .. } => {
                now.has_reached(ready_tick)
            }
            WeaponTiming::Paused { .. } => false,
        }
    }
    /// At least one tick prevents duplicate emission; it is not rate-dependent work.
    pub fn arm(&mut self, now: SimTick, delay: TickSpan) {
        self.timing = WeaponTiming::Interval {
            ready_tick: now.after(delay.at_least_one()),
        };
    }
    /// The game's damage model: the zero-work guard stays one tick even with no worker.
    pub fn arm_load(&mut self, now: SimTick, base: TickSpan, effectiveness: f32) {
        if base.raw() == 0 {
            self.arm(now, base);
            return;
        }
        let work = LoadWork {
            share: LoadShare(FULL_LOAD_WORK),
            rate: LoadRate::default(),
            base,
        };
        self.timing = WeaponTiming::Loading {
            ready_tick: now.after(base),
            since: now,
            work,
        };
        self.load_at_rate(now, base, effectiveness);
    }
    /// The game's damage model: preserve completed work, then derive the deadline from the
    /// bounded remaining share and the local weapon's admitted base. Unchanged rates keep
    /// their absolute deadline. See design §7b for the timing rule.
    pub fn load_at_rate(&mut self, now: SimTick, base: TickSpan, effectiveness: f32) {
        if base.raw() == 0 {
            if self.load_work().is_some() {
                self.arm(now, base);
            }
            return;
        }
        if effectiveness == 0.0 {
            self.pause(now);
            return;
        }
        let rate = LoadRate::try_from(effectiveness)
            .expect("requirement evaluation serves effectiveness in [0, 1]");
        let work = match self.timing {
            WeaponTiming::Loading { since, work, .. } if work.rate != rate => {
                work.advance(since, now)
            }
            WeaponTiming::Paused { work, .. } => work,
            // These phases owe no re-derivation; an unchanged rate retains its deadline.
            WeaponTiming::Ready | WeaponTiming::Interval { .. } | WeaponTiming::Loading { .. } => {
                return;
            }
        };
        let (work, delay) = work.at_rate(base, rate);
        self.timing = WeaponTiming::Loading {
            ready_tick: now.after(delay),
            since: now,
            work,
        };
    }
    pub fn pause(&mut self, now: SimTick) {
        if let WeaponTiming::Loading {
            ready_tick,
            since,
            work,
        } = self.timing
        {
            assert_eq!(
                self.belt_remaining, 0,
                "only load work pauses: a Single holds no belt and an Automatic pauses a dry belt's swap"
            );
            self.timing = WeaponTiming::Paused {
                remaining: TickSpan::of(now.remaining_until(ready_tick)),
                since: now,
                work: work.advance(since, now),
            };
        }
    }
    pub fn remaining_ticks(self, now: SimTick) -> u32 {
        match self.timing {
            WeaponTiming::Ready => 0,
            WeaponTiming::Interval { ready_tick } | WeaponTiming::Loading { ready_tick, .. } => {
                now.remaining_until(ready_tick)
            }
            WeaponTiming::Paused { remaining, .. } => remaining.raw(),
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
