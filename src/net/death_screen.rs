//! Net-client death overlay and respawn request.
//!
//! Life state comes from public `NetTankStatus`; private crew state must not drive an observer's UI.
//! The respawn's opening comes from the server's replicated `score::Scoreboard`: the overlay
//! counts down to it and the key is inert before it.
//! Owner-private `RespawnState` names waiting/refusal independently of that opening.

use bevy::ecs::query::QuerySingleError;
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::{ScheduleSystem, SystemParam};
use bevy::prelude::*;

use super::battle::BattleState;
use super::disclosure::{RespawnState, SpawnPickRefusal};
use super::score::{Opening, Scoreboard};
use crate::command::TankCommand;
use crate::damage::TankKnockedOut;
use crate::damage_cam::pen_cam::game::{GameDeathScreen, KillCamGate};
use crate::flow::{Flow, FlowScoped, InMatchSystems, MatchResourceExt};
use crate::overlay::{self, Overlay, Overlays};
use crate::state::PlayerInputSet;
use crate::tank::Controlled;
use crate::ui_font::UiFonts;
use crate::{CombatantId, CombatantLife, ShotClock};

/// Temporary respawn binding.
const RESPAWN_KEY: KeyCode = KeyCode::KeyR;

/// Message represented by the status node.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
enum DeathScreenNode {
    /// Dead with no outstanding request; the respawn opens in `opens_in_s` whole seconds, `None`
    /// once the key may be pressed ([`OwnRespawn::seconds_left`]).
    Died {
        opens_in_s: Option<u32>,
        pick_refusal: Option<SpawnPickRefusal>,
    },
    /// Request sent; wait for authoritative admission, a live replacement, or the retry deadline.
    Respawning,
    WaitingForSpace {
        pick_refusal: Option<SpawnPickRefusal>,
    },
}

/// The own knocked-out life's respawn opening, as the replicated scoreboard names it.
#[derive(SystemParam)]
struct OwnRespawn<'w, 's> {
    own: Query<
        'w,
        's,
        (&'static CombatantId, &'static CombatantLife),
        (With<Controlled>, With<TankKnockedOut>),
    >,
    boards: Query<'w, 's, &'static Scoreboard>,
    state: Query<'w, 's, Option<&'static RespawnState>, With<Controlled>>,
    battles: Query<'w, 's, &'static BattleState>,
    clock: Option<Res<'w, ShotClock>>,
    time: Res<'w, Time<Fixed>>,
}

impl OwnRespawn<'_, '_> {
    fn state(&self) -> RespawnState {
        match self.state.single() {
            Ok(Some(state)) => *state,
            // Before the owner snapshot arrives, or between links, there is no admission to show.
            Ok(None) | Err(QuerySingleError::NoEntities(_)) => RespawnState::default(),
            Err(QuerySingleError::MultipleEntities(_)) => panic!("a client controls one tank root"),
        }
    }

    /// The replicated phase closes respawn outside combat, even while the old tank's despawn is
    /// still in flight. Without a Battle yet (or in an isolated death-view fixture), the authority
    /// still decides a request against the life; absence cannot invent a phase.
    fn between_combats(&self) -> bool {
        match self.battles.single() {
            Ok(battle) => !battle.is_combat(),
            Err(QuerySingleError::NoEntities(_)) => false,
            Err(QuerySingleError::MultipleEntities(_)) => {
                panic!("a replica has one Battle; disconnect cleanup removes the previous link")
            }
        }
    }

    /// Whole seconds, rounded up, until the own knocked-out life's respawn opens on the client's
    /// local tick (`ShotClock`, at or behind the tick a request pressed now is filed for, which is
    /// the tick the server reads it on), so a request sent at zero is never early.
    /// `None` when the key may be pressed: the opening is past, or no opening for
    /// this life has arrived yet (no scoreboard, or one not yet updated for this knockout), when
    /// the server decides the request.
    fn seconds_left(&self) -> Option<u32> {
        let (&combatant, &life) = match self.own.single() {
            Ok(own) => own,
            // Alive, or no tank yet: nothing opens.
            Err(QuerySingleError::NoEntities(_)) => return None,
            Err(QuerySingleError::MultipleEntities(_)) => {
                panic!("a client controls one tank: `Controlled` marks only its own root")
            }
        };
        let board = match self.boards.single() {
            Ok(board) => board,
            // Before the first snapshot, or between links.
            Err(QuerySingleError::NoEntities(_)) => return None,
            Err(QuerySingleError::MultipleEntities(_)) => panic!(
                "a replica has one scoreboard; disconnect cleanup removes the old link's \
                 replicated entities before a replacement arrives"
            ),
        };
        let clock = self
            .clock
            .as_ref()
            .expect("a replicated scoreboard arrives only where the network clock is published");
        match board.opening(combatant, life, clock.0)? {
            Opening::Open => None,
            Opening::In { ticks } => {
                let seconds = f64::from(ticks) * self.time.timestep().as_secs_f64();
                Some(seconds.ceil() as u32)
            }
        }
    }
}

/// Resolution: bound the unacknowledged request wait so a dropped input can be retried.
/// An authoritative placement wait takes over without a timeout; a pick refusal alone does not
/// acknowledge the respawn request.
const RESPAWN_TIMEOUT_SECS: f64 = 5.0;

/// In-flight request state. The overlay remains visible through the dead-to-replacement gap.
#[derive(Resource, Default)]
struct AwaitingRespawn {
    /// True while the request awaits authoritative admission or a replacement.
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

/// The death screen's `Update` systems, placed among the overlay sets. The declarer and the
/// respawn key read the kill cam's [`KillCamGate`], written before them (`KillCamGateSet`).
fn systems() -> ScheduleConfigs<ScheduleSystem> {
    (
        // All overlay reconcilers must read the same declared state. This is a state-driven
        // owner (crew + respawn deadline + the kill cam's gate), not a gated toggle.
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
        .any(|node| matches!(node, DeathScreenNode::Died { .. }))
}

/// Declare respawning until a live controlled tank arrives or the request expires; otherwise declare
/// death only for a controlled tank carrying `TankKnockedOut`, once the kill cam lets it
/// ([`KillCamGate::holds_death_screen`]).
#[expect(
    clippy::too_many_arguments,
    reason = "a system spanning the clock, the kill cam gate, two controlled-tank states, the respawn's opening, the overlay, the respawn wait, the overlay registry and fonts"
)]
fn toggle_death_screen(
    time: Res<Time>,
    gate: Option<Res<KillCamGate>>,
    dead_own: Query<(), (With<Controlled>, With<TankKnockedOut>)>,
    live_own: Query<(), (With<Controlled>, Without<TankKnockedOut>)>,
    opening: OwnRespawn,
    overlay: Query<(Entity, &DeathScreenNode)>,
    mut awaiting: ResMut<AwaitingRespawn>,
    mut overlays: ResMut<Overlays>,
    fonts: Res<UiFonts>,
    mut commands: Commands,
) {
    let has_live = !live_own.is_empty();
    let between_combats = opening.between_combats();
    let server_state = opening.state();
    if has_live || between_combats || server_state.waiting {
        // Authority admission replaces the local wait; a live root or phase boundary completes it.
        awaiting.active = false;
    } else if respawn_timed_out(&awaiting, has_live, time.elapsed_secs_f64()) {
        // No authority response arrived within the retry horizon; the player can send again.
        awaiting.active = false;
        info!("client: respawn request timed out — reverting to the death screen");
    }

    let desired = if between_combats {
        None
    } else if awaiting.active && !has_live {
        Some(DeathScreenNode::Respawning)
    } else if !dead_own.is_empty() && !gate.is_some_and(|gate| gate.holds_death_screen) {
        Some(if server_state.waiting {
            DeathScreenNode::WaitingForSpace {
                pick_refusal: server_state.pick_refusal,
            }
        } else {
            DeathScreenNode::Died {
                opens_in_s: opening.seconds_left(),
                pick_refusal: server_state.pick_refusal,
            }
        })
    } else {
        None
    };

    // Declare `Death` presence into the overlay authority every frame (idempotent, self-healing): the
    // death overlay is latched whenever there is a message to show. The scrim/visibility consequence is
    // `apply_death_visibility`'s job; existence (spawn/despawn below) stays this system's.
    overlays.declare(Overlay::Death, desired.is_some());

    let shown = match overlay.single() {
        Ok((_, state)) => Some(*state),
        Err(QuerySingleError::NoEntities(_)) => None, // No death state is currently declared.
        Err(QuerySingleError::MultipleEntities(_)) => {
            panic!("one death-screen node represents the declared state")
        }
    };
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
/// ([`KillCamGate::settling`]), so a key held through the hit does not respawn, and while the
/// respawn has not opened ([`OwnRespawn::seconds_left`]), so a request the server would refuse is
/// not sent while the overlay counts down.
fn request_respawn(
    time: Res<Time>,
    gate: Option<Res<KillCamGate>>,
    keys: Res<ButtonInput<KeyCode>>,
    overlays: Res<Overlays>,
    opening: OwnRespawn,
    mut dead_own: Query<&mut TankCommand, (With<Controlled>, With<TankKnockedOut>)>,
    mut awaiting: ResMut<AwaitingRespawn>,
) {
    if opening.between_combats()
        || awaiting.active
        || opening.state().waiting
        || !keys.just_pressed(RESPAWN_KEY)
        || gate.is_some_and(|gate| gate.settling)
        || opening.seconds_left().is_some()
    {
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

fn refusal_reason(reason: SpawnPickRefusal) -> &'static str {
    match reason {
        SpawnPickRefusal::NonFinite => "invalid map coordinates",
        SpawnPickRefusal::NoClearGround => "no clear ground near this point",
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
        DeathScreenNode::Died {
            opens_in_s,
            pick_refusal,
        } => {
            let mut text = match opens_in_s {
                None => "YOU DIED\npress R to respawn".to_owned(),
                Some(seconds) => format!("YOU DIED\nrespawn in {seconds} s"),
            };
            if let Some(reason) = pick_refusal {
                text.push_str(&format!(
                    "\n{}; previous spawn choice kept\npress M to re-pick",
                    refusal_reason(reason)
                ));
            }
            text
        }
        DeathScreenNode::Respawning => "RESPAWNING…".to_owned(),
        DeathScreenNode::WaitingForSpace { pick_refusal } => {
            let mut text =
                "WAITING FOR SPAWN SPACE\nrespawn is automatic when clear\npress M to re-pick"
                    .to_owned();
            if let Some(reason) = pick_refusal {
                text.push_str(&format!(
                    "\n{}; previous spawn choice kept",
                    refusal_reason(reason)
                ));
            }
            text
        }
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

/// One no-scrim status line carries admission through the map/menu, and refusal while alive.
/// Death's z-layer keeps it above those backdrops; the connect screen still covers it.
#[derive(Component)]
struct DeathStatusLine;

#[derive(Component)]
struct DeathStatusText;

fn spawn_death_status_line(mut commands: Commands, fonts: Res<UiFonts>) {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Look {
        top_px: crate::provenance::Sourced<f32>,
        font_px: crate::provenance::Sourced<f32>,
        text_srgb: crate::provenance::Sourced<[f32; 3]>,
    }
    const FILE: &str = "assets/ui/death_screen.ron";
    static LOOK: std::sync::OnceLock<Look> = std::sync::OnceLock::new();
    let look = LOOK.get_or_init(|| {
        let look: Look = crate::provenance::from_shipped_str(
            FILE,
            include_str!("../../assets/ui/death_screen.ron"),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            look.top_px.0.is_finite() && look.top_px.0 >= 0.0,
            "{FILE}: top_px must be finite and nonnegative; author a valid status-line position"
        );
        assert!(
            look.font_px.0.is_finite() && look.font_px.0 > 0.0,
            "{FILE}: font_px must be finite and positive; author a valid status-line font size"
        );
        assert!(
            look.text_srgb
                .0
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "{FILE}: text_srgb needs finite sRGB channels in [0, 1]; author a valid colour"
        );
        look
    });
    let [r, g, b] = look.text_srgb.0;
    commands
        .spawn((
            FlowScoped(Flow::InMatch),
            DeathStatusLine,
            Node {
                width: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                top: Val::Px(look.top_px.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            GlobalZIndex(Overlay::Death.zindex()),
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent.spawn((
                DeathStatusText,
                Text::new("DEAD — respawn on menu close"),
                TextFont {
                    // SemiBold: a terse all-caps status line.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(look.font_px.0),
                    ..default()
                },
                TextColor(Color::srgb(r, g, b)),
            ));
        });
}

/// The no-scrim status line also carries owner-private admission while a map/menu hides the death
/// screen, or a living player has just picked an invalid point. Its action follows the top overlay.
fn apply_death_status_line(
    overlays: Res<Overlays>,
    opening: OwnRespawn,
    mut status: Query<&mut Visibility, With<DeathStatusLine>>,
    mut texts: Query<&mut Text, With<DeathStatusText>>,
) {
    let state = opening.state();
    let message =
        if opening.between_combats() || overlay::draws_scrim(&overlays, Overlay::ConnectStatus) {
            None
        } else if !state.waiting && state.pick_refusal.is_none() {
            overlay::death_status_line(&overlays).then(|| "DEAD — respawn on menu close".to_owned())
        } else if overlay::draws_scrim(&overlays, Overlay::Death) {
            None // The full death screen already names the admission and action.
        } else {
            let action = if overlay::draws_scrim(&overlays, Overlay::SpawnMap) {
                "click map to re-pick"
            } else if overlay::draws_scrim(&overlays, Overlay::Menu) {
                "close menu, then M to re-pick"
            } else {
                "press M to re-pick"
            };
            let refusal = state
                .pick_refusal
                .map(|reason| format!("{}; previous spawn choice kept; ", refusal_reason(reason)))
                .unwrap_or_default();
            Some(if state.waiting {
                format!("WAITING FOR SPAWN SPACE — {refusal}automatic when clear; {action}")
            } else {
                format!("SPAWN PICK REFUSED — {refusal}{action}")
            })
        };
    let mut visibility = match status.single_mut() {
        Ok(status) => status,
        // The composed bare death-view fixture mounts no status-line node.
        Err(QuerySingleError::NoEntities(_)) => return,
        Err(QuerySingleError::MultipleEntities(_)) => {
            panic!("one death status line is spawned per match")
        }
    };
    visibility.set_if_neq(if message.is_some() {
        Visibility::Visible
    } else {
        Visibility::Hidden
    });
    if let Some(message) = message {
        let mut text = texts
            .single_mut()
            .expect("the death status line spawns one text child");
        if text.0 != message {
            text.0 = message;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's damage model: a player's R prompt and request follow the same opening as the
    /// authority, including across tick wrap. A missing UI or input gate permits an early request;
    /// a late gate hides an allowed request. Fixture identities and starting ticks.
    #[test]
    fn the_respawn_prompt_and_key_follow_the_authority_opening() {
        use super::super::battle_mode::BattleMode;
        use super::super::score::{Respawn, RespawnGate, RespawnWait, Standing};
        use crate::sim_tick::{SimTick, TickSpan};
        use bevy::ecs::system::RunSystemOnce;

        let timestep = crate::sim_tick::sim_timestep();
        let wait = RespawnWait::new(BattleMode::shipped().respawn_wait, timestep).ticks();
        assert!(wait > 0, "the chosen mode waits before allowing respawn");
        let (who, life) = (CombatantId(1), CombatantLife(2));
        for start in [0, u32::MAX - wait / 2] {
            for elapsed in [0, wait - 1, wait, wait + 1] {
                let mut world = World::new();
                world.insert_resource(Time::<()>::default());
                let mut fixed = Time::<Fixed>::default();
                fixed.set_timestep(timestep);
                world.insert_resource(fixed);
                world.insert_resource(ShotClock(SimTick::new(start).after(TickSpan::of(elapsed))));
                world.insert_resource(ButtonInput::<KeyCode>::default());
                world.insert_resource(Overlays::default());
                world.resource_mut::<Overlays>().declare_in_match(true);
                world.insert_resource(UiFonts {
                    hud: Handle::default(),
                    body: Handle::default(),
                });
                world.insert_resource(AwaitingRespawn::default());
                world.spawn((
                    Scoreboard(vec![Standing {
                        combatant: who,
                        knockouts: 0,
                        losses: 1,
                        respawn: Respawn::Opens {
                            life,
                            tick: SimTick::new(start).after(TickSpan::of(wait)),
                        },
                    }]),
                    BattleState::combat_fixture(),
                ));
                let own = world
                    .spawn((
                        Controlled,
                        TankKnockedOut {
                            reason: crate::damage::KnockoutReason::CrewLoss,
                        },
                        TankCommand::default(),
                        who,
                        life,
                    ))
                    .id();
                let accepted = world
                    .run_system_once(move |gate: RespawnGate| {
                        gate.opening(who, life) == Opening::Open
                    })
                    .unwrap();
                assert_eq!(accepted, elapsed >= wait);
                world.run_system_once(toggle_death_screen).unwrap();
                let prompt = world.query::<&Text>().single(&world).unwrap();
                assert_eq!(prompt.0.contains("press R to respawn"), accepted);
                world
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .press(RESPAWN_KEY);
                world.run_system_once(request_respawn).unwrap();
                assert_eq!(world.get::<TankCommand>(own).unwrap().respawn, accepted);
            }
        }
    }

    /// The game's damage model: results cancel an in-flight respawn immediately and suppress a
    /// new request even if the old knocked-out replica has not despawned. Players must see results
    /// rather than a timeout-length respawn wait. Fixture clocks leave the timeout in the future.
    #[test]
    fn results_cancel_a_pending_respawn_and_refuse_another() {
        use super::super::battle::{BattleOutcome, BattlePhase};
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Time::<Fixed>::default());
        world.insert_resource(ButtonInput::<KeyCode>::default());
        world.insert_resource(Overlays::default());
        world.resource_mut::<Overlays>().declare_in_match(true);
        world.insert_resource(UiFonts {
            hud: Handle::default(),
            body: Handle::default(),
        });
        world.insert_resource(awaiting(true, 1000.0));
        world.spawn(BattleState {
            number: 1,
            phase: BattlePhase::Results {
                ends_at: crate::sim_tick::SimTick::new(100),
                outcome: BattleOutcome::NoContest,
            },
        });
        let own = world
            .spawn((
                Controlled,
                TankKnockedOut {
                    reason: crate::damage::KnockoutReason::CrewLoss,
                },
                TankCommand::default(),
            ))
            .id();
        world.run_system_once(toggle_death_screen).unwrap();
        assert!(!world.resource::<AwaitingRespawn>().active);
        assert_eq!(world.query::<&DeathScreenNode>().iter(&world).count(), 0);
        world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(RESPAWN_KEY);
        world.run_system_once(request_respawn).unwrap();
        assert!(!world.get::<TankCommand>(own).unwrap().respawn);
        assert!(!world.resource::<AwaitingRespawn>().active);
    }

    /// The game's damage model: authoritative admission survives the local retry deadline and
    /// refuses redundant R edges. A hidden death screen must still explain a pending automatic spawn.
    #[test]
    fn authoritative_respawn_admission_outlives_the_request_timeout() {
        use bevy::ecs::system::RunSystemOnce;
        for pick_refusal in [None, Some(SpawnPickRefusal::NoClearGround)] {
            let state = RespawnState {
                waiting: true,
                pick_refusal,
            };
            let node = DeathScreenNode::WaitingForSpace { pick_refusal };
            let mut world = World::new();
            let mut time = Time::<()>::default();
            time.advance_by(std::time::Duration::from_secs_f64(
                RESPAWN_TIMEOUT_SECS * 2.0,
            )); // Fixture past the local deadline.
            world.insert_resource(time);
            world.insert_resource(Time::<Fixed>::default());
            world.insert_resource(ButtonInput::<KeyCode>::default());
            world.insert_resource(Overlays::default());
            world.resource_mut::<Overlays>().declare_in_match(true);
            world.insert_resource(UiFonts {
                hud: Handle::default(),
                body: Handle::default(),
            });
            world.insert_resource(awaiting(true, RESPAWN_TIMEOUT_SECS));
            let own = world
                .spawn((
                    Controlled,
                    TankKnockedOut {
                        reason: crate::damage::KnockoutReason::CrewLoss,
                    },
                    TankCommand::default(),
                    state,
                ))
                .id();
            world.run_system_once(toggle_death_screen).unwrap();
            assert_eq!(
                *world.query::<&DeathScreenNode>().single(&world).unwrap(),
                node
            );
            assert!(
                !world.resource::<AwaitingRespawn>().active,
                "the authority replaces the local timeout"
            );
            world
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(RESPAWN_KEY);
            world.run_system_once(request_respawn).unwrap();
            assert!(!world.get::<TankCommand>(own).unwrap().respawn);
            assert!(!world.resource::<AwaitingRespawn>().active);
            world.run_system_once(spawn_death_status_line).unwrap();
            let line = world
                .query_filtered::<Entity, With<DeathStatusLine>>()
                .single(&world)
                .unwrap();
            let text = world
                .query_filtered::<Entity, With<DeathStatusText>>()
                .single(&world)
                .unwrap();
            for top in [Overlay::Menu, Overlay::SpawnMap] {
                for overlay in [Overlay::Menu, Overlay::SpawnMap] {
                    world
                        .resource_mut::<Overlays>()
                        .declare(overlay, overlay == top);
                }
                world.run_system_once(apply_death_status_line).unwrap();
                assert_eq!(world.get::<Visibility>(line), Some(&Visibility::Visible));
                let message = &world.get::<Text>(text).unwrap().0;
                assert!(
                    message.contains("WAITING FOR SPAWN SPACE")
                        && message.contains("automatic when clear"),
                    "a hidden death screen must explain the automatic respawn: {message}"
                );
                assert!(
                    message.contains("re-pick"),
                    "the status supplies the way out"
                );
            }
        }
    }

    /// The game's damage model: the player's first R edge starts a wait; another edge cannot extend its retry deadline.
    #[test]
    fn pending_request_refuses_another_edge_without_extending_its_deadline() {
        use bevy::ecs::system::RunSystemOnce;
        for pick_refusal in [
            None,
            Some(SpawnPickRefusal::NonFinite),
            Some(SpawnPickRefusal::NoClearGround),
        ] {
            let mut world = World::new();
            world.insert_resource(Time::<()>::default());
            world.insert_resource(Time::<Fixed>::default());
            world.insert_resource(ButtonInput::<KeyCode>::default());
            world.insert_resource(Overlays::default());
            world.resource_mut::<Overlays>().declare_in_match(true);
            world.insert_resource(AwaitingRespawn::default());
            let own = world
                .spawn((
                    Controlled,
                    TankKnockedOut {
                        reason: crate::damage::KnockoutReason::CrewLoss,
                    },
                    TankCommand::default(),
                    RespawnState {
                        waiting: false,
                        pick_refusal,
                    },
                ))
                .id();
            world
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(RESPAWN_KEY);
            world.run_system_once(request_respawn).unwrap();
            assert!(world.get::<TankCommand>(own).unwrap().respawn);
            let deadline = world.resource::<AwaitingRespawn>().deadline;
            world.get_mut::<TankCommand>(own).unwrap().respawn = false;
            world
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f64(
                    RESPAWN_TIMEOUT_SECS / 2.0,
                )); // Fixture within the retry horizon.
            world.resource_mut::<ButtonInput<KeyCode>>().reset_all();
            world
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(RESPAWN_KEY);
            world.run_system_once(request_respawn).unwrap();
            assert!(!world.get::<TankCommand>(own).unwrap().respawn);
            assert_eq!(world.resource::<AwaitingRespawn>().deadline, deadline);
        }
    }

    fn awaiting(active: bool, deadline: f64) -> AwaitingRespawn {
        AwaitingRespawn { active, deadline }
    }

    /// A respawn request reverts only when it is in flight, no live tank has arrived, and the
    /// deadline has passed. Before it, an in-flight request keeps the overlay on `RESPAWNING…`; a
    /// live tank means the round-trip succeeded and there is nothing to revert; with no request in
    /// flight the timeout cannot manufacture a revert out of an idle death screen.
    #[test]
    fn not_timed_out_before_deadline() {
        let a = awaiting(true, 10.0);
        assert!(!respawn_timed_out(&a, false, 9.999), "just before deadline");
        assert!(respawn_timed_out(&a, false, 10.0), "at the deadline");
        assert!(respawn_timed_out(&a, false, 12.0), "past the deadline");
        assert!(
            !respawn_timed_out(&a, true, 100.0),
            "a live tank never times out"
        );
        let idle = awaiting(false, 0.0);
        assert!(
            !respawn_timed_out(&idle, false, 100.0),
            "no request, no timeout"
        );
    }
}
