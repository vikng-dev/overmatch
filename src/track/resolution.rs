//! The track-lean experiment's process-wide pricing lever. The orchestrator's pricing script
//! and `scripts/track-resolution-scorecard` own it; remove it when the arm lands.

use super::forces::{CONTACT_RESOLUTION, ContactResolution};
use std::sync::OnceLock;

const VARIABLE: &str = "OVERMATCH_TRACK_RESOLUTION";
static SETTING: OnceLock<Result<ContactResolution, ResolutionError>> = OnceLock::new();

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResolutionError {
    NotUnicode,
    Format,
    Stations,
    Points,
}

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let repair = match self {
            Self::NotUnicode => "must contain Unicode text",
            Self::Format => {
                return write!(
                    f,
                    "{VARIABLE} must be STATIONSxPOINTS, for example 48x2 (default {}x{})",
                    CONTACT_RESOLUTION.stations, CONTACT_RESOLUTION.lateral_points
                );
            }
            Self::Stations => "needs an integer station count of at least three",
            Self::Points => "needs two or three lateral points",
        };
        write!(f, "{VARIABLE} {repair}")
    }
}
impl std::error::Error for ResolutionError {}

fn parse(raw: Option<&str>) -> Result<ContactResolution, ResolutionError> {
    let Some(raw) = raw else {
        return Ok(CONTACT_RESOLUTION);
    };
    let (stations, points) = raw.split_once('x').ok_or(ResolutionError::Format)?;
    let stations = stations
        .parse::<usize>()
        .map_err(|_| ResolutionError::Stations)?;
    if stations < 3 {
        return Err(ResolutionError::Stations);
    }
    let lateral_points = points
        .parse::<usize>()
        .map_err(|_| ResolutionError::Points)?;
    if !matches!(lateral_points, 2 | 3) {
        return Err(ResolutionError::Points);
    }
    Ok(ContactResolution {
        stations,
        lateral_points,
    })
}

/// Read once, before the server constructs its app. A caller refuses invalid outside input;
/// test scorecards call this same edge before selecting their measurement field.
pub(crate) fn configure() -> Result<ContactResolution, &'static ResolutionError> {
    SETTING
        .get_or_init(|| match std::env::var(VARIABLE) {
            Ok(raw) => parse(Some(&raw)),
            Err(std::env::VarError::NotPresent) => parse(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(ResolutionError::NotUnicode),
        })
        .as_ref()
        .copied()
}

/// Standalone clients and fixtures use the shipped resolution. Server startup publishes its
/// selection before calibration or any spawn; both consumers read the same parsed value.
pub(crate) fn selected() -> ContactResolution {
    match SETTING.get() {
        None => CONTACT_RESOLUTION,
        Some(result) => *result
            .as_ref()
            .expect("startup refuses invalid resolution before simulation"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// An operator typo must never become the default, and every admitted setting must size
    /// both a closed force loop and its supported lateral quadrature.
    #[test]
    fn pricing_resolution_admits_only_a_closed_supported_field() {
        for stations in [3, 16, 24, 48, 97] {
            for points in [2, 3] {
                let setting = parse(Some(&format!("{stations}x{points}"))).unwrap();
                assert_eq!(
                    (setting.stations, setting.lateral_points),
                    (stations, points)
                );
            }
        }
        for raw in [
            "", "48", "48X2", "0x2", "2x3", "-1x2", "48x0", "48x4", "48x2x3",
        ] {
            assert!(parse(Some(raw)).is_err(), "{raw}");
        }
        let default = parse(None).unwrap();
        assert_eq!(
            (default.stations, default.lateral_points),
            (
                CONTACT_RESOLUTION.stations,
                CONTACT_RESOLUTION.lateral_points
            )
        );
    }
}
