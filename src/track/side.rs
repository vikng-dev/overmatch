//! One side encoding for the track core and its sandbox: the [`Side`] enum plus the [`PerSide`]
//! pair. Left sits at −X, right at +X (matching the tank rig's `TrackSide`). The `[T; 2]` layout
//! is index 0 = left, 1 = right, and iteration is ALWAYS left-then-right — the order is
//! load-bearing (force accumulation and replicated side-array order are part of determinism), so
//! [`Side::index`]/[`Side::ALL`] pin it in one place rather than in every hand-written `match`.
//!
//! Replicated component arrays (`TrackDrive.sides`, `TrackGrip.sides`) stay bare `[T; 2]` — this
//! type owns the ACCESS convention, not the wire shape.

/// Which track. Left at −X, right at +X.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// Both sides in the canonical left-then-right order.
    pub const ALL: [Side; 2] = [Side::Left, Side::Right];

    /// Array index into a side `[T; 2]`: `0` left, `1` right.
    pub fn index(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }

    /// Lateral sign: −1 left, +1 right (exact — a bare sign flip).
    pub fn sign(self) -> f32 {
        match self {
            Side::Left => -1.0,
            Side::Right => 1.0,
        }
    }

    /// This side's plane offset (m) from the centreline for a given half-tread.
    pub fn plane_x(self, half_tread: f32) -> f32 {
        self.sign() * half_tread
    }
}

/// A value per track side, `[left, right]`. A thin wrapper over `[T; 2]` so the side↔index
/// convention and the fixed left-then-right iteration live in exactly one place.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct PerSide<T>(pub [T; 2]);

impl<T> PerSide<T> {
    /// Build from explicit left/right values.
    pub fn new(left: T, right: T) -> Self {
        Self([left, right])
    }

    pub fn get(&self, side: Side) -> &T {
        &self.0[side.index()]
    }

    pub fn get_mut(&mut self, side: Side) -> &mut T {
        &mut self.0[side.index()]
    }

    /// Map each side to a new value, preserving left-then-right order.
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> PerSide<U> {
        PerSide(self.0.map(f))
    }

    /// `(side, &value)` pairs in fixed left-then-right order.
    pub fn iter(&self) -> impl Iterator<Item = (Side, &T)> {
        Side::ALL.into_iter().zip(self.0.iter())
    }

    /// Values only, left-then-right.
    pub fn values(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }

    /// Mutable values, left-then-right.
    pub fn values_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.0.iter_mut()
    }
}
