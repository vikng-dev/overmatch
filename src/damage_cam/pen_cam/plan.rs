//! What the corner draws for one record, read off it and the tank asset's component shapes: the
//! line as an arc-length polyline, the crossings, the bounce, the stop or the point the round left
//! the tank, each spall cone's directions and reach, and each component's dose and the moment the
//! picture reaches it. Positions are in the record's frame (hull-local, metres).
//!
//! Whether the round stopped, where it left the tank and where the line entered a component are the
//! record's own facts; nothing here re-derives them from geometry. A component's shape serves only
//! framing (its centre). Two things are derived rather than read, and are labelled where they are
//! made: the line's remaining capability, `(v/v₀)²` from the path's times (the record carries no
//! capability), and the moment a component is reached (the arc of the line's recorded entry, or
//! the cone that dosed it).

use bevy::prelude::*;

use super::timeline::CornerKind;
use crate::damage_cam::shapes::TankShapes;
use crate::damage_cam::{
    ConsequenceState, PenetrationReport, PlateClass, Role, RoundEnd, ShotVerdict, SpallCone,
};

/// The line is drawn from this far (m) before first contact.
pub const LEAD_M: f32 = 1.5;
/// A path that leaves the tank is drawn this far (m) past the point it left.
pub const RUN_OUT_M: f32 = 1.5;
/// Two arc lengths this close (m) name the same point of the line: a node's arc against the arc
/// [`Polyline::arc_of`] finds for that node. DERIVED: arc lengths are f32 sums of legs under 100 m,
/// so their rounding is under 1e-5 m; nodes are at least 1e-6 m apart ([`polyline`] merges closer
/// ones) and a leg shorter than this is never one a tracer flies.
pub const LEG_EPS_M: f32 = 1.0e-4;
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
    /// Where the record has the round leave the tank, and its arc length; `None` when it rested.
    pub left_at: Option<(Vec3, f32)>,
    pub crossings: Vec<Crossing>,
    /// Outward normal and incidence (rad) at first contact.
    pub impact_point: Vec3,
    pub impact_normal: Vec3,
    pub incidence: f32,
    pub bounce: Option<Bounce>,
    /// Where the round came to rest, inside armour or a component, if the record has it rest.
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

/// One component of the struck tank, by its name in the tank asset.
pub struct BoxPlan {
    pub name: String,
    pub role: Role,
    /// The centre of its real shape's bounds, for framing; `None` when the tank asset has no
    /// component of that name.
    pub center: Option<Vec3>,
    pub count: f32,
    pub line: f32,
    pub field: f32,
    /// When the picture reaches it: the arc where the record has the line enter it, else the arc of
    /// the first cone that dosed it.
    pub reached_at: Option<Reach>,
    /// Lost before the shot (no hit points left at it), or lost or cooked off by it.
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
    /// The plan of `report`, its components' shapes resolved in `shapes`.
    pub fn new(report: &PenetrationReport, shapes: &TankShapes) -> Self {
        let verdict = report.impact.verdict;
        let line = polyline(report);
        let contact = Vec3::from(report.impact.point);
        let contact_arc = line.arc_of(contact);
        let calibre = report.projectile.caliber_mm * 1.0e-3;
        let reach = |metres: f32| metres.min(REACH_CALIBRES * calibre);
        let left_at = match &report.end {
            RoundEnd::Rest { .. } => None,
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
        let cones: Vec<Cone> = report.spall.iter().map(|c| cone(c, &line)).collect();
        let boxes = report
            .components
            .iter()
            .map(|component| {
                let dose = report.doses.iter().find(|d| d.volume == component.name);
                let (line_dose, field_dose) = dose.map_or((0.0, 0.0), |d| (d.line, d.field));
                let center = shapes.get(&component.name).map(|shape| shape.center());
                let by_line = (line_dose > 0.0)
                    .then(|| dose.and_then(|d| d.entered))
                    .flatten()
                    .map(|entered| Reach::Line(line.arc_of(Vec3::from(entered))));
                let by_cone = report
                    .spall
                    .iter()
                    .position(|cone| {
                        cone.fragments.iter().any(|fragment| {
                            fragment
                                .hits
                                .iter()
                                .any(|hit| hit.volume == component.name && hit.damage > 0.0)
                        })
                    })
                    .map(Reach::Cone);
                let reached_at = if line_dose + field_dose > 0.0 {
                    by_line.or(by_cone).or(Some(Reach::Line(
                        center.map_or(contact_arc, |c| line.arc_of(c)),
                    )))
                } else {
                    None
                };
                let lost_before = component.count > 0.0 && component.hp <= 0.0;
                BoxPlan {
                    name: component.name.clone(),
                    role: component.role,
                    center,
                    count: component.count,
                    line: line_dose,
                    field: field_dose,
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
            verdict,
            kind: CornerKind::of(verdict),
            calibre,
            start_arc: (contact_arc - reach(LEAD_M)).max(0.0),
            contact_arc,
            end_arc,
            left: end_arc < line.length() - 1e-4,
            left_at,
            crossings,
            impact_point: contact,
            impact_normal: normal,
            incidence: report.impact.incidence_deg.to_radians(),
            bounce,
            stop: report.rested().then(|| line.at(line.length())),
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

    pub(crate) fn shapes(report: &PenetrationReport) -> TankShapes {
        let vehicle = report
            .vehicle
            .as_deref()
            .expect("a recorded scenario names its vehicle");
        crate::damage_cam::shapes::TankAsset::load(vehicle)
            .unwrap()
            .shapes(report.turret_yaw_deg)
    }

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
        let all = records();
        let asset =
            crate::damage_cam::shapes::TankAsset::load(all[0].1.vehicle.as_deref().unwrap())
                .unwrap();
        for (name, report) in all {
            let plan = Plan::new(&report, &asset.shapes(report.turret_yaw_deg));
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
            for b in &plan.boxes {
                assert!(b.center.is_some(), "{name}: {} has a shape", b.name);
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
        let report = record("s1_through_crew");
        let plan = Plan::new(&report, &shapes(&report));
        let before = plan.brightness_at(plan.contact_arc - 0.5);
        let after = plan.brightness_at(plan.crossings[0].entry_arc + 0.3);
        assert_eq!(before, 1.0);
        assert!(after < before && after >= LINE_FLOOR);
    }
}
