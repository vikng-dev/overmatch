//! Bottom-right net-client debug panel: ping (RTT), FPS, and frame time. Net-only — ping is
//! meaningless in single-player, so this lives in the net module and is mounted solely by
//! `NetClientPlugin` (never SP, never the headless server). WIP instrumentation: hardcoded styling
//! mirrored from `crew_ui`'s top-left status card, one spawn system + one update system, no config
//! knobs (per the minimalism directive).
//!
//! RTT comes from lightyear's `Link::stats` on the connected client entity; FPS/frame time from
//! `FrameTimeDiagnosticsPlugin` (registered in `client::run`, since it is NOT in `DefaultPlugins`).
//!
//! Anti-jitter: the card is a **fixed-width** row of label/value columns. Each metric is its own
//! two-column flex row (`SpaceBetween`), so the value's *right* edge is pinned to the card's right
//! padding — the numbers stay right-aligned and the whole card's left edge never moves as the digit
//! count changes (`42` -> `138`). The default Bevy font is proportional (no monospace bundled), so
//! right-pinning the value column is what keeps the digits from shimmering, not space padding. The
//! readout is also refreshed at ~1 Hz off a *rolling average* (not the raw per-frame value) so the
//! numbers are legible instead of churning every frame — the usual game-overlay treatment.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use lightyear::prelude::*;

/// How often the readout text is rebuilt. Slow enough that the digits are readable, fast enough to
/// still track the sim — the standard per-second refresh of a game perf overlay.
const REFRESH_SECS: f32 = 1.0;

/// Which metric a value column renders. Lets one update system fan out over the three value nodes.
#[derive(Component, Clone, Copy)]
enum Metric {
    Ping,
    Fps,
    Frame,
}

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn_debug_hud)
        .add_systems(Update, update_debug_hud);
}

fn spawn_debug_hud(mut commands: Commands) {
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
            BackgroundColor(Color::srgba(0.04, 0.06, 0.08, 0.62)),
        ))
        .with_children(|card| {
            for (metric, label) in [
                (Metric::Ping, "Ping"),
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
                            font_size: FontSize::Px(15.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.85, 0.95, 1.0)),
                    ));
                    row.spawn((
                        metric,
                        // Placeholder until the first ~1 Hz refresh populates real numbers.
                        Text::new("--"),
                        TextFont {
                            font_size: FontSize::Px(15.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.85, 0.95, 1.0)),
                    ));
                });
            }
        });
}

fn update_debug_hud(
    time: Res<Time>,
    mut since_refresh: Local<f32>,
    diagnostics: Res<DiagnosticsStore>,
    // The client connection entity (`client::run`) carries `Link` + `Connected`; one on a client.
    links: Query<&Link, With<Connected>>,
    mut values: Query<(&mut Text, &Metric)>,
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

    for (mut text, metric) in &mut values {
        let value = match metric {
            Metric::Ping => ping.clone(),
            Metric::Fps => format!("{fps:.0}"),
            Metric::Frame => format!("{frame_ms:.1} ms"),
        };
        *text = Text::new(value);
    }
}
