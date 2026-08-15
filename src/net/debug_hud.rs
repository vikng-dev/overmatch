//! Bottom-right net-client debug panel: ping (RTT), jitter buffer, FPS, and frame time. Net-only —
//! ping is meaningless in single-player, so this lives in the net module and is mounted solely by
//! `NetClientPlugin` (never SP, never the headless server). WIP instrumentation: hardcoded styling
//! mirrored from `crew_ui`'s top-left status card, one spawn system + one update system, no config
//! knobs (per the minimalism directive).
//!
//! Jitter is the delay law's instability term — `ArrivalStats::spread()` less
//! `extrapolate::horizon()`, floored at zero: the exact milliseconds `net::interp_delay` buffers
//! because the link is unstable, read from the SAME estimator resource (never a second estimator).
//! Placeholder until the estimator warms (`ArrivalStats::warmed`, the delay law's own arming gate,
//! latched like it). The value colour steps at one tick (amber) and the extrapolation horizon
//! (red), both read from their single sources — `TickDuration` and `extrapolate::horizon()`.
//!
//! RTT comes from lightyear's `Link::stats` on the connected client entity; FPS/frame time from
//! `FrameTimeDiagnosticsPlugin`, which is NOT in `DefaultPlugins` and is therefore registered by
//! THIS plugin — the sole registrar today; the `is_plugin_added` guard is insurance for a second
//! windowed root, since `add_plugins` PANICS on a duplicate unique plugin.
//!
//! Anti-jitter: the card is a **fixed-width** row of label/value columns. Each metric is its own
//! two-column flex row (`SpaceBetween`), so the value's *right* edge is pinned to the card's right
//! padding — the numbers stay right-aligned and the whole card's left edge never moves as the digit
//! count changes (`42` -> `138`). The default Bevy font is proportional (no monospace bundled), so
//! right-pinning the value column is what keeps the digits from shimmering, not space padding. The
//! readout is also refreshed at ~1 Hz off a *rolling average* (not the raw per-frame value) so the
//! numbers are legible instead of churning every frame — the usual game-overlay treatment.

use core::time::Duration;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::prelude::*;

use crate::ui_font::{PANEL_BG, TEXT, UiFonts};

use super::sync_margin::ArrivalDelay;

/// How often the readout text is rebuilt. Slow enough that the digits are readable, fast enough to
/// still track the sim — the standard per-second refresh of a game perf overlay.
const REFRESH_SECS: f32 = 1.0;

/// Jitter value at/above one tick of buffered excess. The HUD family's amber (`hud.rs`'s
/// tank-state card draws the same literal); local per this module's hardcoded-styling charter.
const JITTER_WARN: Color = Color::srgb(1.0, 0.8, 0.3);

/// Jitter value above the extrapolation horizon. The HUD family's red, same provenance as
/// [`JITTER_WARN`].
const JITTER_ALARM: Color = Color::srgb(1.0, 0.3, 0.2);

/// Which metric a value column renders. Lets one update system fan out over the value nodes.
#[derive(Component, Clone, Copy)]
enum Metric {
    Ping,
    Jitter,
    Fps,
    Frame,
}

pub fn plugin(app: &mut App) {
    if !app.is_plugin_added::<FrameTimeDiagnosticsPlugin>() {
        app.add_plugins(FrameTimeDiagnosticsPlugin::default());
    }
    app.add_systems(Startup, spawn_debug_hud)
        .add_systems(Update, update_debug_hud);
}

fn spawn_debug_hud(mut commands: Commands, fonts: Res<UiFonts>) {
    // Bottom-right — a subtle dark card mirroring `crew_ui`'s status panel (top-left), so the
    // corners read as one UI family. Fixed width sized to the widest realistic row
    // ("Frame  999.9 ms") at font_size 15px; Bevy UI Nodes are border-box, so this includes the 8px
    // horizontal padding.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(10.0),
                right: Val::Px(10.0),
                width: Val::Px(160.0),
                padding: UiRect::all(Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(PANEL_BG),
        ))
        .with_children(|card| {
            for (metric, label) in [
                (Metric::Ping, "Ping"),
                (Metric::Jitter, "Jitter"),
                (Metric::Fps, "FPS"),
                (Metric::Frame, "Frame"),
            ] {
                // One row per metric: label pinned left, value pinned right (SpaceBetween). Pinning
                // the value's right edge is what right-aligns the numbers.
                card.spawn(Node {
                    width: Val::Percent(100.0),
                    justify_content: JustifyContent::SpaceBetween,
                    column_gap: Val::Px(8.0),
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((
                        Text::new(label),
                        TextFont {
                            // Regular: a dense metric row label.
                            font: fonts.body.clone().into(),
                            font_size: FontSize::Px(15.0),
                            ..default()
                        },
                        TextColor(TEXT),
                    ));
                    row.spawn((
                        metric,
                        // Placeholder until the first ~1 Hz refresh populates real numbers.
                        Text::new("--"),
                        TextFont {
                            // Regular: a dense metric row value.
                            font: fonts.body.clone().into(),
                            font_size: FontSize::Px(15.0),
                            ..default()
                        },
                        TextColor(TEXT),
                    ));
                });
            }
        });
}

/// The jitter readout: the buffered excess `spread − horizon` floored at zero — the SAME term
/// `net::interp_delay::derived_min_delay` consumes — as `+{n} ms` plus a severity colour: [`TEXT`]
/// below one tick of excess, [`JITTER_WARN`] at/above one tick, [`JITTER_ALARM`] above the
/// extrapolation horizon.
fn jitter_readout(spread: Duration, tick: Duration, horizon: Duration) -> (String, Color) {
    let excess = spread.saturating_sub(horizon);
    let color = if excess > horizon {
        JITTER_ALARM
    } else if excess >= tick {
        JITTER_WARN
    } else {
        TEXT
    };
    (format!("+{:.0} ms", excess.as_secs_f64() * 1000.0), color)
}

fn update_debug_hud(
    time: Res<Time>,
    mut since_refresh: Local<f32>,
    diagnostics: Res<DiagnosticsStore>,
    tick: Res<TickDuration>,
    estimator: Res<ArrivalDelay>,
    // The client connection entity (`client::run`) carries `Link` + `Connected`; one on a client.
    links: Query<&Link, With<Connected>>,
    mut values: Query<(&mut Text, &mut TextColor, &Metric)>,
    // Mirrors `interp_delay`'s arming latch: placeholder until the estimator distribution is
    // valid, then live for the session even if the digest later regresses.
    mut armed: Local<bool>,
) {
    // Throttle to ~1 Hz so the numbers are legible instead of churning every frame (repo idiom:
    // `Local<f32>` accumulator vs. a threshold, cf. `net::diagnostics`).
    *since_refresh += time.delta_secs();
    if *since_refresh < REFRESH_SECS {
        return;
    }
    *since_refresh = 0.0;

    // `Link::stats.rtt` is `Duration::ZERO` until the first pong (~100 ms after connect) — show a
    // placeholder rather than a fake `0 ms`. lightyear already exposes an EMA-smoothed RTT.
    let ping = links
        .iter()
        .next()
        .map(|link| link.stats.rtt)
        .filter(|rtt| !rtt.is_zero())
        .map_or_else(
            || "--".to_string(),
            |rtt| format!("{:.0} ms", rtt.as_secs_f64() * 1000.0),
        );
    // The delay law's instability term, from the SAME estimator `interp_delay` reads; `None`
    // (placeholder) until the law would itself be armed.
    if !*armed && estimator.stats.warmed() {
        *armed = true;
    }
    let jitter = (*armed).then(|| {
        jitter_readout(
            estimator.stats.spread(),
            tick.0,
            super::extrapolate::horizon(),
        )
    });
    // Rolling averages over the diagnostic history buffer (not the raw per-frame value), so a single
    // slow frame doesn't make the number jump. FRAME_TIME is already in milliseconds.
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.average())
        .unwrap_or(0.0);
    let frame_ms = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|d| d.average())
        .unwrap_or(0.0);

    for (mut text, mut color, metric) in &mut values {
        let (value, tint) = match metric {
            Metric::Ping => (ping.clone(), TEXT),
            Metric::Jitter => jitter.clone().unwrap_or_else(|| ("--".to_string(), TEXT)),
            Metric::Fps => (format!("{fps:.0}"), TEXT),
            Metric::Frame => (format!("{frame_ms:.1} ms"), TEXT),
        };
        *text = Text::new(value);
        color.0 = tint;
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use crate::ui_font::TEXT;

    use super::{JITTER_ALARM, JITTER_WARN, jitter_readout};

    /// The game's fixed tick (64 Hz), spelled out rather than read from the runtime binding — the
    /// sibling-test discipline (cf. `net::interp_delay`'s tests).
    const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);

    /// A test horizon larger than one tick, as `g*` is at every shipped operating point. Arbitrary
    /// round value: the readout is pure in it, so the thresholds are pinned without restating
    /// `extrapolate`'s expression.
    const HORIZON: Duration = Duration::from_millis(50);

    /// MUTANT — dropping the saturating floor at zero: a spread at or under the horizon is the
    /// extrapolator's to cover, so the readout is exactly `+0 ms` in the resting colour, never a
    /// negative or wrapped value.
    #[test]
    fn a_spread_under_the_horizon_floors_to_zero() {
        for spread in [Duration::ZERO, Duration::from_millis(30), HORIZON] {
            let (label, color) = jitter_readout(spread, TICK, HORIZON);
            assert_eq!(label, "+0 ms", "spread {spread:?}");
            assert_eq!(color, TEXT, "spread {spread:?}");
        }
    }

    /// MUTANT — off-by-one at the tick boundary: exactly one tick of excess is amber (the bound is
    /// inclusive); one microsecond under it keeps the resting colour.
    #[test]
    fn the_tick_boundary_is_inclusive_for_amber() {
        let under = jitter_readout(HORIZON + TICK - Duration::from_micros(1), TICK, HORIZON);
        assert_eq!(under.1, TEXT, "excess under one tick rests");
        let at = jitter_readout(HORIZON + TICK, TICK, HORIZON);
        assert_eq!(at.1, JITTER_WARN, "excess of one tick warns");
    }

    /// MUTANT — swapping the two thresholds: an excess between one tick and the horizon is amber,
    /// the horizon itself included; only an excess beyond the horizon is red.
    #[test]
    fn amber_below_the_horizon_red_above_it() {
        let mid = jitter_readout(HORIZON + Duration::from_millis(30), TICK, HORIZON);
        assert_eq!(mid.1, JITTER_WARN, "excess between the thresholds warns");
        let at_horizon = jitter_readout(HORIZON + HORIZON, TICK, HORIZON);
        assert_eq!(
            at_horizon.1, JITTER_WARN,
            "excess of the horizon still warns"
        );
        let beyond = jitter_readout(HORIZON + HORIZON + Duration::from_micros(1), TICK, HORIZON);
        assert_eq!(beyond.1, JITTER_ALARM, "excess beyond the horizon alarms");
    }

    /// The label is the buffered excess in whole milliseconds with the explicit sign — the buffer
    /// bought, not the raw spread.
    #[test]
    fn the_label_is_the_excess_in_whole_ms() {
        let (label, color) = jitter_readout(HORIZON + Duration::from_millis(68), TICK, HORIZON);
        assert_eq!(label, "+68 ms");
        assert_eq!(color, JITTER_ALARM);
    }
}
