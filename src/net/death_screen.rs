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

use crate::command::TankCommand;
use crate::damage::TankKnockedOut;
use crate::tank::Controlled;

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

/// Latched true the moment the player presses respawn, cleared once a fresh live `Controlled` tank is
/// re-acquired. Bridges the ~1-RTT gap in which the old tank has despawned but the new one has not yet
/// replicated back — the window the overlay must NOT drop through.
#[derive(Resource, Default)]
struct AwaitingRespawn(bool);

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
    dead_own: Query<(), (With<Controlled>, With<TankKnockedOut>)>,
    live_own: Query<(), (With<Controlled>, Without<TankKnockedOut>)>,
    overlay: Query<(Entity, &DeathScreenNode)>,
    mut awaiting: ResMut<AwaitingRespawn>,
    mut commands: Commands,
) {
    let has_live = !live_own.is_empty();
    if has_live {
        // The fresh tank is claimed and alive — the round-trip is over.
        awaiting.0 = false;
    }

    let desired = if awaiting.0 && !has_live {
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
        spawn_death_screen(&mut commands, state);
    }
}

/// Latch the respawn edge on the player's own dead tank when the respawn key is pressed. Writes the
/// edge onto the `Controlled` tank's `TankCommand`; `net::client::feed_action_state` copies it into
/// the networked `ActionState` next tick, the wire carries it, and the server validates it against the
/// tank's own death before honoring it. Scoped `With<TankKnockedOut>` so the key does nothing while
/// alive (there is no overlay up then either). `just_pressed` is one press = one edge; `consume_edges`
/// (and, under starvation, the input bridge's `clear_edges`) drops it after one tick, so a held key
/// can't spam respawns.
fn request_respawn(
    keys: Res<ButtonInput<KeyCode>>,
    mut dead_own: Query<&mut TankCommand, (With<Controlled>, With<TankKnockedOut>)>,
    mut awaiting: ResMut<AwaitingRespawn>,
) {
    if !keys.just_pressed(RESPAWN_KEY) {
        return;
    }
    for mut command in &mut dead_own {
        command.respawn = true;
        // Latch the wait so the overlay switches to "respawning…" and stays up across the round-trip
        // even after the old tank despawns (`toggle_death_screen`). Scoped to the dead-tank query, so
        // it can only latch when there is actually a dead own tank to respawn.
        awaiting.0 = true;
    }
}

/// Spawn the graybox death overlay: a dim full-screen backdrop with centered white text, its message
/// chosen by `state`. Deliberately minimal; mirrors the menu overlay's node/text shape in
/// `net::client` so the two read as one UI family.
fn spawn_death_screen(commands: &mut Commands, state: DeathScreenNode) {
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
                    font_size: FontSize::Px(48.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
        });
}
