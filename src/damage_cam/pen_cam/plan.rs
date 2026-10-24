//! What the corner draws for one record, read off it and the tank asset's component shapes: the
//! line as an arc-length polyline, the crossings, the bounce, the stop or the point the round left
//! the tank, each field's directions and reach (a spall cone's or a burst's), and each component's
//! dose and the moment the picture reaches it. Positions are in the record's frame (hull-local,
//! metres).
//!
//! Whether the round stopped, where it left the tank and where the line entered a component are the
//! record's own facts; nothing here re-derives them from geometry. A component's shape, and the
//! whole tank's bounds, serve only framing, never timing. Three things are derived rather than
//! read, and are labelled where they are made: the line's remaining capability, `(v/v₀)²` from the
//! path's times (the record carries no capability), the moment a component is reached (the
//! earliest of the line's recorded entry and each dosing field's front arriving; a synthetic clip
//! that names neither is reached at contact), and each field front's speed ([`Cone::speed`]; the
//! record holds no fragment speed). Every time here is the record's own (s); the corner plays it at
//! a fixed slowdown (`super::timeline::REPLAY_SLOWDOWN`).

use bevy::prelude::*;

use super::field::Disc;
use super::timeline::CornerKind;
use crate::damage_cam::shapes::TankShapes;
use crate::damage_cam::{
    ConsequenceState, PenetrationReport, PlateClass, Role, RoundEnd, SpallCone,
};

/// The line is drawn from this far (m) before first contact. TUNED: a diagram choice.
pub const LEAD_M: f32 = 1.5;
/// A path that leaves the tank is drawn this far (m) past the point it left. TUNED: a diagram
/// choice.
pub const RUN_OUT_M: f32 = 1.5;
/// Two arc lengths this close (m) name the same point of the line: a node's arc against the arc
/// [`Polyline::arc_of`] finds for that node. DERIVED: arc lengths are f32 sums of legs under 100 m,
/// so their rounding is under 1e-5 m; nodes are at least 1e-6 m apart ([`polyline`] merges closer
/// ones) and a leg shorter than this is never one a tracer flies.
pub const LEG_EPS_M: f32 = 1.0e-4;
/// Every length the corner draws or frames around the round, [`LEAD_M`] and [`RUN_OUT_M`]
/// included, is at most this many calibres ([`Plan::reach`]), so a small round is framed as tightly
/// as its size asks: 0.16 m for a 7.9 mm round, while an 88 mm round's 1.76 m leaves every metre
/// figure in force (DERIVED from the TUNED 20).
pub const REACH_CALIBRES: f32 = 20.0;
/// The line's brightness never falls below this: a diagram rule for legibility, not physics.
/// TUNED.
pub const LINE_FLOOR: f32 = 0.35;

/// The record, prepared for drawing.
pub struct Plan {
    pub kind: CornerKind,
    /// Calibre (m): the shell's diameter and the scale of the trail, the rings and the framing.
    pub calibre: f32,
    pub line: Polyline,
    /// The drawn span of the line, arc length (m).
    pub start_arc: f32,
    pub contact_arc: f32,
    pub end_arc: f32,
    /// Where the record has the round leave the tank ([`RoundEnd::Left`]), and its arc length;
    /// `None` when it rested or burst.
    pub left_at: Option<(Vec3, f32)>,
    pub crossings: Vec<Crossing>,
    /// Outward normal and incidence (rad) at first contact.
    pub impact_point: Vec3,
    pub impact_normal: Vec3,
    pub incidence: f32,
    pub bounce: Option<Bounce>,
    /// Where the round came to rest, inside armour or a component, if the record has it rest.
    pub stop: Option<Vec3>,
    /// Every field the record threw, in [`PenetrationReport::fields`]' order: its spall cones,
    /// then its burst's.
    pub cones: Vec<Cone>,
    pub boxes: Vec<BoxPlan>,
    /// The whole struck tank's bounds (low, high corners), posed as the record has it
    /// ([`TankShapes::bounds`]); `None` without the tank asset's shapes.
    pub tank_bounds: Option<(Vec3, Vec3)>,
}

/// The path as points with cumulative arc length, the record's time at each point and the
/// remaining-capability fraction of the segment that ends at each point.
pub struct Polyline {
    pub points: Vec<Vec3>,
    pub arc: Vec<f32>,
    /// The record's time (s) at each point ([`crate::damage_cam::PathPoint::t`]), never decreasing
    /// along the path, as the record's self-consistency check holds every record to.
    pub times: Vec<f32>,
    /// `(v/v₀)²` of the segment ending at point `i` (DERIVED from the path's times), `1` at `0`.
    pub energy: Vec<f32>,
}

impl Polyline {
    pub fn length(&self) -> f32 {
        self.arc.last().copied().unwrap_or(0.0)
    }

    /// The point at arc length `s`: the line's first point before it, and past its end run on along
    /// its last leg, every recorded point left where it is.
    pub fn at(&self, s: f32) -> Vec3 {
        let Some(first) = self.points.first() else {
            return Vec3::ZERO;
        };
        if s <= 0.0 {
            return *first;
        }
        for i in 1..self.points.len() {
            if s <= self.arc[i] {
                let span = self.arc[i] - self.arc[i - 1];
                let f = if span > 0.0 {
                    (s - self.arc[i - 1]) / span
                } else {
                    1.0
                };
                return self.points[i - 1].lerp(self.points[i], f);
            }
        }
        let last = self.points.len() - 1;
        if last == 0 {
            return *first;
        }
        self.points[last] + self.leg_direction(last) * (s - self.length())
    }

    /// The direction of travel at arc length `s`.
    pub fn direction_at(&self, s: f32) -> Vec3 {
        for i in 1..self.points.len() {
            if s <= self.arc[i] || i + 1 == self.points.len() {
                let d = self.points[i] - self.points[i - 1];
                if let Some(d) = d.try_normalize() {
                    return d;
                }
            }
        }
        Vec3::NEG_Z
    }

    /// The record time (s) at arc length `s`: linear in arc along each leg, the line's first time
    /// before it, and past its end run on at the last leg's speed.
    pub fn time_at(&self, s: f32) -> f32 {
        let (Some(&first), Some(&last)) = (self.times.first(), self.times.last()) else {
            return 0.0;
        };
        if s <= 0.0 {
            return first;
        }
        for i in 1..self.points.len() {
            if s <= self.arc[i] {
                let span = self.arc[i] - self.arc[i - 1];
                let f = if span > 0.0 {
                    (s - self.arc[i - 1]) / span
                } else {
                    1.0
                };
                return self.times[i - 1] + f * (self.times[i] - self.times[i - 1]);
            }
        }
        self.last_speed()
            .map_or(last, |v| last + (s - self.length()) / v)
    }

    /// The arc length the round's nose stands at at record time `t` (s): linear in arc along each
    /// leg, a leg with no time span crossed at once, the line's start before it, and past its end
    /// run on at the last leg's speed.
    pub fn arc_at_time(&self, t: f32) -> f32 {
        let Some(&last) = self.times.last() else {
            return 0.0;
        };
        for i in 1..self.points.len() {
            if t < self.times[i] {
                let dt = self.times[i] - self.times[i - 1];
                let f = if dt > 0.0 {
                    ((t - self.times[i - 1]) / dt).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return self.arc[i - 1] + f * (self.arc[i] - self.arc[i - 1]);
            }
        }
        self.last_speed()
            .map_or(self.length(), |v| self.length() + (t - last) * v)
    }

    /// The record's speed (m/s) on leg `i` (from point `i − 1` to point `i`): its length over its
    /// time span, infinite for a leg with none; `None` for no leg.
    pub fn leg_speed(&self, i: usize) -> Option<f32> {
        if i == 0 || i >= self.points.len() {
            return None;
        }
        let dt = self.times[i] - self.times[i - 1];
        let length = self.arc[i] - self.arc[i - 1];
        Some(if dt > 0.0 { length / dt } else { f32::INFINITY })
    }

    /// The last leg's speed (m/s), where it is finite and positive.
    fn last_speed(&self) -> Option<f32> {
        self.leg_speed(self.points.len().saturating_sub(1))
            .filter(|v| v.is_finite() && *v > 0.0)
    }

    /// The index of the leg the line runs on just past arc length `s`: the first point whose arc
    /// lies beyond `s`, so a node's arc names the leg leaving it. The last leg past the line's end.
    pub fn leg_after(&self, s: f32) -> usize {
        (1..self.points.len())
            .find(|&i| self.arc[i] > s + LEG_EPS_M)
            .unwrap_or(self.points.len().saturating_sub(1))
    }

    /// The unit direction of leg `i` (from point `i − 1` to point `i`).
    pub fn leg_direction(&self, i: usize) -> Vec3 {
        match (self.points.get(i.wrapping_sub(1)), self.points.get(i)) {
            (Some(a), Some(b)) => (*b - *a).try_normalize().unwrap_or(Vec3::NEG_Z),
            _ => Vec3::NEG_Z,
        }
    }

    /// The index of the line's point nearest `p`; 0 on a line with no point.
    pub fn nearest_node(&self, p: Vec3) -> usize {
        (0..self.points.len())
            .min_by(|&a, &b| {
                let d = |i: usize| self.points[i].distance_squared(p);
                d(a).total_cmp(&d(b))
            })
            .unwrap_or(0)
    }

    /// The arc length of the point on the line nearest `p`.
    pub fn arc_of(&self, p: Vec3) -> f32 {
        let mut best = (f32::INFINITY, 0.0);
        for i in 1..self.points.len() {
            let (a, b) = (self.points[i - 1], self.points[i]);
            let span = b - a;
            let len2 = span.length_squared();
            let f = if len2 > 0.0 {
                ((p - a).dot(span) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let d = p.distance(a + span * f);
            if d < best.0 {
                best = (d, self.arc[i - 1] + f * (self.arc[i] - self.arc[i - 1]));
            }
        }
        best.1
    }
}

/// One armour crossing.
pub struct Crossing {
    pub entry: Vec3,
    pub exit: Option<Vec3>,
    pub external: bool,
    pub entry_arc: f32,
}

pub struct Bounce {
    pub point: Vec3,
    pub normal: Vec3,
    pub incoming: Vec3,
    pub outgoing: Vec3,
    pub arc: f32,
}

/// Where and when a field is thrown, and how fast its front runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thrown {
    /// The line's arc length at its origin.
    pub arc: f32,
    /// The record time (s) it was thrown ([`SpallCone::t`]).
    pub t: f32,
    /// Its front's speed (m/s of record): [`Cone::speed`].
    pub speed: f32,
}

/// One field as the record marched it: a spall cone, or a burst's field
/// ([`PenetrationReport::fields`]).
pub struct Cone {
    pub origin: Vec3,
    pub axis: Vec3,
    /// Half-angle (rad): the support's rim, π for a field over the whole sphere.
    pub half_angle: f32,
    /// The strength `S` (ref-mm): a direction was thrown with `S` times its weight; zero on a
    /// hand-authored clip.
    pub strength: f32,
    /// Where the line throws it, arc length.
    pub arc: f32,
    /// The record time (s) it was thrown.
    pub t: f32,
    /// Its front's speed (m/s of record), the same in every direction: a spall cone's is the
    /// round's exit speed off the plate that threw it (the record's speed on the path leg leaving
    /// the node where that crossing ends, [`Plan::new`]), a burst's field's the burst's sideways
    /// speed ([`crate::damage_cam::Burst`]). DERIVED stand-ins: the record holds no fragment speed.
    /// A front with no finite positive speed (a leg with no time span) is whole once thrown.
    pub speed: f32,
    pub rays: Vec<Ray>,
    /// The directions' reach and strength as one function of direction, baked from `rays`.
    pub disc: Disc,
}

impl Cone {
    /// The cone of these parts, its support from `inner` to `half_angle` (rad) off its axis,
    /// `thrown` as given, its [`Disc`] baked from `rays`.
    pub fn new(
        origin: Vec3,
        axis: Vec3,
        half_angle: f32,
        inner: f32,
        strength: f32,
        thrown: Thrown,
        rays: Vec<Ray>,
    ) -> Self {
        let disc = Disc::of(axis, half_angle, inner, strength, &rays);
        Self {
            origin,
            axis,
            half_angle,
            strength,
            arc: thrown.arc,
            t: thrown.t,
            speed: thrown.speed,
            rays,
            disc,
        }
    }

    /// Record seconds its front takes to run `distance` (m) from its origin; none for a front with
    /// no finite positive speed.
    pub fn run_time(&self, distance: f32) -> f32 {
        if self.speed.is_finite() && self.speed > 0.0 {
            distance / self.speed
        } else {
            0.0
        }
    }

    /// How far (m) its front has run `dt` record seconds after it was thrown: negative before, and
    /// for a front with no finite positive speed its longest reach from then on.
    pub fn front(&self, dt: f32) -> f32 {
        if self.speed.is_finite() && self.speed > 0.0 {
            self.speed * dt
        } else if dt >= 0.0 {
            self.max_reach()
        } else {
            dt
        }
    }

    /// Its longest direction's reach (m); 0 with no directions.
    pub fn max_reach(&self) -> f32 {
        self.rays.iter().map(|ray| ray.reach).fold(0.0, f32::max)
    }
}

/// One direction of a cone: unit direction, reach (m) and its weight `f_k`, the multiple of the
/// cone's strength it was thrown with (about 0.84 on the axis of a 26° ball, DERIVED).
pub struct Ray {
    pub direction: Vec3,
    pub reach: f32,
    pub weight: f32,
}

/// One component of the struck tank, by its name in the tank asset.
pub struct BoxPlan {
    pub name: String,
    pub role: Role,
    /// The centre of its real shape's bounds, for framing; `None` when the tank asset has no
    /// component of that name.
    pub center: Option<Vec3>,
    /// The low and high heights (m, the record frame's up, tank-up) of its real shape; `None` with
    /// `center`.
    pub heights: Option<(f32, f32)>,
    /// Its HP pool (HP points): the points that disable it whole.
    pub count: u32,
    /// Its HP points at the shot.
    pub hp: u32,
    /// The line's and the fields' dose to it (ref-mm).
    pub line: f32,
    pub field: f32,
    /// The HP points the shot took from it.
    pub lost_hp: u32,
    /// When the picture reaches it: the earliest, in record time, of the line entering it where the
    /// record has it enter and each dosing field's front arriving ([`Reach::Cone`]), the line first
    /// on a tie; a synthetic clip that names neither reaches it at the contact.
    pub reached_at: Option<Reach>,
    /// Lost before the shot (no hit points left at it), or lost or cooked off by it.
    pub lost: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reach {
    Line(f32),
    /// Cone `cone`, by the time its front has run `distance` (m) from its origin: the reach of the
    /// shortest of its directions that dosed the component. The record holds no point where a
    /// direction met the component, only where it ended, so this is the latest the front can have
    /// reached it.
    Cone {
        cone: usize,
        distance: f32,
    },
}

impl BoxPlan {
    pub fn dose(&self) -> f32 {
        self.line + self.field
    }

    /// The shot took HP from it: what the shooter's views draw on the state scale.
    pub fn struck(&self) -> bool {
        self.lost_hp > 0
    }

    /// The share of its whole pool gone once the shot has dosed it, 1 when lost; the victim's own
    /// view of its tank, prior damage included.
    pub fn pool_lost(&self) -> f32 {
        if self.lost {
            return 1.0;
        }
        if self.count == 0 {
            return 0.0;
        }
        let left = self.hp.saturating_sub(self.lost_hp);
        (1.0 - left as f32 / self.count as f32).clamp(0.0, 1.0)
    }
}

impl Plan {
    /// The plan of `report`, its components' shapes resolved in `shapes`.
    pub fn new(report: &PenetrationReport, shapes: &TankShapes) -> Self {
        let verdict = report.impact.verdict;
        let line = polyline(report);
        let contact = Vec3::from(report.impact.point);
        let contact_arc = line.arc_of(contact);
        let calibre = report.projectile.caliber_mm * 1.0e-3;
        let reach = |metres: f32| metres.min(REACH_CALIBRES * calibre);
        let left_at = match &report.end {
            RoundEnd::Rest { .. } | RoundEnd::Burst { .. } => None,
            RoundEnd::Left { at } => {
                let at = Vec3::from(*at);
                Some((at, line.arc_of(at)))
            }
        };
        let end_arc = match left_at {
            None => line.length(),
            Some((_, arc)) => (arc + reach(RUN_OUT_M)).min(line.length()),
        };
        let crossings = report
            .plates
            .iter()
            .map(|plate| Crossing {
                entry: Vec3::from(plate.entry),
                exit: plate.exit.map(Vec3::from),
                external: plate.class == PlateClass::External,
                entry_arc: line.arc_of(Vec3::from(plate.entry)),
            })
            .collect();
        let bounce = report.ricochets.first().map(|bounce| Bounce {
            point: Vec3::from(bounce.point),
            normal: Vec3::from(bounce.normal).normalize_or(Vec3::Y),
            incoming: Vec3::from(bounce.incoming).normalize_or(Vec3::NEG_Z),
            outgoing: Vec3::from(bounce.outgoing).normalize_or(Vec3::NEG_Z),
            arc: line.arc_of(Vec3::from(bounce.point)),
        });
        // A spall cone is thrown where its crossing ends, a node of the path, and its front runs at
        // the speed of the leg leaving that node. Its origin, the mean of the exits the round's
        // disc reached, stands off the node across the path and may project inside the crossing
        // (0.7 mm short of it in s4, MEASURED), so the node is the one nearest the origin, never
        // the leg its projection falls on. Invariant: an origin lies nearer its own node than any
        // other, so within half the crossing's chord of it and half the leg leaving it.
        let spall = report.spall.iter().map(|c| {
            let node = line.nearest_node(Vec3::from(c.origin));
            let exit = line.leg_speed(node + 1).unwrap_or(0.0);
            cone(c, &line, exit)
        });
        let burst = report
            .burst
            .iter()
            .filter_map(|b| b.field.as_ref().map(|c| cone(c, &line, b.sideways_mps)));
        let cones: Vec<Cone> = spall.chain(burst).collect();
        let boxes = report
            .components
            .iter()
            .map(|component| {
                let dose = report.doses.iter().find(|d| d.volume == component.name);
                let (line_dose, field_dose, lost_hp) =
                    dose.map_or((0.0, 0.0, 0), |d| (d.line, d.field, d.lost));
                let shape = shapes.get(&component.name);
                let center = shape.map(|shape| shape.center());
                let heights = shape.map(|shape| {
                    let (lo, hi) = shape.bounds();
                    (lo.y, hi.y)
                });
                // Each way the picture reaches it, at its record time: the line's entry, and each
                // dosing field's front at that field's own speed.
                let by_line = (line_dose > 0.0)
                    .then(|| dose.and_then(|d| d.entered))
                    .flatten()
                    .map(|entered| {
                        let arc = line.arc_of(Vec3::from(entered));
                        (line.time_at(arc), Reach::Line(arc))
                    });
                let fields = report.fields().zip(&cones).enumerate();
                let by_cone = fields.filter_map(|(index, (field, cone))| {
                    let distance = dosing_reach(field, &component.name)?;
                    let reach = Reach::Cone {
                        cone: index,
                        distance,
                    };
                    Some((cone.t + cone.run_time(distance), reach))
                });
                let reached_at = (line_dose + field_dose > 0.0).then(|| {
                    by_line
                        .into_iter()
                        .chain(by_cone)
                        .reduce(|first, next| if next.0 < first.0 { next } else { first })
                        .map_or(Reach::Line(contact_arc), |(_, reach)| reach)
                });
                let lost_before = component.count > 0 && component.hp == 0;
                BoxPlan {
                    name: component.name.clone(),
                    role: component.role,
                    center,
                    heights,
                    count: component.count,
                    hp: component.hp,
                    line: line_dose,
                    field: field_dose,
                    lost_hp,
                    reached_at,
                    lost: lost_before
                        || report.consequences.iter().any(|c| {
                            c.volume == component.name
                                && matches!(
                                    c.state,
                                    ConsequenceState::Lost | ConsequenceState::Cookoff
                                )
                        }),
                }
            })
            .collect();
        let normal = Vec3::from(report.impact.normal).normalize_or(Vec3::NEG_Z);
        Self {
            kind: CornerKind::of(verdict),
            calibre,
            start_arc: (contact_arc - reach(LEAD_M)).max(0.0),
            contact_arc,
            end_arc,
            left_at,
            crossings,
            impact_point: contact,
            impact_normal: normal,
            incidence: report.impact.incidence_deg.to_radians(),
            bounce,
            stop: report.rested().then(|| line.at(line.length())),
            cones,
            boxes,
            tank_bounds: shapes.bounds(),
            line,
        }
    }

    /// The victim glass with no record: `boxes` over the tank `shapes` poses, and no round — no
    /// line, crossing, bounce or cone.
    pub fn bare(shapes: &TankShapes, boxes: Vec<BoxPlan>) -> Self {
        Self {
            kind: CornerKind::Victim,
            calibre: 0.0,
            line: Polyline {
                points: Vec::new(),
                arc: Vec::new(),
                times: Vec::new(),
                energy: Vec::new(),
            },
            start_arc: 0.0,
            contact_arc: 0.0,
            end_arc: 0.0,
            left_at: None,
            crossings: Vec::new(),
            impact_point: Vec3::ZERO,
            impact_normal: Vec3::NEG_Z,
            incidence: 0.0,
            bounce: None,
            stop: None,
            cones: Vec::new(),
            boxes,
            tank_bounds: shapes.bounds(),
        }
    }

    /// Whether the record has the round leave the tank.
    pub fn left(&self) -> bool {
        self.left_at.is_some()
    }

    /// `metres`, or [`REACH_CALIBRES`] calibres if that is shorter.
    pub fn reach(&self, metres: f32) -> f32 {
        metres.min(REACH_CALIBRES * self.calibre)
    }

    /// The line's brightness at arc length `s`: remaining capability over initial, floored.
    pub fn brightness_at(&self, s: f32) -> f32 {
        let line = &self.line;
        for i in 1..line.points.len() {
            if s <= line.arc[i] {
                return line.energy[i].max(LINE_FLOOR);
            }
        }
        line.energy.last().copied().unwrap_or(1.0).max(LINE_FLOOR)
    }

    /// The points the corner's camera frames: the drawn line's span and every crossing, the
    /// bounce and each dosed box.
    pub fn framed_points(&self) -> Vec<Vec3> {
        let mut points = vec![
            self.line.at(self.start_arc.max(self.contact_arc - 0.6)),
            self.line.at(self.end_arc),
            self.impact_point,
        ];
        for crossing in &self.crossings {
            points.push(crossing.entry);
            points.extend(crossing.exit);
        }
        points.extend(self.bounce.as_ref().map(|b| b.point));
        points.extend(
            self.boxes
                .iter()
                .filter(|b| b.dose() > 0.0)
                .filter_map(|b| b.center),
        );
        points
    }

    /// The direction the round travels inside the stopping (or last) crossing.
    pub fn chord_direction(&self) -> Vec3 {
        let at = self
            .crossings
            .last()
            .map_or(self.contact_arc, |c| c.entry_arc);
        self.line.direction_at(at + 1e-3)
    }
}

fn polyline(report: &PenetrationReport) -> Polyline {
    let mut points: Vec<Vec3> = Vec::with_capacity(report.path.len());
    let mut times: Vec<f32> = Vec::with_capacity(report.path.len());
    for node in &report.path {
        let p = Vec3::from(node.point);
        if points.last().is_some_and(|last| last.distance(p) < 1e-6) {
            continue;
        }
        points.push(p);
        times.push(node.t);
    }
    let mut arc = vec![0.0];
    let mut speeds = vec![0.0];
    for i in 1..points.len() {
        let length = points[i].distance(points[i - 1]);
        arc.push(arc[i - 1] + length);
        let dt = times[i] - times[i - 1];
        speeds.push(if dt > 0.0 { length / dt } else { 0.0 });
    }
    let initial = speeds.get(1).copied().filter(|v| *v > 0.0).unwrap_or(1.0);
    let mut energy: Vec<f32> = speeds
        .iter()
        .map(|v| ((v / initial).powi(2)).clamp(0.0, 1.0))
        .collect();
    if let Some(first) = energy.first_mut() {
        *first = 1.0;
    }
    Polyline {
        points,
        arc,
        times,
        energy,
    }
}

/// How far (m) from `field`'s origin its shortest direction that dosed component `name` ended: the
/// latest its front can have reached it, the record holding no point where a direction met it.
/// `None` when none of its directions dosed it.
fn dosing_reach(field: &SpallCone, name: &str) -> Option<f32> {
    let origin = Vec3::from(field.origin);
    field
        .fragments
        .iter()
        .filter(|fragment| {
            fragment
                .hits
                .iter()
                .any(|hit| hit.volume == name && hit.damage > 0.0)
        })
        .map(|fragment| Vec3::from(fragment.end).distance(origin))
        .reduce(f32::min)
}

/// The plan's field of `cone`, thrown on `line`, its front running at `speed` (m/s of record).
fn cone(cone: &SpallCone, line: &Polyline, speed: f32) -> Cone {
    let origin = Vec3::from(cone.origin);
    let axis = Vec3::from(cone.axis).normalize_or(Vec3::NEG_Z);
    let half_angle = cone.field.half_angle();
    let rays = cone
        .fragments
        .iter()
        .filter_map(|fragment| {
            let span = Vec3::from(fragment.end) - origin;
            let direction = span.try_normalize()?;
            let weight = if cone.strength > 0.0 {
                (fragment.strength / cone.strength).max(0.0)
            } else {
                cone.field.weight_at(f64::from(direction.dot(axis))) as f32
            };
            Some(Ray {
                direction,
                reach: span.length(),
                weight,
            })
        })
        .collect();
    Cone::new(
        origin,
        axis,
        half_angle,
        cone.field.inner_angle(),
        cone.strength,
        Thrown {
            arc: line.arc_of(origin),
            t: cone.t,
            speed,
        },
        rays,
    )
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::damage_cam::PenetrationReport;

    pub(crate) fn shapes(report: &PenetrationReport) -> TankShapes {
        let vehicle = report
            .vehicle
            .as_deref()
            .expect("a recorded scenario names its vehicle");
        crate::damage_cam::shapes::TankAsset::load(vehicle)
            .unwrap()
            .shapes(&report.servo_deg)
    }

    fn record(name: &str) -> PenetrationReport {
        let path = crate::assets::asset_root()
            .join("dev/pen_cam")
            .join(format!("{name}.ron"));
        PenetrationReport::load(&path).unwrap()
    }

    /// Every recorded scenario in `assets/dev/pen_cam/`, by name, name-sorted.
    pub(in super::super) fn records() -> Vec<(String, PenetrationReport)> {
        let dir = crate::assets::asset_root().join("dev/pen_cam");
        let mut out: Vec<(String, PenetrationReport)> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let stem = path.file_stem()?.to_str()?.to_owned();
                (path.extension()? == "ron" && stem != "scenarios")
                    .then(|| (stem.clone(), record(&stem)))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// For every recorded scenario: the drawn span starts before contact and ends at or after
    /// it, the energy never rises along the path, the plan holds every field the record threw,
    /// each with a ray for every fragment that left its origin and none for one that did not, a
    /// box is reached exactly when it took a dose, and a round rested in a plate stops, where the
    /// drawn line ends. The look, its volumes and the kill cam draw every field from the plan, so
    /// a field the plan drops (a burst's, say) is one the corner never draws.
    #[test]
    fn every_record_plans_a_drawable_picture() {
        let all = records();
        let asset =
            crate::damage_cam::shapes::TankAsset::load(all[0].1.vehicle.as_deref().unwrap())
                .unwrap();
        for (name, report) in all {
            let plan = Plan::new(&report, &asset.shapes(&report.servo_deg));
            assert!(plan.start_arc <= plan.contact_arc, "{name}");
            assert!(plan.contact_arc <= plan.end_arc + 1e-4, "{name}");
            assert!(plan.end_arc <= plan.line.length() + 1e-4, "{name}");
            for pair in plan.line.energy.windows(2).skip(1) {
                assert!(pair[1] <= pair[0] + 1e-3, "{name}: energy rises");
            }
            assert_eq!(plan.cones.len(), report.fields().count(), "{name}");
            for (cone, field) in plan.cones.iter().zip(report.fields()) {
                assert_eq!(cone.origin, Vec3::from(field.origin), "{name}");
                let left = field.fragments.iter().filter(|fragment| {
                    (Vec3::from(fragment.end) - cone.origin)
                        .try_normalize()
                        .is_some()
                });
                assert_eq!(cone.rays.len(), left.count(), "{name}");
                assert!(cone.rays.iter().all(|ray| ray.reach > 0.0), "{name}");
            }
            for b in &plan.boxes {
                assert_eq!(b.dose() > 0.0, b.reached_at.is_some(), "{name}: {}", b.name);
            }
            if report.plates.last().is_some_and(|p| p.exit.is_none()) {
                assert!(plan.stop.is_some(), "{name}: rested in a plate");
            }
            for b in &plan.boxes {
                assert!(b.center.is_some(), "{name}: {} has a shape", b.name);
            }
            if let Some(stop) = plan.stop {
                assert_eq!(Some(&stop), plan.line.points.last(), "{name}");
                assert_eq!(plan.end_arc, plan.line.length(), "{name}");
            }
        }
    }

    /// When the picture reaches a component never reads the asset's geometry: every record restated
    /// with its places stripped (a synthetic clip naming no entry and no fragment) plans the same
    /// reach with the asset's shapes as with none.
    #[test]
    fn a_components_reach_never_reads_the_assets_geometry() {
        let all = records();
        let asset =
            crate::damage_cam::shapes::TankAsset::load(all[0].1.vehicle.as_deref().unwrap())
                .unwrap();
        let mut stripped_any = false;
        for (name, mut report) in all {
            report.synthetic = true;
            for dose in &mut report.doses {
                stripped_any |= dose.line > 0.0 && dose.entered.is_some();
                dose.entered = None;
            }
            let burst = report.burst.iter_mut().filter_map(|b| b.field.as_mut());
            for fragment in report
                .spall
                .iter_mut()
                .chain(burst)
                .flat_map(|c| &mut c.fragments)
            {
                fragment.hits.clear();
            }
            let with = Plan::new(&report, &asset.shapes(&report.servo_deg));
            let without = Plan::new(&report, &TankShapes::default());
            for (a, b) in with.boxes.iter().zip(&without.boxes) {
                assert_eq!(a.reached_at, b.reached_at, "{name}: {}", a.name);
            }
        }
        assert!(stripped_any, "a record's line entered a component");
    }

    /// Every recorded outcome is placed by the record itself, and a line dose stripped of its entry
    /// is reported.
    #[test]
    fn every_recorded_outcome_is_placed() {
        let mut stripped = 0;
        for (name, report) in records() {
            assert_eq!(report.unresolved(), Vec::<String>::new(), "{name}");
            let mut broken = report.clone();
            for dose in &mut broken.doses {
                if dose.line > 0.0 {
                    dose.entered = None;
                    stripped += 1;
                }
            }
            if broken != report {
                assert!(!broken.unresolved().is_empty(), "{name}");
            }
        }
        assert!(stripped > 0, "a record's line dosed a component");
    }

    /// A round the record has leave the tank is a round that left, however short the recorded run
    /// past the point it left: the record's own end decides, not the drawn span. Each leaving
    /// record is restated with its path cut 5 cm past the point it left (fixture data).
    #[test]
    fn a_round_that_left_left_however_short_its_run_out() {
        let mut leaving = 0;
        for (name, mut report) in records() {
            let RoundEnd::Left { at } = report.end else {
                continue;
            };
            let at = Vec3::from(at);
            let k = report
                .path
                .iter()
                .position(|p| Vec3::from(p.point).distance(at) < 1.0e-5)
                .expect("where the round left is a point of its path");
            let Some(next) = report.path.get(k + 1).copied() else {
                continue;
            };
            let leg = Vec3::from(next.point) - at;
            report.path.truncate(k + 1);
            report.path.push(crate::damage_cam::PathPoint {
                t: report.path[k].t + (next.t - report.path[k].t) * 0.05 / leg.length(),
                point: (at + leg.normalize() * 0.05).to_array(),
            });
            let plan = Plan::new(&report, &shapes(&report));
            assert!(plan.left(), "{name}");
            assert!(plan.stop.is_none(), "{name}");
            leaving += 1;
        }
        assert!(leaving > 0, "a record leaves the tank");
    }
}
