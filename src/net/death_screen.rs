//! Net-client death screen + respawn request. The player-side counterpart to the server's
//! [`crate::net::server`] respawn loop: when the player's OWN tank dies, show a minimal overlay and
//! let the player latch a respawn.
//!
//! **Death is read from replicated state, not decided locally.** The client's own tank is predicted
//! and carries `Remote` (it arrived by replication), so `net::protocol::apply_net_crew` realizes the
//! server's authoritative `NetCrew` snapshot onto it each tick and DERIVES `TankKnockedOut`
//! idempotently from it (a pure function of the snapshot — no monotonic local latch fed by
//! re-assertable HP). The deciding damage chain (`damage::kill_crew`/`mark_dead_tanks`/
//! `process_cookoffs`) is authority-only and does not run on the client. So "my crew are all dead" is
//! a server-driven fact the client merely observes — exactly the source of truth the requirement
//! names.
//!
//! Net-only by construction: this module lives under the `net`-gated `net` module and is mounted
//! solely by `NetClientPlugin`. Single-player has no respawn flow.

use bevy::prelude::*;

use crate::command::TankCommand;
use crate::damage::TankKnockedOut;
use crate::overlay::{self, Overlay, Overlays};
use crate::state::PlayerInputSet;
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
        .add_systems(Startup, spawn_death_status_line)
        .add_systems(
            Update,
            (
                // Presence declaration for the death overlay joins the shared `Declare` phase, so the
                // generic scrim reconciler AND the status-line one below read ONE fully-declared set
                // (the ordering fix: death and menu visuals can no longer be computed from different
                // generations of the set on the Esc edge while dead).
                toggle_death_screen.in_set(overlay::OverlaySet::Declare),
                // `request_respawn` consumes a player keypress, so it carries `PlayerInputSet`'s cursor
                // license. It ALSO runs after `Declare` and re-checks `overlay::input_blocked` on the
                // reconciled set, so a respawn edge pressed the SAME frame a menu opens (R+Esc, or an
                // alt-tab) — before `PlayerInputSet`'s cursor gate has caught up — is refused rather
                // than latched into a command `feed_action_state` is about to zero (the phantom
                // `RESPAWNING…` fix).
                request_respawn
                    .in_set(PlayerInputSet)
                    .after(overlay::OverlaySet::Declare),
                // The death STATUS LINE is the one one-scrim exemption; its tiny reconciler runs after
                // `Declare` alongside the generic `overlay::apply_overlay_visibility`.
                apply_death_status_line.after(overlay::OverlaySet::Declare),
            ),
        );
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
/// health-derived `TankKnockedOut` is absent). Runs in [`overlay::OverlaySet::Declare`] (so its `Death`
/// declaration is reconciled before the scrim/status-line reconcilers read the set); a respawn pressed
/// this frame by [`request_respawn`] — which now runs AFTER `Declare` — is reflected the next frame.
/// Spawn/despawn only on a state change — not every frame.
fn toggle_death_screen(
    time: Res<Time>,
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
    } else if !dead_own.is_empty() {
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
/// edge onto the `Controlled` tank's `TankCommand`; `net::client::feed_action_state` copies it into
/// the networked `ActionState` next tick, the wire carries it, and the server validates it against the
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
fn request_respawn(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    overlays: Res<Overlays>,
    mut dead_own: Query<&mut TankCommand, (With<Controlled>, With<TankKnockedOut>)>,
    mut awaiting: ResMut<AwaitingRespawn>,
) {
    if !keys.just_pressed(RESPAWN_KEY) {
        return;
    }
    // Same-frame guard against a phantom `RESPAWNING…`: a menu opened THIS frame (R+Esc together) or
    // an alt-tab (`focus_declare` declares the menu) is already in the reconciled set — we run after
    // `OverlaySet::Declare` — but `PlayerInputSet`'s cursor gate hasn't caught up yet (the cursor owner
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

/// Spawn the graybox death overlay: a dim full-screen backdrop with centered white text, its message
/// chosen by `state`. Deliberately minimal; shares `ui_font::spawn_overlay` with the menu, connect,
/// and pause overlays so the family reads as one. The backdrop carries a red tint (its only departure
/// from the others' black) and the state enum doubles as the node's marker + despawn handle. Stamped
/// with the one-scrim contract's `GlobalZIndex` (Death sits above the menu's z so the status line —
/// which rides the same z — shows through the menu, though the full screen itself is hidden then).
fn spawn_death_screen(commands: &mut Commands, state: DeathScreenNode, font: &Handle<Font>) {
    let text = match state {
        DeathScreenNode::Died => "YOU DIED\npress R to respawn",
        DeathScreenNode::Respawning => "RESPAWNING…",
    };
    // Two markers: the stateful `DeathScreenNode` enum (text form + despawn handle) and the shared
    // `OverlayNode(Death)`, which stamps the one-scrim `GlobalZIndex` via its hook and hands the full
    // backdrop's visibility to `overlay::apply_overlay_visibility` (the status line is exempt, below).
    crate::ui_font::spawn_overlay(
        commands,
        font,
        (state, overlay::OverlayNode(Overlay::Death)),
        text,
        (),
        Some(Color::srgba(0.15, 0.0, 0.0, 0.6)),
    );
}

/// A thin, top-pinned, NON-interactive status line shown only while the death state is latched but the
/// menu is drawn on top of it ([`overlay::death_status_line`]): "DEAD — respawn on menu close". Exempt
/// from the one-scrim suppression — it draws no backdrop, and its `GlobalZIndex` (Death's, above the
/// menu's) keeps it legible THROUGH the menu, so the player knows the respawn key is merely gated
/// (menu open, cursor released) rather than gone. We never show "press R" while R can't work; this line
/// is what stands in for it. Persistent (spawned once, visibility-swapped) like the connect/menu nodes.
#[derive(Component)]
struct DeathStatusLine;

fn spawn_death_status_line(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn((
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
/// menu — and `Hidden` otherwise. Runs after `OverlaySet::Declare` so it reads the fully-reconciled set.
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
}
