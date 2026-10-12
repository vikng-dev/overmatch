//! Which screen a windowed client is on: [`Flow`].
//!
//! - **View state only.** `Flow` gates UI spawns, UI update systems and the input license; nothing
//!   registered on the fixed clock reads it (`tests/flow_boundary.rs`). The sim gate stays
//!   [`crate::state::AppState`], so a client on a menu screen keeps ticking and keeps sending its
//!   (zeroed) command.
//! - **Derived, one writer per root.** Each windowed root writes [`flow_of`] of its facts every frame
//!   with `NextState::set_if_neq`, so a frame that changes nothing transitions nothing: the offline
//!   root from `AppState` ([`offline_plugin`]), the net client from its link (`net::client`).
//! - **The transition lands one frame after the facts change**: the writer runs in `Update`, and
//!   `StateTransition` applies it before the next frame's `Update`.

use bevy::prelude::*;

/// The client's screen. Overlays (pause, death, reconnecting, the Esc menu) are not screens: they
/// stack inside [`Flow::InMatch`] under `overlay::Overlays`.
#[derive(States, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum Flow {
    /// Before the first derivation, and the offline root's asset load. Nothing is spawned for it.
    #[default]
    Boot,
    /// Matchmaking mode with no link: the Join menu.
    Lobby,
    /// A link exists that has never connected: the connect screen.
    Joining,
    /// The match world is on screen, a dropped link (RECONNECTING…) included.
    InMatch,
}

/// What [`flow_of`] derives the screen from, per root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlowFacts {
    /// The offline root: `playing` is `AppState` past `Loading`.
    Offline { playing: bool },
    /// The net client: whether a link entity exists, whether it has connected this session, and
    /// whether `AppState` is past `Loading`.
    Net {
        link: bool,
        connected_once: bool,
        playing: bool,
    },
}

/// The screen these facts put the client on.
pub(crate) fn flow_of(facts: FlowFacts) -> Flow {
    match facts {
        FlowFacts::Offline { playing: false } => Flow::Boot,
        FlowFacts::Offline { playing: true } => Flow::InMatch,
        FlowFacts::Net { link: false, .. } => Flow::Lobby,
        FlowFacts::Net {
            connected_once: true,
            playing: true,
            ..
        } => Flow::InMatch,
        FlowFacts::Net { .. } => Flow::Joining,
    }
}

/// Write the derived screen. `set_if_neq`: `set` re-runs `OnExit`/`OnEnter` even for the same
/// state, which would rebuild every scoped tree every frame.
pub(crate) fn write(next: &mut NextState<Flow>, facts: FlowFacts) {
    next.set_if_neq(flow_of(facts));
}

/// Registers [`Flow`] and logs its transitions. Mounted by every windowed root before any plugin
/// that spawns or gates on it.
pub(crate) fn plugin(app: &mut App) {
    app.init_state::<Flow>()
        .add_systems(Update, log_flow.run_if(state_changed::<Flow>));
}

/// [`plugin`] plus the offline root's writer.
pub(crate) fn offline_plugin(app: &mut App) {
    plugin(app);
    app.add_systems(Update, derive_offline_flow);
}

fn derive_offline_flow(
    state: Res<State<crate::state::AppState>>,
    mut next: ResMut<NextState<Flow>>,
) {
    write(
        &mut next,
        FlowFacts::Offline {
            playing: *state.get() != crate::state::AppState::Loading,
        },
    );
}

fn log_flow(flow: Res<State<Flow>>) {
    info!("flow: {:?}", flow.get());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole derivation, as a truth table over every fact combination.
    #[test]
    fn flow_of_truth_table() {
        for playing in [false, true] {
            assert_eq!(
                flow_of(FlowFacts::Offline { playing }),
                if playing { Flow::InMatch } else { Flow::Boot },
                "offline, playing {playing}",
            );
        }
        for link in [false, true] {
            for connected_once in [false, true] {
                for playing in [false, true] {
                    let expected = match (link, connected_once, playing) {
                        (false, _, _) => Flow::Lobby,
                        (true, true, true) => Flow::InMatch,
                        (true, _, _) => Flow::Joining,
                    };
                    assert_eq!(
                        flow_of(FlowFacts::Net {
                            link,
                            connected_once,
                            playing,
                        }),
                        expected,
                        "net, link {link}, connected once {connected_once}, playing {playing}",
                    );
                }
            }
        }
    }

    /// A dropped link stays in the match: RECONNECTING… is an overlay over the match world, not a
    /// screen. Only losing the link entity (an abandoned or left match) leaves it.
    #[test]
    fn a_dropped_link_stays_in_the_match_and_a_lost_link_returns_to_the_lobby() {
        let reconnecting = FlowFacts::Net {
            link: true,
            connected_once: true,
            playing: true,
        };
        assert_eq!(flow_of(reconnecting), Flow::InMatch);
        let left = FlowFacts::Net {
            link: false,
            connected_once: true,
            playing: true,
        };
        assert_eq!(flow_of(left), Flow::Lobby);
    }
}
