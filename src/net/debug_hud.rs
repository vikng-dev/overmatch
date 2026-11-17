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
//! Jitter is the completed-tick lateness spread measured by `net::view_clock`.
//! The placeholder remains until completed stamped ticks exist. Warning/alarm bands are
//! authored in excess authority tick budgets.
//!
//! RTT comes from lightyear's `Link::stats` on the connected client entity; FPS/frame time from
//! `FrameTimeDiagnosticsPlugin`, which is NOT in `DefaultPlugins` and is therefore registered by
//! THIS plugin — the sole registrar today; the `is_plugin_added` guard is insurance for a second
//! windowed root, since `add_plugins` PANICS on a duplicate unique plugin.
//!
//! Fixed-width `SpaceBetween` rows pin each value to the right padding; Barlow's proportional
//! digits cannot align through spaces. Refresh cadence comes from UI taste data. FPS and frame
//! time use rolling averages; server tick uses the
//! reciprocal of the longest silence between advancing packet ticks in the last refresh interval,
//! including the silence still in progress. Catch-up cannot erase a stall. The client reads the
//! socket once a frame, so a silence splits in two ([`Cadence`]): the part the client watched go
//! by, which is the server's or the network's, and the frame that received the tick, which is the
//! client's own and is shown as a client stall. It does not measure the server's CPU time.

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

use super::view_clock::ViewClock;

/// One advancing tick's arrival, stamped with the frame that read it.
struct Gap {
    end: Duration,
    /// Silence the client's earlier reads saw through: the arrival gap less [`Self::frame`], the
    /// least the real gap can have been.
    watched: Duration,
    /// How long before this read the client last read the socket.
    frame: Duration,
}

/// The slowest delivery in the last refresh interval, by the side it belongs to.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Cadence {
    /// Longest [`Gap::watched`], or the silence in progress: the server or the network.
    watched: Duration,
    /// Longest [`Gap::frame`]: the client.
    frame: Duration,
}

/// First packets carrying strictly newer ticks define gaps; duplicates and reordered packets do
/// not end a stall. Retention is by the gap's end, so catch-up keeps the whole stalled interval.
#[derive(Resource, Default)]
struct ServerTickGaps {
    latest: Option<(Tick, Duration)>,
    gaps: VecDeque<Gap>,
}

impl ServerTickGaps {
    /// `frame` is the real time from the previous frame's read to this one, at `now`.
    fn record(&mut self, tick: Tick, now: Duration, frame: Duration) {
        if let Some((latest, at)) = self.latest {
            if tick <= latest {
                return;
            }
            self.gaps.push_back(Gap {
                end: now,
                watched: (now - at).saturating_sub(frame),
                frame,
            });
        }
        self.latest = Some((tick, now));
        while self
            .gaps
            .front()
            .is_some_and(|gap| now - gap.end > REFRESH_INTERVAL)
        {
            self.gaps.pop_front();
        }
    }

    fn cadence(&self, now: Duration) -> Option<Cadence> {
        // No server tick has arrived on this connection yet.
        let (_, at) = self.latest?;
        let recent = || {
            self.gaps
                .iter()
                .filter(|gap| now - gap.end <= REFRESH_INTERVAL)
        };
        Some(Cadence {
            // Every frame since `at` read the socket and found no newer tick.
            watched: recent()
                .map(|gap| gap.watched)
                .fold(now - at, Duration::max),
            frame: recent().map(|gap| gap.frame).max().unwrap_or_default(),
        })
    }
}

fn reset_tick_gaps(_connected: On<Add<Connected>>, mut gaps: ResMut<ServerTickGaps>) {
    *gaps = default();
}

/// lightyear_transport 0.31.0's `TransportPlugin::buffer_receive` emits the packet header's tick
/// through `PacketReceived` in PreUpdate. Sample receipt, never a locally extrapolated timeline.
fn record_tick_arrival(
    packet: On<PacketReceived>,
    time: Res<Time<Real>>,
    connected: Query<(), With<Connected>>,
    mut gaps: ResMut<ServerTickGaps>,
) {
    // Packets before connection or after disconnect do not belong to the displayed session.
    if connected.contains(packet.entity) {
        gaps.record(packet.remote_tick, time.elapsed(), time.delta());
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
    // ("Server tick  client 9999 ms": DERIVED 137 px of the 144 px content box, from the Regular
    // weight's advance widths at 15px plus the column gap); Bevy UI Nodes are border-box, so the
    // width includes the 8px horizontal padding.
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
            // Both columns of a metric row use the Regular weight: the rows are dense.
            for (metric, label) in [
                (Metric::Ping, "Ping"),
                (Metric::Jitter, "Jitter"),
                (Metric::ServerTick, "Server tick"),
                (Metric::Fps, "FPS"),
                (Metric::Frame, "Frame"),
            ] {
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

/// Jitter and cadence share the authored excess-budget bands. A client frame past the warning
/// budget that outlasts every watched silence is named as the client's stall: the server's rate
/// over it is unknown, not low. Otherwise the server's rate, from the watched silence. Cap at the
/// nominal rate: coalesced arrivals cannot establish a faster server. Never divide a gap by
/// skipped ticks: catch-up would conceal stalls.
fn server_tick_readout(cadence: Cadence, tick: Duration) -> (String, Color) {
    let Cadence { watched, frame } = cadence;
    let stall = frame.saturating_sub(tick);
    if frame > watched && stall >= tick * WARN_EXCESS_TICKS {
        (
            format!("client {:.0} ms", frame.as_secs_f64() * 1000.0),
            excess_colour(stall, tick),
        )
    } else {
        (
            format!("{:.1} Hz", watched.max(tick).as_secs_f64().recip()),
            excess_colour(watched.saturating_sub(tick), tick),
        )
    }
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
    view_clock: Res<ViewClock>,
    tick_gaps: Res<ServerTickGaps>,
    // The client connection entity (`client::run`) carries `Link` + `Connected`; one on a client.
    links: Query<&Link, With<Connected>>,
    mut values: Query<(&mut Text, &mut TextColor, &Metric)>,
) {
    // Wall time keeps a long client frame from being hidden by virtual-time clamping.
    *since_refresh += time.delta();
    if *since_refresh < REFRESH_INTERVAL {
        return;
    }
    // A hidden card stays due so the next visible refresh uses current readings.
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
    let jitter = view_clock
        .lateness_spread()
        .map(|spread| jitter_readout(Duration::from_secs_f64(spread), tick.0));
    let server_tick = tick_gaps
        .cadence(time.elapsed())
        .map(|cadence| server_tick_readout(cadence, tick.0));
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

    /// One client frame beginning at `at`, reading the socket for `ticks` in order.
    fn frame(app: &mut App, link: Entity, at: Duration, ticks: impl IntoIterator<Item = u32>) {
        app.world_mut().resource_mut::<Time<Real>>().advance_to(at);
        for tick in ticks {
            app.world_mut().trigger(PacketReceived {
                entity: link,
                remote_tick: Tick(tick),
            });
        }
        app.world_mut().flush();
    }

    fn readout(app: &App) -> Option<(String, Color)> {
        let world = app.world();
        world
            .resource::<ServerTickGaps>()
            .cadence(world.resource::<Time<Real>>().elapsed())
            .map(|cadence| server_tick_readout(cadence, world.resource::<TickDuration>().0))
    }

    fn hz(gap: Duration) -> String {
        format!("{:.1} Hz", gap.as_secs_f64().recip())
    }

    /// Players see the nominal rate on evenly spaced tick advances, whether the client's frames
    /// are as long as a tick or shorter, so most frames read no tick at all.
    #[test]
    fn steady_ticks_are_healthy() {
        // Fixture: frames per tick.
        for frames_per_tick in [1, 2] {
            let (mut app, link, tick) = tick_app();
            assert_eq!(readout(&app), None, "no packet yet is a placeholder");
            // Fixture: two seconds of steady arrivals, crossing the retention boundary.
            let mut at = Duration::ZERO;
            let mut remote = 0;
            while at <= REFRESH_INTERVAL * 2 {
                frame(&mut app, link, at, [remote]);
                for part in 1..frames_per_tick {
                    frame(&mut app, link, at + tick * part / frames_per_tick, []);
                }
                remote += 1;
                at += tick;
            }
            assert_eq!(readout(&app), Some((hz(tick), TEXT)));
        }
    }

    /// Averages hide a freeze followed by catch-up. While the client keeps reading and no tick
    /// comes, players must see the server's stall while it is in progress and for the next refresh
    /// interval after receipt, then see recovery.
    #[test]
    fn a_stall_then_a_catch_up_burst_stays_visible() {
        let (mut app, link, tick) = tick_app();
        frame(&mut app, link, Duration::ZERO, [0]);
        frame(&mut app, link, tick, [1]);
        // Fixture: a freeze above the authored alarm, then a burst of coalesced ticks.
        let silent_frames = ALARM_EXCESS_TICKS + 2;
        for n in 1..=silent_frames {
            frame(&mut app, link, tick + tick * n, []);
        }
        let stalled = Some((hz(tick * silent_frames), NET_ALARM));
        assert_eq!(readout(&app), stalled, "silence is already a slowdown");
        let resumed = tick * (silent_frames + 2);
        frame(&mut app, link, resumed, silent_frames..=silent_frames + 2);
        assert_eq!(readout(&app), stalled, "catch-up must not erase the gap");
        let mut elapsed = tick;
        let mut remote = silent_frames + 3;
        while elapsed <= REFRESH_INTERVAL {
            frame(&mut app, link, resumed + elapsed, [remote]);
            assert_eq!(readout(&app), stalled, "retain for a panel refresh");
            elapsed += tick;
            remote += 1;
        }
        frame(&mut app, link, resumed + elapsed, [remote]);
        assert_eq!(
            readout(&app),
            Some((hz(tick), TEXT)),
            "expired gaps recover"
        );
    }

    /// A client frame that reads nothing while it runs is the client's stall: the ticks it then
    /// reads at once say nothing against the server, so the panel names the client and its frame,
    /// for the same refresh interval a server stall is kept, and never a low server rate. Frames
    /// shorter than a tick, so the last tick before the stall was read frames earlier. Mutants:
    /// timing the whole arrival gap against the server; calling a gap the client's only when no
    /// frame fell inside it.
    #[test]
    fn a_client_frame_stall_is_not_the_servers() {
        let (mut app, link, tick) = tick_app();
        frame(&mut app, link, Duration::ZERO, [0]);
        frame(&mut app, link, tick, [1]);
        frame(&mut app, link, tick + tick / 2, []);
        // Fixture: one client frame above the authored alarm; the server kept its cadence.
        let stall = tick * (ALARM_EXCESS_TICKS + 2);
        let resumed = tick + tick / 2 + stall;
        frame(&mut app, link, resumed, 2..=ALARM_EXCESS_TICKS + 3);
        let stalled = Some((
            format!("client {:.0} ms", stall.as_secs_f64() * 1000.0),
            NET_ALARM,
        ));
        assert_eq!(readout(&app), stalled);
        let mut elapsed = tick;
        let mut remote = ALARM_EXCESS_TICKS + 4;
        while elapsed <= REFRESH_INTERVAL {
            frame(&mut app, link, resumed + elapsed, [remote]);
            assert_eq!(readout(&app), stalled, "retain for a panel refresh");
            elapsed += tick;
            remote += 1;
        }
        frame(&mut app, link, resumed + elapsed, [remote]);
        assert_eq!(
            readout(&app),
            Some((hz(tick), TEXT)),
            "expired stalls recover"
        );
    }

    /// Only new server ticks end a player's observed gap. A reconnect may restart its tick
    /// numbers; neither the previous stall nor its newest tick may cross that boundary.
    #[test]
    fn duplicates_do_not_end_silence_and_a_reconnect_resets_history() {
        let (mut app, link, tick) = tick_app();
        frame(&mut app, link, Duration::ZERO, [10]);
        frame(&mut app, link, tick, [11]);
        let at = tick * 5;
        frame(&mut app, link, at, [11, 10]);
        assert_eq!(
            app.world()
                .resource::<ServerTickGaps>()
                .cadence(at)
                .map(|cadence| cadence.watched),
            Some(tick * 4)
        );
        app.world_mut().entity_mut(link).remove::<Connected>();
        frame(&mut app, link, at, [12]);
        assert_eq!(
            app.world().resource::<ServerTickGaps>().latest,
            Some((Tick(11), tick)),
            "disconnected packets cannot advance the displayed session"
        );
        app.world_mut().entity_mut(link).insert(Connected);
        app.world_mut().flush();
        assert_eq!(readout(&app), None, "a new session has no samples yet");
        frame(&mut app, link, at, [0]);
        frame(&mut app, link, at + tick, [1]);
        assert_eq!(readout(&app).unwrap().1, TEXT);
    }

    /// The panel reports the full buffered spread in milliseconds. Both readouts' colour bands
    /// follow the session's authority interval; fixture conversions pin the displayed unit.
    #[test]
    fn jitter_and_cadence_share_excess_budget_bands() {
        let (_, _, tick) = tick_app();
        assert_eq!(jitter_readout(Duration::ZERO, tick), ("+0 ms".into(), TEXT));
        for (millis, expected) in [(10, "+10 ms"), (68, "+68 ms")] {
            assert_eq!(
                jitter_readout(Duration::from_millis(millis), tick).0,
                expected
            );
        }
        assert_eq!(jitter_readout(tick / 2, tick).1, TEXT);
        for (excess, colour) in [
            (tick * WARN_EXCESS_TICKS, NET_WARN),
            (tick * ALARM_EXCESS_TICKS, NET_WARN),
            (
                tick * ALARM_EXCESS_TICKS + Duration::from_nanos(1),
                NET_ALARM,
            ),
        ] {
            assert_eq!(jitter_readout(excess, tick).1, colour);
            for cadence in [
                Cadence {
                    watched: excess + tick,
                    frame: tick,
                },
                Cadence {
                    watched: tick,
                    frame: excess + tick,
                },
            ] {
                assert_eq!(server_tick_readout(cadence, tick).1, colour);
            }
        }
    }
}
