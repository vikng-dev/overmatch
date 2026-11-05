//! Bottom-right net-client debug panel: ping (RTT), jitter buffer, server tick, FPS, frame time, and the
//! client's version (`net::CLIENT_VERSION`). Net-only —
//! ping is meaningless in single-player, so this lives in the net module and is mounted solely by
//! `NetClientPlugin` (never SP, never the headless server). The layout mirrors `crew_ui`'s
//! top-left status card; colours come from the required UI styling data.
//!
//! The card belongs to the match screen (`flow::Flow::InMatch`: spawned on entering it, gone on
//! leaving), shows only over a connected link, and only while the player's `DEBUG PANEL` setting
//! (`settings::DebugPanel`, on by default during the beta) is on. There is no toggle key.
//!
//! Jitter is the full receipt spread buffered by `net::interp_delay`, from the same estimator.
//! The placeholder remains until `ArrivalStats::warmed`, latched like the delay law. The value
//! uses the authored warning/alarm bands in excess authority tick budgets.
//!
//! RTT comes from lightyear's `Link::stats` on the connected client entity; FPS/frame time from
//! `FrameTimeDiagnosticsPlugin`, which is NOT in `DefaultPlugins` and is therefore registered by
//! THIS plugin — the sole registrar today; the `is_plugin_added` guard is insurance for a second
//! windowed root, since `add_plugins` PANICS on a duplicate unique plugin.
//!
//! Fixed-width `SpaceBetween` rows pin each value to the right padding; Barlow's proportional
//! digits cannot align through spaces. Refresh cadence comes from UI taste data. FPS and frame
//! time use rolling averages; server tick uses the
//! reciprocal of the longest gap between advancing packet ticks in the last refresh interval,
//! including the gap still in progress. Catch-up cannot erase a stall. This is client-observed
//! delivery cadence: network delays and client frame stalls also lower it, and packets drained
//! together share a frame timestamp. It does not measure the server's CPU time.

use core::time::Duration;
use std::collections::VecDeque;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use lightyear::core::tick::{Tick, TickDuration};
use lightyear::prelude::*;
use lightyear_transport::plugin::PacketReceived;

use crate::flow::{Flow, FlowScoped, InMatchSystems};
use crate::settings::{DebugPanel, Settings};
use crate::ui_font::{
    NET_ALARM, NET_ALARM_EXCESS_TICKS as ALARM_EXCESS_TICKS, NET_DEBUG_REFRESH as REFRESH_INTERVAL,
    NET_WARN, NET_WARN_EXCESS_TICKS as WARN_EXCESS_TICKS, PANEL_BG, TEXT, TEXT_DIM, UiFonts,
};

use super::sync_margin::ArrivalDelay;

/// First packets carrying strictly newer ticks define gaps; duplicates and reordered packets do
/// not end a stall. Retention is by the gap's end, so catch-up keeps the whole stalled interval.
#[derive(Resource, Default)]
struct ServerTickGaps {
    latest: Option<(Tick, Duration)>,
    gaps: VecDeque<(Duration, Duration)>,
}

impl ServerTickGaps {
    fn record(&mut self, tick: Tick, now: Duration) {
        if let Some((latest, at)) = self.latest {
            if tick <= latest {
                return;
            }
            self.gaps.push_back((now, now - at));
        }
        self.latest = Some((tick, now));
        while self
            .gaps
            .front()
            .is_some_and(|(end, _)| now - *end > REFRESH_INTERVAL)
        {
            self.gaps.pop_front();
        }
    }

    fn longest_gap(&self, now: Duration) -> Option<Duration> {
        // No server tick has arrived on this connection yet.
        let (_, at) = self.latest?;
        Some(
            self.gaps
                .iter()
                .filter(|(end, _)| now - *end <= REFRESH_INTERVAL)
                .map(|(_, gap)| *gap)
                .fold(now - at, Duration::max),
        )
    }
}

fn reset_tick_gaps(_connected: On<Add, Connected>, mut gaps: ResMut<ServerTickGaps>) {
    *gaps = default();
}

/// lightyear_transport 0.30.1's `TransportPlugin::buffer_receive` emits the packet header's tick
/// through `PacketReceived` in PreUpdate. Sample receipt, never a locally extrapolated timeline.
fn record_tick_arrival(
    packet: On<PacketReceived>,
    time: Res<Time<Real>>,
    connected: Query<(), With<Connected>>,
    mut gaps: ResMut<ServerTickGaps>,
) {
    // Packets before connection or after disconnect do not belong to the displayed session.
    if connected.contains(packet.entity) {
        gaps.record(packet.remote_tick, time.elapsed());
    }
}

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
    ServerTick,
    Fps,
    Frame,
}

pub fn plugin(app: &mut App) {
    if !app.is_plugin_added::<FrameTimeDiagnosticsPlugin>() {
        app.add_plugins(FrameTimeDiagnosticsPlugin::default());
    }
    crate::flow::require(app, "net::debug_hud");
    app.init_resource::<ServerTickGaps>()
        .add_observer(reset_tick_gaps)
        .add_observer(record_tick_arrival)
        .add_systems(OnEnter(Flow::InMatch), spawn_debug_hud)
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
                (Metric::ServerTick, "Server tick"),
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

/// DERIVED excess budgets: jitter uses receipt spread; cadence uses gap less its nominal tick.
fn excess_colour(excess: Duration, tick: Duration) -> Color {
    if excess > tick * ALARM_EXCESS_TICKS {
        NET_ALARM
    } else if excess >= tick * WARN_EXCESS_TICKS {
        NET_WARN
    } else {
        TEXT
    }
}

/// The full buffered receipt spread, without a physics-dependent discount.
fn jitter_readout(spread: Duration, tick: Duration) -> (String, Color) {
    (
        format!("+{:.0} ms", spread.as_secs_f64() * 1000.0),
        excess_colour(spread, tick),
    )
}

/// Jitter and cadence share the authored excess-budget bands. Cap at the nominal rate:
/// coalesced arrivals cannot establish a faster server. Never divide a gap by skipped ticks:
/// catch-up would conceal stalls.
fn server_tick_readout(gap: Duration, tick: Duration) -> (String, Color) {
    let color = excess_colour(gap.saturating_sub(tick), tick);
    (
        format!("{:.1} Hz", gap.max(tick).as_secs_f64().recip()),
        color,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "a debug readout of time, diagnostics, tick, arrival delay, tick gaps, links and the setting, \
              with two locals"
)]
fn update_debug_hud(
    time: Res<Time<Real>>,
    mut since_refresh: Local<Duration>,
    settings: Res<Settings>,
    diagnostics: Res<DiagnosticsStore>,
    tick: Res<TickDuration>,
    estimator: Res<ArrivalDelay>,
    tick_gaps: Res<ServerTickGaps>,
    // The client connection entity (`client::run`) carries `Link` + `Connected`; one on a client.
    links: Query<&Link, With<Connected>>,
    mut values: Query<(&mut Text, &mut TextColor, &Metric)>,
    // Mirrors `interp_delay`'s arming latch: placeholder until the estimator distribution is
    // valid, then live for the link even if the digest later regresses.
    mut armed: Local<Option<u64>>,
) {
    // Wall time keeps a long client frame from being hidden by virtual-time clamping.
    *since_refresh += time.delta();
    if *since_refresh < REFRESH_INTERVAL {
        return;
    }
    // A hidden card is not written; its refresh stays due, so it shows current values. The arming
    // latch waits with it: `warmed` never falls within a link, so a late latch arms the same.
    if !card_shown(settings.debug_panel, !links.is_empty()) {
        return;
    }
    *since_refresh = Duration::ZERO;

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
    let server_tick = tick_gaps
        .longest_gap(time.elapsed())
        .map(|gap| server_tick_readout(gap, tick.0));
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
            Metric::ServerTick => server_tick
                .clone()
                .unwrap_or_else(|| ("--".to_string(), TEXT)),
            Metric::Fps => (format!("{fps:.0}"), TEXT),
            Metric::Frame => (format!("{frame_ms:.1} ms"), TEXT),
        };
        crate::ui_font::set_text(text.reborrow(), &value);
        color.set_if_neq(TextColor(tint));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mount the shipped observers without running UI/layout. The shared network fixture supplies
    /// the session step; assertions and arrivals read the installed `TickDuration` binding.
    fn tick_app() -> (App, Entity, Duration) {
        let mut app = App::new();
        app.insert_resource(State::new(Flow::Boot))
            .init_resource::<Time<Real>>()
            .insert_resource(TickDuration(super::super::test_harness::TICK))
            .add_plugins(plugin);
        let link = app
            .world_mut()
            .spawn((RemoteId(PeerId::Server), Connected))
            .id();
        app.world_mut().flush();
        let tick = app.world().resource::<TickDuration>().0;
        (app, link, tick)
    }

    fn arrive(app: &mut App, link: Entity, tick: u32, at: Duration) {
        app.world_mut().resource_mut::<Time<Real>>().advance_to(at);
        app.world_mut().trigger(PacketReceived {
            entity: link,
            remote_tick: Tick(tick),
        });
        app.world_mut().flush();
    }

    fn readout(app: &App) -> Option<(String, Color)> {
        let world = app.world();
        world
            .resource::<ServerTickGaps>()
            .longest_gap(world.resource::<Time<Real>>().elapsed())
            .map(|gap| server_tick_readout(gap, world.resource::<TickDuration>().0))
    }

    /// Players see the nominal rate on evenly spaced tick advances. The display bands are in
    /// excess tick budgets, so changing the session rate preserves their meaning.
    #[test]
    fn steady_ticks_are_healthy_and_bands_follow_the_tick_budget() {
        let (mut app, link, tick) = tick_app();
        assert_eq!(readout(&app), None, "no packet yet is a placeholder");
        // Fixture: two seconds of steady arrivals, crossing the retention boundary.
        let mut at = Duration::ZERO;
        let mut remote = 0;
        while at <= REFRESH_INTERVAL * 2 {
            arrive(&mut app, link, remote, at);
            remote += 1;
            at += tick;
        }
        assert_eq!(
            readout(&app),
            Some((format!("{:.1} Hz", tick.as_secs_f64().recip()), TEXT))
        );
        assert_eq!(
            server_tick_readout(tick * WARN_EXCESS_TICKS + tick, tick).1,
            NET_WARN
        );
        assert_eq!(
            server_tick_readout(tick * ALARM_EXCESS_TICKS + tick, tick).1,
            NET_WARN
        );
        assert_eq!(
            server_tick_readout(
                tick * ALARM_EXCESS_TICKS + tick + Duration::from_nanos(1),
                tick
            )
            .1,
            NET_ALARM
        );
    }

    /// Averages hide a freeze followed by catch-up. Players must see the stall while it is in
    /// progress and for the next refresh interval after receipt, then see recovery.
    #[test]
    fn a_stall_then_a_catch_up_burst_stays_visible() {
        let (mut app, link, tick) = tick_app();
        arrive(&mut app, link, 0, Duration::ZERO);
        arrive(&mut app, link, 1, tick);
        // Fixture: a freeze above the authored alarm, then a burst of coalesced ticks.
        let stall = tick * ALARM_EXCESS_TICKS + tick * 2;
        let resumed = tick + stall;
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_to(resumed);
        let stalled = Some((format!("{:.1} Hz", stall.as_secs_f64().recip()), NET_ALARM));
        assert_eq!(readout(&app), stalled, "silence is already a slowdown");
        let missed = (stall.as_nanos() / tick.as_nanos()) as u32;
        for remote in missed..=missed + 2 {
            arrive(&mut app, link, remote, resumed);
        }
        assert_eq!(readout(&app), stalled, "catch-up must not erase the gap");
        let mut elapsed = tick;
        let mut remote = missed + 3;
        while elapsed <= REFRESH_INTERVAL {
            arrive(&mut app, link, remote, resumed + elapsed);
            assert_eq!(readout(&app), stalled, "retain for a panel refresh");
            elapsed += tick;
            remote += 1;
        }
        arrive(&mut app, link, remote, resumed + elapsed);
        assert_eq!(readout(&app).unwrap().1, TEXT, "expired gaps recover");
    }

    /// Only new server ticks end a player's observed gap. A reconnect may restart its tick
    /// numbers; neither the previous stall nor its newest tick may cross that boundary.
    #[test]
    fn duplicates_do_not_end_silence_and_a_reconnect_resets_history() {
        let (mut app, link, tick) = tick_app();
        arrive(&mut app, link, 10, Duration::ZERO);
        arrive(&mut app, link, 11, tick);
        let at = tick * 5;
        for remote in [11, 10] {
            arrive(&mut app, link, remote, at);
        }
        assert_eq!(
            app.world().resource::<ServerTickGaps>().longest_gap(at),
            Some(tick * 4)
        );
        app.world_mut().entity_mut(link).remove::<Connected>();
        arrive(&mut app, link, 12, at);
        assert_eq!(
            app.world().resource::<ServerTickGaps>().latest,
            Some((Tick(11), tick)),
            "disconnected packets cannot advance the displayed session"
        );
        app.world_mut().entity_mut(link).insert(Connected);
        app.world_mut().flush();
        assert_eq!(readout(&app), None, "a new session has no samples yet");
        arrive(&mut app, link, 0, at);
        arrive(&mut app, link, 1, at + tick);
        assert_eq!(readout(&app).unwrap().1, TEXT);
    }

    /// The panel reports the full buffered spread in milliseconds; fixture conversions pin the
    /// unit while colour bands follow the session's authority interval.
    #[test]
    fn jitter_reports_the_full_buffered_spread() {
        let (_, _, tick) = tick_app();
        assert_eq!(jitter_readout(Duration::ZERO, tick), ("+0 ms".into(), TEXT));
        for (millis, expected) in [(10, "+10 ms"), (68, "+68 ms")] {
            assert_eq!(
                jitter_readout(Duration::from_millis(millis), tick).0,
                expected
            );
        }
        assert_eq!(jitter_readout(tick / 2, tick).1, TEXT);
        assert_eq!(jitter_readout(tick * WARN_EXCESS_TICKS, tick).1, NET_WARN);
        assert_eq!(jitter_readout(tick * ALARM_EXCESS_TICKS, tick).1, NET_WARN);
        assert_eq!(
            jitter_readout(tick * ALARM_EXCESS_TICKS + Duration::from_nanos(1), tick).1,
            NET_ALARM
        );
    }
}
