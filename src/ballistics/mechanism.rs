//! Round mechanisms: how a round's state advances through the ordered encounters of a crossing.
//!
//! A round carries a mechanism, bound once from its own data and the substance registry
//! ([`SolidShot::bind`]), and a state. [`super::walk::finish`] — the one driver, for the runtime
//! march and the §13.6 gate alike — hands it each [`Encounter`] of a crossing in order and acts on
//! the [`Control`] it returns; the effects it returns are the crossing's addressed doses. Reference
//! millimetres are a readout of a state ([`Mechanism::reference_mm`]), never the currency the walk
//! advances in.
//!
//! The target structure and its staging are the design doc's:
//! `.agents/scratch/damage-cam-2026-10-08/research/recommendation-solid-shot.md`, Parts 1 and 5.
//! What this module still decides by a compatibility rule rather than a law is the overmatch switch
//! ([`OvermatchProjection`]).

use bevy::prelude::*;

use super::walk::Encounter;
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
        geometry: &Geometry,
    ) -> Advance<Self::State>;

    /// Reference-mm `state` can still defeat: the readout, never the currency.
    fn reference_mm(&self, state: Self::State) -> f64;
}

/// The limits an advance must respect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Limits {
    /// The readout (reference-mm) below which the round has come to rest. An advance that would
    /// take the state below it stops where the readout reaches it.
    pub rest: f64,
}

/// The crossing geometry a mechanism may read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geometry {
    /// Unit travel axis through the crossing.
    pub axis: Vec3,
    /// The entrance surface's aggregate outward normal `n̄`.
    pub entrance_normal: Vec3,
}

/// What the driver does after an advance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Control {
    /// Go on to the next encounter.
    Continue,
    /// Leave the crossing along `direction` from the consumed progress.
    #[expect(
        dead_code,
        reason = "part of the mechanism interface; solid shot turns no round yet, and the driver \
                  fails closed on it"
    )]
    Redirect { direction: Vec3 },
    /// The round is at rest at the consumed progress.
    Stop,
    /// Suspend the round at the consumed progress for work the driver schedules.
    #[expect(
        dead_code,
        reason = "part of the mechanism interface; solid shot schedules nothing yet, and the driver \
                  fails closed on it"
    )]
    Schedule,
}

/// One consequence of an advance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Effect {
    /// The round's own line put `chord` metres (disc mean) of the volume `address` behind it and
    /// deposits `amount` reference-mm into it.
    Line {
        address: Entity,
        chord: f64,
        amount: f64,
    },
}

/// A dose addressed to one volume: the box a consequence lands in, and how much of it, in
/// reference-mm. The round's own line and the spall cloud both deposit through it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Dose {
    pub address: Entity,
    /// Reference-mm.
    pub amount: f32,
    pub source: DoseSource,
}

/// What put a [`Dose`] into its box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DoseSource {
    /// The round's own line, which put `chord` metres (disc mean, as charged) of the volume behind
    /// it.
    Line { chord: f32 },
    /// A spall fragment, at its penetration on striking the box.
    Fragment,
}

impl Dose {
    /// The chord the round's own line put into the box; `None` for a fragment, which has none.
    pub(crate) fn line_chord(&self) -> Option<f32> {
        match self.source {
            DoseSource::Line { chord } => Some(chord),
            DoseSource::Fragment => None,
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

    /// The force law's potential `ln(1 + k v² / a)` at `speed`.
    fn potential(&self, speed: f64) -> f64 {
        (self.inertia * speed * speed).ln_1p()
    }

    /// The speed at potential `potential` — the inverse of [`potential`](Self::potential).
    fn speed_at(&self, potential: f64) -> f64 {
        (potential.exp_m1() / self.inertia).sqrt()
    }
}

impl Mechanism for SolidShot {
    type State = SolidShotState;

    /// The rigid force law across the encounter: on each sample the force is the reference's
    /// `A (a + k v²)` scaled by the present substance's share `factor / REFERENCE_FACTOR`, the disc
    /// force their sample mean, so the potential falls by `share · L / (m / (2 A k))` over `L`
    /// metres. Where a sample's layer holds several substances — an authoring overlap, §13.2 — it is
    /// resisted once, by the most resistant of them (the union's law). A sample's substances are
    /// the same over the whole encounter ([`SampleLayer::present`](super::walk::SampleLayer)) and
    /// each carries one factor (`VolumeTable::new`), so its most resistant factor is read over all
    /// its contacts. Each volume met takes a dose of its own factor over the part of its contact
    /// the consumed progress covers (§13.2's damage law: no ownership).
    fn advance(
        &self,
        state: SolidShotState,
        encounter: &Encounter,
        limits: &Limits,
        _geometry: &Geometry,
    ) -> Advance<SolidShotState> {
        let disc = encounter.disc as f64;
        let length = encounter.end - encounter.start;
        let resisted: f64 = encounter
            .samples
            .iter()
            .map(|sample| {
                sample
                    .present
                    .iter()
                    .fold(0.0f32, |most, presence| most.max(presence.material.factor))
                    as f64
            })
            .sum();
        let drop = length * resisted / disc / REFERENCE_FACTOR as f64 / self.scale;
        let potential = self.potential(state.speed);
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
                    speed: self.speed_at(potential - drop),
                },
                Control::Continue,
            )
        };
        let mut effects: Vec<Effect> = Vec::new();
        for sample in &encounter.samples {
            for contact in &sample.present {
                let covered = (consumed - (contact.start - encounter.start))
                    .min(contact.end - contact.start)
                    .max(0.0);
                let chord = covered / disc;
                effects.push(Effect::Line {
                    address: contact.volume,
                    chord,
                    amount: chord * contact.material.factor as f64,
                });
            }
        }
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
}

/// COMPATIBILITY ADAPTER: the overmatch switch, kept only until turning at the face replaces the
/// entrance rules (design doc Part 5, step F). An overmatched crossing is charged on the
/// perpendicular projection of its chord — `|axis · n̄|` of every encounter's length (§4: it cannot
/// present its slope to a round that dwarfs it) — while the geometry it travels stays the slope
/// chord: the inner mechanism advances through the projected length, and what it consumed maps back
/// onto the slope.
pub(crate) struct OvermatchProjection<'a, M> {
    pub inner: &'a M,
}

impl<M: Mechanism> Mechanism for OvermatchProjection<'_, M> {
    type State = M::State;

    fn advance(
        &self,
        state: M::State,
        encounter: &Encounter,
        limits: &Limits,
        geometry: &Geometry,
    ) -> Advance<M::State> {
        let charge = geometry.axis.dot(geometry.entrance_normal).abs() as f64;
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
        let mut advance = self.inner.advance(state, &projected, limits, geometry);
        advance.consumed = if advance.control == Control::Continue || charge <= 0.0 {
            length
        } else {
            (advance.consumed / charge).min(length)
        };
        advance
    }

    fn reference_mm(&self, state: M::State) -> f64 {
        self.inner.reference_mm(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ballistics::walk::{SampleLayer, VolumeContact, VolumeMaterial};

    /// Fixture data: an 88-class body.
    const BODY: RoundBody = RoundBody {
        mass: 10.2,
        caliber: 0.088,
        nose_factor: 0.5,
    };

    const LIMITS: Limits = Limits { rest: 0.0 };

    const HEAD_ON: Geometry = Geometry {
        axis: Vec3::Z,
        entrance_normal: Vec3::NEG_Z,
    };

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

    /// One stretch advances the speed by the rigid force law integrated in closed form from the
    /// reference steel's facts: `v₁² = (v₀² + a/k) · exp(−s · 2 A k L / m) − a/k`, `s` the disc-mean
    /// share of the reference force.
    #[test]
    fn a_stretch_advances_the_speed_by_the_closed_form_force_law() {
        let facts = SubstanceRegistry::shipped_static().reference_facts();
        let a = facts.cavity_strength();
        let k = super::super::B_INERTIA as f64 * BODY.nose_factor as f64 * facts.density;
        let area = std::f64::consts::FRAC_PI_4 * (BODY.caliber as f64).powi(2);
        let round = round();
        for (samples, share) in [
            (&[&[(1, 1000.0)][..]][..], 1.0),
            (&[&[(1, 500.0)][..], &[][..]][..], 0.125),
            (&[&[(1, 10.0)][..]][..], 0.01),
        ] {
            let disc = if samples.len() == 2 { 4 } else { 1 };
            for (speed, length) in [(800.0f32, 0.05), (450.0, 0.01), (1200.0, 0.002)] {
                let advance = round.advance(
                    round.at(speed),
                    &encounter(length, disc, samples),
                    &LIMITS,
                    &HEAD_ON,
                );
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

    /// An authoring overlap is resisted once, by its most resistant substance; every volume in it
    /// still takes a dose of its own factor (no ownership).
    #[test]
    fn an_overlap_is_resisted_once_by_its_most_resistant_substance() {
        let round = round();
        let alone = round.advance(
            round.at(800.0),
            &encounter(0.05, 1, &[&[(1, 1000.0)]]),
            &LIMITS,
            &HEAD_ON,
        );
        let overlapped = round.advance(
            round.at(800.0),
            &encounter(0.05, 1, &[&[(1, 1000.0), (2, 10.0)]]),
            &LIMITS,
            &HEAD_ON,
        );
        assert_eq!(alone.state, overlapped.state);
        assert_eq!(
            overlapped.effects,
            vec![
                Effect::Line {
                    address: volume(1),
                    chord: 0.05,
                    amount: 0.05 * 1000.0,
                },
                Effect::Line {
                    address: volume(2),
                    chord: 0.05,
                    amount: 0.05 * 10.0,
                },
            ]
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

        let crossed = round.advance(
            state,
            &encounter(length, 1, &[&[(1, 1000.0)]]),
            &LIMITS,
            &HEAD_ON,
        );
        assert_eq!(crossed.control, Control::Continue);
        assert_eq!(crossed.consumed, length);
        assert_eq!(crossed.state.speed, 0.0);

        let next = round.advance(
            crossed.state,
            &encounter(1.0, 1, &[&[(2, 100.0)]]),
            &LIMITS,
            &HEAD_ON,
        );
        assert_eq!(next.control, Control::Stop);
        assert_eq!(next.consumed, 0.0);
        assert!(next.effects.iter().all(|effect| match effect {
            Effect::Line { chord, amount, .. } => *chord == 0.0 && *amount == 0.0,
        }));
    }

    /// The overmatch compatibility adapter advances the inner mechanism through the perpendicular
    /// projection of the stretch and maps a rest point back onto the slope.
    #[test]
    fn the_overmatch_adapter_charges_the_projection_and_maps_the_rest_back() {
        let round = round();
        let adapter = OvermatchProjection { inner: &round };
        // 60° obliquity: the projection is half the slope.
        let oblique = Geometry {
            axis: Vec3::Z,
            entrance_normal: Vec3::new(0.0, 3.0f32.sqrt() / 2.0, -0.5),
        };
        let charge = oblique.axis.dot(oblique.entrance_normal).abs() as f64;
        for (speed, length) in [(800.0f32, 0.04), (300.0, 2.0)] {
            let projected = round.advance(
                round.at(speed),
                &encounter(length * charge, 1, &[&[(1, 1000.0)]]),
                &LIMITS,
                &oblique,
            );
            let adapted = adapter.advance(
                round.at(speed),
                &encounter(length, 1, &[&[(1, 1000.0)]]),
                &LIMITS,
                &oblique,
            );
            assert_eq!(adapted.state, projected.state);
            assert_eq!(adapted.control, projected.control);
            assert_eq!(adapted.effects, projected.effects);
            let expected = match projected.control {
                Control::Continue => length,
                _ => projected.consumed / charge,
            };
            assert!(
                (adapted.consumed - expected).abs() < 1.0e-12,
                "{} against {expected}",
                adapted.consumed
            );
        }
    }
}
