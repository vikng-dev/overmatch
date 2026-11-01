//! Session deadlines use wrapped signed differences. No numeric ordering is implemented:
//! a tick near the start of the counter can follow one near its end. Authored spans must be
//! strictly less than half the counter range (DERIVED), parsed into [`TickSpan`] at their data
//! load gates.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SimTick(u32);

impl SimTick {
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }
    pub const fn raw(self) -> u32 {
        self.0
    }
    /// The tick `span` later. The span's type bounds it below half the counter, so the deadline
    /// is unambiguous under [`Self::has_reached`].
    pub const fn after(self, span: TickSpan) -> Self {
        Self(self.0.wrapping_add(span.0))
    }
    pub const fn has_reached(self, deadline: Self) -> bool {
        self.0.wrapping_sub(deadline.0) as i32 >= 0
    }
    pub fn remaining_until(self, deadline: Self) -> u32 {
        (deadline.0.wrapping_sub(self.0) as i32).max(0) as u32
    }
}

/// A duration whose deadline is unambiguous under the signed wrapped relation. Deserialization
/// calls the same checked constructor as local code, before a peer value reaches a gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32")]
pub struct TickSpan(u32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TickSpanError {
    OutsideHorizon(u128),
    /// Seconds that are no duration: negative, NaN or infinite.
    NotADuration(f32),
}

impl std::fmt::Display for TickSpanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideHorizon(ticks) => write!(
                f,
                "tick span {ticks} must be under 2^31 fixed ticks; reduce the duration"
            ),
            Self::NotADuration(seconds) => write!(f, "{seconds} s must be a duration >= 0 s"),
        }
    }
}

impl TryFrom<u32> for TickSpan {
    type Error = TickSpanError;
    fn try_from(ticks: u32) -> Result<Self, Self::Error> {
        if ticks <= Self::MAX {
            Ok(Self(ticks))
        } else {
            Err(TickSpanError::OutsideHorizon(u128::from(ticks)))
        }
    }
}

impl TickSpan {
    /// DERIVED: the greatest positive signed wrapped difference.
    pub const MAX: u32 = i32::MAX as u32;
    pub const ONE: Self = Self::of(1);
    /// A span named in code. Outside the horizon it fails to compile in a constant, and panics
    /// as a bug anywhere else; outside input goes through [`TryFrom`].
    pub const fn of(ticks: u32) -> Self {
        assert!(ticks <= Self::MAX, "a tick span is under 2^31 ticks");
        Self(ticks)
    }
    pub const fn raw(self) -> u32 {
        self.0
    }
    /// The span's length at the sim's fixed timestep (s).
    pub fn secs(self) -> f32 {
        (sim_timestep() * self.0).as_secs_f32()
    }
    /// The span of `seconds` at the sim's fixed timestep ([`sim_timestep`]), rounded up to the
    /// first tick at or after it. Integer nanoseconds keep the quantization free of a changing
    /// float. Data parses each authored duration here once, at its load gate.
    pub fn from_secs(seconds: f32) -> Result<Self, TickSpanError> {
        let duration = std::time::Duration::try_from_secs_f32(seconds)
            .map_err(|_| TickSpanError::NotADuration(seconds))?;
        Self::from_duration(duration, sim_timestep())
    }
    /// Quantize a duration once at its composition's timestep, rounding up and refusing spans
    /// outside the signed deadline horizon. Battle and weapon load gates share this law.
    pub fn from_duration(
        duration: std::time::Duration,
        timestep: std::time::Duration,
    ) -> Result<Self, TickSpanError> {
        assert!(
            !timestep.is_zero(),
            "composition timesteps are positive code constants"
        );
        let ticks = duration.as_nanos().div_ceil(timestep.as_nanos());
        let count = u32::try_from(ticks).map_err(|_| TickSpanError::OutsideHorizon(ticks))?;
        Self::try_from(count)
    }
    /// At least one tick: a deadline armed now cannot mature in the tick that armed it.
    pub const fn at_least_one(self) -> Self {
        if self.0 == 0 { Self::ONE } else { self }
    }
}

/// The sim's fixed timestep: Bevy's default, the one every composition runs its fixed loop at
/// (none sets another). Spans parsed at load assume it; the weapon composition checks it at startup.
pub fn sim_timestep() -> std::time::Duration {
    bevy::time::Time::<bevy::time::Fixed>::default().timestep()
}

/// A bug outcome when a composition runs its fixed loop at another timestep than the one its
/// data's spans were parsed at.
pub fn assert_sim_timestep(timestep: std::time::Duration) {
    assert_eq!(
        timestep,
        sim_timestep(),
        "every composition runs the fixed loop at the default timestep its tick spans were parsed at"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Battle and weapon load gates rely on the same ceiling and horizon at any timestep.
    /// FIXTURE DATA: exact, fractional and zero durations at two fixed rates.
    #[test]
    fn every_duration_gate_uses_one_quantization_law() {
        use std::time::Duration;
        for timestep in [sim_timestep(), Duration::from_millis(10)] {
            for count in [0, 1, 31, TickSpan::MAX] {
                assert_eq!(
                    TickSpan::from_duration(timestep * count, timestep),
                    Ok(TickSpan::of(count))
                );
            }
            assert_eq!(
                TickSpan::from_duration(Duration::from_nanos(1), timestep),
                Ok(TickSpan::ONE)
            );
            assert!(matches!(
                TickSpan::from_duration(timestep * (TickSpan::MAX + 1), timestep),
                Err(TickSpanError::OutsideHorizon(_))
            ));
        }
    }

    /// Composition code owns the timestep; a zero constant is a bug, never outside input.
    #[test]
    #[should_panic(expected = "composition timesteps are positive code constants")]
    fn a_zero_composition_timestep_is_a_bug() {
        TickSpan::from_duration(std::time::Duration::ZERO, std::time::Duration::ZERO)
            .expect("the zero timestep must panic before a duration verdict");
    }

    /// Battle and weapon callers rely on the same deadline relation across wrap and at the
    /// largest admitted span. Equality is due, and one tick short is not.
    #[test]
    fn signed_deadlines_cross_wrap_at_every_admitted_span() {
        let span_of = |ticks| TickSpan::try_from(ticks).expect("fixture span");
        for start in [0, 1, u32::MAX - 1, u32::MAX] {
            for span in [1, 2, 64, i32::MAX as u32] {
                let now = SimTick::new(start);
                let end = now.after(span_of(span));
                assert!(!now.has_reached(end));
                assert_eq!(now.remaining_until(end), span);
                assert!(!now.after(span_of(span - 1)).has_reached(end));
                assert!(now.after(span_of(span)).has_reached(end));
                assert!(
                    now.after(span_of(span))
                        .after(TickSpan::ONE)
                        .has_reached(end)
                );
            }
        }
    }
}
