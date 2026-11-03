//! Required aim construction shared by input admission and every sky-ray consumer.
use crate::provenance::{Sourced, from_shipped_str};
use serde::Deserialize;
use std::sync::LazyLock;

pub(crate) const INPUT_RON: &str = include_str!("../../assets/input/aim.ron");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AimData {
    sky_range_m: Sourced<f32>,
}

fn range(text: &str) -> Result<f32, String> {
    let data: AimData = from_shipped_str("assets/input/aim.ron", text)?;
    let range = data.sky_range_m.0;
    let envelope = crate::world::VIEW_CAST_MAX_M + range;
    // DERIVED: three squared coordinates bound normalization; both frame translations can
    // contribute the envelope, so reserve a factor of four for their squared difference.
    if !range.is_finite()
        || range <= 0.0
        || envelope + range <= envelope
        || !(12.0 * envelope * envelope).is_finite()
    {
        return Err("assets/input/aim.ron: sky_range_m must be positive, distinguishable at world scale and yield a finite squared input envelope; author a representable sky distance in metres".into());
    }
    Ok(range)
}

/// Where a ray that reaches no surface ends, standing in for a direction at infinity. Every aim
/// ray reads it (the third-person pick, the bore dot, `sight::drive_gunner_aim`), so the sky is one
/// far point in all of them.
pub(crate) static MAX_RANGE: LazyLock<f32> =
    LazyLock::new(|| range(INPUT_RON).unwrap_or_else(|error| panic!("{error}")));

#[cfg(test)]
mod tests {
    use super::*;
    /// Input admission and the ray consumers require a finite, normalizable horizon.
    #[test]
    fn the_sky_horizon_has_a_representable_domain() {
        for value in ["0.0", "-1.0", "NaN", "inf", "1e30", "1e-45"] {
            let err = range(&format!(
                "(sky_range_m: ({value}, StandIn(\"fixture sky point\")))"
            ))
            .unwrap_err();
            assert!(
                err.contains("sky_range_m") && err.contains("author"),
                "{err}"
            );
        }
        assert_eq!(
            range("(sky_range_m: (100.0, StandIn(\"fixture sky point\")))").unwrap(),
            100.0
        );
    }

    /// The handshake and every sky ray trust this file, so it must state the distance and label
    /// its job: absence and the test-only label are both refused before play.
    #[test]
    fn the_sky_distance_is_required_labelled_data() {
        let missing = range("()").unwrap_err();
        assert!(
            missing.contains("assets/input/aim.ron") && missing.contains("sky_range_m"),
            "{missing}"
        );
        let fixture = range("(sky_range_m: (100.0, Fixture(\"fixture sky point\")))").unwrap_err();
        assert!(
            fixture.contains("Fixture provenance is test-only"),
            "{fixture}"
        );
    }

    /// Input parsing (`command::ParsedTankCommand`) admits a pick anywhere a ray can end: on the
    /// world's cast envelope, or at the sky point beyond it.
    #[test]
    fn the_input_bound_spans_the_world_and_the_sky() {
        assert_eq!(
            *crate::aim::AIM_LIMIT,
            crate::world::VIEW_CAST_MAX_M + range(INPUT_RON).unwrap()
        );
    }
}
