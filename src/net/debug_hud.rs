//! Bottom-right net-client debug panel: ping (RTT), jitter buffer, FPS, frame time, and the
//! client's version (`net::CLIENT_VERSION`). Net-only —
//! ping is meaningless in single-player, so this lives in the net module and is mounted solely by
//! `NetClientPlugin` (never SP, never the headless server). WIP instrumentation: hardcoded styling
//! mirrored from `crew_ui`'s top-left status card.
//!
//! The card belongs to the match screen (`flow::Flow::InMatch`: spawned on entering it, gone on
//! leaving), shows only over a connected link, and only while the player's `DEBUG PANEL` setting
//! (`settings::DebugPanel`, on by default during the beta) is on. There is no toggle key.
//!
//! Jitter is the full receipt spread buffered by `net::interp_delay`, from the same estimator.
//! The placeholder remains until `ArrivalStats::warmed`, latched like the delay law. The value
//! warns at one authority tick and alarms above two ticks (DERIVED display bands in tick units).
//!
//! RTT comes from lightyear's `Link::stats` on the connected client entity; FPS/frame time from
//! `FrameTimeDiagnosticsPlugin`, which is NOT in `DefaultPlugins` and is therefore registered by
//! THIS plugin — the sole registrar today; the `is_plugin_added` guard is insurance for a second
//! windowed root, since `add_plugins` PANICS on a duplicate unique plugin.
//!
//! Anti-jitter: the card is a **fixed-width** row of label/value columns. Each metric is its own
//! two-column flex row (`SpaceBetween`), so the value's *right* edge is pinned to the card's right
//! padding — the numbers stay right-aligned and the whole card's left edge never moves as the digit
//! count changes (`42` -> `138`). The bundled Barlow Condensed font is proportional, so
//! right-pinning the value column is what keeps the digits from shimmering, not space padding. The
//! readout is also refreshed at ~1 Hz off a *rolling average* (not the raw per-frame value) so the
//! numbers are legible instead of churning every frame — the usual game-overlay treatment.

use core::time::Duration;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::prelude::*;

use crate::flow::{Flow, FlowScoped, InMatchSystems};
use crate::settings::{DebugPanel, Settings};
use crate::ui_font::{PANEL_BG, TEXT, TEXT_DIM, UiFonts};

use super::sync_margin::ArrivalDelay;

/// How often the readout text is rebuilt. Slow enough that the digits are readable, fast enough to
/// still track the sim — the standard per-second refresh of a game perf overlay.
const REFRESH_SECS: f32 = 1.0;

/// Jitter value at/above one tick of buffered excess. The HUD family's amber (`hud.rs`'s
/// tank-state card draws the same literal); local per this module's hardcoded-styling charter.
const JITTER_WARN: Color = Color::srgb(1.0, 0.8, 0.3);

/// Jitter value above two authority intervals. The HUD family's red, same provenance as
/// [`JITTER_WARN`].
const JITTER_ALARM: Color = Color::srgb(1.0, 0.3, 0.2);

/// The card's root node.
#[derive(Component)]
pub(super) struct DebugCard;

/// Whether the card shows: the setting is on, and the link is connected — every row but the version
/// reads a live session, and without one the ping is meaningless.
fn card_shown(setting: DebugPanel, connected: bool) -> bool {
    setting.shown() && connected
}

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
    crate::flow::require(app, "net::debug_hud");
    app.add_systems(OnEnter(Flow::InMatch), spawn_debug_hud)
        .add_systems(
            Update,
            (show_debug_card, update_debug_hud).in_set(InMatchSystems),
        );
}

fn show_debug_card(
    settings: Res<Settings>,
    links: Query<(), (With<Connected>, With<Link>)>,
    mut card: Query<&mut Visibility, With<DebugCard>>,
) {
    let shown = card_shown(settings.debug_panel, !links.is_empty());
    for mut visibility in &mut card {
        visibility.set_if_neq(if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

/// Mount the real fixed panels without their game-state readers for HUD layout tests.
#[cfg(test)]
pub(crate) fn mount_layout_test(app: &mut App) {
    app.add_systems(Startup, spawn_debug_hud);
}

fn spawn_debug_hud(mut commands: Commands, fonts: Res<UiFonts>) {
    // Bottom-right — a subtle dark card mirroring `crew_ui`'s status panel (top-left), so the
    // corners read as one UI family. Fixed width sized to the widest realistic row
    // ("Frame  999.9 ms") at font_size 15px; Bevy UI Nodes are border-box, so this includes the 8px
    // horizontal padding.
    commands
        .spawn((
            FlowScoped(Flow::InMatch),
            DebugCard,
            crate::hud::HudDock::Fixed,
            Visibility::Hidden,
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
            // The build this client is, constant for the process: the string a bug report and
            // the matchmaking Worker both name.
            card.spawn(Node {
                width: Val::Percent(100.0),
                justify_content: JustifyContent::SpaceBetween,
                column_gap: Val::Px(8.0),
                ..default()
            })
            .with_children(|row| {
                for text in ["Version", super::CLIENT_VERSION] {
                    row.spawn((
                        Text::new(text),
                        TextFont {
                            font: fonts.body.clone().into(),
                            font_size: FontSize::Px(15.0),
                            ..default()
                        },
                        TextColor(TEXT_DIM),
                    ));
                }
            });
        });
}

/// The buffered receipt spread, without a physics-dependent discount. DERIVED display bands:
/// one additional authority interval warns; more than two alarms.
fn jitter_readout(spread: Duration, tick: Duration) -> (String, Color) {
    let color = if spread > tick * 2 {
        JITTER_ALARM
    } else if spread >= tick {
        JITTER_WARN
    } else {
        TEXT
    };
    (format!("+{:.0} ms", spread.as_secs_f64() * 1000.0), color)
}

#[expect(
    clippy::too_many_arguments,
    reason = "a debug readout of time, diagnostics, tick, arrival delay, links and the setting, \
              with two locals"
)]
fn update_debug_hud(
    time: Res<Time>,
    mut since_refresh: Local<f32>,
    settings: Res<Settings>,
    diagnostics: Res<DiagnosticsStore>,
    tick: Res<TickDuration>,
    estimator: Res<ArrivalDelay>,
    // The client connection entity (`client::run`) carries `Link` + `Connected`; one on a client.
    links: Query<&Link, With<Connected>>,
    mut values: Query<(&mut Text, &mut TextColor, &Metric)>,
    // Mirrors `interp_delay`'s arming latch: placeholder until the estimator distribution is
    // valid, then live for the link even if the digest later regresses.
    mut armed: Local<Option<u64>>,
) {
    // Throttle to ~1 Hz so the numbers are legible instead of churning every frame (repo idiom:
    // `Local<f32>` accumulator vs. a threshold, cf. `net::diagnostics`).
    *since_refresh += time.delta_secs();
    if *since_refresh < REFRESH_SECS {
        return;
    }
    // A hidden card is not written; its refresh stays due, so it shows current values. The arming
    // latch waits with it: `warmed` never falls within a link, so a late latch arms the same.
    if !card_shown(settings.debug_panel, !links.is_empty()) {
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
    if *armed != Some(estimator.link()) && estimator.stats.warmed() {
        *armed = Some(estimator.link());
    }
    let jitter = (*armed == Some(estimator.link()))
        .then(|| jitter_readout(estimator.stats.spread(), tick.0));
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
        crate::ui_font::set_text(text.reborrow(), &value);
        color.set_if_neq(TextColor(tint));
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

    /// The debug panel reports the exact spread charged by the buffer. Fixture spreads straddle
    /// the display's authority-interval bands; a hidden extrapolation discount fails the label.
    #[test]
    fn jitter_reports_the_full_buffered_spread() {
        assert_eq!(jitter_readout(Duration::ZERO, TICK), ("+0 ms".into(), TEXT));
        assert_eq!(
            jitter_readout(Duration::from_millis(10), TICK),
            ("+10 ms".into(), TEXT)
        );
        assert_eq!(jitter_readout(TICK, TICK).1, JITTER_WARN);
        assert_eq!(jitter_readout(TICK * 2, TICK).1, JITTER_WARN);
        assert_eq!(
            jitter_readout(TICK * 2 + Duration::from_nanos(1), TICK).1,
            JITTER_ALARM
        );
        assert_eq!(
            jitter_readout(Duration::from_millis(68), TICK),
            ("+68 ms".into(), JITTER_ALARM)
        );
    }
}
