//! The net client's UI, screen by screen: every root has an owner, and each screen shows only its
//! own UI.
//!
//! The app is the net client's UI layer — every plugin `NetClientPlugin` and `net::client` mount that
//! spawns a UI tree — on a GPU-less window, with no link, no sim and no server. Systems whose
//! resources only a live client has are skipped (the error handler ignores failed parameter
//! validation): what is measured is what the spawns and the screen transitions build.
//! The screen is driven through `NextState<Flow>` directly; its derivation is `flow::flow_of`'s.

use bevy::prelude::*;

use crate::flow::{Flow, FlowScoped};
use crate::overlay::OverlayNode;
use crate::settings::SettingsCard;
use crate::state::AppState;

/// What owns a UI root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Screen(Flow),
    Overlay,
    SettingsPage,
    PauseBanner,
}

/// The owners `root` carries; a root must carry exactly one.
fn owners(root: EntityRef) -> Vec<Owner> {
    let mut found = Vec::new();
    if let Some(scope) = root.get::<FlowScoped>() {
        found.push(Owner::Screen(scope.0));
    }
    if root.contains::<OverlayNode>() {
        found.push(Owner::Overlay);
    }
    if root.contains::<SettingsCard>() {
        found.push(Owner::SettingsPage);
    }
    if root.contains::<DespawnOnExit<AppState>>() {
        found.push(Owner::PauseBanner);
    }
    found
}

/// Every UI root: a `Node` with no parent.
fn roots(world: &mut World) -> Vec<Entity> {
    let mut query = world.query_filtered::<Entity, (With<Node>, Without<ChildOf>)>();
    query.iter(world).collect()
}

/// Roots with no owner or more than one, described by their components.
fn misowned(world: &mut World) -> Vec<String> {
    roots(world)
        .into_iter()
        .filter_map(|root| {
            let entity = world.entity(root);
            let owners = owners(entity);
            (owners.len() != 1).then(|| {
                let components: Vec<String> = entity
                    .archetype()
                    .components()
                    .iter()
                    .filter_map(|id| world.components().get_name(*id))
                    .map(|name| name.shortname().to_string())
                    .collect();
                format!("{root}: owners {owners:?}, components {components:?}")
            })
        })
        .collect()
}

/// The screen owners of the roots that are drawn: `Visibility` not `Hidden` (a root's inherited
/// visibility is its own).
fn visible_owners(world: &mut World) -> Vec<Vec<Owner>> {
    roots(world)
        .into_iter()
        .filter(|root| world.get::<Visibility>(*root) != Some(&Visibility::Hidden))
        .map(|root| owners(world.entity(root)))
        .collect()
}

fn client_ui_app() -> App {
    let mut app = App::new();
    app.add_plugins(crate::gpu_less_default_plugins(Some(Window {
        resolution: bevy::window::WindowResolution::new(1280, 720),
        ..default()
    })));
    app.set_error_handler(bevy::ecs::error::ignore);
    app.add_plugins((crate::ui_font::plugin, crate::flow::plugin))
        .add_plugins((
            crate::overlay::plugin,
            crate::aim::client_plugin,
            crate::sight::plugin,
            crate::hud::plugin,
            crate::crew_ui::plugin,
            crate::drive_hud::plugin,
        ))
        .add_plugins((
            super::debug_hud::plugin,
            super::hit_feel::plugin,
            super::death_screen::plugin,
            super::spawn_map::plugin,
            super::client::connect_screen_plugin,
        ))
        .add_plugins(crate::settings::plugin(
            crate::settings::PageEntry::OverlayMenu,
        ));
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    app
}

/// Enter `flow` and let the transition and its spawns settle.
fn enter(app: &mut App, flow: Flow) {
    app.world_mut().resource_mut::<NextState<Flow>>().set(flow);
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(*app.world().resource::<State<Flow>>().get(), flow);
}

/// Every root on every screen has exactly one owner, a screen's trees leave with it, and outside
/// the match nothing of the match is drawn.
#[test]
fn every_root_is_owned_and_each_screen_draws_only_its_own_ui() {
    let mut app = client_ui_app();
    for flow in [
        Flow::Boot,
        Flow::Lobby,
        Flow::Joining,
        Flow::InMatch,
        Flow::Lobby,
        Flow::InMatch,
    ] {
        enter(&mut app, flow);
        let world = app.world_mut();
        let misowned = misowned(world);
        assert!(
            misowned.is_empty(),
            "{flow:?}: every UI root needs exactly one owner (FlowScoped, OverlayNode, the \
             settings page or the offline pause banner):\n{}",
            misowned.join("\n")
        );
        let mut scoped = world.query::<&FlowScoped>();
        let stale: Vec<Flow> = scoped
            .iter(world)
            .map(|scope| scope.0)
            .filter(|scope| *scope != flow)
            .collect();
        assert!(
            stale.is_empty(),
            "{flow:?}: another screen's trees remain: {stale:?}"
        );
        for owners in visible_owners(world) {
            assert_eq!(
                owners,
                vec![Owner::Screen(flow)],
                "{flow:?}: a drawn root must be this screen's; overlays and the settings page are \
                 hidden until declared"
            );
        }
        let debug_cards = world
            .query::<&super::debug_hud::DebugCard>()
            .iter(world)
            .count();
        assert_eq!(
            debug_cards,
            usize::from(flow == Flow::InMatch),
            "{flow:?}: the debug card exists in a match and nowhere else"
        );
    }
}

/// The match's HUD is real: entering it builds trees, and every one of them is the match's.
#[test]
fn the_match_builds_its_hud_and_leaving_takes_it_away() {
    let mut app = client_ui_app();
    enter(&mut app, Flow::InMatch);
    let world = app.world_mut();
    let in_match = world
        .query::<&FlowScoped>()
        .iter(world)
        .filter(|scope| scope.0 == Flow::InMatch)
        .count();
    assert!(
        in_match >= 10,
        "the match spawns its HUD roots on entry (found {in_match})"
    );
    enter(&mut app, Flow::Lobby);
    let world = app.world_mut();
    assert_eq!(
        world.query::<&FlowScoped>().iter(world).count(),
        0,
        "leaving the match takes every one of its trees"
    );
}

/// The audit sees what it guards against: a bare root, and a root claimed twice.
#[test]
fn the_audit_flags_an_unowned_or_doubly_owned_root() {
    let mut world = World::new();
    world.spawn(Node::default());
    world.spawn((Node::default(), FlowScoped(Flow::InMatch)));
    world.spawn((
        Node::default(),
        FlowScoped(Flow::InMatch),
        OverlayNode(crate::overlay::Overlay::Menu),
    ));
    world.flush();
    let flagged = misowned(&mut world);
    assert_eq!(flagged.len(), 2, "{flagged:#?}");
}
