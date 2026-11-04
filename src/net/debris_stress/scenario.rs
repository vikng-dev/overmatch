//! Required construction for the game's operator debris scenario, admitted before play.

use avian3d::prelude::*;
use bevy::prelude::*;
use serde::Deserialize;

use crate::provenance::Sourced;

pub(super) const FILE: &str = "assets/dev/debris-stress.ron";
pub(super) const RON: &str = include_str!("../../../assets/dev/debris-stress.ron");

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Parameters {
    pub fragment_hull_m: Sourced<Vec<[f32; 3]>>,
    pub density_kg_m3: Sourced<f32>,
    pub lattice_m: Sourced<f32>,
    pub rows: Sourced<u32>,
    pub columns: Sourced<u32>,
    pub ground_clearance_m: Sourced<f32>,
    pub tilt_rad: Sourced<f32>,
    pub inward_speed_m_s: Sourced<[f32; 2]>,
    pub upward_speed_m_s: Sourced<f32>,
    pub along_speed_m_s: Sourced<f32>,
    pub spin_rad_s: Sourced<f32>,
    pub albedo_srgb: Sourced<[f32; 3]>,
    pub roughness: Sourced<f32>,
}

#[derive(Debug)]
pub(super) struct Scenario {
    pub parameters: Parameters,
    pub collider: Collider,
    pub half_height_m: f32,
}

#[derive(Debug)]
pub(in crate::net) enum ScenarioError {
    Syntax {
        field: String,
        cause: Box<ron::error::SpannedError>,
    },
    Invalid {
        field: &'static str,
        fix: &'static str,
    },
}

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax { field, cause } => write!(
                f,
                "{FILE}: {field}: {cause}; author every required field as (value, job(\"source or reason\")) and fix the indicated RON value"
            ),
            Self::Invalid { field, fix } => write!(f, "{FILE}: {field}: {fix}"),
        }
    }
}

impl std::error::Error for ScenarioError {}

impl Scenario {
    pub fn shipped() -> Self {
        Self::parse(RON).unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn parse(text: &str) -> Result<Self, ScenarioError> {
        crate::provenance::with_shipped_policy(|| {
            let mut reader =
                ron::de::Deserializer::from_str(text).map_err(|cause| ScenarioError::Syntax {
                    field: "scenario".into(),
                    cause: Box::new(cause),
                })?;
            let parameters = serde_path_to_error::deserialize(&mut reader).map_err(|error| {
                ScenarioError::Syntax {
                    field: error.path().to_string(),
                    cause: Box::new(reader.span_error(error.into_inner())),
                }
            })?;
            reader.end().map_err(|cause| ScenarioError::Syntax {
                field: "scenario".into(),
                cause: Box::new(reader.span_error(cause)),
            })?;
            Self::admit(parameters)
        })
    }

    fn admit(parameters: Parameters) -> Result<Self, ScenarioError> {
        let invalid = |field, fix| ScenarioError::Invalid { field, fix };
        parameters.validate()?;
        let vertices: Vec<_> = parameters
            .fragment_hull_m
            .0
            .iter()
            .copied()
            .map(Vec3::from)
            .collect();
        if vertices.len() < 4 || vertices.iter().any(|point| !point.is_finite()) {
            return Err(invalid(
                "fragment_hull_m",
                "author at least four finite, noncoplanar vertices",
            ));
        }
        let half_height_m = vertices
            .iter()
            .map(|point| point.y.abs())
            .fold(0.0, f32::max);
        let radius = vertices
            .iter()
            .map(|point| point.xz().length())
            .fold(0.0, f32::max);
        // DERIVED conservative support bounds under any yaw and the admitted tilt interval.
        let lean = parameters.tilt_rad.0.sin();
        let horizontal = radius + half_height_m * lean;
        let vertical = half_height_m + radius * lean;
        if !horizontal.is_finite()
            || !vertical.is_finite()
            || parameters.lattice_m.0 <= 2.0 * horizontal.max(vertical)
        {
            return Err(invalid(
                "lattice_m / fragment_hull_m / tilt_rad",
                "space lattice centres beyond twice the tilted hull's horizontal and vertical support bounds",
            ));
        }
        if parameters.ground_clearance_m.0 < radius * lean {
            return Err(invalid(
                "ground_clearance_m",
                "raise clearance to cover the hull's tilt below its unrotated bottom",
            ));
        }
        let collider = Collider::convex_hull(vertices)
            .ok_or_else(|| invalid("fragment_hull_m", "author a nondegenerate convex volume"))?;
        let hull = collider
            .shape()
            .as_convex_polyhedron()
            .expect("convex-hull admission produces a convex polyhedron");
        let (points, triangles) = hull.to_trimesh();
        if triangles.iter().any(|&[a, b, c]| {
            (points[b as usize] - points[a as usize])
                .cross(points[c as usize] - points[a as usize])
                .try_normalize()
                .is_none()
        }) {
            return Err(invalid(
                "fragment_hull_m",
                "author hull faces with finite, representable nonzero areas",
            ));
        }
        let mass = collider.mass_properties(parameters.density_kg_m3.0);
        if !mass.mass.is_finite()
            || mass.mass <= 0.0
            || !mass.mass.recip().is_finite()
            || !mass.principal_angular_inertia.is_finite()
            || mass.principal_angular_inertia.min_element() <= 0.0
            || !mass.principal_angular_inertia.recip().is_finite()
            || !mass.center_of_mass.is_finite()
            || !mass.local_inertial_frame.is_finite()
            || !mass.local_inertial_frame.is_normalized()
        {
            return Err(invalid(
                "fragment_hull_m / density_kg_m3",
                "author a hull and density giving finite positive solver mass and inertia",
            ));
        }
        Ok(Self {
            parameters,
            collider,
            half_height_m,
        })
    }
}

impl Parameters {
    fn validate(&self) -> Result<(), ScenarioError> {
        let invalid = |field, fix| ScenarioError::Invalid { field, fix };
        for (field, value) in [
            ("density_kg_m3", self.density_kg_m3.0),
            ("lattice_m", self.lattice_m.0),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(invalid(field, "author a finite positive value"));
            }
        }
        for (field, value) in [
            ("ground_clearance_m", self.ground_clearance_m.0),
            ("tilt_rad", self.tilt_rad.0),
            ("upward_speed_m_s", self.upward_speed_m_s.0),
            ("along_speed_m_s", self.along_speed_m_s.0),
            ("spin_rad_s", self.spin_rad_s.0),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(invalid(field, "author a finite nonnegative value"));
            }
        }
        if self.rows.0 == 0 || self.columns.0 == 0 {
            return Err(invalid("rows / columns", "author positive lattice counts"));
        }
        if self.tilt_rad.0 > std::f32::consts::FRAC_PI_2 {
            return Err(invalid(
                "tilt_rad",
                "keep release tilt within a right angle",
            ));
        }
        let [min, max] = self.inward_speed_m_s.0;
        if !min.is_finite()
            || !max.is_finite()
            || min < 0.0
            || max < min
            || !(max + self.upward_speed_m_s.0 + self.along_speed_m_s.0).is_finite()
        {
            return Err(invalid(
                "inward_speed_m_s / upward_speed_m_s / along_speed_m_s",
                "author an ordered nonnegative inward range and speeds whose sum stays finite",
            ));
        }
        if !self
            .albedo_srgb
            .0
            .iter()
            .chain([&self.roughness.0])
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        {
            return Err(invalid(
                "albedo_srgb / roughness",
                "author finite values in [0, 1]",
            ));
        }
        // DERIVED: both lattice axes and the first outward offset must fit even at u32 counts.
        if f64::from(self.lattice_m.0) * (2.0 * f64::from(u32::MAX) + 1.0) > f64::from(f32::MAX) {
            return Err(invalid(
                "lattice_m",
                "reduce the pitch so all admitted piece counts have finite layout offsets",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
