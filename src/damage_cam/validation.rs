//! The game's damage-model record admission: local files and received explanations share these
//! semantic invariants before presentation reads them. Synthetic clips may omit dose placement,
//! vehicle identity and servo poses; their numeric and path contracts remain the same.

use super::{PenetrationReport, RoundEnd};
use bevy::prelude::*;

/// A parsed record that cannot be presented as the game state it claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    Number(&'static str),
    Direction(&'static str),
    Path(&'static str),
    Pool(String),
    Reference(String),
    Round(String),
    Field(&'static str),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(field) => write!(f, "{field}: supply a finite value in its stated domain"),
            Self::Direction(field) => write!(f, "{field}: supply a finite unit direction"),
            Self::Path(reason) => write!(f, "path: {reason}"),
            Self::Pool(name) => write!(
                f,
                "component {name:?}: keep held/taken within its positive HP pool"
            ),
            Self::Reference(name) => write!(
                f,
                "component {name:?}: declare the component and its dose placement"
            ),
            Self::Round(key) => write!(
                f,
                "round {key:?}: supply a registered round key; only synthetic local clips may name an unregistered round"
            ),
            Self::Field(reason) => write!(f, "field: {reason}"),
        }
    }
}

impl std::error::Error for ValidationError {}

fn number(value: f32, domain: bool, field: &'static str) -> Result<(), ValidationError> {
    if value.is_finite() && domain {
        Ok(())
    } else {
        Err(ValidationError::Number(field))
    }
}

fn point(value: [f32; 3], field: &'static str) -> Result<(), ValidationError> {
    number(value[0], value.into_iter().all(f32::is_finite), field)
}

fn direction(value: [f32; 3], field: &'static str) -> Result<(), ValidationError> {
    let vector = Vec3::from(value);
    // DERIVED slack: the wire's octahedral decode normalizes in f32; this permits its rounding,
    // while refusing a zero direction or a vector whose magnitude carries scale.
    if vector.is_finite() && (vector.length_squared() - 1.0).abs() <= 32.0 * f32::EPSILON {
        Ok(())
    } else {
        Err(ValidationError::Direction(field))
    }
}

fn ordered(times: impl Iterator<Item = f32>, field: &'static str) -> Result<(), ValidationError> {
    let mut previous = f32::NEG_INFINITY;
    for time in times {
        number(time, time >= previous, field)?;
        previous = time;
    }
    Ok(())
}

/// Admit a parsed report before any view constructs a plan from it.
pub(super) fn check(report: &PenetrationReport) -> Result<(), ValidationError> {
    if !report.synthetic
        && crate::rounds::RoundRegistry::shipped()
            .get(&report.round)
            .is_err()
    {
        return Err(ValidationError::Round(report.round.clone()));
    }
    let projectile = &report.projectile;
    number(
        projectile.caliber_mm,
        projectile.caliber_mm > 0.0,
        "projectile.caliber_mm",
    )?;
    number(
        projectile.mass_kg,
        projectile.mass_kg > 0.0,
        "projectile.mass_kg",
    )?;
    number(
        projectile.speed_mps,
        projectile.speed_mps > 0.0,
        "projectile.speed_mps",
    )?;
    for &angle in report.servo_deg.values() {
        number(angle, true, "servo_deg")?;
    }
    point(report.origin, "origin")?;
    if let Some(muzzle) = report.muzzle {
        point(muzzle, "muzzle")?;
    }
    point(report.impact.point, "impact.point")?;
    direction(report.impact.normal, "impact.normal")?;
    direction(report.impact.incoming, "impact.incoming")?;
    number(
        report.impact.incidence_deg,
        (0.0..=180.0).contains(&report.impact.incidence_deg),
        "impact.incidence_deg",
    )?;
    check_path(report)?;
    check_pools(report)?;
    for cone in report.fields() {
        number(cone.t, true, "field.t")?;
        point(cone.origin, "field.origin")?;
        direction(cone.axis, "field.axis")?;
        cone.field
            .check()
            .map_err(|_| ValidationError::Field("descriptor is outside its law's domain"))?;
        number(cone.strength, cone.strength >= 0.0, "field.strength")?;
        number(
            cone.half_angle_deg,
            (cone.half_angle_deg - cone.field.half_angle().to_degrees()).abs()
                <= 32.0 * f32::EPSILON * 180.0,
            "field.half_angle_deg",
        )?;
        for fragment in &cone.fragments {
            point(fragment.end, "fragment.end")?;
            number(
                fragment.strength,
                fragment.strength >= 0.0,
                "fragment.strength",
            )?;
            for hit in &fragment.hits {
                number(hit.damage, hit.damage >= 0.0, "fragment.dose")?;
                if report.component(&hit.volume).is_none() {
                    return Err(ValidationError::Reference(hit.volume.clone()));
                }
            }
        }
    }
    if let Some(burst) = &report.burst {
        number(burst.t, true, "burst.t")?;
        point(burst.point, "burst.point")?;
        number(burst.speed_mps, burst.speed_mps >= 0.0, "burst.speed_mps")?;
        number(
            burst.sideways_mps,
            burst.sideways_mps > 0.0,
            "burst.sideways_mps",
        )?;
        number(burst.nose_kg, burst.nose_kg > 0.0, "burst.nose_kg")?;
        if let Some(field) = &burst.field {
            super::check_burst_field(field.field).map_err(ValidationError::Field)?;
            if field.origin != burst.point || field.t != burst.t {
                return Err(ValidationError::Field(
                    "burst field must share its source's point and time",
                ));
            }
        }
    }
    if !report.synthetic
        && let Some(unresolved) = report.unresolved().into_iter().next()
    {
        return Err(ValidationError::Reference(unresolved));
    }
    Ok(())
}

fn check_path(report: &PenetrationReport) -> Result<(), ValidationError> {
    if report.path.len() < 2 {
        return Err(ValidationError::Path("supply at least one timed segment"));
    }
    if report.path[0].point != report.origin {
        return Err(ValidationError::Path("the first point must equal origin"));
    }
    ordered(report.path.iter().map(|p| p.t), "path.t")?;
    ordered(report.plates.iter().map(|p| p.t), "plates.t")?;
    ordered(report.ricochets.iter().map(|p| p.t), "ricochets.t")?;
    ordered(report.consequences.iter().map(|p| p.t), "consequences.t")?;
    for node in &report.path {
        point(node.point, "path.point")?;
    }
    for (index, plate) in report.plates.iter().enumerate() {
        point(plate.entry, "plate.entry")?;
        if let Some(exit) = plate.exit {
            point(exit, "plate.exit")?;
        } else if index + 1 != report.plates.len() {
            return Err(ValidationError::Path(
                "only the last plate can retain the round",
            ));
        }
        for (value, field) in [
            (plate.chord_mm, "plate.chord_mm"),
            (plate.cost_mm, "plate.cost_mm"),
            (plate.steel_equiv_mm, "plate.steel_equiv_mm"),
            (plate.factor, "plate.factor"),
        ] {
            number(value, value >= 0.0, field)?;
        }
    }
    for bounce in &report.ricochets {
        point(bounce.point, "ricochet.point")?;
        direction(bounce.normal, "ricochet.normal")?;
        direction(bounce.incoming, "ricochet.incoming")?;
        direction(bounce.outgoing, "ricochet.outgoing")?;
    }
    // DERIVED: two independently mm-quantized copies differ by at most sqrt(3) mm,
    // plus the f32 coordinates' rounding. Geometry checks admit that wire bound.
    let on_path = |point: [f32; 3]| {
        report.path.iter().any(|p| {
            let (a, b) = (Vec3::from(p.point), Vec3::from(point));
            a.distance(b) <= 0.002 + 8.0 * f32::EPSILON * a.abs().max(b.abs()).max_element()
        })
    };
    let first = report.path[0].t;
    let last = report.path[report.path.len() - 1].t;
    // DERIVED: separately rounded microsecond timestamps can straddle a boundary by one quantum.
    for time in report
        .plates
        .iter()
        .map(|p| p.t)
        .chain(report.ricochets.iter().map(|p| p.t))
    {
        if time < first - 2.0e-6 || time > last + 2.0e-6 {
            return Err(ValidationError::Path(
                "plate and bounce times must lie within the path",
            ));
        }
    }
    match &report.end {
        RoundEnd::Left { at } if !on_path(*at) => {
            return Err(ValidationError::Path(
                "the departure point must be a path point",
            ));
        }
        RoundEnd::Rest { volume }
            if !volume.is_empty()
                && report.component(volume).is_none()
                && !report
                    .plates
                    .iter()
                    .any(|p| p.name == *volume && p.exit.is_none()) =>
        {
            return Err(ValidationError::Path(
                "the resting volume must be a component or retaining plate",
            ));
        }
        _ => {}
    }
    if let Some(burst) = &report.burst
        && !on_path(burst.point)
    {
        return Err(ValidationError::Path(
            "the burst point must be a path point",
        ));
    }
    Ok(())
}

fn check_pools(report: &PenetrationReport) -> Result<(), ValidationError> {
    let mut names = std::collections::BTreeSet::new();
    for component in &report.components {
        let capacity = component
            .count
            .checked_mul(crate::ballistics::POOL_UNITS_PER_HP);
        if component.count == 0
            || capacity.is_none_or(|capacity| component.held > capacity)
            || !names.insert(&component.name)
        {
            return Err(ValidationError::Pool(component.name.clone()));
        }
    }
    let mut doses = std::collections::BTreeSet::new();
    for dose in &report.doses {
        let Some(component) = report.component(&dose.volume) else {
            return Err(ValidationError::Reference(dose.volume.clone()));
        };
        if dose.taken > component.held
            || dose
                .shown_taken
                .is_some_and(|shown| shown > component.shown())
            || !doses.insert(&dose.volume)
        {
            return Err(ValidationError::Pool(dose.volume.clone()));
        }
        number(dose.line, dose.line >= 0.0, "dose.line")?;
        number(dose.field, dose.field >= 0.0, "dose.field")?;
        if let Some(entry) = dose.entered {
            point(entry, "dose.entered")?;
        }
    }
    for consequence in &report.consequences {
        if report.component(&consequence.volume).is_none() {
            return Err(ValidationError::Reference(consequence.volume.clone()));
        }
    }
    Ok(())
}

/// Local record input is refused at parsing/admission, before a plan can be constructed.
#[derive(Debug)]
pub enum ReportLoadError {
    Parse(ron::error::SpannedError),
    Invalid(ValidationError),
}

impl std::fmt::Display for ReportLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(error) => error.fmt(f),
            Self::Invalid(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ReportLoadError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's damage-model local parser guards the presentation contract, even for synthetic
    /// clips. Synthetic clips may omit dose places, but cannot introduce invalid time or pool data.
    #[test]
    fn local_record_admission_refuses_invalid_semantics_and_keeps_synthetic_absences() {
        let base = crate::damage_cam::delivery::tests::report("s1_through_crew");
        let spoilers: &[fn(&mut PenetrationReport)] = &[
            |r| r.round = "<unregistered>".to_owned(),
            |r| r.projectile.mass_kg = 0.0,
            |r| r.projectile.caliber_mm = -1.0,
            |r| r.path[1].t = r.path[0].t - 1.0,
            |r| r.path[0].point[0] += 1.0,
            |r| r.components[0].held = u32::MAX,
            |r| r.doses[0].line = -1.0,
            |r| r.impact.incoming = [0.0; 3],
            |r| r.spall[0].field.ratio = -1.0,
            |r| r.end = RoundEnd::Left { at: [1000.0; 3] },
        ];
        for spoil in spoilers {
            let mut report = base.clone();
            spoil(&mut report);
            let text = ron::ser::to_string(&report).unwrap();
            assert!(matches!(
                PenetrationReport::from_ron(&text),
                Err(ReportLoadError::Invalid(_))
            ));
        }
        let mut synthetic = base;
        synthetic.synthetic = true;
        synthetic.round = "<authored synthetic round>".to_owned();
        synthetic.vehicle = None;
        synthetic.servo_deg.clear();
        for dose in &mut synthetic.doses {
            dose.entered = None;
        }
        synthetic.spall.clear();
        let text = ron::ser::to_string(&synthetic).unwrap();
        assert!(PenetrationReport::from_ron(&text).is_ok());
    }
}
