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

/// The full-screen death overlay node, so its presence can be toggled by whether the player's own
/// tank is currently knocked out. One at a time (spawned/despawned by [`toggle_death_screen`]).
#[derive(Component)]
struct DeathScreenNode;

pub fn plugin(app: &mut App) {
    app.add_systems(Update, (toggle_death_screen, request_respawn));
}

/// Show the death overlay exactly while the player's own (`Controlled`) tank carries `TankKnockedOut`,
/// and take it down once a fresh tank is re-acquired (the respawned tank is not knocked out, and
/// `net::client::claim_input_slot` moves `Controlled` onto it). Spawn/despawn on the transition only —
/// not every frame — so the text node is built once per death, matching the menu overlay's idiom.
fn toggle_death_screen(
    dead_own: Query<(), (With<Controlled>, With<TankKnockedOut>)>,
    overlay: Query<Entity, With<DeathScreenNode>>,
    mut commands: Commands,
) {
    let is_dead = !dead_own.is_empty();
    let shown = !overlay.is_empty();
    match (is_dead, shown) {
        (true, false) => spawn_death_screen(&mut commands),
        (false, true) => {
            for node in &overlay {
                commands.entity(node).despawn();
            }
        }
        _ => {}
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
) {
    if !keys.just_pressed(RESPAWN_KEY) {
        return;
    }
    for mut command in &mut dead_own {
        command.respawn = true;
    }
}

/// Spawn the graybox death overlay: a dim full-screen backdrop with centered white text. Deliberately
/// minimal (the friend-fight punch-list asks only for "YOU DIED — press R to respawn"); mirrors the
/// menu overlay's node/text shape in `net::client` so the two read as one UI family.
fn spawn_death_screen(commands: &mut Commands) {
    commands
        .spawn((
            DeathScreenNode,
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
                Text::new("YOU DIED\npress R to respawn"),
                TextFont {
                    font_size: FontSize::Px(48.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
        });
}
