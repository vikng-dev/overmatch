//! Session deadlines use wrapped signed differences. No numeric ordering is implemented:
//! a tick near the start of the counter can follow one near its end. Authored spans must be
//! strictly less than half the counter range (DERIVED), checked at their data load gates.

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
    pub const fn after(self, ticks: u32) -> Self {
        Self(self.0.wrapping_add(ticks))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickSpanError {
    OutsideHorizon(u32),
}

impl std::fmt::Display for TickSpanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideHorizon(ticks) => write!(
                f,
                "tick span {ticks} must be under 2^31 ticks; reduce the duration"
            ),
        }
    }
}

impl TryFrom<u32> for TickSpan {
    type Error = TickSpanError;
    fn try_from(ticks: u32) -> Result<Self, Self::Error> {
        if ticks <= Self::MAX {
            Ok(Self(ticks))
        } else {
            Err(TickSpanError::OutsideHorizon(ticks))
        }
    }
}

impl TickSpan {
    /// DERIVED: the greatest positive signed wrapped difference.
    pub const MAX: u32 = i32::MAX as u32;
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Battle and weapon callers rely on the same deadline relation across wrap and at the
    /// largest admitted span. Equality is due, and one tick short is not.
    #[test]
    fn signed_deadlines_cross_wrap_at_every_admitted_span() {
        for start in [0, 1, u32::MAX - 1, u32::MAX] {
            for span in [1, 2, 64, i32::MAX as u32] {
                let now = SimTick::new(start);
                let end = now.after(span);
                assert!(!now.has_reached(end));
                assert_eq!(now.remaining_until(end), span);
                assert!(!now.after(span - 1).has_reached(end));
                assert!(now.after(span).has_reached(end));
                assert!(now.after(span).after(1).has_reached(end));
            }
        }
    }
}
