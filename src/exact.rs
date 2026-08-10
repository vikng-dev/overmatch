//! Exact integer arithmetic — where a geometric predicate that must not have a tolerance is
//! decided.
//!
//! Every finite `f32` is a dyadic rational `m · 2^e` with `|m| < 2^24` and `e ≥ -149`, so
//! `v · 2^149` is an INTEGER for every one of them. A polynomial in `f32` coordinates is therefore
//! an integer once every term carries the same power of two, and "is this determinant zero", "is
//! this crossing parameter that vertex" have exact integer answers. [`Int`] is the arithmetic those
//! answers are taken in.
//!
//! The scale is the caller's choice, because the caller knows its own domain: a shift of `149`
//! encodes any `f32` at all, while a shift of `87` suffices for a coordinate inside
//! [`crate::bake::CERTIFIED_RANGE`] and keeps every value two limbs wide.
//!
//! Fixed width and `Copy`: no allocation on a path a bake sweep runs millions of times. Every
//! operation asserts on overflow rather than wrapping — a silently truncated determinant is a
//! wrong sign, which is the one thing this module exists to make impossible.

use std::cmp::Ordering;

/// 64-bit limbs per [`Int`] — 1024 bits, against the widest value either caller forms:
///
/// * `ballistics::collect`'s parallel determinant, over ARBITRARY `f32` at shift `149`: a
///   coordinate is ≤ 277 bits, a difference ≤ 278, a triple product ≤ 834, the six-term sum ≤ 837.
/// * `bake::embedding`'s interval comparison, over CERTIFIED coordinates at shift `87`: a
///   coordinate is ≤ 104 bits, a plane sign ≤ 318, an endpoint numerator ≤ 423, and the
///   cross-multiplication that compares two endpoints ≤ 743.
const LIMBS: usize = 16;

/// A signed integer of up to [`LIMBS`] × 64 bits, in sign-magnitude form.
///
/// Canonical: `len` counts the significant limbs, every limb above it is zero, and zero is
/// `len == 0` with `negative == false`. So the derived `PartialEq` IS numeric equality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Int {
    negative: bool,
    limbs: [u64; LIMBS],
    len: usize,
}

impl Int {
    pub(crate) const ZERO: Self = Self {
        negative: false,
        limbs: [0; LIMBS],
        len: 0,
    };

    pub(crate) fn from_i128(value: i128) -> Self {
        let magnitude = value.unsigned_abs();
        let mut limbs = [0u64; LIMBS];
        limbs[0] = magnitude as u64;
        limbs[1] = (magnitude >> 64) as u64;
        Self {
            negative: value < 0,
            limbs,
            len: 2,
        }
        .trim()
    }

    /// `value · 2^shift`, exactly. The shift must make the product an integer — for an arbitrary
    /// `f32` that is `149`, and for a coordinate this crate has already certified, less.
    pub(crate) fn from_f32_scaled(value: f32, shift: i32) -> Self {
        assert!(value.is_finite(), "exact: {value} is not finite");
        let bits = value.to_bits();
        let biased = ((bits >> 23) & 0xff) as i32;
        let fraction = (bits & 0x007f_ffff) as i128;
        // Subnormals carry no implicit leading bit and sit at the fixed exponent `-149`.
        let (mantissa, exponent) = if biased == 0 {
            (fraction, -149)
        } else {
            (fraction | (1 << 23), biased - 150)
        };
        if mantissa == 0 {
            return Self::ZERO;
        }
        // The significand is not normalized, so a value well inside the shift's reach can still
        // present a negative place count; only bits that would actually be LOST are a failure.
        let places = exponent + shift;
        let (mantissa, places) = if places < 0 {
            let dropped = places.unsigned_abs();
            assert!(
                dropped < 24 && mantissa.trailing_zeros() >= dropped,
                "exact: {value} is not an integer multiple of 2^-{shift}"
            );
            (mantissa >> dropped, 0)
        } else {
            (mantissa, places)
        };
        Self::from_i128(mantissa)
            .shifted(places as usize)
            .with_sign(bits >> 31 == 1)
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn signum(&self) -> i32 {
        if self.len == 0 {
            0
        } else if self.negative {
            -1
        } else {
            1
        }
    }

    pub(crate) fn negated(mut self) -> Self {
        if self.len != 0 {
            self.negative = !self.negative;
        }
        self
    }

    pub(crate) fn add(self, other: Self) -> Self {
        if self.negative == other.negative {
            let (limbs, len) = magnitude_add(&self, &other);
            Self {
                negative: self.negative,
                limbs,
                len,
            }
            .trim()
        } else {
            match self.magnitude_cmp(&other) {
                Ordering::Equal => Self::ZERO,
                Ordering::Greater => {
                    let (limbs, len) = magnitude_sub(&self, &other);
                    Self {
                        negative: self.negative,
                        limbs,
                        len,
                    }
                    .trim()
                }
                Ordering::Less => {
                    let (limbs, len) = magnitude_sub(&other, &self);
                    Self {
                        negative: other.negative,
                        limbs,
                        len,
                    }
                    .trim()
                }
            }
        }
    }

    pub(crate) fn sub(self, other: Self) -> Self {
        self.add(other.negated())
    }

    pub(crate) fn mul(self, other: Self) -> Self {
        if self.len == 0 || other.len == 0 {
            return Self::ZERO;
        }
        assert!(self.len + other.len <= LIMBS, "exact: product overflow");
        let mut limbs = [0u64; LIMBS];
        for i in 0..self.len {
            let mut carry = 0u128;
            for j in 0..other.len {
                let acc =
                    self.limbs[i] as u128 * other.limbs[j] as u128 + limbs[i + j] as u128 + carry;
                limbs[i + j] = acc as u64;
                carry = acc >> 64;
            }
            let mut slot = i + other.len;
            while carry != 0 {
                let acc = limbs[slot] as u128 + carry;
                limbs[slot] = acc as u64;
                carry = acc >> 64;
                slot += 1;
            }
        }
        Self {
            negative: self.negative != other.negative,
            limbs,
            len: self.len + other.len,
        }
        .trim()
    }

    /// The nearest `f64`, to within a relative error of `2^-52`.
    ///
    /// For a RATIO of two exact integers of the same homogeneous degree, which is where the callers
    /// use it: the scale cancels, and the quotient of the two conversions carries the accuracy the
    /// division has left, rather than the accuracy the cancelled float determinant did not.
    pub(crate) fn to_f64(self) -> f64 {
        if self.len == 0 {
            return 0.0;
        }
        let top = self.len - 1;
        let magnitude = if top == 0 {
            self.limbs[0] as f64
        } else {
            // The top 64 significant bits, and the power of two they were taken from.
            let leading = 64 - self.limbs[top].leading_zeros() as usize;
            let window = ((self.limbs[top] as u128) << 64) | self.limbs[top - 1] as u128;
            (window >> leading) as u64 as f64 * exp2((top as i32 - 1) * 64 + leading as i32)
        };
        if self.negative { -magnitude } else { magnitude }
    }

    fn magnitude_cmp(&self, other: &Self) -> Ordering {
        if self.len != other.len {
            return self.len.cmp(&other.len);
        }
        for slot in (0..self.len).rev() {
            if self.limbs[slot] != other.limbs[slot] {
                return self.limbs[slot].cmp(&other.limbs[slot]);
            }
        }
        Ordering::Equal
    }

    fn with_sign(mut self, negative: bool) -> Self {
        if self.len != 0 {
            self.negative = negative;
        }
        self
    }

    fn shifted(self, bits: usize) -> Self {
        if self.len == 0 {
            return self;
        }
        let (words, rest) = (bits / 64, bits % 64);
        let len = self.len + words + usize::from(rest > 0);
        assert!(len <= LIMBS, "exact: shift overflow");
        let mut limbs = [0u64; LIMBS];
        for slot in 0..self.len {
            limbs[slot + words] |= self.limbs[slot] << rest;
            if rest > 0 {
                limbs[slot + words + 1] |= self.limbs[slot] >> (64 - rest);
            }
        }
        Self {
            negative: self.negative,
            limbs,
            len,
        }
        .trim()
    }

    fn trim(mut self) -> Self {
        while self.len > 0 && self.limbs[self.len - 1] == 0 {
            self.len -= 1;
        }
        if self.len == 0 {
            self.negative = false;
        }
        self
    }
}

impl Ord for Int {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.signum(), other.signum()) {
            (mine, theirs) if mine != theirs => mine.cmp(&theirs),
            (0, _) => Ordering::Equal,
            (1, _) => self.magnitude_cmp(other),
            _ => other.magnitude_cmp(self),
        }
    }
}

impl PartialOrd for Int {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `2^power`, exactly, over the whole `f64` range including the subnormals.
fn exp2(power: i32) -> f64 {
    assert!(power <= 1023, "exact: 2^{power} is not finite");
    if power >= -1022 {
        return f64::from_bits(((power + 1023) as u64) << 52);
    }
    // Two steps, so the intermediate stays normal and the only rounding is the intended one.
    f64::from_bits(1) * exp2(power + 1074)
}

fn magnitude_add(a: &Int, b: &Int) -> ([u64; LIMBS], usize) {
    let len = a.len.max(b.len);
    let mut limbs = [0u64; LIMBS];
    let mut carry = 0u64;
    for (slot, limb) in limbs.iter_mut().enumerate().take(len) {
        let (sum, over) = a.limbs[slot].overflowing_add(b.limbs[slot]);
        let (sum, again) = sum.overflowing_add(carry);
        *limb = sum;
        carry = u64::from(over) + u64::from(again);
    }
    if carry != 0 {
        assert!(len < LIMBS, "exact: sum overflow");
        limbs[len] = carry;
        return (limbs, len + 1);
    }
    (limbs, len)
}

/// `|a| − |b|`, which the caller has already established is non-negative.
fn magnitude_sub(a: &Int, b: &Int) -> ([u64; LIMBS], usize) {
    let mut limbs = [0u64; LIMBS];
    let mut borrow = 0u64;
    for (slot, limb) in limbs.iter_mut().enumerate().take(a.len) {
        let (difference, under) = a.limbs[slot].overflowing_sub(b.limbs[slot]);
        let (difference, again) = difference.overflowing_sub(borrow);
        *limb = difference;
        borrow = u64::from(under) + u64::from(again);
    }
    (limbs, a.len)
}

/// A ratio of two [`Int`], with the sign carried on the numerator.
///
/// Comparison is cross-multiplication, so two ratios formed from the same homogeneous degree order
/// exactly. Never reduced: a common factor costs nothing but a few limbs of width.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ratio {
    numerator: Int,
    denominator: Int,
}

impl Ratio {
    pub(crate) fn new(numerator: Int, denominator: Int) -> Self {
        assert!(!denominator.is_zero(), "exact: a ratio over zero");
        if denominator.signum() < 0 {
            return Self {
                numerator: numerator.negated(),
                denominator: denominator.negated(),
            };
        }
        Self {
            numerator,
            denominator,
        }
    }

    /// The integer `value`, as a ratio over one.
    pub(crate) fn whole(value: Int) -> Self {
        Self {
            numerator: value,
            denominator: Int::from_i128(1),
        }
    }
}

impl Ord for Ratio {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numerator
            .mul(other.denominator)
            .cmp(&other.numerator.mul(self.denominator))
    }
}

impl PartialOrd for Ratio {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Ratio {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ratio {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_f32_scales_to_the_integer_it_is() {
        assert_eq!(
            Int::from_f32_scaled(0.5, 1),
            Int::from_i128(1),
            "0.5 · 2 is one"
        );
        assert_eq!(Int::from_f32_scaled(-3.0, 0), Int::from_i128(-3));
        assert_eq!(Int::from_f32_scaled(0.0, 149), Int::ZERO);
        assert_eq!(Int::from_f32_scaled(-0.0, 149), Int::ZERO);
        // The smallest subnormal is one unit at the universal shift, and the largest finite f32 is
        // the widest value that shift produces.
        assert_eq!(
            Int::from_f32_scaled(f32::from_bits(1), 149),
            Int::from_i128(1)
        );
        let widest = Int::from_f32_scaled(f32::MAX, 149);
        assert_eq!(widest.to_f64() / exp2(149), f64::from(f32::MAX));
    }

    #[test]
    fn the_arithmetic_is_exact_where_f64_cancels() {
        // (2^80 + 1) − 2^80 is one, which f64 subtraction cannot say.
        let big = Int::from_i128(1i128 << 80);
        let one = Int::from_i128(1);
        assert_eq!(big.add(one).sub(big), one);
        assert!(big.add(one).sub(big).sub(one).is_zero());
        // A product wider than i128, compared against the same product built the other way.
        let a = Int::from_i128((1i128 << 100) + 7);
        let b = Int::from_i128((1i128 << 100) - 7);
        assert_eq!(
            a.mul(b),
            Int::from_i128(1i128 << 100)
                .mul(Int::from_i128(1i128 << 100))
                .sub(Int::from_i128(49))
        );
        assert_eq!(a.mul(b).signum(), 1);
        assert_eq!(a.mul(b.negated()).signum(), -1);
        assert_eq!(a.mul(Int::ZERO), Int::ZERO);
    }

    #[test]
    fn a_ratio_orders_by_cross_multiplication() {
        let ratio = |n: i128, d: i128| Ratio::new(Int::from_i128(n), Int::from_i128(d));
        assert!(ratio(1, 3) < ratio(1, 2));
        assert!(ratio(-1, 3) < ratio(1, 300000));
        assert_eq!(ratio(2, 4), ratio(1, 2));
        // A negative denominator is normalized onto the numerator, so the order survives it.
        assert_eq!(ratio(1, -2), ratio(-1, 2));
        assert!(ratio(-1, -2) > ratio(0, 1));
        assert_eq!(Ratio::whole(Int::from_i128(3)), ratio(3, 1));
    }

    #[test]
    fn the_f64_conversion_keeps_the_ratio_a_cancelled_determinant_loses() {
        let numerator = Int::from_i128((1i128 << 100) + 1);
        let denominator = Int::from_i128(1i128 << 100);
        let quotient = numerator.to_f64() / denominator.to_f64();
        assert!((quotient - 1.0).abs() <= 4.0 * f64::EPSILON, "{quotient}");
        assert_eq!(Int::from_i128(-5).to_f64(), -5.0);
        assert_eq!(Int::ZERO.to_f64(), 0.0);
        assert_eq!(exp2(-1074), f64::from_bits(1));
        assert_eq!(exp2(0), 1.0);
    }
}
