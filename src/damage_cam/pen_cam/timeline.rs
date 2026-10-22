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
/// A box fills to its dose tint over this.
pub const BOX_FILL_S: f32 = 0.25;
/// The slice's cut reveal: the eye's half dissolves over this window.
pub const SLICE_REVEAL: (f32, f32) = (0.18, 0.48);
/// Reduce motion: an element fades in whole over this.
pub const REDUCED_FADE_S: f32 = 0.15;
/// The Pen frames' and the kill cam lesson's orbit, and the Damaged and ricochet drift, degrees
/// per second.
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
/// then, and only the latest waiting record is kept ([`Lineup`]).
pub fn turns(arrivals: &[f32]) -> Vec<Turn> {
    let mut lineup = Lineup::default();
    for (record, &at) in arrivals.iter().enumerate() {
        lineup.arrive(record, at);
    }
    let mut out = Vec::new();
    lineup.advance(f32::INFINITY, |&record, start| {
        out.push(Turn { record, start });
    });
    out
}

/// The replacement rule on a running clock: records arrive, each to open at its own time, and
/// [`Self::advance`] starts their turns as the clock reaches them. A record whose time comes while
/// the current turn is inside its first [`MIN_SHOWN_S`] waits until that minimum is out, displacing
/// any record already waiting; otherwise it starts at its own time. Never more than one turn is
/// current. [`turns`] is this rule over a whole run.
#[derive(Clone, Debug)]
pub struct Lineup<T> {
    /// Records not yet due, ascending by the time each opens at.
    arriving: Vec<(T, f32)>,
    waiting: Option<T>,
    /// The current turn's record and start; it stays current after its own end until another
    /// starts or [`Self::clear`].
    current: Option<(T, f32)>,
}

impl<T> Default for Lineup<T> {
    fn default() -> Self {
        Self {
            arriving: Vec::new(),
            waiting: None,
            current: None,
        }
    }
}

impl<T> Lineup<T> {
    /// `record` opens at `at` (clock seconds), by the rule.
    pub fn arrive(&mut self, record: T, at: f32) {
        let index = self.arriving.partition_point(|(_, due)| *due <= at);
        self.arriving.insert(index, (record, at));
    }

    /// Run the rule up to `now`, calling `started` with each turn that starts on the way and its
    /// start, in order.
    pub fn advance(&mut self, now: f32, mut started: impl FnMut(&T, f32)) {
        loop {
            let due = self
                .arriving
                .first()
                .map(|(_, at)| *at)
                .filter(|at| *at <= now);
            let before = due.unwrap_or(now);
            if let Some((_, start)) = &self.current
                && self.waiting.is_some()
            {
                let free = start + MIN_SHOWN_S;
                if free <= before
                    && let Some(record) = self.waiting.take()
                {
                    started(&record, free);
                    self.current = Some((record, free));
                }
            }
            let Some(at) = due else {
                return;
            };
            let (record, _) = self.arriving.remove(0);
            match &self.current {
                Some((_, start)) if at < start + MIN_SHOWN_S => self.waiting = Some(record),
                _ => {
                    started(&record, at);
                    self.current = Some((record, at));
                }
            }
        }
    }

    /// The current turn's record and start.
    pub fn current(&self) -> Option<(&T, f32)> {
        self.current
            .as_ref()
            .map(|(record, start)| (record, *start))
    }

    /// Whether a record is still to start: due later, or waiting on the current turn's minimum.
    pub fn pending(&self) -> bool {
        self.waiting.is_some() || !self.arriving.is_empty()
    }

    /// The record to start next, if one is still to start.
    pub fn next(&self) -> Option<&T> {
        self.waiting
            .as_ref()
            .or_else(|| self.arriving.first().map(|(record, _)| record))
    }

    /// Hold the current turn's start at `now` or later: a turn whose stage cannot draw yet waits.
    pub fn hold_until(&mut self, now: f32) {
        if let Some((_, start)) = &mut self.current {
            *start = start.max(now);
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// A turn's look at corner time `c` of `duration`: opacity, scale and, while it replaces a turn
/// that is still open, the replaced turn's opacity in the crossfade. A replacement keeps the frame
/// open and crossfades its content over [`CROSSFADE_S`]; any other turn opens and closes by
/// [`presence`].
pub fn turn_presence(c: f32, duration: f32, replacing: bool) -> (f32, f32, Option<f32>) {
    if replacing && (0.0..OPEN_S).contains(&c) {
        return (1.0, 1.0, Some(1.0 - progress(c, 0.0, CROSSFADE_S)));
    }
    let (opacity, scale) = presence(c, duration);
    (opacity, scale, None)
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

/// The component state scale, one for every view that draws a component. A component works fully
/// until it has no hit points left, so damage is a quantity, never a hue: an untouched component is
/// a hueless grey, a damaged one fills with one red from the bottom up (tank-up) to the share of
/// its hit points lost, and a dead one is charcoal under a red diagonal hatch, which carries the
/// state without its colour. TUNED.
pub const STATE_UNTOUCHED: Color = Color::srgb_u8(0x7C, 0x7C, 0x7C);
pub const STATE_RED: Color = Color::srgb_u8(0xE0, 0x30, 0x28);
pub const STATE_DEAD: Color = Color::srgb_u8(0x26, 0x28, 0x2C);
/// The line a damaged component's fill ends in: lighter than the red and the grey either side.
pub const STATE_LINE: Color = Color::srgb_u8(0xFF, 0xC8, 0xB8);

/// A struck component's place on the state scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Condition {
    /// Working, the share of its hit points lost in `(0, 1)`.
    Damaged(f32),
    Dead,
}

impl Condition {
    /// The condition of a component lost or not, `share` of its hit points gone; `None` when it is
    /// untouched.
    pub fn of(lost: bool, share: f32) -> Option<Self> {
        if lost || share >= 1.0 {
            Some(Self::Dead)
        } else if share > 0.0 {
            Some(Self::Damaged(share))
        } else {
            None
        }
    }

    /// The share of the component's height its fill reaches.
    pub fn fill(self) -> f32 {
        match self {
            Self::Damaged(share) => share,
            Self::Dead => 1.0,
        }
    }
}

/// The spall field's hue: a cool teal apart from the state scale and the glass hull's blue-grey,
/// so a field never reads as damage or as hull.
pub const FIELD: Color = Color::srgb_u8(0x3F, 0xD0, 0xC8);
/// The field's edge shell: near-white, a contour with no hue of its own.
pub const FIELD_EDGE: Color = Color::srgb_u8(0xE6, 0xEE, 0xF5);

/// The cone strength (ref-mm) a spall field is drawn at full level: the one display gain of the
/// field's brightness and its footprint's opacity, both linear in the strength. TUNED: a steel
/// plate's exit at the demo's ranges.
pub const FIELD_FULL_MM: f32 = 10.0;

/// A field's display level, linear in its cone's on-axis strength `strength_mm` (ref-mm): 1 at
/// [`FIELD_FULL_MM`], unbounded above, 0 for none.
pub fn field_level(strength_mm: f32) -> f32 {
    strength_mm.max(0.0) / FIELD_FULL_MM
}

/// The share of its HP points a component lost to the shot, of the points it had at the shot,
/// clamped at 1 — full exactly when the shot disabled it. A component with none left at the shot
/// reads full.
pub fn hp_share(lost: u32, hp: u32) -> f32 {
    if hp == 0 {
        return 1.0;
    }
    (lost as f32 / hp as f32).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// On a live clock the rule starts the same turns as over the whole run, a record arriving
    /// before its time is not shown early, and a held start waits for the stage.
    #[test]
    fn the_running_lineup_is_the_rule() {
        let arrivals = [1.0, 1.2, 1.4, 3.0, 3.3, 9.0];
        let mut lineup = Lineup::default();
        let mut live = Vec::new();
        let mut fed = 0;
        for frame in 0..400 {
            let now = frame as f32 / 30.0;
            while fed < arrivals.len() && arrivals[fed] <= now + 0.2 {
                lineup.arrive(fed, arrivals[fed]);
                fed += 1;
            }
            lineup.advance(now, |&record, start| live.push(Turn { record, start }));
            if let Some((_, start)) = lineup.current() {
                assert!(start <= now + 1e-6);
            }
        }
        assert_eq!(live, turns(&arrivals));
        let mut held = Lineup::default();
        held.arrive(0, 1.0);
        held.advance(1.0, |_, _| {});
        held.hold_until(1.5);
        assert_eq!(held.current().map(|(_, start)| start), Some(1.5));
        assert!(!held.pending());
    }

    #[test]
    fn the_hp_share_is_the_points_lost_of_the_points_held() {
        assert_eq!(hp_share(10, 30), 1.0 / 3.0);
        assert_eq!(hp_share(90, 30), 1.0);
        assert_eq!(hp_share(0, 30), 0.0);
        assert_eq!(hp_share(10, 0), 1.0);
    }

    /// A component is untouched with no share lost, damaged below the whole of it, dead at the
    /// whole or when lost, and its fill reaches exactly its share.
    #[test]
    fn the_state_scale_fills_by_the_share_lost() {
        assert_eq!(Condition::of(false, 0.0), None);
        assert_eq!(Condition::of(false, 0.05), Some(Condition::Damaged(0.05)));
        assert_eq!(Condition::of(false, 1.0), Some(Condition::Dead));
        assert_eq!(Condition::of(true, 0.0), Some(Condition::Dead));
        assert_eq!(Condition::Damaged(0.3).fill(), 0.3);
        assert_eq!(Condition::Dead.fill(), 1.0);
        let hue = |c: Color| {
            let c = c.to_srgba();
            c.red.max(c.green).max(c.blue) - c.red.min(c.green).min(c.blue)
        };
        assert_eq!(
            hue(STATE_UNTOUCHED),
            0.0,
            "an untouched component carries no hue"
        );
    }
}
