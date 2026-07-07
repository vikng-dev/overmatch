//! Bottom-right net-client debug panel: ping (RTT), FPS, and frame time. Net-only — ping is
//! meaningless in single-player, so this lives in the net module and is mounted solely by
//! `NetClientPlugin` (never SP, never the headless server). WIP instrumentation: hardcoded styling
//! mirrored from `crew_ui`'s top-left status card, one spawn system + one update system, no config
//! knobs (per the minimalism directive).
//!
//! RTT comes from lightyear's `Link::stats` on the connected client entity; FPS/frame time from
//! `FrameTimeDiagnosticsPlugin` (registered in `client::run`, since it is NOT in `DefaultPlugins`).

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use lightyear::prelude::*;

/// The bottom-right debug panel's text node.
#[derive(Component)]
struct DebugHudText;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn_debug_hud)
        .add_systems(Update, update_debug_hud);
}

fn spawn_debug_hud(mut commands: Commands) {
    // Bottom-right — a subtle dark card mirroring `crew_ui`'s status panel (top-left), so the
    // corners read as one UI family.
    commands.spawn((
        DebugHudText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgb(0.85, 0.95, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(10.0),
            right: Val::Px(10.0),
            padding: UiRect::all(Val::Px(8.0)),
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.04, 0.06, 0.08, 0.62)),
    ));
}

fn update_debug_hud(
    diagnostics: Res<DiagnosticsStore>,
    // The client connection entity (`client::run`) carries `Link` + `Connected`; one on a client.
    links: Query<&Link, With<Connected>>,
    mut panel: Query<&mut Text, With<DebugHudText>>,
) {
    let Ok(mut text) = panel.single_mut() else {
        return;
    };
    // `Link::stats.rtt` is `Duration::ZERO` until the first pong (~100 ms after connect) — show a
    // placeholder rather than a fake `0 ms`.
    let ping = links
        .iter()
        .next()
        .map(|link| link.stats.rtt)
        .filter(|rtt| !rtt.is_zero())
        .map_or_else(
            || "--".to_string(),
            |rtt| format!("{:.0} ms", rtt.as_secs_f64() * 1000.0),
        );
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);
    // FRAME_TIME is already in milliseconds.
    let frame_ms = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);
    *text = Text::new(format!("Ping {ping}\nFPS {fps:.0}\nFrame {frame_ms:.1} ms"));
}
