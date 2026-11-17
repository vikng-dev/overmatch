//! Required audio tastes, independent of any vehicle or networking composition.

use crate::provenance::{Sourced, from_shipped_str};
use std::sync::LazyLock;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Mix {
    pub spatial_scale: Sourced<f32>,
    pub listener_ear_gap: Sourced<f32>,
    pub click_volume: Sourced<f32>,
    pub report_volume: Sourced<f32>,
    pub engine_volume: Sourced<f32>,
    pub engine_speed_tau: Sourced<f32>,
    pub engine_lug_fraction: Sourced<f32>,
    pub engine_overshoot_fraction: Sourced<f32>,
}

pub(super) fn mix() -> &'static Mix {
    static MIX: LazyLock<Mix> = LazyLock::new(|| {
        let mix: Mix = from_shipped_str("assets/sfx.ron", include_str!("../../assets/sfx.ron"))
            .unwrap_or_else(|error| {
                panic!("{error}; fix the required audio tastes and provenance")
            });
        for (field, value) in [
            ("spatial_scale", mix.spatial_scale.0),
            ("listener_ear_gap", mix.listener_ear_gap.0),
            ("click_volume", mix.click_volume.0),
            ("report_volume", mix.report_volume.0),
            ("engine_volume", mix.engine_volume.0),
            ("engine_speed_tau", mix.engine_speed_tau.0),
            ("engine_lug_fraction", mix.engine_lug_fraction.0),
            ("engine_overshoot_fraction", mix.engine_overshoot_fraction.0),
        ] {
            let muted = matches!(
                field,
                "click_volume" | "report_volume" | "engine_volume" | "listener_ear_gap"
            );
            assert!(
                value.is_finite() && if muted { value >= 0.0 } else { value > 0.0 },
                "assets/sfx.ron: {field} must be finite and within its positive scale or nonnegative mix domain; fix the mix data"
            );
        }
        assert!(
            mix.engine_lug_fraction.0 <= 1.0 && mix.engine_overshoot_fraction.0 >= 1.0,
            "assets/sfx.ron: playback floor must be at or below idle, ceiling at or above governed; fix the playback fractions"
        );
        mix
    });
    &MIX
}
