//! Round mechanisms: how a round's state advances through the ordered encounters of a crossing.
//!
//! A round carries a mechanism, bound once from its own data and the substance registry
//! ([`SolidShot::bind`]), and a state. [`super::walk::finish`] — the one driver, for the runtime
//! march and the §13.6 gate alike — hands it each [`Encounter`] of a crossing in order and acts on
//! the [`Control`] it returns; the effects it returns are the crossing's addressed doses, in joules
//! of the energy the round lost ([`Effect::Line`]). Reference millimetres are a readout of a state
//! ([`Mechanism::reference_mm`]) and of a chord ([`Effect::Line`]'s `cost`), never the currency the
//! walk advances in nor a dose.
//!
//! The target structure and its staging are the design doc's:
//! `.agents/scratch/damage-cam-2026-10-08/research/recommendation-solid-shot.md`, Parts 1 and 5.
//! The slope charge is a type preset proxy ([`SlopeCharge`]); its basis is
//! `.agents/scratch/research/penetration-proxy-2026-11-02.md`. The modelling convention is how an
//! authoring overlap of different substances resists ([`overlap_factor`]) and is dosed: only its
//! strongest volume counts ([`SolidShot`]'s advance, §13.2).

use bevy::prelude::*;

use super::walk::{Encounter, VolumeContact};
use super::{RoundBody, capability_of};
use crate::substances::{REFERENCE_FACTOR, SubstanceRegistry};

/// What a mechanism advances through one encounter.
pub(crate) trait Mechanism {
    /// The round's state as this mechanism reads and advances it.
    type State: Copy + std::fmt::Debug + PartialEq;

    /// Advance `state` through `encounter` from its start, at most to its end.
    fn advance(
        &self,
        state: Self::State,
        encounter: &Encounter,
        limits: &Limits,
    ) -> Advance<Self::State>;

    /// Reference-mm `state` can still defeat: the readout, never the currency.
    fn reference_mm(&self, state: Self::State) -> f64;

    /// The round's speed (m/s) in `state`.
    fn speed(&self, state: Self::State) -> f64;

    /// The game's damage model: seconds spent over `length` metres between two resolved states.
    fn flight_time(&self, before: Self::State, after: Self::State, length: f64) -> f64;
}

/// The limits an advance must respect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Limits {
    /// The readout (reference-mm) below which the round has come to rest. An advance that would
    /// take the state below it stops where the readout reaches it.
    pub rest: f64,
}

/// What the driver does after an advance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Control {
    /// Go on to the next encounter.
    Continue,
    /// The round is at rest at the consumed progress.
    Stop,
}

/// One consequence of an advance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Effect {
    /// The round's own line, along disc sample `sample`, put `chord` metres (disc mean) of the
    /// volume `address` behind it, a readout of `cost` reference-mm (chord × the volume's factor),
    /// and `energy` joules of the kinetic energy the round lost over the advance were lost in it.
    /// The sample names which body the energy was lost in, so the walk can part it between the
    /// body and the field its exit throws ([`super::walk::SpallSource::field_share`]).
    Line {
        address: Entity,
        sample: usize,
        chord: f64,
        cost: f64,
        energy: f64,
        /// The game's damage model: addressed speed loss (m/s), independently integrated at these
        /// depths. The crossing multiplies by the penetrator mass; a carrier stops at entry.
        speed_loss: f64,
    },
}

/// A dose addressed to one volume: the box a consequence lands in, and how much of it. The round's
/// own line and the spall cone both deposit through it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Dose {
    pub address: Entity,
    /// The game's damage model: joules retained after outgoing debris for a [`DoseSource::Line`],
    /// converting at the line's unit ([`super::energy_units`]); a [`DoseSource::Field`]
    /// dose a direction's strength (the energy its field carries times its profile) times its
    /// share of the field, converting at its mechanism's unit ([`super::field_units`]).
    pub amount: f32,
    pub source: DoseSource,
}

/// What put a [`Dose`] into its box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DoseSource {
    /// The round's own line, which put `chord` metres (disc mean, as charged) of the volume behind
    /// it, a readout of `cost` reference-mm (chord × factor). The dose is the energy, not the cost.
    Line { chord: f32, cost: f32 },
    /// A field (a spall cone): one direction's strength at entry times its share of the cone.
    Field,
}

impl Dose {
    /// The chord the round's own line put into the box; `None` for a field dose, which has none.
    pub(crate) fn line_chord(&self) -> Option<f32> {
        match self.source {
            DoseSource::Line { chord, .. } => Some(chord),
            DoseSource::Field => None,
        }
    }
}

/// The result of one advance.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Advance<S> {
    pub state: S,
    /// Metres of the encounter's progress consumed: all of it on [`Control::Continue`].
    pub consumed: f64,
    pub control: Control,
    pub effects: Vec<Effect>,
}

/// A rigid solid shot: its body, and the reference force-law terms it was bound against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SolidShot {
    body: RoundBody,
    /// `m / (2 A k)` (m), [`RoundBody::terms_against`].
    scale: f64,
    /// `k / a` (s²/m²).
    inertia: f64,
}

/// A solid shot's state: its speed (m/s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SolidShotState {
    pub speed: f64,
}

impl SolidShot {
    #[cfg(test)]
    fn energy(&self, state: SolidShotState) -> f64 {
        0.5 * self.body.mass as f64 * state.speed * state.speed
    }

    /// Bind `round` against `registry`'s reference steel — synchronous, from data alone.
    pub(crate) fn bind(round: RoundBody, registry: &SubstanceRegistry) -> Self {
        let (scale, inertia) = round.terms_against(&registry.reference_facts());
        Self {
            body: round,
            scale,
            inertia,
        }
    }

    /// The body the mechanism was bound from.
    pub(crate) fn body(&self) -> RoundBody {
        self.body
    }

    /// The state at `speed` m/s.
    pub(crate) fn at(&self, speed: f32) -> SolidShotState {
        SolidShotState {
            speed: speed as f64,
        }
    }

    /// The game's damage model: DERIVED entry resisting force dE/dx (J/m = N), using the same
    /// factor and bound terms as `advance`; no finite-difference chord or stopping cutoff.
    pub(crate) fn resisting_force(&self, speed: f32, factor: f64) -> f64 {
        f64::from(self.body.mass) / (2.0 * self.scale)
            * (1.0 / self.inertia + f64::from(speed).powi(2))
            * factor
            / f64::from(REFERENCE_FACTOR)
    }

    /// The game's damage model interpolates a resolved stretch in force-law potential, not
    /// speed. The immutable endpoints fix the resistance over its entire distance.
    pub(crate) fn speed_between(&self, before: f64, after: f64, fraction: f64) -> f64 {
        if fraction <= 0.0 {
            return before;
        }
        if fraction >= 1.0 {
            return after;
        }
        let drop = libm::log1p(
            self.inertia * (before - after) * (before + after)
                / (1.0 + self.inertia * after * after),
        );
        let squared = before * before
            + (before * before + 1.0 / self.inertia) * libm::expm1(-fraction * drop);
        // The resolved endpoints bound nonnegative v²; the subtraction can round below zero
        // at rest, where the force-law speed is exactly zero.
        squared.max(0.0).sqrt()
    }

    /// The force law's potential `ln(1 + k v² / a)` at `speed`, by `libm` (see
    /// [`super::capability_of`]).
    fn potential(&self, speed: f64) -> f64 {
        libm::log1p(self.inertia * speed * speed)
    }

    /// The speed at potential `potential` — the inverse of [`potential`](Self::potential), by
    /// `libm`.
    fn speed_at(&self, potential: f64) -> f64 {
        (libm::expm1(potential) / self.inertia).sqrt()
    }
}

impl Mechanism for SolidShot {
    type State = SolidShotState;

    /// The rigid force law across the encounter: on each sample the force is the reference's
    /// `A (a + k v²)` scaled by the present substance's share `factor / REFERENCE_FACTOR`, the disc
    /// force their sample mean, so the potential falls by `share · L / (m / (2 A k))` over `L`
    /// metres, each sample's share read by [`overlap_factor`]. Encounters are advanced strictly in
    /// order, each from the state the last one left: nothing is reduced across them.
    ///
    /// THE ENERGY DOSE. The round loses `½ m (v_in² − v_out²)` over the advance, and all of
    /// `½ m v_in²` when it comes to rest in it: a stopped round deals what it had left, whatever
    /// `limits.rest` leaves its readout at. Between consecutive volume boundaries, each sample's
    /// strongest volumes there (the highest factor present in that sample, ties all of them) take
    /// that energy in proportion to their cost at those depths, and a weaker volume overlapped by
    /// a stronger one takes none of it (§13.2's damage law), so the doses sum to the energy lost.
    /// A volume's `chord` and `cost` are its dosed ones: its chord less what a stronger volume
    /// covers in its own sample. An advance that covers no contact deals nothing.
    fn advance(
        &self,
        state: SolidShotState,
        encounter: &Encounter,
        limits: &Limits,
    ) -> Advance<SolidShotState> {
        let disc = encounter.disc as f64;
        let length = encounter.end - encounter.start;
        let resisted: f64 = encounter
            .samples
            .iter()
            .map(|sample| overlap_factor(&sample.present) as f64)
            .sum();
        let drop = length * resisted / disc / REFERENCE_FACTOR as f64 / self.scale;
        let before = state.speed;
        let potential = self.potential(before);
        let rest = limits.rest / (REFERENCE_FACTOR as f64 * self.scale);
        let (consumed, state, control) = if drop > potential - rest {
            let share = ((potential - rest) / drop).max(0.0);
            (
                length * share,
                SolidShotState {
                    speed: self.speed_at(rest.min(potential).max(0.0)),
                },
                Control::Stop,
            )
        } else {
            (
                length,
                SolidShotState {
                    // The game's damage model: integrate the loss from the incoming speed.
                    // A gap has zero drop and preserves that speed bit for bit. Inverting its
                    // unchanged potential can instead round upward and break the flight clock.
                    speed: if drop == 0.0 {
                        before
                    } else if drop == potential {
                        // Exact rest is an endpoint, not a cancellation of two squared speeds.
                        0.0
                    } else {
                        (before * before
                            + (before * before + 1.0 / self.inertia) * libm::expm1(-drop))
                        .max(0.0)
                        .sqrt()
                    },
                },
                Control::Continue,
            )
        };
        let left = match control {
            Control::Stop => 0.0,
            _ => state.speed * state.speed,
        };
        let lost = (0.5 * self.body.mass as f64 * (before * before - left)).max(0.0);
        // The game's damage model integrates dose at every volume boundary, including seams
        // inside an encounter whose substance set is constant. The force-law advance above stays
        // whole, so adding such a seam cannot change the round's state or its rest point.
        let contacts: Vec<(usize, f32, &[VolumeContact], &VolumeContact)> = encounter
            .samples
            .iter()
            .flat_map(|sample| {
                let strongest = overlap_factor(&sample.present);
                sample.present.iter().map(move |contact| {
                    (sample.sample, strongest, sample.present.as_slice(), contact)
                })
            })
            .collect();
        let mut cuts = vec![0.0, consumed];
        for (_, _, _, contact) in &contacts {
            cuts.push((contact.start - encounter.start).clamp(0.0, consumed));
            cuts.push((contact.end - encounter.start).clamp(0.0, consumed));
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        let lost_at = |progress: f64| {
            if progress <= 0.0 {
                0.0
            } else if progress >= consumed {
                lost
            } else {
                let speed = self.speed_at((potential - drop * (progress / length)).max(0.0));
                (0.5 * self.body.mass as f64 * (before * before - speed * speed)).max(0.0)
            }
        };
        let mut energies = vec![0.0; contacts.len()];
        let mut speed_losses = vec![0.0; contacts.len()];
        let speed_lost_at = |progress: f64| {
            let speed = if progress <= 0.0 {
                before
            } else if progress >= consumed {
                left.sqrt()
            } else {
                self.speed_at((potential - drop * (progress / length)).max(0.0))
            };
            before - speed
        };
        // The game's damage model holds each sample's substance set constant over an encounter,
        // including an absorbed same-substance seam. Its strongest factor is therefore constant;
        // active tied contacts divide that sample's one cost share, never multiply it.
        for bounds in cuts.windows(2) {
            let (a, b) = (bounds[0], bounds[1]);
            let inside = |contact: &VolumeContact| {
                contact.start - encounter.start < b && contact.end - encounter.start > a
            };
            let weights: Vec<f64> = contacts
                .iter()
                .map(|(_, strongest, layer, contact)| {
                    if !inside(contact) || contact.material.factor < *strongest {
                        return 0.0;
                    }
                    // This active strongest contact is itself a tie, so the divisor is positive.
                    let ties = layer
                        .iter()
                        .filter(|other| inside(other) && other.material.factor == *strongest)
                        .count();
                    contact.material.factor as f64 / ties as f64
                })
                .collect();
            let total: f64 = weights.iter().sum();
            let energy = (lost_at(b) - lost_at(a)).max(0.0);
            let speed_loss = (speed_lost_at(b) - speed_lost_at(a)).max(0.0);
            if total > 0.0 {
                for (index, weight) in weights.into_iter().enumerate() {
                    if weight > 0.0 {
                        energies[index] += energy * (weight / total);
                        speed_losses[index] += speed_loss * (weight / total);
                    }
                }
            }
        }
        let effects = contacts
            .into_iter()
            .zip(energies)
            .zip(speed_losses)
            .map(|(((sample, strongest, _, contact), energy), speed_loss)| {
                let covered = (consumed - (contact.start - encounter.start))
                    .min(contact.end - contact.start)
                    .max(0.0);
                let chord = if contact.material.factor == strongest {
                    covered / disc
                } else {
                    0.0
                };
                Effect::Line {
                    address: contact.volume,
                    sample,
                    chord,
                    cost: chord * contact.material.factor as f64,
                    energy,
                    speed_loss,
                }
            })
            .collect();
        Advance {
            state,
            consumed,
            control,
            effects,
        }
    }

    fn reference_mm(&self, state: SolidShotState) -> f64 {
        capability_of((self.scale, self.inertia), state.speed)
    }

    fn speed(&self, state: SolidShotState) -> f64 {
        state.speed
    }

    fn flight_time(&self, before: SolidShotState, after: SolidShotState, length: f64) -> f64 {
        // The game's damage model: DERIVED from d(v²)/ds = -q (v² + 1/inertia).
        // Integrating ds/v gives 2 sqrt(inertia)/q times the difference of arctangents.
        // The ratio form preserves a small speed loss without subtracting two potentials.
        if length == 0.0 {
            return 0.0;
        }
        let (a, b) = (before.speed, after.speed);
        if a == b {
            assert!(a > 0.0, "positive flight length has positive speed");
            return length / a;
        }
        assert!(a > b && b >= 0.0, "a resisted stretch slows toward rest");
        let root = self.inertia.sqrt();
        let drop = libm::log1p(self.inertia * (a - b) * (a + b) / (1.0 + self.inertia * b * b));
        let angle = libm::atan(root * (a - b) / (1.0 + self.inertia * a * b));
        2.0 * root * length / drop * angle
    }
}

/// THE UNION-FIELD OVERLAP RULE: what one sample is resisted by where several volumes are present
/// at once. Where they are of different substances — an authoring overlap, §13.2 — the sample is
/// resisted once, by the most resistant of them. This is a modelling convention for overlap in the
/// authored geometry (armour is a union field, §13), not a material law: no physical body is two
/// substances at one point. It reads factors over all `contacts`, which is the factor at every
/// point of the encounter because a sample's substances are the same over the whole encounter
/// ([`SampleLayer::present`](super::walk::SampleLayer)) and each substance carries one factor
/// (`VolumeTable::new`).
pub(crate) fn overlap_factor(contacts: &[VolumeContact]) -> f32 {
    overlap_of(contacts.iter().map(|contact| contact.material.factor))
}

/// [`overlap_factor`] over the factors of the volumes present at one point.
pub(crate) fn overlap_of(factors: impl IntoIterator<Item = f32>) -> f32 {
    factors.into_iter().fold(0.0f32, f32::max)
}

/// The game's damage model: Resolution ceiling keeps the resistance charge finite as θ + β → 90°.
const SLOPE_CHARGE_CEILING: f64 = 5.0;

/// Resistance over LOS, exactly one head-on; a softer entrance pays only its reference share.
pub(crate) fn slope_charge(cosine: f32, penalty_tan: f32, factor: f32) -> f64 {
    let cosine = f64::from(cosine.clamp(0.0, 1.0));
    let sine = (1.0 - cosine * cosine).max(0.0).sqrt();
    let denominator = cosine - sine * f64::from(penalty_tan);
    let charge = if denominator <= cosine / SLOPE_CHARGE_CEILING {
        SLOPE_CHARGE_CEILING
    } else {
        cosine / denominator
    };
    1.0 + f64::from(factor.min(1.0)) * (charge - 1.0)
}

/// The game's damage model: resistance reads a scaled chord; consumed progress and the flight
/// clock read the real chord. Effects and reference-mm read the same charged state.
pub(crate) struct SlopeCharge<'a, M> {
    pub inner: &'a M,
    pub charge: f64,
}

impl<M: Mechanism> Mechanism for SlopeCharge<'_, M> {
    type State = M::State;

    fn advance(
        &self,
        state: M::State,
        encounter: &Encounter,
        limits: &Limits,
    ) -> Advance<M::State> {
        let charge = self.charge;
        let length = encounter.end - encounter.start;
        let project = |t: f64| encounter.start + (t - encounter.start) * charge;
        let mut projected = Encounter {
            end: encounter.start + length * charge,
            ..encounter.clone()
        };
        for contact in projected
            .samples
            .iter_mut()
            .flat_map(|sample| sample.present.iter_mut())
        {
            contact.start = project(contact.start);
            contact.end = project(contact.end);
        }
        let mut advance = self.inner.advance(state, &projected, limits);
        advance.consumed = if advance.control == Control::Continue {
            length
        } else {
            (advance.consumed / charge).min(length)
        };
        advance
    }

    fn reference_mm(&self, state: M::State) -> f64 {
        self.inner.reference_mm(state)
    }

    fn speed(&self, state: M::State) -> f64 {
        self.inner.speed(state)
    }

    fn flight_time(&self, before: M::State, after: M::State, length: f64) -> f64 {
        // The clock uses the travelled length between the charged states.
        self.inner.flight_time(before, after, length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ballistics::walk::{SampleLayer, VolumeMaterial};

    /// Fixture data: an 88-class body.
    const BODY: RoundBody = RoundBody {
        mass: 10.2,
        caliber: 0.088,
        nose_factor: 0.5,
    };

    const LIMITS: Limits = Limits { rest: 0.0 };

    fn round() -> SolidShot {
        SolidShot::bind(BODY, SubstanceRegistry::shipped_static())
    }

    fn volume(index: u32) -> Entity {
        Entity::from_raw_u32(index).expect("a test entity index")
    }

    /// An encounter of `length` over a `disc`-sample disc, each listed sample presenting the listed
    /// `(volume, factor)` pairs.
    fn encounter(length: f64, disc: usize, samples: &[&[(u32, f32)]]) -> Encounter {
        Encounter {
            start: 0.0,
            end: length,
            interface: true,
            samples: samples
                .iter()
                .enumerate()
                .map(|(sample, present)| SampleLayer {
                    sample,
                    present: present
                        .iter()
                        .map(|(index, factor)| VolumeContact {
                            volume: volume(*index),
                            material: VolumeMaterial::fixture(*factor),
                            start: 0.0,
                            end: length,
                        })
                        .collect(),
                })
                .collect(),
            disc,
        }
    }

    /// The game's damage model partitions a finite nonnegative energy loss into finite
    /// nonnegative line deposits. Quantization relies on that sign even at rest, tiny chords and
    /// overlaps. FIXTURE DATA: independent speeds, chords and material factors span those edges.
    #[test]
    fn line_energy_remains_nonnegative_at_resistance_and_rest_edges() {
        let round = round();
        for speed in [0.0, 0.001, 1.0, 100.0, 1000.0] {
            for length in [0.0, 1.0e-14, 1.0e-7, 0.01, 10.0] {
                for factor in [0.0, 0.001, 10.0, 1000.0] {
                    let state = round.at(speed);
                    let advance = round.advance(
                        state,
                        &encounter(length, 2, &[&[(1, factor)], &[(2, factor), (3, factor)]]),
                        &LIMITS,
                    );
                    let mut total = 0.0;
                    for Effect::Line { energy, .. } in advance.effects {
                        assert!(energy.is_finite() && energy >= 0.0);
                        total += energy;
                        super::super::energy_units(energy as f32);
                    }
                    assert!(total <= round.energy(state) * (1.0 + 1.0e-12));
                }
            }
        }
    }

    /// One stretch advances the speed by the rigid force law integrated in closed form from the
    /// reference steel's facts: `v₁² = (v₀² + a/k) · exp(−s · 2 A k L / m) − a/k`, `s` the disc-mean
    /// share of the reference force.
    #[test]
    fn a_stretch_advances_the_speed_by_the_closed_form_force_law() {
        let facts = SubstanceRegistry::shipped_static().reference_facts();
        let a = facts.cavity_strength();
        let k = super::super::physical_laws::PhysicalLaws::shipped().cavity_inertia
            * BODY.nose_factor as f64
            * facts.density;
        let area = std::f64::consts::FRAC_PI_4 * (BODY.caliber as f64).powi(2);
        let round = round();
        for (samples, share) in [
            (&[&[(1, 1000.0)][..]][..], 1.0),
            (&[&[(1, 500.0)][..], &[][..]][..], 0.125),
            (&[&[(1, 10.0)][..]][..], 0.01),
        ] {
            let disc = if samples.len() == 2 { 4 } else { 1 };
            for (speed, length) in [(800.0f32, 0.05), (450.0, 0.01), (1200.0, 0.002)] {
                let advance =
                    round.advance(round.at(speed), &encounter(length, disc, samples), &LIMITS);
                assert_eq!(advance.control, Control::Continue);
                let v0 = speed as f64;
                let expected = ((v0 * v0 + a / k)
                    * (-share * 2.0 * area * k * length / BODY.mass as f64).exp()
                    - a / k)
                    .sqrt();
                assert!(
                    (advance.state.speed / expected - 1.0).abs() < 1.0e-9,
                    "share {share}, {speed} m/s over {length} m: {} against {expected}",
                    advance.state.speed
                );
            }
        }
    }

    /// The game's damage model must spend seconds inside material, including a finite time
    /// to rest. Playback relies on this integral rather than endpoint-speed division. The
    /// independent Simpson integral below is in velocity space, where the rest endpoint is smooth.
    #[test]
    fn a_stretch_clock_integrates_the_force_law_including_rest() {
        let round = round();
        // Fixture data: incoming/outgoing speeds and path lengths.
        for (a, b, length) in [
            (1000.0, 200.0, 0.1),
            (800.0, 0.0, 0.3),
            (700.0, 699.999, 0.01),
        ] {
            let before = SolidShotState { speed: a };
            let after = SolidShotState { speed: b };
            let q = (round.potential(a) - round.potential(b)) / length;
            let intervals = 4096;
            let step = (a - b) / f64::from(intervals);
            let integrand = |v: f64| 2.0 * round.inertia / (q * (1.0 + round.inertia * v * v));
            let sum: f64 = (0..=intervals)
                .map(|i| {
                    let weight = if i == 0 || i == intervals {
                        1.0
                    } else if i % 2 == 0 {
                        2.0
                    } else {
                        4.0
                    };
                    weight * integrand(b + f64::from(i) * step)
                })
                .sum();
            let measured = round.flight_time(before, after, length);
            let integrated = sum * step / 3.0;
            assert!(
                (measured / integrated - 1.0).abs() < 1.0e-9,
                "{a} -> {b}: {measured} vs {integrated}"
            );
        }
        let state = round.at(800.0);
        assert_eq!(round.flight_time(state, state, 2.0), 2.0 / 800.0);
        assert_eq!(round.flight_time(round.at(0.0), round.at(0.0), 0.0), 0.0);
    }

    /// The game's damage model: an unresisted gap in a crossing preserves speed exactly;
    /// playback relies on its clock remaining monotone after a resisted stretch.
    #[test]
    fn an_unresisted_gap_preserves_the_speed_a_material_left() {
        let round = round();
        // Fixture data: the slab leaves a speed whose potential round-trip rounds upward.
        let before = round.advance(
            round.at(82.0),
            &encounter(0.001, 1, &[&[(1, 1000.0)]]),
            &LIMITS,
        );
        assert_eq!(before.control, Control::Continue);
        let gap = 0.001;
        let after = round.advance(before.state, &encounter(gap, 1, &[]), &LIMITS);
        let seconds = round.flight_time(before.state, after.state, after.consumed);
        assert_eq!(after.state, before.state);
        assert_eq!(seconds, gap / before.state.speed);
        assert!(after.effects.is_empty(), "air deposits no dose");
    }

    /// The game's damage model's clock and energy dose compose over one material path. A
    /// consumer splitting playback into ticks must reach the same path time and total dose.
    #[test]
    fn partitioning_a_material_path_preserves_seconds_and_dose() {
        let round = round();
        let state = round.at(800.0);
        let whole = round.advance(state, &encounter(0.1, 1, &[&[(1, 1000.0)]]), &LIMITS);
        let duration = round.flight_time(state, whole.state, whole.consumed);
        let energy = |advance: &Advance<SolidShotState>| {
            advance
                .effects
                .iter()
                .map(|effect| {
                    let Effect::Line { energy, .. } = effect;
                    energy
                })
                .sum::<f64>()
        };
        for cuts in [vec![0.03, 0.07], vec![0.01; 10], vec![0.025; 4]] {
            let (mut current, mut seconds, mut dose) = (state, 0.0, 0.0);
            for length in cuts {
                let part =
                    round.advance(current, &encounter(length, 1, &[&[(1, 1000.0)]]), &LIMITS);
                seconds += round.flight_time(current, part.state, part.consumed);
                current = part.state;
                dose += energy(&part);
            }
            assert!((seconds / duration - 1.0).abs() < 1.0e-12);
            assert!((current.speed / whole.state.speed - 1.0).abs() < 1.0e-12);
            assert!((dose / energy(&whole) - 1.0).abs() < 1.0e-12);
        }
    }

    /// THE UNION-FIELD OVERLAP RULE. An overlap of two different substances is resisted once, by
    /// the most resistant of them, in either listing order, and only the strongest volume is dosed
    /// (§13.2's damage law): it takes the whole energy the round lost there, and the weaker volume
    /// none of it and no chord, so a pool clipped into a stronger one is never dosed through it.
    /// Two volumes of one factor both count: the energy is shared between them by cost. Field
    /// directions stop at their first entered hard volume; the HP tally relies on the line's
    /// doses summing to the energy lost. Factors are fixture
    /// data.
    #[test]
    fn an_overlap_is_resisted_once_and_dosed_only_by_its_strongest_volume() {
        let round = round();
        let lines = |advance: &Advance<SolidShotState>| -> Vec<(Entity, f64, f64, f64)> {
            advance
                .effects
                .iter()
                .map(
                    |&Effect::Line {
                         address,
                         chord,
                         cost,
                         energy,
                         ..
                     }| (address, chord, cost, energy),
                )
                .collect()
        };
        for (strong, weak) in [(1000.0, 10.0), (50.0, 10.0)] {
            let alone = round.advance(
                round.at(800.0),
                &encounter(0.05, 1, &[&[(1, strong)]]),
                &LIMITS,
            );
            let lost = 0.5 * BODY.mass as f64 * (800.0f64.powi(2) - alone.state.speed.powi(2));
            for listed in [[(1, strong), (2, weak)], [(2, weak), (1, strong)]] {
                let overlapped =
                    round.advance(round.at(800.0), &encounter(0.05, 1, &[&listed]), &LIMITS);
                assert_eq!(alone.state, overlapped.state);
                for (address, chord, cost, energy) in lines(&overlapped) {
                    if address == volume(1) {
                        assert_eq!((chord, cost), (0.05, 0.05 * strong as f64));
                        assert!(
                            (energy / lost - 1.0).abs() < 1.0e-12,
                            "the strongest takes what the round lost: {energy} against {lost}"
                        );
                    } else {
                        assert_eq!(
                            (chord, cost, energy),
                            (0.0, 0.0, 0.0),
                            "the weaker volume ({weak} under {strong}) is not dosed"
                        );
                    }
                }
            }
        }
        let equal = round.advance(
            round.at(800.0),
            &encounter(0.05, 1, &[&[(1, 40.0), (2, 40.0)]]),
            &LIMITS,
        );
        let lost = 0.5 * BODY.mass as f64 * (800.0f64.powi(2) - equal.state.speed.powi(2));
        for (address, chord, cost, energy) in lines(&equal) {
            assert_eq!((chord, cost), (0.05, 0.05 * 40.0), "{address}");
            assert!(
                (energy / (0.5 * lost) - 1.0).abs() < 1.0e-12,
                "equal factors share: {energy} against half of {lost}"
            );
        }
    }

    /// The game's damage model assigns each sample one maximum-factor cost share. Duplicating
    /// a tied contact inside one sample must split that share, never take energy from another
    /// sample; the pool tally relies on the sum still equalling the round's energy loss.
    #[test]
    fn tied_contacts_split_their_samples_share_once() {
        let round = round();
        for (a, b) in [(1000.0, 1000.0), (1000.0, 10.0)] {
            let alone = round.advance(
                round.at(800.0),
                &encounter(0.05, 2, &[&[(1, a)], &[(3, b)]]),
                &LIMITS,
            );
            for tied in [[(1, a), (2, a)], [(2, a), (1, a)]] {
                let advance = round.advance(
                    round.at(800.0),
                    &encounter(0.05, 2, &[&tied, &[(3, b)]]),
                    &LIMITS,
                );
                assert_eq!(advance.state, alone.state, "a tie adds no resistance");
                let lost = round.energy(round.at(800.0)) - round.energy(advance.state);
                let mut sum = 0.0;
                for &Effect::Line {
                    address, energy, ..
                } in &advance.effects
                {
                    let expected = if address == volume(3) {
                        b as f64 / (a + b) as f64
                    } else {
                        0.5 * a as f64 / (a + b) as f64
                    };
                    assert!(
                        (energy / lost - expected).abs() < 1.0e-12,
                        "{address:?}: {energy} of {lost}, expected share {expected}"
                    );
                    sum += energy;
                }
                assert!((sum / lost - 1.0).abs() < 1.0e-12);
            }
        }
    }

    /// The strongest volume is read per sample: a weak volume one sample meets in the open is
    /// dosed by its own cost's share even where another sample of the disc meets a stronger
    /// volume, since no stronger volume covers it at its own points. Factors are fixture data.
    #[test]
    fn a_volume_is_hidden_only_by_a_stronger_one_in_its_own_sample() {
        let round = round();
        let advance = round.advance(
            round.at(800.0),
            &encounter(0.05, 2, &[&[(1, 1000.0)], &[(2, 10.0)]]),
            &LIMITS,
        );
        let lost = 0.5 * BODY.mass as f64 * (800.0f64.powi(2) - advance.state.speed.powi(2));
        let energy = |index: u32| {
            advance
                .effects
                .iter()
                .find_map(
                    |&Effect::Line {
                         address, energy, ..
                     }| (address == volume(index)).then_some(energy),
                )
                .expect("both volumes are dosed")
        };
        assert!(
            (energy(2) / lost - 10.0 / 1010.0).abs() < 1.0e-12,
            "the open weak volume keeps its cost share: {} of {lost}",
            energy(2)
        );
        assert!(((energy(1) + energy(2)) / lost - 1.0).abs() < 1.0e-12);
    }

    /// THE ENERGY DOSE. A round's line dose is the kinetic energy it lost across the chord, so two
    /// rounds of different mass and speed over one chord deal different HP, ordered by the energy
    /// each lost (the reference-mm law charged both the same chord × factor, which their `cost`
    /// readouts still show), and a round that comes to rest deals all it had left. The HP is the
    /// shipped rate's ([`crate::ballistics::energy_units`]); every body, speed and plate is fixture
    /// data: a 0.5 m slab of factor 40 that the heavy body perforates and the light one stops in.
    #[test]
    fn a_line_dose_is_the_energy_lost_and_a_stopped_round_deals_what_it_had_left() {
        use crate::ballistics::{energy_units, pool_units_of};
        let registry = SubstanceRegistry::shipped_static();
        let light_body = RoundBody {
            mass: 0.0118,
            caliber: 0.0079,
            nose_factor: 0.5,
        };
        let (heavy, light) = (round(), SolidShot::bind(light_body, registry));
        let slab = encounter(0.5, 1, &[&[(1, 40.0)]]);
        let through = heavy.advance(heavy.at(700.0), &slab, &LIMITS);
        let stopped = light.advance(light.at(800.0), &slab, &LIMITS);
        assert_eq!(
            through.control,
            Control::Continue,
            "the heavy body perforates"
        );
        assert_eq!(
            stopped.control,
            Control::Stop,
            "the light body comes to rest"
        );
        let energy = |advance: &Advance<SolidShotState>| {
            let Effect::Line { energy, .. } = advance.effects[0];
            energy
        };
        let lost = 0.5 * BODY.mass as f64 * (700.0f64.powi(2) - through.state.speed.powi(2));
        assert!(
            (energy(&through) / lost - 1.0).abs() < 1.0e-12,
            "{} against {lost}",
            energy(&through)
        );
        assert_eq!(
            energy(&stopped),
            0.5 * light_body.mass as f64 * 800.0f64.powi(2),
            "a stopped round deals what it had left"
        );
        let hp =
            |advance: &Advance<SolidShotState>| pool_units_of(energy_units(energy(advance) as f32));
        assert!(energy(&through) > energy(&stopped));
        assert!(
            hp(&through) > hp(&stopped) && hp(&stopped) > 0,
            "the HP follow the energy: {} against {}",
            hp(&through),
            hp(&stopped)
        );
    }

    /// A stretch that asks exactly the round's whole potential is crossed, and the round leaves it
    /// at rest; the next stretch stops it at its start with nothing consumed and no dose.
    #[test]
    fn spending_exactly_the_potential_leaves_the_round_at_rest_at_the_far_face() {
        let round = round();
        let state = round.at(755.0);
        let potential = round.potential(state.speed);
        let drop = |length: f64| length * 1000.0 / 1.0 / REFERENCE_FACTOR as f64 / round.scale;
        // The length whose drop is the potential to the bit, searched over the ULPs around its
        // real-number value.
        let mut length = potential * round.scale;
        for _ in 0..64 {
            if drop(length) >= potential {
                break;
            }
            length = length.next_up();
        }
        while drop(length.next_down()) >= potential {
            length = length.next_down();
        }
        assert_eq!(
            drop(length),
            potential,
            "no length spends exactly the potential"
        );

        let crossed = round.advance(state, &encounter(length, 1, &[&[(1, 1000.0)]]), &LIMITS);
        assert_eq!(crossed.control, Control::Continue);
        assert_eq!(crossed.consumed, length);
        assert_eq!(crossed.state.speed, 0.0);
        let Effect::Line { energy, .. } = crossed.effects[0];
        assert_eq!(
            energy,
            0.5 * BODY.mass as f64 * 755.0f64.powi(2),
            "a round left at rest dealt all it had"
        );

        let next = round.advance(crossed.state, &encounter(1.0, 1, &[&[(2, 100.0)]]), &LIMITS);
        assert_eq!(next.control, Control::Stop);
        assert_eq!(next.consumed, 0.0);
        assert!(next.effects.iter().all(|effect| match effect {
            Effect::Line {
                chord,
                cost,
                energy,
                ..
            } => *chord == 0.0 && *cost == 0.0 && *energy == 0.0,
        }));
    }

    /// The game's damage model's slope proxy is head-on exact, monotone and bounded. The
    /// mutation cos(θ+β) without cos β charges head-on; shot potency relies on normal calibration.
    #[test]
    fn slope_charge_is_head_on_exact_monotone_bounded_and_shared_by_soft_faces() {
        let registry = crate::rounds::RoundRegistry::shipped();
        for (_, preset) in registry.contact_presets() {
            // Fixture chord: every preset preserves the uncharged head-on advance bit for bit.
            let mechanism = round();
            let state = mechanism.at(800.0);
            let chord = encounter(0.04, 1, &[&[(1, 1000.0)]]);
            let adapted = SlopeCharge {
                inner: &mechanism,
                charge: slope_charge(1.0, preset.slope_tan, 1.0),
            };
            assert_eq!(
                adapted.advance(state, &chord, &LIMITS),
                mechanism.advance(state, &chord, &LIMITS)
            );

            for factor in [0.0, 0.1, 1.0, 2.0] {
                assert_eq!(slope_charge(1.0, preset.slope_tan, factor), 1.0);
                let mut previous = 1.0;
                for step in 0..=900 {
                    let cosine = (step as f32 * 0.1).to_radians().cos().max(0.0);
                    let charge = slope_charge(cosine, preset.slope_tan, factor);
                    assert!(charge.is_finite() && (1.0..=SLOPE_CHARGE_CEILING).contains(&charge));
                    assert!(charge + 1.0e-7 >= previous);
                    previous = charge;
                }
            }
        }
        for angle in [10.0_f32, 30.0, 60.0, 89.99] {
            let cosine = angle.to_radians().cos();
            let mut previous = 1.0;
            for beta in [0.0_f32, 2.0, 8.0, 15.0, 60.0, 89.99] {
                let tangent = beta.to_radians().tan();
                let charge = slope_charge(cosine, tangent, 1.0);
                assert!(charge >= previous);
                let soft = slope_charge(cosine, tangent, 0.2);
                if charge > 1.0 {
                    assert!(soft < charge);
                } else {
                    assert_eq!(soft, 1.0);
                }
                previous = charge;
            }
        }
    }

    /// The game's damage model: charging resistance changes state and work once; the real chord
    /// still owns stop progress and flight time. Mutant: charged geometry leaks into the clock.
    #[test]
    fn slope_adapter_maps_resistance_back_to_real_progress_and_time() {
        let round = round();
        let adapter = SlopeCharge {
            inner: &round,
            charge: 3.0,
        }; // Fixture charge.
        for (speed, length) in [(800.0_f32, 0.04), (300.0, 2.0)] {
            let before = round.at(speed);
            let charged = round.advance(
                before,
                &encounter(length * adapter.charge, 1, &[&[(1, 1000.0)]]),
                &LIMITS,
            );
            let actual = adapter.advance(before, &encounter(length, 1, &[&[(1, 1000.0)]]), &LIMITS);
            assert_eq!(actual.state, charged.state);
            assert_eq!(actual.effects, charged.effects);
            assert_eq!(actual.control, charged.control);
            assert!((actual.consumed - charged.consumed / adapter.charge).abs() < 1.0e-12);
            assert_eq!(
                adapter.flight_time(before, actual.state, actual.consumed),
                round.flight_time(before, charged.state, actual.consumed)
            );
        }
    }
}
