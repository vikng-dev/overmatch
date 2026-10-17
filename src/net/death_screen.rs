//! Net-client death overlay and respawn request.
//!
//! Life state comes from public `NetTankStatus`; private crew state must not drive an observer's UI.

use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;

use crate::command::TankCommand;
use crate::damage::TankKnockedOut;
use crate::damage_cam::pen_cam::game::{GameDeathScreen, KillCamGate};
use crate::flow::{Flow, FlowScoped, InMatchSystems, MatchResourceExt};
use crate::overlay::{self, Overlay, Overlays};
use crate::state::PlayerInputSet;
use crate::tank::Controlled;
use crate::ui_font::UiFonts;

/// Temporary respawn binding.
const RESPAWN_KEY: KeyCode = KeyCode::KeyR;

/// Message represented by the status node.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum DeathScreenNode {
    /// Dead with no outstanding request.
    Died,
    /// Request sent; keep the overlay until a live controlled tank arrives or the request expires.
    Respawning,
}

/// Bound an unfulfilled request so the player can retry.
const RESPAWN_TIMEOUT_SECS: f64 = 5.0;

/// In-flight request state. The overlay remains visible through the dead-to-replacement gap.
#[derive(Resource, Default)]
struct AwaitingRespawn {
    /// True from the respawn press until a live tank is re-acquired or the request times out.
    active: bool,
    /// Wall-clock deadline (`Time::elapsed_secs_f64`) past which an unfulfilled request reverts to
    /// `Died`. Meaningful only while `active`.
    deadline: f64,
}

/// Whether an in-flight respawn request has timed out: still awaiting, no live tank has arrived, and
/// the wall-clock deadline has passed. Pure so the revert threshold is unit-testable without an app.
fn respawn_timed_out(awaiting: &AwaitingRespawn, has_live: bool, now: f64) -> bool {
    awaiting.active && !has_live && now >= awaiting.deadline
}

pub fn plugin(app: &mut App) {
    crate::flow::require(app, "net::death_screen");
    app.init_resource::<GameDeathScreen>()
        .init_match_resource::<AwaitingRespawn>()
        .add_systems(OnEnter(Flow::InMatch), spawn_death_status_line)
        .add_systems(Update, systems().in_set(InMatchSystems));
}

/// The death screen's `Update` systems, placed among the overlay sets. Both read the kill cam's
/// [`KillCamGate`], written before them (`KillCamGateSet`).
fn systems() -> ScheduleConfigs<ScheduleSystem> {
    (
        // All overlay reconcilers must read the same declared state. This is a state-driven
        // owner (crew + respawn deadline), not a gated toggle: it reads nothing.
        toggle_death_screen.in_set(overlay::OverlaySet::Declare),
        // Re-check the reconciled overlay input gate before latching a respawn edge — after
        // `Toggle`, so a menu or spawn map opened this frame already counts.
        request_respawn
            .in_set(PlayerInputSet)
            .after(overlay::OverlaySet::Toggle),
        // The death STATUS LINE is the one one-scrim exemption; its tiny reconciler runs after
        // `Toggle` alongside the generic `overlay::apply_overlay_visibility`.
        apply_death_status_line.after(overlay::OverlaySet::Toggle),
    )
        .into_configs()
}

/// The death screen's resources and `Update` systems without the match flow, for a composed
/// schedule under test.
#[cfg(test)]
pub(crate) fn mount_bare(app: &mut App) {
    app.init_resource::<GameDeathScreen>()
        .init_resource::<AwaitingRespawn>()
        .add_systems(Update, systems());
}

/// Whether the death screen shows "YOU DIED" (not "RESPAWNING…").
#[cfg(test)]
pub(crate) fn shows_died(world: &mut World) -> bool {
    world
        .query::<&DeathScreenNode>()
        .iter(world)
        .any(|node| *node == DeathScreenNode::Died)
}

/// Declare respawning until a live controlled tank arrives or the request expires; otherwise declare
/// death only for a controlled tank carrying `TankKnockedOut`, once the kill cam lets it
/// ([`KillCamGate::holds_death_screen`]).
#[allow(clippy::too_many_arguments)]
fn toggle_death_screen(
    time: Res<Time>,
    gate: Option<Res<KillCamGate>>,
    dead_own: Query<(), (With<Controlled>, With<TankKnockedOut>)>,
    live_own: Query<(), (With<Controlled>, Without<TankKnockedOut>)>,
    overlay: Query<(Entity, &DeathScreenNode)>,
    mut awaiting: ResMut<AwaitingRespawn>,
    mut overlays: ResMut<Overlays>,
    fonts: Res<UiFonts>,
    mut commands: Commands,
) {
    let has_live = !live_own.is_empty();
    if has_live {
        // The fresh tank is claimed and alive — the round-trip is over.
        awaiting.active = false;
    } else if respawn_timed_out(&awaiting, has_live, time.elapsed_secs_f64()) {
        // The request stalled past the timeout (asset-gate skip, or a drop while dead): drop the
        // wait so the overlay reverts to `Died` and the player can press R again. The dead own tank
        // still exists in that case, so precedence below falls to `Died`.
        awaiting.active = false;
        info!("client: respawn request timed out — reverting to the death screen");
    }

    let desired = if awaiting.active && !has_live {
        Some(DeathScreenNode::Respawning)
    } else if !dead_own.is_empty() && !gate.is_some_and(|gate| gate.holds_death_screen) {
        Some(DeathScreenNode::Died)
    } else {
        None
    };

    // Declare `Death` presence into the overlay authority every frame (idempotent, self-healing): the
    // death overlay is latched whenever there is a message to show. The scrim/visibility consequence is
    // `apply_death_visibility`'s job; existence (spawn/despawn below) stays this system's.
    overlays.declare(Overlay::Death, desired.is_some());

    let shown = overlay.single().ok().map(|(_, state)| *state);
    if desired == shown {
        return;
    }
    for (node, _) in &overlay {
        commands.entity(node).despawn();
    }
    if let Some(state) = desired {
        spawn_death_screen(&mut commands, state, &fonts.hud);
    }
}

/// Latch the respawn edge on the player's own dead tank when the respawn key is pressed. Writes the
/// edge onto the `Controlled` tank's `TankCommand`; the next frame's `command::DeviceCommand` takes
/// it, `net::client::feed_action_state` files it into the networked `ActionState` on that frame's
/// first tick, the wire carries it, and the server validates it against the
/// tank's own death before honoring it. Scoped `With<TankKnockedOut>` so the key does nothing while
/// alive (there is no overlay up then either). `just_pressed` is one press = one edge; `consume_edges`
/// (and, under starvation, the input bridge's `clear_edges`) drops it after one tick, so a held key
/// can't spam respawns.
///
/// Runs in [`PlayerInputSet`] (see the registration comment): while the cursor is released — menu
/// open, window unfocused — this system doesn't run at all, so a respawn edge can never be latched
/// into a command `feed_action_state` is zeroing on the wire. R with the menu up does nothing and the
/// overlay stays on `press R`. The `overlay::input_blocked` re-check below closes the ONE remaining
/// gap the cursor gate can't: the frame a menu opens in the SAME tick R is pressed, before the cursor
/// owner has released the cursor — refusing the latch there is what prevents a phantom `RESPAWNING…`.
///
/// Inert while the knockout is younger than the kill cam's input dead-zone
/// ([`KillCamGate::settling`]), so a key held through the hit does not respawn.
fn request_respawn(
    time: Res<Time>,
    gate: Option<Res<KillCamGate>>,
    keys: Res<ButtonInput<KeyCode>>,
    overlays: Res<Overlays>,
    mut dead_own: Query<&mut TankCommand, (With<Controlled>, With<TankKnockedOut>)>,
    mut awaiting: ResMut<AwaitingRespawn>,
) {
    if !keys.just_pressed(RESPAWN_KEY) || gate.is_some_and(|gate| gate.settling) {
        return;
    }
    // Same-frame guard against a phantom `RESPAWNING…`: a menu opened THIS frame (R+Esc together) or
    // an alt-tab (`focus_declare` declares the menu) is already in the reconciled set — we run after
    // `OverlaySet::Toggle`, so both the declarers AND Esc have written — but `PlayerInputSet`'s cursor
    // gate hasn't caught up yet (the cursor owner
    // releases later this frame). If input is blocked, refuse: do not write the wire `respawn` edge
    // (`feed_action_state` would zero it anyway) nor latch `AwaitingRespawn`, so the overlay never
    // sticks on `RESPAWNING…` for a request the server never sees. `window_focused = true` because a
    // focus loss is itself represented in the set as the declared menu.
    if overlay::input_blocked(&overlays, true) {
        return;
    }
    for mut command in &mut dead_own {
        command.respawn = true;
        // Latch the wait so the overlay switches to "respawning…" and stays up across the round-trip
        // even after the old tank despawns (`toggle_death_screen`). Scoped to the dead-tank query, so
        // it can only latch when there is actually a dead own tank to respawn. Arm the timeout so a
        // request that never yields a live tank reverts to `Died` instead of sticking forever.
        awaiting.active = true;
        awaiting.deadline = time.elapsed_secs_f64() + RESPAWN_TIMEOUT_SECS;
    }
}

/// Spawn the graybox death overlay: centered white text, its message chosen by `state`, over the
/// death tint. Shares `ui_font::spawn_overlay` with the menu, connect and pause overlays so the
/// family reads as one, but draws no backdrop of its own on Death's z: its wash is the tint child
/// ([`overlay::death_tint`]), which escapes to the bottom UI rung, so the HUD and this text stay
/// untinted on top of it. The tint inherits the overlay node's visibility, so the one-scrim rule
/// hides it with the death screen. The state enum doubles as the node's marker + despawn handle.
fn spawn_death_screen(commands: &mut Commands, state: DeathScreenNode, font: &Handle<Font>) {
    let text = match state {
        DeathScreenNode::Died => "YOU DIED\npress R to respawn",
        DeathScreenNode::Respawning => "RESPAWNING…",
    };
    // Two markers: the stateful `DeathScreenNode` enum (text form + despawn handle) and the shared
    // `OverlayNode(Death)`, which stamps the one-scrim `GlobalZIndex` via its hook and hands the full
    // backdrop's visibility to `overlay::apply_overlay_visibility` (the status line is exempt, below).
    let screen = crate::ui_font::spawn_overlay(
        commands,
        font,
        (
            state,
            overlay::OverlayNode(Overlay::Death),
            // An overlay node whose life is a death inside one match.
            DespawnOnExit(Flow::InMatch),
        ),
        text,
        (),
        None,
    );
    commands.entity(screen).with_child(overlay::death_tint());
}

/// A thin, top-pinned, NON-interactive status line shown only while the death state is latched but the
/// menu is drawn on top of it ([`overlay::death_status_line`]): "DEAD — respawn on menu close". Exempt
/// from the one-scrim suppression — it draws no backdrop, and its `GlobalZIndex` (Death's, above the
/// menu's) keeps it legible THROUGH the menu, so the player knows the respawn key is merely gated
/// (menu open, cursor released) rather than gone. We never show "press R" while R can't work; this line
/// is what stands in for it. Spawned once per match and visibility-swapped.
#[derive(Component)]
struct DeathStatusLine;

fn spawn_death_status_line(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn((
            FlowScoped(Flow::InMatch),
            DeathStatusLine,
            Node {
                width: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                top: Val::Px(12.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            GlobalZIndex(Overlay::Death.zindex()),
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("DEAD — respawn on menu close"),
                TextFont {
                    // SemiBold: a terse all-caps status line.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.4, 0.3)),
            ));
        });
}

/// Reconcile ONLY the death STATUS LINE — the one one-scrim exemption. The full backdrop node is a
/// shared `overlay::OverlayNode(Death)`, so its visibility (red backdrop + "YOU DIED / press R", shown
/// exactly while Death owns the scrim) is `overlay::apply_overlay_visibility`'s job now, and its
/// existence (spawn/despawn on the death STATE) stays `toggle_death_screen`'s. This system only swaps
/// the thin status line: `Visible` exactly while the death state is latched but the menu is drawn on
/// top of it ([`overlay::death_status_line`]) — "DEAD — respawn on menu close", legible THROUGH the
/// menu — and `Hidden` otherwise. Runs after `OverlaySet::Toggle` so it reads the fully-reconciled set
/// (the declarers AND the Esc / `M` toggles have both written by then).
fn apply_death_status_line(
    overlays: Res<Overlays>,
    mut status: Query<&mut Visibility, With<DeathStatusLine>>,
) {
    if let Ok(mut vis) = status.single_mut() {
        vis.set_if_neq(if overlay::death_status_line(&overlays) {
            Visibility::Visible
        } else {
            Visibility::Hidden
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn awaiting(active: bool, deadline: f64) -> AwaitingRespawn {
        AwaitingRespawn { active, deadline }
    }

    /// The timeout fires only once the deadline is genuinely past — before it, an in-flight request
    /// keeps the overlay on `RESPAWNING…`.
    #[test]
    fn not_timed_out_before_deadline() {
        let a = awaiting(true, 10.0);
        assert!(!respawn_timed_out(&a, false, 9.999), "just before deadline");
        assert!(respawn_timed_out(&a, false, 10.0), "at the deadline");
        assert!(respawn_timed_out(&a, false, 12.0), "past the deadline");
    }

    /// A live tank arriving cancels the timeout regardless of the clock: the round-trip succeeded,
    /// so there is nothing to revert.
    #[test]
    fn live_tank_never_times_out() {
        let a = awaiting(true, 10.0);
        assert!(!respawn_timed_out(&a, true, 100.0));
    }

    /// With no request in flight there is nothing to time out — the timeout can't manufacture a
    /// revert out of an idle death screen.
    #[test]
    fn inactive_never_times_out() {
        let a = awaiting(false, 0.0);
        assert!(!respawn_timed_out(&a, false, 100.0));
    }

    /// The node's draw layer as bevy_ui stacks it: its own `GlobalZIndex`, else its nearest
    /// ancestor's, else 0 (a root without one).
    fn layer(world: &World, mut node: Entity) -> i32 {
        loop {
            if let Some(z) = world.get::<GlobalZIndex>(node) {
                return z.0;
            }
            match world.get::<ChildOf>(node) {
                Some(parent) => node = parent.parent(),
                None => return 0,
            }
        }
    }

    /// Through the real death-screen and HUD spawners, with the own tank knocked out: exactly one
    /// node carries the death tint's colour, and it draws below every other UI node — the HUD and
    /// the death screen's own — so it tints the world and no UI element.
    #[test]
    fn the_death_tint_draws_beneath_every_ui_node() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Overlays>()
            .init_resource::<AwaitingRespawn>()
            .insert_resource(UiFonts {
                hud: Handle::default(),
                body: Handle::default(),
            })
            .add_systems(
                Startup,
                (
                    crate::crew_ui::spawn_crew_bar,
                    crate::crew_ui::spawn_status_panel,
                ),
            )
            .add_systems(Update, toggle_death_screen);
        app.world_mut().spawn((
            Controlled,
            TankKnockedOut {
                reason: crate::damage::KnockoutReason::CrewLoss,
            },
        ));
        app.update();

        let world = app.world_mut();
        let mut nodes = world.query_filtered::<(Entity, Option<&BackgroundColor>), With<Node>>();
        let nodes: Vec<_> = nodes
            .iter(world)
            .map(|(node, fill)| {
                let tint = fill.is_some_and(|fill| fill.0 == crate::ui_font::DEATH_TINT);
                (tint, layer(world, node))
            })
            .collect();
        let tints: Vec<i32> = nodes.iter().filter(|n| n.0).map(|n| n.1).collect();
        let rest: Vec<i32> = nodes.iter().filter(|n| !n.0).map(|n| n.1).collect();
        assert_eq!(tints.len(), 1, "one death tint: {tints:?}");
        assert!(
            world
                .query::<&DeathScreenNode>()
                .iter(world)
                .eq([&DeathScreenNode::Died]),
            "fixture: the death screen is up",
        );
        assert!(rest.len() >= 3, "fixture: HUD and death text: {rest:?}");
        assert!(
            rest.iter().all(|&other| other > tints[0]),
            "the tint at {} must draw below every UI node: {rest:?}",
            tints[0],
        );
    }
}
