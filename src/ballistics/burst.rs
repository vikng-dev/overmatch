//! A burst round's derived burst ([`Burst::derived`]) and the fuze the march advances per shell
//! ([`Fuze`]). Every burst is a ring: the wall around the burst core is driven as a band about the
//! flight axis, and the nose piece flies on. The burst's story — the trigger, the ring, the derived
//! masses and speeds — is design doc §5a, "The burst"
//! (`.agents/docs/design/armor-penetration-and-damage.md`); the march that runs the fuze and throws
//! the burst's field is the parent module's.
//!
//! Units: masses kg, lengths m, speeds m/s, the delay s, the arming threshold reference-mm, the
//! energy J. Everything here is derived once, at load, in f64 with IEEE `+`, `−`, `×`, `/` and
//! `sqrt` alone, and stored in f32, so a round's burst is the same bits on every target.

use bevy::prelude::Entity;

use super::RoundBody;

/// A burst block as the round registry resolves it, with its core substance's burst facts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BurstInputs {
    /// Reference-mm the round must spend in one body to arm.
    pub(crate) arming_spend: f32,
    /// Seconds from the arming body's exit to the burst.
    pub(crate) delay: f32,
    /// The burst core's mass `C`.
    pub(crate) core_mass: f64,
    /// The core substance's density (kg/m³).
    pub(crate) core_density: f64,
    /// The core substance's speed constant `K`.
    pub(crate) speed_constant: f64,
    /// The parts at the base, behind the cavity.
    pub(crate) base_mass: f64,
    /// The cavity's diameter.
    pub(crate) cavity_diameter: f64,
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
    /// Reference-mm one body's spend must reach to arm the fuze.
    pub(crate) arming_spend: f32,
    /// Seconds from the exit of the body that armed or last restarted the fuze to the burst.
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
}

impl Burst {
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
            ("body steel density", steel_density),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{field} must be finite and positive (got {value})"));
            }
        }
        for (field, value) in [
            ("arming spend", inputs.arming_spend),
            ("delay", inputs.delay),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format!(
                    "{field} must be finite and not negative (got {value})"
                ));
            }
        }
        let parts = Parts::of(round, steel_density, inputs)?;
        let driven = parts.wall_mass;
        let sideways_speed = inputs.speed_constant / (driven / inputs.core_mass + 0.5).sqrt();
        Ok(Self {
            arming_spend: inputs.arming_spend,
            delay: inputs.delay,
            nose: RoundBody {
                mass: parts.nose_mass as f32,
                ..round
            },
            sideways_speed: sideways_speed as f32,
            energy: (0.5 * driven * sideways_speed * sideways_speed) as f32,
        })
    }
}

/// A shell's fuze: built at spawn from its round's burst (ADR-0014), advanced by the authority's
/// march alone. A replica's march decides no crossing, so its fuze never arms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fuze {
    /// No burst ahead: a round without a burst block, or a nose piece flying on from its burst.
    Inert,
    /// Not yet armed: no body's spend has reached the threshold.
    Unarmed(Burst),
    /// Armed: the round bursts after `remaining` more metres of path, unless a body met first
    /// restarts the delay. At or below zero the delay has run out and the burst is due where the
    /// round stands: nothing restarts it. `speed` is the speed the delay started at, which turns
    /// those metres into the delay's seconds left. `tank` owns the body that armed or last
    /// restarted it, and is the tank the burst's field walks.
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

    /// The round left a body it spent `spend` reference-mm across, at `speed`, the body owned by
    /// `tank`. An unarmed fuze arms when `spend` reaches the threshold; an armed one whose delay
    /// still runs restarts whatever the spend. Either way the burst then lies `speed × delay`
    /// metres of path past this exit. A delay that has run out is not restarted.
    pub(crate) fn exited(&mut self, spend: f32, speed: f32, tank: Option<Entity>) {
        let burst = match *self {
            Self::Inert => return,
            Self::Unarmed(burst) if spend < burst.arming_spend => return,
            Self::Armed { remaining, .. } if remaining <= 0.0 => return,
            Self::Unarmed(burst) | Self::Armed { burst, .. } => burst,
        };
        *self = Self::Armed {
            burst,
            remaining: speed * burst.delay,
            speed,
            tank,
        };
    }

    /// The round bounced off a face of `tank`'s, leaving at `speed`: a body met, so an armed fuze
    /// whose delay still runs restarts from the bounce. An unarmed fuze stays unarmed: a bounce
    /// spends nothing. A delay that has run out is not restarted.
    pub(crate) fn bounced(&mut self, speed: f32, tank: Option<Entity>) {
        if let Self::Armed {
            burst, remaining, ..
        } = *self
            && remaining > 0.0
        {
            *self = Self::Armed {
                burst,
                remaining: speed * burst.delay,
                speed,
                tank,
            };
        }
    }

    /// The round flew `metres` of path and met no body.
    pub(crate) fn flown(&mut self, metres: f32) {
        if let Self::Armed { remaining, .. } = self {
            *remaining -= metres;
        }
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
        arming_spend: 35.0,
        delay: 0.012,
        core_mass: 0.06,
        core_density: 1650.0,
        speed_constant: 2680.0,
        base_mass: 0.33,
        cavity_diameter: 0.0357,
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

    /// The fuze arms when one body's spend reaches the threshold and not below it, and the burst
    /// then lies `speed × delay` of path past that body's exit: the march reads both to place the
    /// burst. A spend just under the threshold leaves the round a solid shot, and a round without
    /// a burst never arms.
    #[test]
    fn the_fuze_arms_at_the_threshold() {
        let burst = burst();
        let mut fuze = Fuze::of(Some(burst));
        fuze.exited(burst.arming_spend.next_down(), 300.0, None);
        assert_eq!(fuze, Fuze::Unarmed(burst));
        assert_eq!(fuze.due_within(f32::MAX), None);
        fuze.exited(burst.arming_spend, 300.0, None);
        assert_eq!(fuze.due_within(f32::MAX), Some(300.0 * burst.delay));
        let mut inert = Fuze::of(None);
        inert.exited(1.0e6, 300.0, None);
        assert_eq!(inert, Fuze::Inert);
        assert_eq!(inert.due_within(f32::MAX), None);
    }

    /// The delay runs down with the path flown, and a body met before it ends restarts it from
    /// that body's exit at that exit's speed, whatever its spend; so does a bounce. An unarmed
    /// fuze is not armed by a bounce.
    #[test]
    fn a_body_met_restarts_the_delay() {
        let burst = burst();
        let mut fuze = Fuze::of(Some(burst));
        fuze.exited(burst.arming_spend * 2.0, 250.0, None);
        let full = 250.0 * burst.delay;
        fuze.flown(1.0);
        assert_eq!(fuze.due_within(full - 1.0), Some(full - 1.0));
        assert_eq!(fuze.due_within(full - 1.5), None);
        fuze.exited(0.5, 200.0, None);
        assert_eq!(fuze.due_within(f32::MAX), Some(200.0 * burst.delay));
        fuze.bounced(100.0, None);
        assert_eq!(fuze.due_within(f32::MAX), Some(100.0 * burst.delay));
        let mut unarmed = Fuze::of(Some(burst));
        unarmed.bounced(100.0, None);
        assert_eq!(unarmed, Fuze::Unarmed(burst));
    }

    /// A delay run out to zero or below is due where the round stands, and no later exit or bounce
    /// restarts it or takes its tank, whatever the spend: the march bursts the round where the
    /// crossing hands the flight back, through the tank that last restarted it. A mutant that
    /// restarts a due delay fails here.
    #[test]
    fn a_delay_run_out_is_not_restarted() {
        let burst = burst();
        let mut fuze = Fuze::of(Some(burst));
        fuze.exited(burst.arming_spend, 250.0, None);
        fuze.flown(250.0 * burst.delay);
        assert_eq!(fuze.due_within(0.0), Some(0.0), "due at zero");
        let due = fuze;
        fuze.exited(burst.arming_spend * 2.0, 200.0, Some(Entity::PLACEHOLDER));
        assert_eq!(fuze, due, "an exit restarts no due delay");
        fuze.bounced(100.0, Some(Entity::PLACEHOLDER));
        assert_eq!(fuze, due, "nor does a bounce");
        fuze.flown(1.0);
        assert_eq!(fuze.due_within(0.0), Some(0.0), "past zero it stays due");
        assert_eq!(fuze.delay_left(), Some(0.0));
    }
}
