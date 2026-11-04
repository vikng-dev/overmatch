//! A burst round's derived burst ([`Burst::derived`]) and the fuze the march advances per shell
//! ([`Fuze`]). Every burst is a ring: the wall around the burst core is driven as a band about the
//! flight axis, and the nose piece flies on. The burst's story — the trigger, the ring, the derived
//! masses and speeds — is design doc §5a, "The burst"
//! (`.agents/docs/design/armor-penetration-and-damage.md`); the march that runs the fuze and throws
//! the burst's field is the parent module's.
//!
//! Units: masses kg, lengths m, speeds m/s, the delay s, the arming threshold reference-mm, the
//! impact sensitivity N, the energy J. Everything here is derived once, at load, in f64 with IEEE `+`, `−`, `×`, `/` and
//! `sqrt` and deterministic `libm::cbrt`, and stored in f32, so a round's burst is the same bits on every target.

use bevy::prelude::{Dir3, Entity, Vec3};

use super::RoundBody;
use crate::rounds::fuze::Trigger;

/// A burst block as the round registry resolves it, with its core substance's burst facts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BurstInputs {
    /// What functions the fuze: the selected fuze's trigger.
    pub(crate) trigger: Trigger,
    /// Seconds from functioning to the burst: the selected mode's delay.
    pub(crate) delay: f32,
    /// The burst core's mass `C`.
    pub(crate) core_mass: f64,
    /// The core substance's density (kg/m³).
    pub(crate) core_density: f64,
    /// The core substance's speed constant `K`.
    pub(crate) speed_constant: f64,
    /// The chemical energy one kilogram of the core releases (J/kg), its row's required datum.
    pub(crate) chemical_energy: f64,
    /// The parts at the base, behind the cavity.
    pub(crate) base_mass: f64,
    /// The cavity's diameter.
    pub(crate) cavity_diameter: f64,
    /// Authored distance from the tip to the filler centre along the negative flight axis (m).
    pub(crate) filler_setback: f64,
}

/// The round's parts, DERIVED from its body, its body steel's density and its burst block: the
/// cavity is a cylinder of the core's volume `C / ρ_core` and the block's diameter; the wall is
/// the steel annulus around it over its length, outer diameter the calibre; the nose is the rest,
/// `m − wall − base − C`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Parts {
    pub(crate) cavity_length: f64,
    pub(crate) wall_mass: f64,
    pub(crate) nose_mass: f64,
}

impl Parts {
    /// The parts of `round` of body steel `steel_density` (kg/m³) under `inputs`; refuses a cavity
    /// as wide as the round, and parts that leave no nose.
    pub(crate) fn of(
        round: RoundBody,
        steel_density: f64,
        inputs: &BurstInputs,
    ) -> Result<Self, String> {
        let caliber = round.caliber as f64;
        let cavity = inputs.cavity_diameter;
        if cavity >= caliber {
            return Err(format!(
                "the cavity's diameter {cavity} m is not inside the calibre {caliber} m"
            ));
        }
        let quarter_pi = std::f64::consts::FRAC_PI_4;
        let cavity_length = inputs.core_mass / inputs.core_density / (quarter_pi * cavity * cavity);
        let wall_mass =
            steel_density * quarter_pi * (caliber * caliber - cavity * cavity) * cavity_length;
        let nose_mass = round.mass as f64 - wall_mass - inputs.base_mass - inputs.core_mass;
        if nose_mass.is_nan() || nose_mass <= 0.0 {
            return Err(format!(
                "the wall ({wall_mass} kg), the base parts ({} kg) and the core ({} kg) leave no \
                 nose of the round's {} kg",
                inputs.base_mass, inputs.core_mass, round.mass
            ));
        }
        Ok(Self {
            cavity_length,
            wall_mass,
            nose_mass,
        })
    }
}

/// A round's burst, derived at load from its burst block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Burst {
    /// What functions the fuze ([`Fuze`]).
    pub(crate) trigger: Trigger,
    /// Seconds from functioning to the burst: from the arming exit for `SpendExit`, from contact
    /// for `Impact`. [`crate::tank::WeaponRound::in_mode`] sets it to the fired mode's.
    pub(crate) delay: f32,
    /// The nose piece, which flies on from a burst in flight through the march as a round of its
    /// own: its mass the derived nose's, its calibre and nose factor the round's.
    pub(crate) nose: RoundBody,
    /// `v_s = K · (M / C + ½)^−½`, the greatest sideways speed the burst gives its driven mass `M`,
    /// the wall.
    pub(crate) sideways_speed: f32,
    /// `½ M v_s²` (J), the driven mass's kinetic energy across the burst: what the burst's
    /// potency is scaled from.
    pub(crate) energy: f32,
    /// The burst core's mass `C` (kg).
    pub(crate) core_mass: f32,
    /// The blast's energy (J): the core's chemical energy less what the casing took,
    /// `C · e − ½ M v_s²`; a core whose chemistry cannot pay its casing is refused at load. What
    /// the constructions' blast field carries.
    pub(crate) blast: f32,
    /// Authored distance behind the tip (m), used by the environment airblast.
    pub(crate) filler_setback: f32,
    /// Equal-volume filler-sphere radius (m), DERIVED from core mass and density. The blast's
    /// distance floor keeps its point-source fit outside that charge.
    pub(crate) charge_radius: f32,
}

/// The scalar momentum (N·s) of a burst core's detonation products, `C` kg of them expanding
/// at `speed` m/s at their outer surface: `(2/3) C v`. DERIVED from the burst law's own Gurney
/// profile, the products' speed rising linearly from the axis to the driven wall, which is what
/// gives `v_s = K (M/C + ½)^−½` its `½`. A cased core's outer speed is its casing's `v_s`; a bare
/// one's is the same law with no casing, `K √2`. The game's environment blast field reads it
/// near the charge (`construction::fracture::Blast::products_floor`).
pub(crate) fn products_momentum(core_mass: f32, speed: f32) -> f32 {
    2.0 / 3.0 * core_mass * speed
}

impl Burst {
    /// The game's environment blast originates at the authored filler centre, while the fuze's
    /// march position remains the tip. The field's geometry is independent of fuze timing.
    pub(crate) fn filler_center(&self, tip: Vec3, axis: Dir3) -> Vec3 {
        tip - axis * self.filler_setback
    }

    /// Its detonation products' scalar momentum, N·s ([`products_momentum`] at the casing's
    /// `v_s`): positive, refused at load otherwise.
    pub(crate) fn products_momentum(&self) -> f32 {
        products_momentum(self.core_mass, self.sideways_speed)
    }

    /// The burst of `round` (its whole body, of body steel `steel_density`) under `inputs`; refuses
    /// a non-finite or non-positive input and parts that leave no nose ([`Parts::of`]).
    pub(crate) fn derived(
        round: RoundBody,
        steel_density: f64,
        inputs: &BurstInputs,
    ) -> Result<Self, String> {
        for (field, value) in [
            ("core mass", inputs.core_mass),
            ("core density", inputs.core_density),
            ("speed constant", inputs.speed_constant),
            ("base mass", inputs.base_mass),
            ("cavity diameter", inputs.cavity_diameter),
            ("filler setback", inputs.filler_setback),
            ("chemical energy", inputs.chemical_energy),
            ("body steel density", steel_density),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{field} must be finite and positive (got {value})"));
            }
        }
        let threshold = match inputs.trigger {
            Trigger::SpendExit { arming_spend } => arming_spend,
            Trigger::Impact { sensitivity_n } => sensitivity_n,
        };
        for (field, value) in [
            ("fuze threshold", f64::from(threshold)),
            ("delay", f64::from(inputs.delay)),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format!(
                    "{field} must be finite and not negative (got {value})"
                ));
            }
        }
        let parts = Parts::of(round, steel_density, inputs)?;
        let charge_radius =
            libm::cbrt(3.0 * inputs.core_mass / (4.0 * std::f64::consts::PI * inputs.core_density))
                as f32;
        let filler_setback = inputs.filler_setback as f32;
        if !charge_radius.is_finite()
            || charge_radius <= 0.0
            || !filler_setback.is_finite()
            || filler_setback < charge_radius
        {
            return Err(
                "filler_setback must be finite and at least the positive equal-volume \
                charge radius in simulation units; author the filler centre behind the tip"
                    .into(),
            );
        }
        let driven = parts.wall_mass;
        let sideways_speed = inputs.speed_constant / (driven / inputs.core_mass + 0.5).sqrt();
        let casing_energy = 0.5 * driven * sideways_speed * sideways_speed;
        let chemical = inputs.core_mass * inputs.chemical_energy;
        if !chemical.is_finite() || chemical < casing_energy {
            return Err(
                "chemical energy must cover the casing kinetic energy; author consistent \
                core chemistry and speed_constant from their sources"
                    .into(),
            );
        }
        let blast = chemical - casing_energy;
        for (field, value) in [
            ("sideways_speed", sideways_speed),
            ("casing kinetic energy", casing_energy),
            ("casing ejection momentum", driven * sideways_speed),
        ] {
            let stored = value as f32;
            if !stored.is_finite() || stored <= 0.0 {
                return Err(format!(
                    "{field} must be finite and positive in simulation units; \
                    author usable core mass, cavity diameter and speed_constant with their sources"
                ));
            }
        }
        let products = products_momentum(inputs.core_mass as f32, sideways_speed as f32);
        if !products.is_finite() || products <= 0.0 {
            return Err(
                "the detonation products' momentum (2/3 core mass x sideways_speed) is not \
                positive and finite in simulation units; author usable core mass and \
                speed_constant with their sources"
                    .into(),
            );
        }
        if !(blast as f32).is_finite() {
            return Err(
                "chemical energy leaves an unrepresentable airblast energy; author a finite \
                energy in simulation units with its source"
                    .into(),
            );
        }
        Ok(Self {
            trigger: inputs.trigger,
            delay: inputs.delay,
            nose: RoundBody {
                mass: parts.nose_mass as f32,
                ..round
            },
            sideways_speed: sideways_speed as f32,
            energy: casing_energy as f32,
            core_mass: inputs.core_mass as f32,
            filler_setback,
            charge_radius,
            blast: blast as f32,
        })
    }
}

/// A shell's fuze: built at spawn from its round's burst (ADR-0014), advanced by the authority's
/// march alone. A replica's march decides no crossing, so its fuze never arms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fuze {
    /// No burst ahead: a round without a burst block, or a nose piece flying on from its burst.
    Inert,
    /// Not yet functioning: no body has met the trigger (`Trigger`).
    Unarmed(Burst),
    /// Armed: the round bursts after `remaining` more metres of path. At or below zero the delay
    /// has run out and the burst is due where the round stands: nothing restarts it. `speed` is
    /// the speed the count is kept at, which turns those metres into the delay's seconds left.
    /// `tank` owns the body that armed the fuze or last restarted it, for attribution only. The
    /// game's damage-model fields reach matter independently of tank ownership.
    ///
    /// The two triggers keep the count differently. `SpendExit` counts metres from the speed it
    /// armed at, and a body met before the burst restarts it. `Impact` counts seconds: a change of
    /// speed rescales the metres left ([`Fuze::at_speed`]), and no later body restarts it.
    Armed {
        burst: Burst,
        remaining: f32,
        speed: f32,
        tank: Option<Entity>,
    },
}

impl Fuze {
    /// The fuze a round of `burst` is fired with.
    pub(crate) fn of(burst: Option<Burst>) -> Self {
        burst.map_or(Self::Inert, Self::Unarmed)
    }

    /// The burst still ahead, `None` once the round has burst or never carried one.
    pub(crate) fn burst(&self) -> Option<Burst> {
        match *self {
            Self::Inert => None,
            Self::Unarmed(burst) | Self::Armed { burst, .. } => Some(burst),
        }
    }

    /// The game's damage model: the round left a body with `plate_met` material-weighted
    /// reference-mm of real chord, at `speed`, the body owned by
    /// `tank`. An unarmed `SpendExit` fuze arms when `plate_met` reaches its threshold; an armed one
    /// whose delay still runs restarts whatever the plate met. Either way the burst then lies
    /// `speed × delay` metres of path past this exit. A delay that has run out is not restarted,
    /// and an exit neither arms nor restarts an `Impact` fuze.
    pub(crate) fn exited(&mut self, plate_met: f32, speed: f32, tank: Option<Entity>) {
        let burst = match *self {
            Self::Inert => return,
            Self::Unarmed(burst) | Self::Armed { burst, .. }
                if matches!(burst.trigger, Trigger::Impact { .. }) =>
            {
                return;
            }
            Self::Unarmed(burst) => match burst.trigger {
                Trigger::SpendExit { arming_spend } if plate_met < arming_spend => return,
                _ => burst,
            },
            Self::Armed { remaining, .. } if remaining <= 0.0 => return,
            Self::Armed { burst, .. } => burst,
        };
        *self = Self::Armed {
            burst,
            remaining: speed * burst.delay,
            speed,
            tank,
        };
    }

    /// The game's damage model: the round meets a face of `tank`'s and leaves or rests at `speed`.
    /// An armed `SpendExit` fuze whose delay still runs restarts there. This method arms no fuze
    /// and restarts neither a due delay nor an `Impact` fuze.
    pub(crate) fn bounced(&mut self, speed: f32, tank: Option<Entity>) {
        if let Self::Armed {
            burst,
            remaining,
            speed: previous_speed,
            ..
        } = *self
            && remaining > 0.0
            && matches!(burst.trigger, Trigger::SpendExit { .. })
        {
            // The game's damage model: a held round waits out the restarted delay in time.
            let reference_speed = if speed > 0.0 { speed } else { previous_speed };
            *self = Self::Armed {
                burst,
                remaining: reference_speed * burst.delay,
                speed: reference_speed,
                tank,
            };
        }
    }

    /// The round meets a body `approach` metres of path ahead, which resists it with `force_n`
    /// newtons (the walk's dE/dx at entry) at `speed`, the body owned by `tank`. An unarmed
    /// `Impact` fuze functions when the force reaches its sensitivity: the burst then lies
    /// `speed × delay` metres past the contact, so `approach + speed × delay` from where the round
    /// stands. Any other fuze is left as it is: an `Impact` fuze functions once.
    pub(crate) fn entered(
        &mut self,
        force_n: f64,
        speed: f32,
        tank: Option<Entity>,
        approach: f32,
    ) {
        if let Self::Unarmed(burst) = *self
            && let Trigger::Impact { sensitivity_n } = burst.trigger
            && force_n >= f64::from(sensitivity_n)
        {
            *self = Self::Armed {
                burst,
                remaining: approach + speed * burst.delay,
                speed,
                tank,
            };
        }
    }

    /// The round now flies at `next` m/s: an armed `Impact` fuze rescales its metres left so the
    /// seconds left are unchanged. A `SpendExit` count is metres and is left as it is.
    pub(crate) fn at_speed(&mut self, next: f32) {
        if let Self::Armed {
            burst,
            remaining,
            speed,
            ..
        } = self
            && matches!(burst.trigger, Trigger::Impact { .. })
            && *speed > 0.0
            && next > 0.0
        {
            *remaining = (*remaining / *speed) * next;
            *speed = next;
        }
    }

    /// The round flew `metres` of path and met no body.
    pub(crate) fn flown(&mut self, metres: f32) {
        if let Self::Armed { remaining, .. } = self {
            *remaining -= metres;
        }
    }

    /// The game's damage model: wait at world rest for up to `seconds`, using the armed count's
    /// retained clock speed. If the delay expires, leave it in place and return true so the
    /// burst record can read the remaining wait; otherwise consume those seconds. An inert or
    /// unarmed round has no delay to advance.
    pub(crate) fn wait_at_rest(&mut self, seconds: f32) -> bool {
        if let Self::Armed {
            remaining, speed, ..
        } = self
        {
            let elapsed = *speed * seconds;
            if *remaining <= elapsed {
                return true;
            }
            *remaining -= elapsed;
        }
        false
    }

    /// Metres along the next `metres` of path where the round bursts, when an armed fuze runs out
    /// within them.
    pub(crate) fn due_within(&self, metres: f32) -> Option<f32> {
        match *self {
            Self::Armed { remaining, .. } if remaining <= metres => Some(remaining.max(0.0)),
            _ => None,
        }
    }

    /// The delay's seconds left, when the fuze is armed: the metres left at the speed the delay
    /// started at. A round that stopped waits them out where it lies.
    pub(crate) fn delay_left(&self) -> Option<f32> {
        match *self {
            Self::Armed {
                remaining, speed, ..
            } => Some(if speed > 0.0 {
                (remaining / speed).max(0.0)
            } else {
                0.0
            }),
            _ => None,
        }
    }

    /// The burst and its tank, when the fuze is armed.
    pub(crate) fn armed(&self) -> Option<(Burst, Option<Entity>)> {
        match *self {
            Self::Armed { burst, tank, .. } => Some((burst, tank)),
            _ => None,
        }
    }
}

/// Body steel density (kg/m³): fixture data of the shipped body steel's magnitude.
#[cfg(test)]
pub(super) const FIXTURE_STEEL: f64 = 7800.0;

/// A burst block whose numbers are fixture data of the shipped rounds' magnitude, but for a delay
/// long enough that the fixtures' bodies, metres apart, lie inside it.
#[cfg(test)]
pub(super) fn fixture_inputs() -> BurstInputs {
    BurstInputs {
        trigger: Trigger::SpendExit { arming_spend: 35.0 },
        delay: 0.012,
        core_mass: 0.06,
        core_density: 1650.0,
        speed_constant: 2680.0,
        chemical_energy: 4.184e6,
        base_mass: 0.33,
        cavity_diameter: 0.0357,
        filler_setback: 0.18,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEEL: f64 = FIXTURE_STEEL;

    /// A fixture round of 10 kg and 88 mm under [`fixture_inputs`].
    fn fixture() -> (RoundBody, BurstInputs) {
        (
            RoundBody {
                mass: 10.0,
                caliber: 0.088,
                nose_factor: 0.5,
            },
            fixture_inputs(),
        )
    }

    fn burst() -> Burst {
        let (round, inputs) = fixture();
        Burst::derived(round, STEEL, &inputs).unwrap()
    }

    /// The parts account for the whole round: wall, nose, base and core sum to its mass, and the
    /// cavity holds the core's volume. The nose piece, which the march flies on as, reads the nose
    /// mass with the round's calibre and nose; a mutant that drops a part from the subtraction, or
    /// flies the nose at another mass, fails here.
    #[test]
    fn the_parts_sum_to_the_round() {
        let (round, inputs) = fixture();
        let parts = Parts::of(round, STEEL, &inputs).unwrap();
        let sum = parts.wall_mass + parts.nose_mass + inputs.base_mass + inputs.core_mass;
        assert!((sum - round.mass as f64).abs() < 1e-12, "{sum}");
        let cavity = std::f64::consts::FRAC_PI_4
            * inputs.cavity_diameter
            * inputs.cavity_diameter
            * parts.cavity_length;
        assert!((cavity * inputs.core_density - inputs.core_mass).abs() < 1e-15);
        let burst = Burst::derived(round, STEEL, &inputs).unwrap();
        assert_eq!(
            burst.nose,
            RoundBody {
                mass: parts.nose_mass as f32,
                ..round
            }
        );
    }

    /// THE GAME'S ENVIRONMENT AIRBLAST STARTS AT THE AUTHORED FILLER CENTRE. The tip remains
    /// the fuze's march position; changing a sourced setback changes only the origin metadata.
    /// The contact field relies on the source lying behind the face instead of on it. A tip
    /// origin, a forward offset, or a radius inferred from blast energy breaks this invariant.
    /// ASSUMED fixture geometry and filler data.
    #[test]
    fn the_filler_center_and_radius_come_from_the_burst_data() {
        let (round, mut inputs) = fixture();
        let radius =
            (3.0 * inputs.core_mass / (4.0 * std::f64::consts::PI * inputs.core_density)).cbrt();
        let tip = Vec3::new(3.0, 4.0, 5.0);
        for setback in [0.12, 0.3] {
            inputs.filler_setback = setback;
            let burst = Burst::derived(round, STEEL, &inputs).unwrap();
            assert!((f64::from(burst.charge_radius) / radius - 1.0).abs() < 1e-6);
            assert_eq!(
                burst.filler_center(tip, Dir3::NEG_Z),
                tip + Vec3::Z * setback as f32
            );
        }
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY, radius / 2.0, 1e100] {
            inputs.filler_setback = invalid;
            let error = Burst::derived(round, STEEL, &inputs).unwrap_err();
            assert!(error.contains("filler"), "{error}");
        }
    }

    /// EACH JOULE OF THE GAME'S BURST CHEMISTRY IS COUNTED ONCE (study test 5). The environment
    /// airblast and casing loads rely on `blast + casing_KE = C e`, to f32 storage precision. A
    /// source that restores the casing's share to the blast, or counts another driven mass,
    /// breaks this invariant. ASSUMED fixture charges and round construction.
    #[test]
    fn each_joule_of_chemistry_is_counted_once() {
        let (round, mut inputs) = fixture();
        inputs.chemical_energy = 4.184e6;
        let mut worst = 0.0_f64;
        for mass in [0.02, 0.06, 0.12] {
            inputs.core_mass = mass;
            let parts = Parts::of(round, STEEL, &inputs).unwrap();
            let burst = Burst::derived(round, STEEL, &inputs).unwrap();
            let chemical = inputs.core_mass * inputs.chemical_energy;
            let casing = 0.5 * parts.wall_mass * f64::from(burst.sideways_speed).powi(2);
            let residual =
                ((f64::from(burst.blast) + f64::from(burst.energy)) / chemical - 1.0).abs();
            worst = worst.max(residual);
            assert!(residual <= f64::from(f32::EPSILON), "{residual}");
            assert!(
                (f64::from(burst.energy) / casing - 1.0).abs() <= 2.0 * f64::from(f32::EPSILON)
            );
        }
        println!("MEASURED chemistry split: maximum relative residual {worst:.6e}");
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 0.0, 1.0, f64::MAX] {
            inputs.chemical_energy = invalid;
            let error = Burst::derived(round, STEEL, &inputs).unwrap_err();
            assert!(error.contains("chemical energy"), "{error}");
        }
    }

    /// `v_s = K (M/C + ½)^−½` with `M` the wall, and the energy `½ M v_s²`: the burst's field ratio
    /// and its potency read both. A mutant that drives another mass than the wall fails here.
    #[test]
    fn the_sideways_speed_follows_the_driven_mass() {
        let (round, inputs) = fixture();
        let driven = Parts::of(round, STEEL, &inputs).unwrap().wall_mass;
        let burst = Burst::derived(round, STEEL, &inputs).unwrap();
        let speed = inputs.speed_constant * (driven / inputs.core_mass + 0.5).powf(-0.5);
        assert!((burst.sideways_speed as f64 - speed).abs() < 1e-3);
        let energy = 0.5 * driven * speed * speed;
        assert!((burst.energy as f64 / energy - 1.0).abs() < 1e-6);
    }

    /// A cavity as wide as the round, and parts heavier than the round, are refused at load.
    #[test]
    fn a_burst_without_a_nose_is_refused() {
        let (round, mut inputs) = fixture();
        inputs.cavity_diameter = 0.088;
        assert!(Burst::derived(round, STEEL, &inputs).is_err());
        let (round, mut inputs) = fixture();
        inputs.base_mass = 9.0;
        assert!(Burst::derived(round, STEEL, &inputs).is_err());
    }

    /// The game's damage model: the fuze arms when one body's plate met reaches the threshold, and the burst
    /// then lies `speed × delay` of path past that body's exit: the march reads both to place the
    /// burst. Plate met just under the threshold leaves the round a solid shot, and a round without
    /// a burst never arms.
    #[test]
    fn the_fuze_arms_at_the_threshold() {
        let burst = burst();
        let mut fuze = Fuze::of(Some(burst));
        fuze.exited(35.0_f32.next_down(), 300.0, None);
        assert_eq!(fuze, Fuze::Unarmed(burst));
        assert_eq!(fuze.due_within(f32::MAX), None);
        fuze.exited(35.0_f32, 300.0, None);
        assert_eq!(fuze.due_within(f32::MAX), Some(300.0 * burst.delay));
        let mut inert = Fuze::of(None);
        inert.exited(1.0e6, 300.0, None);
        assert_eq!(inert, Fuze::Inert);
        assert_eq!(inert.due_within(f32::MAX), None);
    }

    /// The delay runs down with the path flown, and a body met before it ends restarts it from
    /// that body's exit at that exit's speed, whatever its plate met; so does a bounce. An unarmed
    /// fuze is not armed by a bounce.
    #[test]
    fn a_body_met_restarts_the_delay() {
        let burst = burst();
        let mut fuze = Fuze::of(Some(burst));
        fuze.exited(35.0_f32 * 2.0, 250.0, None);
        let full = 250.0 * burst.delay;
        fuze.flown(1.0);
        assert_eq!(fuze.due_within(full - 1.0), Some(full - 1.0));
        assert_eq!(fuze.due_within(full - 1.5), None);
        fuze.exited(0.5, 200.0, None);
        assert_eq!(fuze.due_within(f32::MAX), Some(200.0 * burst.delay));
        fuze.bounced(100.0, None);
        assert_eq!(fuze.due_within(f32::MAX), Some(100.0 * burst.delay));
        fuze.bounced(0.0, None);
        assert_eq!(
            fuze.delay_left(),
            Some(burst.delay),
            "a held round waits out the delay"
        );
        let mut unarmed = Fuze::of(Some(burst));
        unarmed.bounced(100.0, None);
        assert_eq!(unarmed, Fuze::Unarmed(burst));
    }

    /// A delay run out to zero or below is due where the round stands, and no later exit or bounce
    /// restarts it or takes its tank, whatever the plate met: the march bursts the round where the
    /// crossing hands the flight back, through the tank that last restarted it. A mutant that
    /// restarts a due delay fails here.
    #[test]
    fn a_delay_run_out_is_not_restarted() {
        let burst = burst();
        let mut fuze = Fuze::of(Some(burst));
        fuze.exited(35.0_f32, 250.0, None);
        fuze.flown(250.0 * burst.delay);
        assert_eq!(fuze.due_within(0.0), Some(0.0), "due at zero");
        let due = fuze;
        fuze.exited(35.0_f32 * 2.0, 200.0, Some(Entity::PLACEHOLDER));
        assert_eq!(fuze, due, "an exit restarts no due delay");
        fuze.bounced(100.0, Some(Entity::PLACEHOLDER));
        assert_eq!(fuze, due, "nor does a bounce");
        fuze.flown(1.0);
        assert_eq!(fuze.due_within(0.0), Some(0.0), "past zero it stays due");
        assert_eq!(fuze.delay_left(), Some(0.0));
    }
}
