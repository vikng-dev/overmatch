//! Presentation time: the corner's beats per frame, its easing curves, the replacement rule for a
//! record that arrives while another shows, and the colour scales. Pure functions of a clock, so
//! a scrub or a replay lands on the same picture. Every duration, colour and scale here is a design
//! choice (`demo/ux-spec.md` §3–§5, INFERENCE grade), not physics.

use bevy::math::cubic_splines::CubicSegment;
use bevy::prelude::*;

use crate::damage_cam::ShotVerdict;

/// The corner's look while it plays one record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CornerKind {
    /// Penetration: the see-through hull.
    Glass,
    /// Non-pen: the plate slice.
    Slice,
    /// Ricochet: the tank from outside, following the deflection leg.
    Ricochet,
    /// Damaged: the tank from outside, the dosed components tinted.
    Damaged,
    /// The struck player's own tank: live state under the latest record.
    Victim,
}

impl CornerKind {
    pub fn of(verdict: ShotVerdict) -> Self {
        match verdict {
            ShotVerdict::Penetration => Self::Glass,
            ShotVerdict::NonPen => Self::Slice,
            ShotVerdict::Ricochet => Self::Ricochet,
            ShotVerdict::Damaged => Self::Damaged,
        }
    }

    /// Seconds from the start of the open to the end of the close.
    pub fn duration(self) -> f32 {
        match self {
            Self::Ricochet => 2.18,
            Self::Slice | Self::Damaged => 2.90,
            Self::Glass => 4.30,
            Self::Victim => 3.00,
        }
    }

    /// Where the hold beat starts: the hold key freezes the clock here and after.
    pub fn hold_from(self) -> f32 {
        match self {
            Self::Ricochet => 0.78,
            Self::Slice => 1.28,
            Self::Damaged => 1.03,
            Self::Glass | Self::Victim => 0.88,
        }
    }

    /// The window in which the shell crosses the drawn span, corner seconds.
    pub fn line_window(self) -> (f32, f32) {
        match self {
            Self::Slice => (0.48, 1.08),
            Self::Ricochet | Self::Damaged => (0.18, 0.78),
            Self::Glass | Self::Victim => (0.18, 0.88),
        }
    }
}

/// The open, 180 ms.
pub const OPEN_S: f32 = 0.18;
/// The close, 200 ms, also the dismiss.
pub const CLOSE_S: f32 = 0.20;
/// The accent bar draws along the top over this.
pub const ACCENT_S: f32 = 0.20;
/// The corner opens this long after the world shows the impact.
pub const OPEN_LATENCY_S: f32 = 0.20;
/// A record shows at least this long before a newer one replaces it.
pub const MIN_SHOWN_S: f32 = 0.60;
/// The replacement crossfade.
pub const CROSSFADE_S: f32 = 0.12;
/// A spall cone's reach surface grows over this.
pub const CONE_GROW_S: f32 = 0.40;
/// A box fills to its dose tint over this.
pub const BOX_FILL_S: f32 = 0.25;
/// The slice's cut reveal: the eye's half dissolves over this window.
pub const SLICE_REVEAL: (f32, f32) = (0.18, 0.48);
/// Reduce motion: an element fades in whole over this.
pub const REDUCED_FADE_S: f32 = 0.15;
/// The kill cam lesson's orbit and the ricochet drift, degrees per second.
pub const ORBIT_DEG_S: f32 = 10.0;
pub const DRIFT_DEG_S: f32 = 3.0;

/// Entrances: emphasized decelerate.
pub fn ease_enter(x: f32) -> f32 {
    curve((0.05, 0.7), (0.1, 1.0), x)
}

/// Exits: emphasized accelerate.
pub fn ease_exit(x: f32) -> f32 {
    curve((0.3, 0.0), (0.8, 0.15), x)
}

/// A move from rest to rest: standard, both ends flat.
pub fn ease_standard(x: f32) -> f32 {
    curve((0.2, 0.0), (0.0, 1.0), x)
}

fn curve(p1: (f32, f32), p2: (f32, f32), x: f32) -> f32 {
    CubicSegment::new_bezier_easing(Vec2::from(p1), Vec2::from(p2)).ease(x.clamp(0.0, 1.0))
}

/// `(now − from) / span`, clamped to `0..=1`.
pub fn progress(now: f32, from: f32, span: f32) -> f32 {
    if span <= 0.0 {
        return if now >= from { 1.0 } else { 0.0 };
    }
    ((now - from) / span).clamp(0.0, 1.0)
}

/// The corner's presence at corner time `c` of a run lasting `duration`: opacity and scale.
/// Opens 0 → 1 with scale 0.96 → 1, closes 1 → 0 over the last [`CLOSE_S`].
pub fn presence(c: f32, duration: f32) -> (f32, f32) {
    if c < 0.0 || c > duration {
        return (0.0, 0.96);
    }
    let open = ease_enter(progress(c, 0.0, OPEN_S));
    let close = ease_exit(progress(c, duration - CLOSE_S, CLOSE_S));
    let opacity = open * (1.0 - close);
    (opacity, 0.96 + 0.04 * open)
}

/// One record's turn in the corner: shown from `start` (run seconds) until the next turn's start
/// or its own end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turn {
    /// Index of the record among the run's arrivals.
    pub record: usize,
    pub start: f32,
}

/// The corner's turns for records arriving at `arrivals` (run seconds, ascending): the newest
/// record wins once the current one has shown [`MIN_SHOWN_S`]; one that arrives sooner waits until
/// then, and only the latest waiting record is kept.
pub fn turns(arrivals: &[f32]) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    let mut waiting: Option<usize> = None;
    let flush = |out: &mut Vec<Turn>, waiting: &mut Option<usize>, before: f32| {
        if let (Some(record), Some(current)) = (*waiting, out.last()) {
            let free = current.start + MIN_SHOWN_S;
            if free <= before {
                out.push(Turn {
                    record,
                    start: free,
                });
                *waiting = None;
            }
        }
    };
    for (record, &at) in arrivals.iter().enumerate() {
        flush(&mut out, &mut waiting, at);
        match out.last() {
            Some(current) if at < current.start + MIN_SHOWN_S => waiting = Some(record),
            _ => out.push(Turn { record, start: at }),
        }
    }
    flush(&mut out, &mut waiting, f32::INFINITY);
    out
}

/// The verdict colours (`ux-spec.md` §4): CIEDE2000 ≥ 24 between every pair under normal, protan,
/// deutan and tritan vision (DERIVED, Machado 2009 at severity 1.0). Damaged, the fourth verdict
/// the spec predates, is a dark raspberry chosen by the same test: 25.9 from its nearest neighbour
/// (DERIVED, tritan), lightness separating it from the other three.
pub fn verdict_color(kind: CornerKind) -> Color {
    match kind {
        CornerKind::Glass => Color::srgb_u8(0xF5, 0x90, 0x3D),
        CornerKind::Slice => Color::srgb_u8(0x4F, 0xA3, 0xE0),
        CornerKind::Ricochet => Color::srgb_u8(0xED, 0xE6, 0xD6),
        CornerKind::Damaged => Color::srgb_u8(0xA8, 0x32, 0x5F),
        CornerKind::Victim => Color::srgb_u8(0x8A, 0x96, 0xA3),
    }
}

/// The dose ramp, dark to light; lightness rises monotonically under every simulated vision.
const DOSE_RAMP: [(u8, u8, u8); 5] = [
    (0x5A, 0x1E, 0x0E),
    (0xA8, 0x40, 0x1A),
    (0xE5, 0x77, 0x2A),
    (0xFF, 0xC2, 0x7A),
    (0xFF, 0xF1, 0xDC),
];

/// The ramp at `x` in `0..=1`, linear colour, interpolated between stops in linear space.
pub fn dose_ramp(x: f32) -> LinearRgba {
    let x = x.clamp(0.0, 1.0) * (DOSE_RAMP.len() - 1) as f32;
    let lo = (x.floor() as usize).min(DOSE_RAMP.len() - 2);
    let t = x - lo as f32;
    let stop = |i: usize| {
        let (r, g, b) = DOSE_RAMP[i];
        LinearRgba::from(Color::srgb_u8(r, g, b))
    };
    stop(lo).mix(&stop(lo + 1), t)
}

/// The absolute dose scale (option (a), the default): log-mapped reference millimetres, the same
/// for every box and every cone. [`DOSE_SCALE_MM`] spans the doses the scenario records hold,
/// 0.02 to 40 ref-mm (MEASURED from `assets/dev/pen_cam/`), with headroom.
pub const DOSE_SCALE_MM: (f32, f32) = (0.02, 60.0);

pub fn absolute_dose(dose_mm: f32) -> f32 {
    if dose_mm <= 0.0 {
        return 0.0;
    }
    let (lo, hi) = DOSE_SCALE_MM;
    ((dose_mm.ln() - lo.ln()) / (hi.ln() - lo.ln())).clamp(0.0, 1.0)
}

/// Option (b): the dose over the box's count, clamped at 1 — full exactly when the box is disabled.
pub fn relative_dose(dose_mm: f32, count: f32) -> f32 {
    if dose_mm <= 0.0 || count <= 0.0 {
        return 0.0;
    }
    (dose_mm / count).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easings_run_from_zero_to_one() {
        for ease in [ease_enter, ease_exit, ease_standard] {
            assert!(ease(0.0).abs() < 1e-4);
            assert!((ease(1.0) - 1.0).abs() < 1e-4);
            assert!(ease(0.5) > 0.0 && ease(0.5) < 1.0);
        }
        // The decelerating curve is ahead of linear at mid-time, the accelerating one behind.
        assert!(ease_enter(0.5) > 0.5);
        assert!(ease_exit(0.5) < 0.5);
    }

    /// Every frame ends before 4.5 s, its hold starts after its line window opens, and its line
    /// window closes before its close.
    #[test]
    fn every_frame_fits_its_cap() {
        for kind in [
            CornerKind::Glass,
            CornerKind::Slice,
            CornerKind::Ricochet,
            CornerKind::Damaged,
            CornerKind::Victim,
        ] {
            let (from, to) = kind.line_window();
            assert!(kind.duration() <= 4.5, "{kind:?}");
            assert!(from >= OPEN_S && to <= kind.hold_from(), "{kind:?}");
            assert!(kind.hold_from() < kind.duration() - CLOSE_S, "{kind:?}");
        }
    }

    #[test]
    fn presence_opens_and_closes() {
        let d = 2.18;
        assert_eq!(presence(-0.1, d).0, 0.0);
        assert!(presence(0.05, d).0 > 0.0 && presence(0.05, d).0 < 1.0);
        assert!((presence(1.0, d).0 - 1.0).abs() < 1e-4);
        assert!((presence(1.0, d).1 - 1.0).abs() < 1e-4);
        assert!(presence(d - 0.01, d).0 < 0.2);
        assert_eq!(presence(d + 0.01, d).0, 0.0);
    }

    /// A record 0.8 s behind the first replaces it on arrival; one 0.3 s behind waits until the
    /// first has shown 0.6 s; of two that arrive inside that window only the latest shows; one
    /// arriving long after starts on arrival.
    #[test]
    fn the_newest_record_replaces_after_the_minimum() {
        assert_eq!(
            turns(&[1.0, 1.8]),
            vec![
                Turn {
                    record: 0,
                    start: 1.0
                },
                Turn {
                    record: 1,
                    start: 1.8
                }
            ]
        );
        assert_eq!(
            turns(&[1.0, 1.3]),
            vec![
                Turn {
                    record: 0,
                    start: 1.0
                },
                Turn {
                    record: 1,
                    start: 1.6
                }
            ]
        );
        assert_eq!(
            turns(&[1.0, 1.2, 1.4]),
            vec![
                Turn {
                    record: 0,
                    start: 1.0
                },
                Turn {
                    record: 2,
                    start: 1.6
                }
            ]
        );
        assert_eq!(
            turns(&[1.0, 5.0]),
            vec![
                Turn {
                    record: 0,
                    start: 1.0
                },
                Turn {
                    record: 1,
                    start: 5.0
                }
            ]
        );
        assert!(turns(&[]).is_empty());
    }

    #[test]
    fn dose_scales_are_monotonic_and_bounded() {
        assert_eq!(absolute_dose(0.0), 0.0);
        assert!(absolute_dose(0.1) < absolute_dose(1.0));
        assert!(absolute_dose(1.0) < absolute_dose(10.0));
        assert_eq!(absolute_dose(1.0e6), 1.0);
        assert_eq!(relative_dose(1.0, 3.0), 1.0 / 3.0);
        assert_eq!(relative_dose(9.0, 3.0), 1.0);
        assert_eq!(relative_dose(1.0, 0.0), 0.0);
    }

    /// The ramp gets lighter as the dose rises.
    #[test]
    fn the_dose_ramp_rises_in_lightness() {
        let luminance = |c: LinearRgba| 0.2126 * c.red + 0.7152 * c.green + 0.0722 * c.blue;
        let mut last = -1.0;
        for i in 0..=20 {
            let l = luminance(dose_ramp(i as f32 / 20.0));
            assert!(l > last, "step {i}");
            last = l;
        }
    }
}
