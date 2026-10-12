//! What the corner draws for one record, read off it and nothing else: the line as an
//! arc-length polyline, the crossings, the bounce, the stop, each spall cone's directions and
//! reach, and each box's dose and the moment the picture reaches it. Positions are in the record's
//! frame (hull-local, metres).
//!
//! Two things are derived here rather than read, and are labelled where they are made: the line's
//! remaining capability, `(v/v₀)²` from the path's times (the record carries no capability), and
//! the moment a box is reached (where the line first enters its box, or the cone that dosed it).

use bevy::prelude::*;

use super::timeline::CornerKind;
use crate::damage_cam::{
    ConsequenceState, PenetrationReport, PlateClass, Role, ShotVerdict, SpallCone,
};

/// The line is drawn from this far (m) before first contact.
pub const LEAD_M: f32 = 1.5;
/// A path that leaves the tank is drawn this far (m) past its last event.
pub const RUN_OUT_M: f32 = 1.5;
/// A path that ends more than this far (m) inside a box ended at rest there; a crossing's exit
/// lies on the box's face.
pub const REST_DEPTH_M: f32 = 1.0e-3;
/// Every length the corner draws or frames around the round, [`LEAD_M`] and [`RUN_OUT_M`]
/// included, is at most this many calibres ([`Plan::reach`]), so a small round is framed as tightly
/// as its size asks: 0.16 m for a 7.9 mm round, while an 88 mm round's 1.76 m leaves every metre
/// figure in force (DERIVED).
pub const REACH_CALIBRES: f32 = 20.0;
/// The line's brightness never falls below this: a diagram rule for legibility, not physics.
pub const LINE_FLOOR: f32 = 0.35;

/// The record, prepared for drawing.
pub struct Plan {
    pub verdict: ShotVerdict,
    pub kind: CornerKind,
    /// Calibre (m): the shell's diameter and the scale of the trail, the rings and the framing.
    pub calibre: f32,
    pub line: Polyline,
    /// The drawn span of the line, arc length (m).
    pub start_arc: f32,
    pub contact_arc: f32,
    pub end_arc: f32,
    /// The record's path runs on past the drawn span: the round left.
    pub left: bool,
    pub crossings: Vec<Crossing>,
    /// Outward normal and incidence (rad) at first contact.
    pub impact_point: Vec3,
    pub impact_normal: Vec3,
    pub incidence: f32,
    pub bounce: Option<Bounce>,
    /// Where the round came to rest, inside armour or a box, if it did.
    pub stop: Option<Vec3>,
    pub cones: Vec<Cone>,
    pub boxes: Vec<BoxPlan>,
}

/// The path as points with cumulative arc length and the remaining-capability fraction of the
/// segment that ends at each point.
pub struct Polyline {
    pub points: Vec<Vec3>,
    pub arc: Vec<f32>,
    /// `(v/v₀)²` of the segment ending at point `i` (DERIVED from the path's times), `1` at `0`.
    pub energy: Vec<f32>,
}

impl Polyline {
    pub fn length(&self) -> f32 {
        self.arc.last().copied().unwrap_or(0.0)
    }

    /// The point at arc length `s`, clamped to the line.
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
        *self.points.last().unwrap_or(first)
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

    /// The first arc length at which the line is inside the oriented box, if it ever is.
    pub fn enters(&self, center: Vec3, half: Vec3, rotation: Quat) -> Option<f32> {
        let to_local = rotation.inverse();
        for i in 1..self.points.len() {
            let a = to_local * (self.points[i - 1] - center);
            let b = to_local * (self.points[i] - center);
            let d = b - a;
            let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
            let mut hit = true;
            for axis in 0..3 {
                if d[axis].abs() < 1e-9 {
                    if a[axis].abs() > half[axis] {
                        hit = false;
                        break;
                    }
                    continue;
                }
                let t0 = (-half[axis] - a[axis]) / d[axis];
                let t1 = (half[axis] - a[axis]) / d[axis];
                lo = lo.max(t0.min(t1));
                hi = hi.min(t0.max(t1));
                if lo > hi {
                    hit = false;
                    break;
                }
            }
            if hit {
                return Some(self.arc[i - 1] + lo * (self.arc[i] - self.arc[i - 1]));
            }
        }
        None
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

/// One spall cone as the record marched it.
pub struct Cone {
    pub origin: Vec3,
    pub axis: Vec3,
    /// Half-angle (rad).
    pub half_angle: f32,
    /// On-axis strength `S` (ref-mm); zero on a hand-authored clip.
    pub strength: f32,
    /// Where the line throws it, arc length.
    pub arc: f32,
    pub rays: Vec<Ray>,
}

/// One direction of a cone: unit direction, reach (m) and the strength fraction `s(θ)/S` it was
/// thrown with.
pub struct Ray {
    pub direction: Vec3,
    pub reach: f32,
    pub weight: f32,
}

/// One box of the struck tank.
pub struct BoxPlan {
    pub name: String,
    pub role: Role,
    pub center: Vec3,
    pub half: Vec3,
    pub rotation: Quat,
    pub count: f32,
    pub line: f32,
    pub field: f32,
    /// When the picture reaches it: the arc where the line enters its box, else the arc of the
    /// first cone that dosed it.
    pub reached_at: Option<Reach>,
    /// Lost or cooked off by this shot.
    pub lost: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reach {
    Line(f32),
    Cone(usize),
}

impl BoxPlan {
    pub fn dose(&self) -> f32 {
        self.line + self.field
    }
}

impl Plan {
    pub fn new(report: &PenetrationReport) -> Self {
        let verdict = report.impact.verdict;
        let line = polyline(report);
        let contact = Vec3::from(report.impact.point);
        let contact_arc = line.arc_of(contact);
        let rested = rested(report, &line);
        let calibre = report.projectile.caliber_mm * 1.0e-3;
        let reach = |metres: f32| metres.min(REACH_CALIBRES * calibre);
        let n = line.points.len();
        let end_arc = if rested || n < 3 {
            line.length()
        } else {
            (line.arc[n - 2] + reach(RUN_OUT_M)).min(line.length())
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
        let cones: Vec<Cone> = report.spall.iter().map(|c| cone(c, &line)).collect();
        let boxes = report
            .internals
            .iter()
            .map(|internal| {
                let dose = report.doses.iter().find(|d| d.volume == internal.name);
                let (line_dose, field_dose) = dose.map_or((0.0, 0.0), |d| (d.line, d.field));
                let center = Vec3::from(internal.center);
                let half = Vec3::from(internal.half_extents);
                let rotation = Quat::from_array(internal.rotation).normalize();
                let by_line = (line_dose > 0.0)
                    .then(|| line.enters(center, half, rotation))
                    .flatten()
                    .map(Reach::Line);
                let by_cone = report
                    .spall
                    .iter()
                    .position(|cone| {
                        cone.fragments.iter().any(|fragment| {
                            fragment
                                .hits
                                .iter()
                                .any(|hit| hit.volume == internal.name && hit.damage > 0.0)
                        })
                    })
                    .map(Reach::Cone);
                let reached_at = if line_dose + field_dose > 0.0 {
                    by_line
                        .or(by_cone)
                        .or(Some(Reach::Line(line.arc_of(center))))
                } else {
                    None
                };
                BoxPlan {
                    name: internal.name.clone(),
                    role: internal.role,
                    center,
                    half,
                    rotation,
                    count: internal.count,
                    line: line_dose,
                    field: field_dose,
                    reached_at,
                    lost: report.consequences.iter().any(|c| {
                        c.volume == internal.name
                            && matches!(c.state, ConsequenceState::Lost | ConsequenceState::Cookoff)
                    }),
                }
            })
            .collect();
        let normal = Vec3::from(report.impact.normal).normalize_or(Vec3::NEG_Z);
        Self {
            verdict,
            kind: CornerKind::of(verdict),
            calibre,
            start_arc: (contact_arc - reach(LEAD_M)).max(0.0),
            contact_arc,
            end_arc,
            left: end_arc < line.length() - 1e-4,
            crossings,
            impact_point: contact,
            impact_normal: normal,
            incidence: report.impact.incidence_deg.to_radians(),
            bounce,
            stop: rested.then(|| line.at(line.length())),
            cones,
            boxes,
            line,
        }
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
        for b in self.boxes.iter().filter(|b| b.dose() > 0.0) {
            points.push(b.center);
        }
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

/// Whether the round came to rest: in the last plate it crossed, when that plate has no exit, or
/// in a box, when the path ends more than [`REST_DEPTH_M`] inside one. A round that flies on has a
/// path ending outside the tank, at the march's next point past its last crossing or bounce.
fn rested(report: &PenetrationReport, line: &Polyline) -> bool {
    if report
        .plates
        .last()
        .is_some_and(|plate| plate.exit.is_none())
    {
        return true;
    }
    let Some(&end) = line.points.last() else {
        return false;
    };
    report.internals.iter().any(|internal| {
        let rotation = Quat::from_array(internal.rotation).normalize();
        let local = rotation.inverse() * (end - Vec3::from(internal.center));
        (local.abs() - Vec3::from(internal.half_extents)).max_element() < -REST_DEPTH_M
    })
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
        energy,
    }
}

fn cone(cone: &SpallCone, line: &Polyline) -> Cone {
    let origin = Vec3::from(cone.origin);
    let axis = Vec3::from(cone.axis).normalize_or(Vec3::NEG_Z);
    let half_angle = cone.half_angle_deg.to_radians();
    let profile = |direction: Vec3| {
        let cos_alpha = half_angle.cos();
        ((direction.dot(axis) - cos_alpha) / (1.0 - cos_alpha)).clamp(0.0, 1.0)
    };
    let rays = cone
        .fragments
        .iter()
        .filter_map(|fragment| {
            let span = Vec3::from(fragment.end) - origin;
            let direction = span.try_normalize()?;
            let weight = if cone.strength > 0.0 {
                (fragment.strength / cone.strength).clamp(0.0, 1.0)
            } else {
                profile(direction)
            };
            Some(Ray {
                direction,
                reach: span.length(),
                weight,
            })
        })
        .collect();
    Cone {
        origin,
        axis,
        half_angle,
        strength: cone.strength,
        arc: line.arc_of(origin),
        rays,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage_cam::PenetrationReport;

    fn record(name: &str) -> PenetrationReport {
        let path = crate::assets::asset_root()
            .join("dev/pen_cam")
            .join(format!("{name}.ron"));
        PenetrationReport::load(&path).unwrap()
    }

    fn records() -> Vec<(String, PenetrationReport)> {
        let dir = crate::assets::asset_root().join("dev/pen_cam");
        let mut out: Vec<(String, PenetrationReport)> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let stem = path.file_stem()?.to_str()?.to_owned();
                (stem != "scenarios").then(|| (stem.clone(), record(&stem)))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// For every recorded scenario: the drawn span starts before contact and ends at or after
    /// it, the energy never rises along the path, every cone ray reaches somewhere, a box is
    /// reached exactly when it took a dose, and a round rested in a plate stops, where the drawn
    /// line ends.
    #[test]
    fn every_record_plans_a_drawable_picture() {
        for (name, report) in records() {
            let plan = Plan::new(&report);
            assert!(plan.start_arc <= plan.contact_arc, "{name}");
            assert!(plan.contact_arc <= plan.end_arc + 1e-4, "{name}");
            assert!(plan.end_arc <= plan.line.length() + 1e-4, "{name}");
            for pair in plan.line.energy.windows(2).skip(1) {
                assert!(pair[1] <= pair[0] + 1e-3, "{name}: energy rises");
            }
            for cone in &plan.cones {
                assert!(!cone.rays.is_empty(), "{name}");
                assert!(cone.rays.iter().all(|ray| ray.reach > 0.0), "{name}");
            }
            for b in &plan.boxes {
                assert_eq!(b.dose() > 0.0, b.reached_at.is_some(), "{name}: {}", b.name);
            }
            if report.plates.last().is_some_and(|p| p.exit.is_none()) {
                assert!(plan.stop.is_some(), "{name}: rested in a plate");
            }
            if let Some(stop) = plan.stop {
                assert_eq!(Some(&stop), plan.line.points.last(), "{name}");
                assert_eq!(plan.end_arc, plan.line.length(), "{name}");
            }
        }
    }

    /// The line's brightness steps down after a crossing and never under the floor.
    #[test]
    fn the_line_dims_after_each_crossing() {
        let plan = Plan::new(&record("s1_through_crew"));
        let before = plan.brightness_at(plan.contact_arc - 0.5);
        let after = plan.brightness_at(plan.crossings[0].entry_arc + 0.3);
        assert_eq!(before, 1.0);
        assert!(after < before && after >= LINE_FLOOR);
    }

    #[test]
    fn a_segment_through_a_box_enters_it() {
        let line = Polyline {
            points: vec![Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, 5.0)],
            arc: vec![0.0, 10.0],
            energy: vec![1.0, 1.0],
        };
        let at = line.enters(Vec3::ZERO, Vec3::splat(1.0), Quat::IDENTITY);
        assert!((at.unwrap() - 4.0).abs() < 1e-4);
        assert!(
            line.enters(Vec3::new(3.0, 0.0, 0.0), Vec3::splat(1.0), Quat::IDENTITY)
                .is_none()
        );
    }
}
