//! Net-client spawn map: `M` opens terrain; a click requests a persistent spawn point.
//!
//! The click is a REQUEST ([`SetSpawnPoint`]) — nothing teleports, nothing moves now. The authority
//! validates bounds and standing geometry, resolves ground height, and retains the last accepted
//! point for subsequent respawns (`net::server::respawn_player_tanks`). This module is view + input only.
//!
//! It joins the `overlay::Overlays` authority as [`Overlay::SpawnMap`], which is what releases the
//! cursor (a click needs a cursor position) and zeroes the wire command while the map is up.

use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;

use crate::provenance::Sourced;
use bevy::window::PrimaryWindow;
use lightyear::prelude::*;
use serde::Deserialize;

use super::protocol::{SetSpawnPoint, SpawnChannel};
use crate::damage_cam::pen_cam::game::KillCamGate;
use crate::flow::{InMatchSystems, MatchResourceExt};
use crate::overlay::{self, Overlay, OverlayNode, Overlays};
use crate::tank::Controlled;
use crate::ui_font::UiFonts;

/// Opens / closes the map. Free in the game client (`command::Bindings` and the screens' own
/// keys): W/A/S/D drive, Space is the secondary trigger, R respawns, Q/E step the round, V
/// toggles the fuze, C holds the kill cam, X is the x-ray, Lshift switches view, Tab holds the scoreboard,
/// G is the dev-tools toggle, 1–5 are crew stations, F3 is the debug panel, Esc is the menu.
/// The game's damage model uses F to start/cancel module repair. B and T are unbound.
/// The offline mode uses Tab for possession; the track sandbox's M is a different binary.
const SPAWN_MAP_KEY: KeyCode = KeyCode::KeyM;

/// Half-extent of the terrain square, metres — the LIVE world's, read off the decoded grid
/// (`terrain_grid::world_extent`, which answers for the flat-slab fixture world too), never
/// re-stated here. Both the server clamp ([`spawn_limit`]) and the map's UV↔world mapping take it,
/// so a re-authored world moves the map and the clamp together.
pub(super) fn world_half_extent(grid: Option<&crate::terrain_grid::HeightGrid>) -> f32 {
    crate::terrain_grid::world_extent(grid).half_extent()
}

/// Coordinate bound shared by client and authority; it does not admit standing geometry.
pub(crate) fn spawn_limit(half_extent: f32) -> f32 {
    half_extent * style().limit_fraction.0
}

/// The map image, in the selected map's `derived/` folder: an 8-BIT RGB copy of the terrain
/// heightmap, generated from the 16-bit source the manifest names (python3/PIL: the manifest's own
/// row order applied first — rows reversed for a `-Z` export, exactly as `terrain_grid::RowOrder`
/// does at decode — then numpy `>> 8` downshift, LANCZOS resize to 1024², grayscale → RGB).
///
/// The row order is not a detail of the recipe, it is the whole point: this image is what the
/// player clicks on, and [`uv_to_world`] answers in the world the GRID describes. Generated from
/// the file's raw rows instead, the panel would show a map mirrored north–south against the terrain
/// it points at — `ui_map_shows_the_world_the_click_math_points_at` is the standing check.
///
/// The UI must NOT load the 16-bit file directly: bevy decodes a Luma16 PNG into an `R16Uint`
/// GPU texture, and `Uint` textures are not filterable — `bevy_ui`'s `ui_material_bind_group`
/// requires `Float { filterable: true }`, so pressing `M` produced a wgpu validation error and
/// `Quitting the application due to Validation RenderError`. The 16-bit source stays exclusively
/// on the CPU-side decode path (`terrain_grid::grid_from_png`); this derived copy is pure view
/// with no determinism constraints. The `ui_map_is_8_bit_and_square` test pins the format.
const UI_MAP_FILE: &str = "map_ui.png";

/// Why the map's panel and its two markers each match exactly once: [`spawn_map_overlay`] spawns
/// them at `Startup` and nothing despawns them.
const PANEL_SPAWNED: &str =
    "spawn_map_overlay spawns one panel and one of each marker at startup; nothing despawns them";

const STYLE_FILE: &str = "assets/ui/spawn_map.ron";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnMapStyle {
    limit_fraction: Sourced<f32>,
    panel_fraction: Sourced<f32>,
    marker_px: Sourced<f32>,
    border_px: Sourced<f32>,
    title_gap_percent: Sourced<f32>,
    title_font_px: Sourced<f32>,
    backdrop: Sourced<[f32; 4]>,
    frame: Sourced<[f32; 4]>,
    chosen: Sourced<[f32; 4]>,
    refused: Sourced<[f32; 4]>,
    own: Sourced<[f32; 4]>,
}

pub(super) fn validate_spawn_map_data() {
    style();
}

fn style() -> &'static SpawnMapStyle {
    static STYLE: std::sync::OnceLock<SpawnMapStyle> = std::sync::OnceLock::new();
    STYLE.get_or_init(|| {
        let style: SpawnMapStyle = crate::provenance::from_shipped_str(
            STYLE_FILE,
            include_str!("../../assets/ui/spawn_map.ron"),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        for (field, value) in [
            ("limit_fraction", style.limit_fraction.0),
            ("panel_fraction", style.panel_fraction.0),
        ] {
            assert!(
                value.is_finite() && value > 0.0 && value <= 1.0,
                "{STYLE_FILE}: {field} must be in (0, 1]; author a fraction of the map"
            );
        }
        for (field, value) in [
            ("marker_px", style.marker_px.0),
            ("border_px", style.border_px.0),
            ("title_gap_percent", style.title_gap_percent.0),
            ("title_font_px", style.title_font_px.0),
        ] {
            assert!(
                value.is_finite() && value > 0.0,
                "{STYLE_FILE}: {field} must be finite and positive; author a positive size"
            );
        }
        for (field, colour) in [
            ("backdrop", &style.backdrop),
            ("frame", &style.frame),
            ("chosen", &style.chosen),
            ("refused", &style.refused),
            ("own", &style.own),
        ] {
            assert!(
                colour
                    .0
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                "{STYLE_FILE}: {field} needs finite sRGBA channels in [0, 1]; author a valid colour"
            );
        }
        assert_ne!(
            style.chosen.0, style.refused.0,
            "{STYLE_FILE}: refused must differ from chosen; make refusal visible on the marker"
        );
        style
    })
}

fn colour(value: &Sourced<[f32; 4]>) -> Color {
    let [r, g, b, a] = value.0;
    Color::srgba(r, g, b, a)
}

/// The marker remembers the last requested point across map toggles and respawns. Replicated
/// `RespawnState` supplies refusal feedback; the authority retains its last accepted destination.
#[derive(Resource, Default)]
struct SpawnMap {
    open: bool,
    chosen: Option<Vec2>,
    battle: Option<u32>,
}

/// The full-screen backdrop + its centred square image panel. The panel's px rect is the ONE thing
/// the click math needs, so it is written by [`size_spawn_map`] rather than inferred from layout.
///
/// **The panel node carries NO border and NO padding, deliberately.** Bevy 0.19's `ImageNode` draws
/// into the node's CONTENT box (`VisualBox::ContentBox`) and absolutely-positioned children are laid
/// out against the PADDING box, while `Node::width` under the default `BoxSizing::BorderBox` names the
/// OUTER box. Put a 2 px border on this node and those three boxes stop coinciding: the recorded rect
/// describes a square 4 px larger than the drawn image, which skewed every click by up to ~6.4 m at
/// the terrain edge. The frame therefore lives on a wrapper (the authored border width), leaving content ==
/// padding == border box here, so the ONE recorded rect is unambiguously the image's rect and the
/// click math, the markers, and the pixels all read the same square.
#[derive(Component)]
struct SpawnMapPanel {
    /// Panel edge length in logical px, and the panel's top-left corner in window coordinates —
    /// recomputed every frame from the window size, so a resize can never desync the click math.
    edge_px: f32,
    origin_px: Vec2,
}

/// The dot at the last point this client picked.
#[derive(Component)]
struct ChosenMarker;

/// The dot at the player's own tank right now.
#[derive(Component)]
struct SelfMarker;

pub fn plugin(app: &mut App) {
    style();
    crate::flow::require(app, "net::spawn_map");
    app.init_match_resource::<SpawnMap>()
        .add_systems(Startup, spawn_map_overlay)
        .add_systems(Update, systems().in_set(InMatchSystems));
}

/// The map's `Update` systems, placed among the overlay sets.
fn systems() -> ScheduleConfigs<ScheduleSystem> {
    (
        // The `M` toggle is a GATED one — it reads the set to decide whether the map may
        // open — so it runs in `OverlaySet::Toggle`, after every state-driven declarer has
        // written this frame. Declaring it alongside them would let `M` pressed on the same
        // frame a connect or death screen appears read a set that did not have that overlay
        // in it yet, pass `may_open`, and latch an invisible map.
        toggle_spawn_map.in_set(overlay::OverlaySet::Toggle),
        // Everything below reads the reconciled set (`draws_scrim`), so a click can never
        // land through a menu drawn on top of the map.
        (size_spawn_map, click_spawn_map, place_markers)
            .chain()
            .after(overlay::OverlaySet::Toggle),
    )
        .into_configs()
}

/// The map's state and `Update` systems without the match flow, for a composed schedule under
/// test. Its panel and markers stand as bare nodes (no image, no font): the systems rely on one of
/// each ([`PANEL_SPAWNED`]) in every composition that mounts them.
#[cfg(test)]
pub(crate) fn mount_bare(app: &mut App) {
    app.init_resource::<SpawnMap>()
        .add_systems(Update, systems());
    app.world_mut().spawn((
        Node::default(),
        SpawnMapPanel {
            edge_px: 0.0,
            origin_px: Vec2::ZERO,
        },
    ));
    app.world_mut().spawn((
        Node::default(),
        Visibility::Hidden,
        BackgroundColor::default(),
        ChosenMarker,
    ));
    app.world_mut()
        .spawn((Node::default(), Visibility::Hidden, SelfMarker));
}

/// `M` toggles the map; the declaration into [`Overlays`] is absolute and re-run every frame, so it
/// self-heals like every other overlay owner. Deliberately NOT in `PlayerInputSet`: that set is
/// gated on the cursor being LOCKED, and this overlay releases the cursor — reading M there would
/// make the map impossible to close.
fn toggle_spawn_map(
    keys: Res<ButtonInput<KeyCode>>,
    gate: Option<Res<KillCamGate>>,
    mut map: ResMut<SpawnMap>,
    mut overlays: ResMut<Overlays>,
    battles: Query<&super::battle::BattleState>,
) {
    let battle = match battles.single() {
        Ok(battle) => Some(battle.number),
        // Before the first snapshot, or between links, no Battle owns a pick.
        Err(bevy::ecs::query::QuerySingleError::NoEntities(_)) => None,
        Err(bevy::ecs::query::QuerySingleError::MultipleEntities(_)) => {
            panic!("a replica has one Battle")
        }
    };
    if map.battle != battle {
        map.chosen = None;
        map.battle = battle;
    }
    // Not in the kill cam's input dead-zone after a knockout ([`KillCamGate::settling`], this
    // frame's: `KillCamGateSet` runs before `OverlaySet::Toggle`).
    let settling = gate.is_some_and(|gate| gate.settling);
    if keys.just_pressed(SPAWN_MAP_KEY) && !(settling && !map.open) {
        // CLOSING is unconditional; OPENING requires that nothing latched outranks the map
        // ([`overlay::may_open`]). Without that gate, `M` pressed under the menu or the connect
        // screen latched a map the one-scrim rule immediately hid — invisible, yet input-blocking,
        // and it seized the cursor the instant the higher overlay closed.
        //
        // The gate reads the set, which is why this system lives in `OverlaySet::Toggle` rather than
        // with the declarers: the OPEN decision is a one-shot latch held in `map.open`, so a read of a
        // half-written generation does NOT self-heal — the absolute declaration below faithfully
        // re-asserts the wrong latch every frame after.
        map.open = !map.open && overlay::may_open(&overlays, Overlay::SpawnMap);
    }
    overlays.declare(Overlay::SpawnMap, map.open);
}

/// Keep the panel a centred square of the authored window fraction, and record
/// its px rect for the click math. Both the `Node` size and the recorded rect come from the same
/// number in the same frame, so the mapping cannot drift from what is drawn.
fn size_spawn_map(
    map: Res<SpawnMap>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut panel: Query<(&mut Node, &mut SpawnMapPanel)>,
) {
    // Writing `Node` marks it changed and re-lays-out the tree, so a closed map costs nothing. The
    // toggle declares in `OverlaySet::Toggle`, before this, so the opening frame is already sized.
    if !map.open {
        return;
    }
    // No primary window: it is closing at exit, and there is nothing to size.
    let Some(window) = crate::state::primary_window(windows.single()) else {
        return;
    };
    let (width, height) = (window.width(), window.height());
    let edge = width.min(height) * style().panel_fraction.0;
    let (mut node, mut rect) = panel.single_mut().expect(PANEL_SPAWNED);
    node.width = Val::Px(edge);
    node.height = Val::Px(edge);
    rect.edge_px = edge;
    // The backdrop centres the FRAME, whose border box is `edge + twice the authored border width`; the border sits
    // OUTSIDE the panel, so the panel's own top-left lands at `(window - edge) / 2` on both axes —
    // the border cancels exactly. (The title is absolutely positioned above, not stacked in a column,
    // so it displaces nothing.) That is why this restatement of the layout is exact rather than an
    // approximation, and why the recorded rect is the drawn IMAGE's rect, not a box around it.
    rect.origin_px = Vec2::new((width - edge) * 0.5, (height - edge) * 0.5);
}

/// A left click inside the panel picks a spawn point: panel-relative px → UV → world XZ → one
/// reliable [`SetSpawnPoint`]. Ignored unless the map is the scrim owner (`draws_scrim`), so a menu
/// opened on top of the map swallows clicks instead of re-placing the player's spawn.
fn click_spawn_map(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    overlays: Res<Overlays>,
    panel: Query<&SpawnMapPanel>,
    mut map: ResMut<SpawnMap>,
    mut senders: Query<&mut MessageSender<SetSpawnPoint>, With<Client>>,
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
) {
    if !map.open
        || !overlay::draws_scrim(&overlays, Overlay::SpawnMap)
        || !buttons.just_pressed(MouseButton::Left)
    {
        return;
    }
    // No primary window (closing at exit), or no cursor over it: nothing was clicked on the map.
    let Some(cursor) =
        crate::state::primary_window(windows.single()).and_then(Window::cursor_position)
    else {
        return;
    };
    let Some(uv) = panel_uv(panel.single().expect(PANEL_SPAWNED), cursor) else {
        return;
    };
    // Mirror the authority's coordinate clamp; geometry admission still belongs to the server.
    let half = world_half_extent(grid.as_deref());
    let world = clamp_to_spawn_limit(half, uv_to_world(half, uv));
    let mut sender = match senders.single_mut() {
        Ok(sender) => sender,
        // Between links there is no authority to receive a map choice.
        Err(bevy::ecs::query::QuerySingleError::NoEntities(_)) => {
            warn!("client: no server link — spawn point not sent");
            return;
        }
        Err(bevy::ecs::query::QuerySingleError::MultipleEntities(_)) => {
            panic!("a match client has one server link");
        }
    };
    map.chosen = Some(world);
    sender.send::<SpawnChannel>(SetSpawnPoint {
        x: world.x,
        z: world.y,
    });
    info!(
        "client: requested next spawn at ({:.1}, {:.1})",
        world.x, world.y
    );
}

/// Cursor → panel UV in 0..1, or `None` when the click is outside the panel. Window cursor
/// coordinates and UI px share the same origin (top-left, +y down), which is why this is a plain
/// subtract-and-divide.
fn panel_uv(panel: &SpawnMapPanel, cursor: Vec2) -> Option<Vec2> {
    if panel.edge_px <= 0.0 {
        return None;
    }
    let uv = (cursor - panel.origin_px) / panel.edge_px;
    (uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0).then_some(uv)
}

/// UV → world XZ. **ORIENTATION CONVENTION, pinned here and used by every direction of this
/// mapping:** the map reads as a top view with **-z UP** (north at the top of the panel), so image
/// row 0 / `uv.y = 0` is world `z = -HALF_EXTENT`, and `uv.x = 0` is world `x = -HALF_EXTENT`.
/// Both axes are then plain increasing maps of the same form — no flip anywhere.
///
/// This is not a free choice: it is the SAME convention `terrain_grid::HeightGrid` holds its
/// samples in (row-major, row = z, column = x, sample `(i, j)` at `x = -HALF + i·step`,
/// `z = -HALF + j·step` — after the decode has folded in the map's declared
/// `terrain_grid::RowOrder`), so the pixel the player clicks is the pixel whose height the authority
/// spawns them on. That holds only while [`UI_MAP_FILE`] is generated in the same row order; the
/// own-tank marker is the standing empirical check, and the tank dot sitting mirrored about the
/// panel's horizontal centreline while driving is what a disagreement looks like.
fn uv_to_world(half_extent: f32, uv: Vec2) -> Vec2 {
    Vec2::new(
        (uv.x * 2.0 - 1.0) * half_extent,
        (uv.y * 2.0 - 1.0) * half_extent,
    )
}

/// Mirror the authority's coordinate bound. Geometry can refuse a bounded point, and occupancy
/// can nudge an admitted one: the marker names a request, not a guaranteed destination.
pub(super) fn clamp_to_spawn_limit(half_extent: f32, world: Vec2) -> Vec2 {
    let limit = spawn_limit(half_extent);
    world.clamp(Vec2::splat(-limit), Vec2::splat(limit))
}

/// World XZ → UV, the exact inverse of [`uv_to_world`] (same convention), clamped to the panel so a
/// tank outside the terrain square still shows at the edge instead of drawing off-panel.
fn world_to_uv(half_extent: f32, world: Vec2) -> Vec2 {
    ((world / half_extent) * 0.5 + Vec2::splat(0.5)).clamp(Vec2::ZERO, Vec2::ONE)
}

/// Park both dots: the picked point (persistent client memory) and the player's own tank, projected
/// every frame while the map is open. Markers are absolutely positioned children of the panel, so
/// their offsets are just `uv * edge`, centred on the dot.
fn place_markers(
    map: Res<SpawnMap>,
    panel: Query<&SpawnMapPanel>,
    own: Query<(&GlobalTransform, Option<&super::disclosure::RespawnState>), With<Controlled>>,
    mut chosen: Query<
        (&mut Node, &mut Visibility, &mut BackgroundColor),
        (With<ChosenMarker>, Without<SelfMarker>),
    >,
    mut own_marker: Query<(&mut Node, &mut Visibility), (With<SelfMarker>, Without<ChosenMarker>)>,
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
) {
    if !map.open {
        return;
    }
    let half = world_half_extent(grid.as_deref());
    let edge = panel.single().expect(PANEL_SPAWNED).edge_px;
    let (mut chosen, mut own_marker) = (
        chosen.single_mut().expect(PANEL_SPAWNED),
        own_marker.single_mut().expect(PANEL_SPAWNED),
    );
    let place = |uv: Vec2, node: &mut Node| {
        node.left = Val::Px(uv.x * edge - style().marker_px.0 * 0.5);
        node.top = Val::Px(uv.y * edge - style().marker_px.0 * 0.5);
    };
    let own = crate::tank::controlled_one(own.single());
    let refused =
        own.is_some_and(|(_, state)| state.is_some_and(|state| state.pick_refusal.is_some()));
    chosen.2.set_if_neq(BackgroundColor(if refused {
        colour(&style().refused)
    } else {
        colour(&style().chosen)
    }));
    match map.chosen {
        Some(world) => {
            place(world_to_uv(half, world), &mut chosen.0);
            chosen.1.set_if_neq(Visibility::Inherited);
        }
        None => {
            chosen.1.set_if_neq(Visibility::Hidden);
        }
    }
    match own {
        Some((transform, _)) => {
            let pos = transform.translation();
            place(
                world_to_uv(half, Vec2::new(pos.x, pos.z)),
                &mut own_marker.0,
            );
            own_marker.1.set_if_neq(Visibility::Inherited);
        }
        None => {
            own_marker.1.set_if_neq(Visibility::Hidden);
        }
    }
}

/// Spawn the map once, hidden. The backdrop carries [`OverlayNode`], so the shared one-scrim
/// reconciler owns its visibility and the `on_add` hook stamps its `GlobalZIndex` — this module
/// never touches either.
fn spawn_map_overlay(mut commands: Commands, fonts: Res<UiFonts>, assets: Res<AssetServer>) {
    let heightmap = assets.load(crate::map::derived_asset(UI_MAP_FILE));
    commands
        .spawn((
            OverlayNode(Overlay::SpawnMap),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(colour(&style().backdrop)),
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            // The title is pinned ABOVE the panel absolutely rather than stacked in a column: the
            // panel then sits exactly at the window centre, which is what `size_spawn_map` restates
            // for the click math.
            parent.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    top: Val::Percent(
                        (1.0 - style().panel_fraction.0) * 50.0 - style().title_gap_percent.0,
                    ),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                children![(
                    Text::new("CLICK TO PICK — red: refused — M to close"),
                    TextFont {
                        font: fonts.hud.clone().into(),
                        font_size: FontSize::Px(style().title_font_px.0),
                        ..default()
                    },
                    TextColor(Color::WHITE),
                )],
            ));
            // The FRAME. It owns the border so the panel inside it does not (see [`SpawnMapPanel`]);
            // with `width`/`height` left `auto` it shrink-wraps its one fixed-size child, so nothing
            // has to re-derive the outer size when `size_spawn_map` resizes the panel. Centred by the
            // backdrop, its border box is `edge + twice the authored border width`, which puts the panel's own
            // top-left at exactly `(window - edge) / 2` — the rect `size_spawn_map` records.
            parent
                .spawn((
                    Node {
                        border: UiRect::all(Val::Px(style().border_px.0)),
                        ..default()
                    },
                    BorderColor::all(colour(&style().frame)),
                ))
                .with_children(|frame| {
                    frame
                        .spawn((
                            SpawnMapPanel {
                                edge_px: 0.0,
                                origin_px: Vec2::ZERO,
                            },
                            Node {
                                // Real size is written every frame by `size_spawn_map`. No border and
                                // no padding here, on purpose — that is what keeps this node's
                                // content, padding, and border boxes the same square.
                                width: Val::Px(0.0),
                                height: Val::Px(0.0),
                                ..default()
                            },
                            ImageNode::new(heightmap),
                        ))
                        .with_children(|panel| {
                            panel.spawn((
                                ChosenMarker,
                                marker_node(),
                                BackgroundColor(colour(&style().chosen)),
                                Visibility::Hidden,
                            ));
                            panel.spawn((
                                SelfMarker,
                                marker_node(),
                                BackgroundColor(colour(&style().own)),
                                Visibility::Hidden,
                            ));
                        });
                });
        });
}

/// The shared dot node — absolutely positioned inside the panel; `place_markers` writes left/top.
fn marker_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        width: Val::Px(style().marker_px.0),
        height: Val::Px(style().marker_px.0),
        ..default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The square these pure-mapping pins are made over: the flat-slab fixture world's,
    /// which `world_half_extent` answers with when no heightmap decoded.
    const HALF: f32 = crate::terrain_grid::FIXTURE_EXTENT.half_extent();

    /// The player's marker displays the authority's refusal, including while a respawn is pending.
    #[test]
    fn the_requested_marker_shows_refusal_independently_of_waiting() {
        use super::super::disclosure::RespawnState;
        use bevy::ecs::system::RunSystemOnce;
        for waiting in [false, true] {
            let mut app = App::new();
            mount_bare(&mut app);
            app.world_mut().resource_mut::<SpawnMap>().open = true;
            app.world_mut().resource_mut::<SpawnMap>().chosen = Some(Vec2::ZERO);
            let own = app
                .world_mut()
                .spawn((
                    Controlled,
                    GlobalTransform::default(),
                    RespawnState {
                        waiting,
                        pick_refusal: None,
                    },
                ))
                .id();
            app.world_mut().run_system_once(place_markers).unwrap();
            let colour = |world: &mut World| {
                world
                    .query_filtered::<&BackgroundColor, With<ChosenMarker>>()
                    .single(world)
                    .unwrap()
                    .0
            };
            let accepted = colour(app.world_mut());
            app.world_mut()
                .get_mut::<RespawnState>(own)
                .unwrap()
                .pick_refusal = Some(super::super::disclosure::SpawnPickRefusal::NoClearGround);
            app.world_mut().run_system_once(place_markers).unwrap();
            assert_ne!(
                colour(app.world_mut()),
                accepted,
                "refusal must change the marker itself"
            );
            app.world_mut()
                .get_mut::<RespawnState>(own)
                .unwrap()
                .pick_refusal = None;
            app.world_mut().run_system_once(place_markers).unwrap();
            assert_eq!(
                colour(app.world_mut()),
                accepted,
                "an admitted re-pick clears refusal"
            );
        }
    }

    /// The authority clears picks at the Battle boundary, so the client cannot advertise an old dot.
    #[test]
    fn a_new_battle_clears_the_requested_marker() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<SpawnMap>();
        world.init_resource::<Overlays>();
        world.init_resource::<ButtonInput<KeyCode>>();
        let battle = world
            .spawn(super::super::battle::BattleState::combat_fixture())
            .id();
        world.run_system_once(toggle_spawn_map).unwrap();
        world.resource_mut::<SpawnMap>().chosen = Some(Vec2::ZERO);
        world.run_system_once(toggle_spawn_map).unwrap();
        assert_eq!(world.resource::<SpawnMap>().chosen, Some(Vec2::ZERO));
        world
            .get_mut::<super::super::battle::BattleState>(battle)
            .unwrap()
            .number += 1; // Fixture next Battle.
        world.run_system_once(toggle_spawn_map).unwrap();
        assert_eq!(world.resource::<SpawnMap>().chosen, None);
    }

    fn panel(edge: f32, origin: Vec2) -> SpawnMapPanel {
        SpawnMapPanel {
            edge_px: edge,
            origin_px: origin,
        }
    }

    /// The panel centre is the world origin and the corners are the terrain corners under the
    /// pinned "-z up" convention: top-left = (-half, -half), bottom-right = (+half, +half).
    /// `world_to_uv` is the exact inverse, so the dot a player clicks and the dot drawn for their
    /// tank at that same point land on the same pixel; a tank driven past the terrain edge still
    /// shows, pinned to the panel edge.
    #[test]
    fn world_to_uv_inverts_uv_to_world() {
        assert_eq!(uv_to_world(HALF, Vec2::splat(0.5)), Vec2::ZERO);
        assert_eq!(
            uv_to_world(HALF, Vec2::ZERO),
            Vec2::splat(-HALF),
            "the top-left pixel is -x/-z — north (-z) is UP on the map",
        );
        assert_eq!(uv_to_world(HALF, Vec2::ONE), Vec2::splat(HALF));
        for uv in [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.25, 0.75),
            Vec2::new(0.5, 0.5),
            Vec2::new(1.0, 1.0),
        ] {
            let round_trip = world_to_uv(HALF, uv_to_world(HALF, uv));
            assert!(
                (round_trip - uv).length() < 1e-6,
                "uv {uv:?} round-tripped to {round_trip:?}",
            );
        }
        assert_eq!(world_to_uv(HALF, Vec2::splat(HALF * 4.0)), Vec2::ONE);
        assert_eq!(world_to_uv(HALF, Vec2::splat(-HALF * 4.0)), Vec2::ZERO);
    }

    /// Clicks outside the panel are refused, so the surrounding scrim is dead space rather than a
    /// silent placement at a clamped edge point.
    #[test]
    fn clicks_outside_the_panel_are_refused() {
        let p = panel(400.0, Vec2::new(100.0, 50.0));
        assert_eq!(
            panel_uv(&p, Vec2::new(300.0, 250.0)),
            Some(Vec2::splat(0.5))
        );
        assert_eq!(panel_uv(&p, Vec2::new(99.0, 250.0)), None, "left of panel");
        assert_eq!(panel_uv(&p, Vec2::new(300.0, 451.0)), None, "below panel");
        assert_eq!(
            panel_uv(&panel(0.0, Vec2::ZERO), Vec2::ZERO),
            None,
            "an unsized panel (first frame) never accepts a click",
        );
    }

    /// An edge click is clamped CLIENT-SIDE to the same square the authority accepts, so the request
    /// the server receives is already a fixed point of its own clamp — nothing moves on arrival. A
    /// click inside the limit is untouched, so the clamp only ever bites at the margin.
    #[test]
    fn edge_clicks_clamp_to_the_square_the_server_accepts() {
        let corner = clamp_to_spawn_limit(HALF, uv_to_world(HALF, Vec2::ONE));
        assert_eq!(corner, Vec2::splat(spawn_limit(HALF)));
        assert_eq!(
            clamp_to_spawn_limit(HALF, uv_to_world(HALF, Vec2::ZERO)),
            Vec2::splat(-spawn_limit(HALF)),
        );
        assert_eq!(
            clamp_to_spawn_limit(HALF, corner),
            corner,
            "the clamp is idempotent — the server re-clamping a sent point is a no-op",
        );
        let inside = Vec2::new(120.0, -400.0);
        assert_eq!(
            clamp_to_spawn_limit(HALF, inside),
            inside,
            "an ordinary click is passed through untouched",
        );
    }

    /// Whether the fixture's state-driven owner latches the connect screen this frame — the stand-in
    /// for `net::client::update_connect_status` reading the live link.
    #[derive(Resource)]
    struct LinkDown(bool);

    fn declare_connect_status(link: Res<LinkDown>, mut overlays: ResMut<Overlays>) {
        overlays.declare(Overlay::ConnectStatus, link.0);
    }

    /// The same-frame declare race on the `M` gate, scheduled rather than pure — `may_open`'s own unit
    /// tests prove the rule, this proves the toggle READS a settled set.
    ///
    /// The declarer is registered in `OverlaySet::Declare` and, deliberately, LAST: absent the
    /// `Declare → Toggle` chain the single-threaded executor would run `toggle_spawn_map` first,
    /// against a generation with no connect screen in it, and `may_open` would wave the map through.
    ///
    /// Phase 2 is why the race matters and why an absolute declaration does not save us: the OPEN
    /// decision is a one-shot latch in `SpawnMap::open`, so a map that won the race would be re-declared
    /// every frame afterwards and surface — input-blocking, cursor-grabbing — the instant the connect
    /// screen cleared, over a player who never saw a map.
    #[test]
    fn m_cannot_latch_a_map_under_an_overlay_declared_the_same_frame() {
        let mut app = App::new();
        app.init_resource::<Overlays>();
        app.world_mut()
            .resource_mut::<Overlays>()
            .declare_in_match(true);
        app.init_resource::<SpawnMap>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(LinkDown(true));
        app.configure_sets(
            Update,
            (
                overlay::OverlaySet::Declare,
                overlay::OverlaySet::Toggle,
                overlay::OverlaySet::Cursor,
            )
                .chain(),
        );
        app.add_systems(
            Update,
            (
                toggle_spawn_map.in_set(overlay::OverlaySet::Toggle),
                declare_connect_status.in_set(overlay::OverlaySet::Declare),
            ),
        );
        app.edit_schedule(Update, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });

        // Phase 1: `M` on the very frame the connect screen is declared.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(SPAWN_MAP_KEY);
        app.update();
        assert!(
            !app.world().resource::<SpawnMap>().open,
            "M pressed the frame a connect screen appears must latch no map",
        );

        // Phase 2: the link comes back and the connect screen withdraws. Nothing may be left latched.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut().resource_mut::<LinkDown>().0 = false;
        app.update();
        assert!(
            !app.world().resource::<SpawnMap>().open,
            "no map may surface once the connect screen clears",
        );
        assert!(
            !overlay::input_blocked(app.world().resource::<Overlays>(), true),
            "with nothing latched the cursor is the player's again — an invisible map would have \
             kept input blocked here",
        );
    }

    /// The UI map asset must stay 8-bit: a 16-bit grayscale PNG decodes to an `R16Uint` GPU
    /// texture, which `bevy_ui` cannot sample (`Float { filterable: true }` bind group) — the
    /// exact class of crash the `M` overlay shipped with (see [`UI_MAP_FILE`]). Square, so
    /// the panel's UV↔world mapping holds without letterboxing.
    #[test]
    fn ui_map_is_8_bit_and_square() {
        let path = crate::assets::asset_root().join(crate::map::derived_asset(UI_MAP_FILE));
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|err| panic!("UI map missing at {}: {err}", path.display()));
        let image = image::load_from_memory(&bytes).expect("UI map must decode");
        assert!(
            matches!(image, image::DynamicImage::ImageRgb8(_)),
            "UI map must be 8-bit RGB (got {:?}) — a 16-bit grayscale PNG decodes to a \
             non-filterable R16Uint texture and crashes bevy_ui",
            image.color(),
        );
        assert_eq!(image.width(), image.height(), "UI map must be square");
    }

    /// THE FULL CHAIN, end to end: heightmap rows → derived UI rows → panel UV → world XZ. A click
    /// on a hill in the panel must land on THAT hill in the world, which is only true if the derived
    /// image carries the same row order the decode folded into the grid ([`UI_MAP_FILE`]).
    ///
    /// Pinned as a correlation between what the panel DRAWS and what [`uv_to_world`] ANSWERS, over
    /// the whole square: brighter pixel, higher ground. The mirrored reading is measured alongside
    /// it rather than assumed absent — on this map it scores 0.56 against 1.00, so the test would
    /// fail loudly if the derived image were ever regenerated from the file's raw rows.
    #[test]
    fn ui_map_shows_the_world_the_click_math_points_at() {
        let path = crate::assets::asset_root().join(crate::map::derived_asset(UI_MAP_FILE));
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|err| panic!("UI map missing at {}: {err}", path.display()));
        let image = image::load_from_memory(&bytes)
            .expect("UI map must decode")
            .to_luma8();
        let grid = crate::terrain_grid::tests::shipped_grid();
        let half = grid.half_extent();
        let (mut drawn, mut mirrored, mut ground) = (Vec::new(), Vec::new(), Vec::new());
        const SAMPLES: u32 = 64;
        let pixel = |uv: Vec2| {
            let px = |t: f32, n: u32| ((t * n as f32) as u32).min(n - 1);
            f32::from(
                image
                    .get_pixel(px(uv.x, image.width()), px(uv.y, image.height()))
                    .0[0],
            )
        };
        for j in 0..SAMPLES {
            for i in 0..SAMPLES {
                let uv = (Vec2::new(i as f32, j as f32) + 0.5) / SAMPLES as f32;
                drawn.push(pixel(uv));
                mirrored.push(pixel(Vec2::new(uv.x, 1.0 - uv.y)));
                let world = uv_to_world(half, uv);
                ground.push(grid.height_at(world.x, world.y));
            }
        }
        let honest = correlation(&drawn, &ground);
        let flipped = correlation(&mirrored, &ground);
        assert!(
            honest > 0.95,
            "the panel's pixels and the click math's world disagree (correlation {honest:.3}) — \
             the derived UI map is not in the grid's row order",
        );
        assert!(
            flipped < honest - 0.2,
            "north–south mirroring the panel reads just as well ({flipped:.3} vs {honest:.3}), so \
             this claim proves nothing about orientation",
        );
    }

    /// Pearson correlation of two equal-length series — the orientation check's one statistic.
    fn correlation(a: &[f32], b: &[f32]) -> f32 {
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        let (ma, mb) = (mean(a), mean(b));
        let (mut cov, mut va, mut vb) = (0.0f32, 0.0f32, 0.0f32);
        for (x, y) in a.iter().zip(b) {
            cov += (x - ma) * (y - mb);
            va += (x - ma) * (x - ma);
            vb += (y - mb) * (y - mb);
        }
        cov / (va * vb).sqrt()
    }
}
