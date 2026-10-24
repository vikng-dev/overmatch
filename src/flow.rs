//! Which screen a windowed client is on: [`Flow`].
//!
//! - **View state only.** `Flow` gates UI spawns, UI update systems and the input license; nothing
//!   registered on the fixed clock reads it (`src/tests/flow_boundary.rs`). The sim gate stays
//!   [`crate::state::AppState`], so a client on a menu screen keeps ticking and keeps sending its
//!   (zeroed) command.
//! - **Derived, one writer per root.** Each windowed root writes [`flow_of`] of its facts every frame
//!   with `NextState::set_if_neq`, so a frame that changes nothing transitions nothing: the offline
//!   root from `AppState` ([`offline_plugin`]), the net client from its link (`net::client`). A
//!   root that is always in a match — a sandbox — inserts [`Flow::InMatch`] once
//!   ([`in_match_plugin`]).
//! - **The transition lands one frame after the facts change**: the writer runs in `Update`, and
//!   `StateTransition` applies it before the next frame's `Update`.
//! - **Every UI root has an owner**: [`FlowScoped`] (spawned on `OnEnter`, despawned on exit), an
//!   `overlay::OverlayNode` (persistent, shown by the overlay authority), the settings page (the
//!   Esc menu's content), or the offline pause banner (`DespawnOnExit(AppState::Paused)`).
//!   `net::ui_ownership` fails on a root with none.

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
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

/// The view systems that run only on the match screen, in `Update` and `PostUpdate`. Never a
/// fixed-clock set: `Flow` stays off the fixed clock (`src/tests/flow_boundary.rs`).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InMatchSystems;

fn configure_in_match(app: &mut App) {
    app.configure_sets(Update, InMatchSystems.run_if(in_state(Flow::InMatch)))
        .configure_sets(PostUpdate, InMatchSystems.run_if(in_state(Flow::InMatch)));
}

/// Registers [`Flow`] and [`InMatchSystems`] and logs transitions. Mounted by every windowed root
/// before any plugin that spawns or gates on it.
pub(crate) fn plugin(app: &mut App) {
    app.init_state::<Flow>()
        .add_systems(Update, log_flow.run_if(state_changed::<Flow>));
    configure_in_match(app);
}

/// [`plugin`] plus the offline root's writer.
pub(crate) fn offline_plugin(app: &mut App) {
    plugin(app);
    app.add_systems(Update, derive_offline_flow);
}

/// A root that is always in a match: no writer, no transitions. Only test apps are such a root.
#[cfg(test)]
pub(crate) fn in_match_plugin(app: &mut App) {
    app.insert_state(Flow::InMatch);
    configure_in_match(app);
}

/// Per-match view state: a resource reset to its initial value when the client leaves a match, so
/// nothing one match latched (a sight mode, an open map, a pending respawn) carries into the next.
/// A root that never leaves a match (offline, a sandbox) never resets it.
pub(crate) trait MatchResourceExt {
    /// `init_resource::<R>()`, plus the reset on leaving a match.
    fn init_match_resource<R: Resource + FromWorld>(&mut self) -> &mut Self;
}

impl MatchResourceExt for App {
    fn init_match_resource<R: Resource + FromWorld>(&mut self) -> &mut Self {
        self.init_resource::<R>()
            .add_systems(OnExit(Flow::InMatch), reset_resource::<R>)
    }
}

fn reset_resource<R: Resource + FromWorld>(world: &mut World) {
    let initial = R::from_world(world);
    world.insert_resource(initial);
}

/// Panics unless [`Flow`] is registered: a plugin that spawns on `OnEnter(Flow::…)` or gates on
/// `in_state(Flow::…)` would otherwise never draw, since `in_state` is false while the state is
/// absent.
pub(crate) fn require(app: &App, plugin: &str) {
    assert!(
        app.world().contains_resource::<State<Flow>>(),
        "{plugin} spawns and gates on `flow::Flow`; mount `flow::plugin` (or, in a test, \
         `in_match_plugin`) before it"
    );
}

/// Owns a UI root for one screen: the hook stamps `DespawnOnExit` of that screen, so the tree is
/// spawned on `OnEnter` and leaves with the screen, and no spawn site can forget the despawn.
#[derive(Component, Clone, Copy, Debug)]
#[component(on_add = stamp_despawn_on_exit)]
pub(crate) struct FlowScoped(pub(crate) Flow);

fn stamp_despawn_on_exit(mut world: DeferredWorld, HookContext { entity, .. }: HookContext) {
    let flow = world
        .entity(entity)
        .get::<FlowScoped>()
        .expect("FlowScoped present in its own on_add hook")
        .0;
    world.commands().entity(entity).insert(DespawnOnExit(flow));
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

    /// A scoped root despawns, children and all, when its screen exits — and only then.
    #[test]
    fn a_scoped_root_leaves_with_its_screen() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<Flow>();
        let go = |app: &mut App, flow: Flow| {
            app.world_mut().resource_mut::<NextState<Flow>>().set(flow);
            app.update();
        };
        go(&mut app, Flow::InMatch);
        let root = app
            .world_mut()
            .spawn((FlowScoped(Flow::InMatch), Node::default()))
            .with_child(Node::default())
            .id();
        let child = app.world().entity(root).get::<Children>().unwrap()[0];
        app.update();
        assert!(
            app.world().entity(root).contains::<DespawnOnExit<Flow>>(),
            "the hook stamps the despawn"
        );
        assert!(app.world().get_entity(root).is_ok(), "same screen, kept");

        go(&mut app, Flow::Lobby);
        assert!(app.world().get_entity(root).is_err(), "screen exited");
        assert!(app.world().get_entity(child).is_err(), "with its children");
    }
}
