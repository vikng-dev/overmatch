//! The net client's UI, screen by screen: every root has an owner, and each screen shows only its
//! own UI.
//!
//! The app is the net client's UI layer — every plugin `NetClientPlugin` and `net::client` mount that
//! spawns a UI tree — on a GPU-less window, with no link, no sim and no server. The resources the
//! client's sim, link and window plugins would provide are inserted at their defaults, and the error
//! handler panics: a UI system whose parameters fail validation fails the test rather than being
//! skipped unseen. The screen is driven through `NextState<Flow>` directly; its derivation is
//! `flow::flow_of`'s.

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

/// The short type names of each root's components: how the test names a module's private marker.
fn root_names(world: &mut World) -> Vec<Vec<String>> {
    roots(world)
        .into_iter()
        .map(|root| {
            let entity = world.entity(root);
            entity
                .archetype()
                .components()
                .iter()
                .filter_map(|id| world.components().get_name(*id))
                .map(|name| name.shortname().to_string())
                .collect()
        })
        .collect()
}

/// Every entity carrying a component whose short type name is `marker`.
fn entities_named(world: &mut World, marker: &str) -> Vec<Entity> {
    let mut query = world.query::<EntityRef>();
    query
        .iter(world)
        .filter(|entity| {
            entity.archetype().components().iter().any(|id| {
                world
                    .components()
                    .get_name(*id)
                    .is_some_and(|name| name.shortname().to_string() == marker)
            })
        })
        .map(|entity| entity.id())
        .collect()
}

/// Every root marker in `expected` is on some root.
fn assert_roots(world: &mut World, flow: Flow, expected: &[&str]) {
    let names = root_names(world);
    for marker in expected {
        assert!(
            names
                .iter()
                .any(|root| root.iter().any(|name| name == marker)),
            "{flow:?}: no UI root carries `{marker}`"
        );
    }
}

/// The persistent roots: the overlays and the settings page, on every screen.
const PERSISTENT_ROOTS: &[&str] = &["SettingsCard", "ViewDeathOverlay", "OverlayNode"];

/// The match's HUD roots, one marker each (`TankNameplate` and `ComponentHpLabel` are pools).
const MATCH_ROOTS: &[&str] = &[
    "DebugCard",
    "CrewBarText",
    "StatusPanelText",
    "StandardDriveHudText",
    "DriveDebugHudText",
    "OpticMask",
    "RangeReadout",
    "IntentReticle",
    "AimIndicator",
    "BoreIndicator",
    "DeathStatusLine",
    "DamageFlashNode",
    "TankNameplate",
    "ComponentHpLabel",
];

fn client_ui_app() -> App {
    let mut app = App::new();
    app.add_plugins(crate::gpu_less_default_plugins(Some(Window {
        resolution: bevy::window::WindowResolution::new(1280, 720),
        ..default()
    })));
    app.set_error_handler(bevy::ecs::error::panic);
    // What the sim, the link and winit provide on a live client.
    app.insert_resource(lightyear::core::tick::TickDuration(
        std::time::Duration::from_secs_f64(1.0 / 64.0),
    ))
    .init_resource::<super::sync_margin::ArrivalDelay>()
    .init_resource::<crate::WeaponClock>()
    .init_resource::<crate::firecontrol::Ranging>()
    .init_resource::<avian3d::collider_tree::ColliderTrees>()
    .init_resource::<bevy::winit::WinitMonitors>();
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
    // A closed local port: the location fetch fails at once, and the lobby draws that failure.
    super::matchmaking::install(&mut app, "http://127.0.0.1:9");
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
        assert_roots(app.world_mut(), flow, PERSISTENT_ROOTS);
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
        let drawn = visible_owners(world);
        for owners in &drawn {
            assert_eq!(
                *owners,
                vec![Owner::Screen(flow)],
                "{flow:?}: a drawn root must be this screen's; overlays and the settings page are \
                 hidden until declared"
            );
        }
        if matches!(flow, Flow::Lobby | Flow::Joining) {
            assert!(!drawn.is_empty(), "{flow:?}: the screen draws its own root");
        }
        if flow == Flow::Lobby {
            let lists = entities_named(world, "LobbyList");
            assert_eq!(lists.len(), 1, "one lobby card");
            assert!(
                world
                    .get::<Children>(lists[0])
                    .is_some_and(|children| !children.is_empty()),
                "the lobby's list is drawn on every entry, a return included"
            );
            let texts: Vec<String> = world
                .query::<&Text>()
                .iter(world)
                .map(|text| text.0.clone())
                .collect();
            assert!(
                !texts.iter().any(|text| text.contains("Tick the regions")),
                "the lobby carries no instruction text: {texts:?}"
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

/// The match's HUD is real: entering it builds each of its roots, a dead controlled tank puts the
/// death screen over them, and leaving takes every one of them away.
#[test]
fn the_match_builds_its_hud_and_leaving_takes_it_away() {
    let mut app = client_ui_app();
    enter(&mut app, Flow::InMatch);
    assert_roots(app.world_mut(), Flow::InMatch, PERSISTENT_ROOTS);
    assert_roots(app.world_mut(), Flow::InMatch, MATCH_ROOTS);

    app.world_mut().spawn((
        crate::tank::Controlled,
        crate::damage::TankKnockedOut {
            reason: crate::damage::KnockoutReason::CrewLoss,
        },
    ));
    for _ in 0..3 {
        app.update();
    }
    let world = app.world_mut();
    assert!(misowned(world).is_empty(), "{:#?}", misowned(world));
    let drawn_overlays: Vec<Vec<String>> = roots(world)
        .into_iter()
        .filter(|root| world.get::<Visibility>(*root) != Some(&Visibility::Hidden))
        .filter(|root| owners(world.entity(*root)) == vec![Owner::Overlay])
        .map(|root| {
            let entity = world.entity(root);
            entity
                .archetype()
                .components()
                .iter()
                .filter_map(|id| world.components().get_name(*id))
                .map(|name| name.shortname().to_string())
                .collect()
        })
        .collect();
    assert_eq!(
        drawn_overlays.len(),
        1,
        "a dead controlled tank draws exactly one overlay: {drawn_overlays:?}"
    );
    assert!(
        drawn_overlays[0]
            .iter()
            .any(|name| name == "DeathScreenNode"),
        "the drawn overlay is the death screen: {drawn_overlays:?}"
    );

    enter(&mut app, Flow::Lobby);
    let world = app.world_mut();
    let left = world
        .query::<&FlowScoped>()
        .iter(world)
        .filter(|scope| scope.0 == Flow::InMatch)
        .count();
    assert_eq!(left, 0, "leaving the match takes every one of its trees");
    let deaths = root_names(world)
        .iter()
        .filter(|root| root.iter().any(|name| name == "DeathScreenNode"))
        .count();
    assert_eq!(deaths, 0, "the death screen leaves with the match");
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
