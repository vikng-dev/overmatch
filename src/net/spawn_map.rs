//! Net-client spawn map: `M` opens a top view of the terrain, a click picks where the player's NEXT
//! respawn lands.
//!
//! The click is a REQUEST ([`SetSpawnPoint`]) — nothing teleports, nothing moves now. The authority
//! validates the bounds, resolves the ground height itself, and consumes the override at the
//! player's next respawn (`net::server::respawn_player_tanks`). This module is view + input only.
//!
//! It joins the `overlay::Overlays` authority as [`Overlay::SpawnMap`], which is what releases the
//! cursor (a click needs a cursor position) and zeroes the wire command while the map is up.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use lightyear::prelude::*;

use super::protocol::{SetSpawnPoint, SpawnChannel};
use crate::overlay::{self, Overlay, OverlayNode, Overlays};
use crate::tank::Controlled;
use crate::ui_font::UiFonts;

/// Opens / closes the map. Free in the game client: W/A/S/D + Space drive, R respawns, T cycles,
/// V/Lshift switch view, Tab is SP-only, F/G/X are the dev-tools toggles, 1–5 are crew stations,
/// F3 is the debug panel, Esc is the menu. (The track sandbox's `M` is a different binary.)
const SPAWN_MAP_KEY: KeyCode = KeyCode::KeyM;

/// Half-extent of the terrain square, metres — the LIVE world's, read off the decoded grid
/// (`terrain_grid::world_extent`, which answers for the flat-slab fallback world too), never
/// re-stated here. Both the server clamp ([`spawn_limit`]) and the map's UV↔world mapping take it,
/// so a re-authored world moves the map and the clamp together.
pub(super) fn world_half_extent(grid: Option<&crate::terrain_grid::HeightGrid>) -> f32 {
    crate::terrain_grid::world_extent(grid).half_extent()
}

/// Fraction of the half-extent a spawn request is clamped to: a tank placed at the limit still has
/// ground under its whole hull.
const SPAWN_LIMIT_FRACTION: f32 = 0.95;

/// How far into the terrain a spawn request is clamped, metres.
///
/// ONE home for the bound, read by BOTH ends: the authority clamps every request through it
/// (`net::server::validate_spawn_request`), and the client applies the SAME clamp before sending
/// ([`clamp_to_spawn_limit`]). That symmetry is what makes the marker honest — an edge click used to
/// draw its dot the whole 5 % outside the square the server would actually place the tank in, so the
/// player was shown one point and respawned at another.
pub(crate) fn spawn_limit(half_extent: f32) -> f32 {
    half_extent * SPAWN_LIMIT_FRACTION
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

/// Panel edge as a fraction of the smaller window dimension — a centred square that always fits.
const PANEL_FRACTION: f32 = 0.8;

/// Marker dot edge, px.
const MARKER_PX: f32 = 12.0;

/// The panel's visible frame, px. It lives on a WRAPPER node ([`spawn_map_overlay`]), never on the
/// panel itself — see [`SpawnMapPanel`] for why the panel must stay border-free.
const PANEL_BORDER_PX: f32 = 2.0;

/// Client-side map state. `chosen` is the client's MEMORY of the last point it sent (the feedback
/// loop: the marker persists across map open/close and across respawns, and the next respawn
/// empirically lands under it). There is no server→client confirmation message — the reliable
/// client→server lane plus the observable respawn is the ack.
#[derive(Resource, Default)]
struct SpawnMap {
    open: bool,
    chosen: Option<Vec2>,
}

/// The full-screen backdrop + its centred square image panel. The panel's px rect is the ONE thing
/// the click math needs, so it is written by [`size_spawn_map`] rather than inferred from layout.
///
/// **The panel node carries NO border and NO padding, deliberately.** Bevy 0.19's `ImageNode` draws
/// into the node's CONTENT box (`VisualBox::ContentBox`) and absolutely-positioned children are laid
/// out against the PADDING box, while `Node::width` under the default `BoxSizing::BorderBox` names the
/// OUTER box. Put a 2 px border on this node and those three boxes stop coinciding: the recorded rect
/// describes a square 4 px larger than the drawn image, which skewed every click by up to ~6.4 m at
/// the terrain edge. The frame therefore lives on a wrapper ([`PANEL_BORDER_PX`]), leaving content ==
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
    app.init_resource::<SpawnMap>()
        .add_systems(Startup, spawn_map_overlay)
        .add_systems(
            Update,
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
            ),
        );
}

/// `M` toggles the map; the declaration into [`Overlays`] is absolute and re-run every frame, so it
/// self-heals like every other overlay owner. Deliberately NOT in `PlayerInputSet`: that set is
/// gated on the cursor being LOCKED, and this overlay releases the cursor — reading M there would
/// make the map impossible to close.
fn toggle_spawn_map(
    keys: Res<ButtonInput<KeyCode>>,
    mut map: ResMut<SpawnMap>,
    mut overlays: ResMut<Overlays>,
) {
    if keys.just_pressed(SPAWN_MAP_KEY) {
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

/// Keep the panel a centred square of [`PANEL_FRACTION`] × the smaller window dimension, and record
/// its px rect for the click math. Both the `Node` size and the recorded rect come from the same
/// number in the same frame, so the mapping cannot drift from what is drawn.
fn size_spawn_map(
    map: Res<SpawnMap>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut panel: Single<(&mut Node, &mut SpawnMapPanel)>,
) {
    // Writing `Node` marks it changed and re-lays-out the tree, so a closed map costs nothing. The
    // toggle declares in `OverlaySet::Toggle`, before this, so the opening frame is already sized.
    if !map.open {
        return;
    }
    let (width, height) = (window.width(), window.height());
    let edge = width.min(height) * PANEL_FRACTION;
    let (node, rect) = &mut *panel;
    node.width = Val::Px(edge);
    node.height = Val::Px(edge);
    rect.edge_px = edge;
    // The backdrop centres the FRAME, whose border box is `edge + 2·PANEL_BORDER_PX`; the border sits
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
    window: Single<&Window, With<PrimaryWindow>>,
    overlays: Res<Overlays>,
    panel: Single<&SpawnMapPanel>,
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
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Some(uv) = panel_uv(&panel, cursor) else {
        return;
    };
    // Clamp BEFORE recording the marker, so `chosen` is the point the authority will accept rather
    // than the raw pixel: marker = truth.
    let half = world_half_extent(grid.as_deref());
    let world = clamp_to_spawn_limit(half, uv_to_world(half, uv));
    map.chosen = Some(world);
    let Ok(mut sender) = senders.single_mut() else {
        warn!("client: no server link — spawn point not sent");
        return;
    };
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

/// Clamp a picked world XZ into the placeable square, applying the SAME [`spawn_limit`] the
/// authority applies — one rule, read by both ends, so this is a mirror rather than a guess.
///
/// The map maps clicks across the FULL ±`half_extent`, but the server only ever places a tank
/// within ±[`spawn_limit`]. Before this, an edge click drew its marker on the terrain edge and
/// then respawned the player the 5 % gap inward of it. Clamping here (and marking the clamped point) makes
/// the server's clamp a no-op on everything the client sends: the dot IS the destination.
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
    panel: Single<&SpawnMapPanel>,
    own: Query<&GlobalTransform, With<Controlled>>,
    mut chosen: Single<(&mut Node, &mut Visibility), (With<ChosenMarker>, Without<SelfMarker>)>,
    mut own_marker: Single<(&mut Node, &mut Visibility), (With<SelfMarker>, Without<ChosenMarker>)>,
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
) {
    if !map.open {
        return;
    }
    let half = world_half_extent(grid.as_deref());
    let edge = panel.edge_px;
    let place = |uv: Vec2, node: &mut Node| {
        node.left = Val::Px(uv.x * edge - MARKER_PX * 0.5);
        node.top = Val::Px(uv.y * edge - MARKER_PX * 0.5);
    };
    match map.chosen {
        Some(world) => {
            place(world_to_uv(half, world), &mut chosen.0);
            chosen.1.set_if_neq(Visibility::Inherited);
        }
        None => {
            chosen.1.set_if_neq(Visibility::Hidden);
        }
    }
    match own.iter().next() {
        Some(transform) => {
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
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75)),
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
                    top: Val::Percent((1.0 - PANEL_FRACTION) * 50.0 - 4.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                children![(
                    Text::new("CLICK TO SET SPAWN — M to close"),
                    TextFont {
                        font: fonts.hud.clone().into(),
                        font_size: FontSize::Px(22.0),
                        ..default()
                    },
                    TextColor(Color::WHITE),
                )],
            ));
            // The FRAME. It owns the border so the panel inside it does not (see [`SpawnMapPanel`]);
            // with `width`/`height` left `auto` it shrink-wraps its one fixed-size child, so nothing
            // has to re-derive the outer size when `size_spawn_map` resizes the panel. Centred by the
            // backdrop, its border box is `edge + 2·PANEL_BORDER_PX`, which puts the panel's own
            // top-left at exactly `(window - edge) / 2` — the rect `size_spawn_map` records.
            parent
                .spawn((
                    Node {
                        border: UiRect::all(Val::Px(PANEL_BORDER_PX)),
                        ..default()
                    },
                    BorderColor::all(Color::srgb(0.8, 0.8, 0.85)),
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
                                BackgroundColor(Color::srgb(0.2, 1.0, 0.4)),
                                Visibility::Hidden,
                            ));
                            panel.spawn((
                                SelfMarker,
                                marker_node(),
                                BackgroundColor(Color::srgb(1.0, 0.85, 0.2)),
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
        width: Val::Px(MARKER_PX),
        height: Val::Px(MARKER_PX),
        ..default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The square these pure-mapping pins are made over: the flat-slab fallback world's,
    /// which `world_half_extent` answers with when no heightmap decoded.
    const HALF: f32 = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
    const LIMIT: f32 = HALF * SPAWN_LIMIT_FRACTION;

    fn panel(edge: f32, origin: Vec2) -> SpawnMapPanel {
        SpawnMapPanel {
            edge_px: edge,
            origin_px: origin,
        }
    }

    /// The panel centre is the world origin, and the corners are the terrain corners under the
    /// pinned "-z up" convention: top-left = (-half, -half), bottom-right = (+half, +half).
    #[test]
    fn uv_maps_the_panel_onto_the_terrain_square() {
        assert_eq!(uv_to_world(HALF, Vec2::splat(0.5)), Vec2::ZERO);
        assert_eq!(
            uv_to_world(HALF, Vec2::ZERO),
            Vec2::splat(-HALF),
            "the top-left pixel is -x/-z — north (-z) is UP on the map",
        );
        assert_eq!(uv_to_world(HALF, Vec2::ONE), Vec2::splat(HALF));
    }

    /// `world_to_uv` is the exact inverse, so the dot a player clicks and the dot drawn for their
    /// tank at that same point land on the same pixel.
    #[test]
    fn world_to_uv_inverts_uv_to_world() {
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
    }

    /// A tank driven past the terrain edge still shows, pinned to the panel edge.
    #[test]
    fn world_to_uv_clamps_outside_the_square() {
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

    /// The spawn clamp stays inside the terrain square with real margin, on whatever square the
    /// live world turns out to be.
    #[test]
    fn spawn_limit_is_inside_the_terrain() {
        for half in [HALF, 750.0, 12.5] {
            assert!(spawn_limit(half) < half);
            assert!(spawn_limit(half) > half * 0.9);
        }
    }

    /// An edge click is clamped CLIENT-SIDE to the same square the authority accepts, so the request
    /// the server receives is already a fixed point of its own clamp — nothing moves on arrival. A
    /// click inside the limit is untouched, so the clamp only ever bites at the margin.
    #[test]
    fn edge_clicks_clamp_to_the_square_the_server_accepts() {
        let corner = clamp_to_spawn_limit(HALF, uv_to_world(HALF, Vec2::ONE));
        assert_eq!(corner, Vec2::splat(LIMIT));
        assert_eq!(
            clamp_to_spawn_limit(HALF, uv_to_world(HALF, Vec2::ZERO)),
            Vec2::splat(-LIMIT),
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

    /// MARKER = TRUTH: the dot drawn for an edge click sits at the CLAMPED point, visibly inside the
    /// panel edge, not on it. This is the finding — the old marker sat 64 m outside the square the
    /// server would place the tank in, promising a spawn that could never happen.
    #[test]
    fn the_marker_shows_the_clamped_point_not_the_raw_click() {
        let raw = uv_to_world(HALF, Vec2::ONE);
        let clamped = clamp_to_spawn_limit(HALF, raw);
        assert!(
            (raw - clamped).max_element() > 1.0,
            "an edge click really is moved by the clamp ({raw:?} → {clamped:?})",
        );
        let uv = world_to_uv(HALF, clamped);
        assert!(
            uv.cmplt(Vec2::ONE).all() && uv.cmpgt(Vec2::ZERO).all(),
            "the clamped marker draws strictly inside the panel, at {uv:?}",
        );
        // 95 % of the half-extent maps to 97.5 % of the panel under the centred 0..1 mapping.
        assert!(
            (uv - Vec2::splat(0.975)).length() < 1e-6,
            "marker at {uv:?}"
        );
    }

    /// The frame's border is OUTSIDE the recorded panel rect, so the panel's top-left is the plain
    /// window-centred offset — the exact identity `size_spawn_map` relies on. Restated here as the
    /// algebra rather than the layout, so a future border change that breaks the cancellation shows
    /// up as a failing arithmetic claim instead of a silent few-pixel click skew.
    #[test]
    fn the_border_cancels_out_of_the_panel_origin() {
        let (window, edge) = (1920.0f32, 864.0f32);
        // Backdrop centres the frame's BORDER box; the panel is its content.
        let frame_outer = edge + 2.0 * PANEL_BORDER_PX;
        let frame_origin = (window - frame_outer) * 0.5;
        let panel_origin = frame_origin + PANEL_BORDER_PX;
        assert!(
            (panel_origin - (window - edge) * 0.5).abs() < 1e-4,
            "panel origin {panel_origin} must equal the plain centred offset",
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
