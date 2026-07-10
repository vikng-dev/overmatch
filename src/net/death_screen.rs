//! Net-client death screen + respawn request. The player-side counterpart to the server's
//! [`crate::net::server`] respawn loop: when the player's OWN tank dies, show a minimal overlay and
//! let the player latch a respawn.
//!
//! **Death is read from replicated state, not decided locally.** The client's own tank is predicted
//! and carries `Remote` (it arrived by replication), so `net::protocol::apply_net_health` writes the
//! server's authoritative per-volume `NetHealth` onto it each tick, and the SAME damage-consequence
//! chain the server runs (`damage::kill_crew` → `damage::mark_dead_tanks` / `damage::process_cookoffs`)
//! then latches `TankKnockedOut` off that authoritative health. So "my crew are all dead" is a
//! server-driven fact the client merely observes — exactly the source of truth the requirement names.
//!
//! Net-only by construction: this module lives under the `net`-gated `net` module and is mounted
//! solely by `NetClientPlugin`. Single-player has no respawn flow.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use super::client::{MenuOverlay, command_idled};
use crate::command::TankCommand;
use crate::damage::TankKnockedOut;
use crate::tank::Controlled;
use crate::ui_font::UiFonts;

/// The respawn key. `R` for respawn — a graybox binding; a rebind screen later would move it into
/// `command::Bindings` like the drive/fire keys, but the death screen is the only reader for now.
const RESPAWN_KEY: KeyCode = KeyCode::KeyR;

/// Which message the overlay is showing. Stored on the node so a state change (dead → respawning)
/// rebuilds the text without churning the node every frame.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum DeathScreenNode {
    /// The player's crew are dead and no respawn has been requested — offer the key.
    Died,
    /// A respawn was requested and we are waiting out the round-trip for the fresh tank to replicate
    /// back and be re-claimed. Keep the overlay up so there is never a bare, tankless frame.
    Respawning,
}

/// How long a respawn request may sit unfulfilled before the overlay reverts from `RESPAWNING…` back
/// to `Died` and re-enables the request. The happy path re-acquires a live tank within ~1 RTT (well
/// under this), so the timeout only fires when the request genuinely stalls: the server asset-gate
/// skipped the spawn, or the link dropped while dead and no fresh tank will ever replicate back.
/// Without it the overlay sticks on `RESPAWNING…` forever with no way to re-request.
const RESPAWN_TIMEOUT_SECS: f64 = 5.0;

/// Tracks an in-flight respawn request. Latched the moment the player presses respawn, cleared once a
/// fresh live `Controlled` tank is re-acquired — bridging the ~1-RTT gap in which the old tank has
/// despawned but the new one has not yet replicated back (the window the overlay must NOT drop
/// through) — OR cleared by the [`RESPAWN_TIMEOUT_SECS`] wall-clock timeout when the request stalls.
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
    app.init_resource::<AwaitingRespawn>()
        .add_systems(Update, (request_respawn, toggle_death_screen).chain());
}

/// Drive the death overlay through the full death→respawn round-trip, so it never drops through the
/// tankless gap. The states, in order of precedence:
///   - **Respawning** — a respawn was requested (`AwaitingRespawn`) and no live own tank exists yet.
///     Covers both the still-dead-but-requested window AND the ~1-RTT gap after the old tank despawns
///     but before the new one replicates back and `net::client::claim_input_slot` re-grants
///     `Controlled`. This is the fix: the old code keyed solely on the dead tank existing, so the
///     overlay vanished the instant that tank despawned, leaving the player with neither death screen
///     nor a camera-bound tank until the replica arrived.
///   - **Died** — the crew are dead and no respawn has been requested yet: offer the key.
///   - **hidden** — a live own tank exists; clear `AwaitingRespawn` and take the overlay down.
///
/// A live own tank is `Controlled` without `TankKnockedOut` (the fresh tank spawns full-health, so its
/// health-derived `TankKnockedOut` is absent). Runs after [`request_respawn`] so a press this frame is
/// reflected immediately. Spawn/despawn only on a state change — not every frame.
fn toggle_death_screen(
    time: Res<Time>,
    dead_own: Query<(), (With<Controlled>, With<TankKnockedOut>)>,
    live_own: Query<(), (With<Controlled>, Without<TankKnockedOut>)>,
    overlay: Query<(Entity, &DeathScreenNode)>,
    mut awaiting: ResMut<AwaitingRespawn>,
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
    } else if !dead_own.is_empty() {
        Some(DeathScreenNode::Died)
    } else {
        None
    };

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
/// edge onto the `Controlled` tank's `TankCommand`; `net::client::feed_action_state` copies it into
/// the networked `ActionState` next tick, the wire carries it, and the server validates it against the
/// tank's own death before honoring it. Scoped `With<TankKnockedOut>` so the key does nothing while
/// alive (there is no overlay up then either). `just_pressed` is one press = one edge; `consume_edges`
/// (and, under starvation, the input bridge's `clear_edges`) drops it after one tick, so a held key
/// can't spam respawns.
///
/// Gated on the SAME idle condition `feed_action_state` uses ([`command_idled`], one source of truth):
/// while the command is being zeroed on the wire (menu open, or the window unfocused), a respawn edge
/// would be swallowed before it ever reaches the server, yet latching `AwaitingRespawn` would flip the
/// overlay to `RESPAWNING…` for the full timeout while nothing was actually sent. So while idled we do
/// not accept OR latch the request — R with the menu up does nothing, the overlay stays on `press R`,
/// and the player closes the menu first. `MenuOverlay` is mounted on the SAME windowed condition as
/// this module (both ride `NetClientPlugin` / the cursor-grab block), so it is normally present; the
/// `Option` is a defensive fallback that treats "menu absent" as "not idled".
fn request_respawn(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    menu: Option<Res<MenuOverlay>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut dead_own: Query<&mut TankCommand, (With<Controlled>, With<TankKnockedOut>)>,
    mut awaiting: ResMut<AwaitingRespawn>,
) {
    if !keys.just_pressed(RESPAWN_KEY) {
        return;
    }
    // The wire is zeroing the command out from under us (menu / unfocused) — a latched edge would
    // never be sent. Don't accept the request; the overlay keeps showing "press R".
    if menu
        .as_deref()
        .is_some_and(|menu| command_idled(menu, window.focused))
    {
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

/// Spawn the graybox death overlay: a dim full-screen backdrop with centered white text, its message
/// chosen by `state`. Deliberately minimal; mirrors the menu overlay's node/text shape in
/// `net::client` so the two read as one UI family.
fn spawn_death_screen(commands: &mut Commands, state: DeathScreenNode, font: &Handle<Font>) {
    let text = match state {
        DeathScreenNode::Died => "YOU DIED\npress R to respawn",
        DeathScreenNode::Respawning => "RESPAWNING…",
    };
    commands
        .spawn((
            state,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.15, 0.0, 0.0, 0.6)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(text),
                TextFont {
                    // SemiBold: a big all-caps death overlay.
                    font: font.into(),
                    font_size: FontSize::Px(48.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
        });
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
}
