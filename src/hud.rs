//! Shared HUD layers and placement for game and armor sandbox, plus pooled world nameplates.
//!
//! [`HudLayer`] owns semantic draw order. [`HudDock`] panels reserve their measured edge bounds
//! for modal content before UI layout. The design/evidence record is
//! `.agents/docs/design/view-sweep-review.md`.
//!
//! Invariant: labels read simulation state but never write it; the camera it reprojects through is
//! the composition's declared [`PlayerView`], never a camera implementation of its own.

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::ui::{ComputedUiRenderTargetInfo, ComputedUiTargetCamera, UiSystems};

use crate::net::NetBot;

use crate::camera::GunnerCameraPlaced;
use crate::flow::{Flow, FlowScoped, InMatchSystems};
use crate::spec::{TankSpec, TankSpecHandle};
use crate::tank::Tank;
use crate::ui_font::{UiFonts, set_text};
use crate::view::PlayerView;

/// The HUD's semantic draw order, independent of plugin/spawn order. World labels sit above
/// sight furniture, fixed panels above labels, and modal boards above both. Full-screen overlay
/// precedence remains owned by [`crate::overlay::Overlay`]. A modal card
/// supplies its own opaque backing (`ui_font::MODAL_CARD_BG`); stamping a layer sets only z order.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[component(on_add = stamp_hud_layer)]
pub(crate) enum HudLayer {
    WorldLabel,
    Fixed,
    Inset,
    Modal,
}

impl HudLayer {
    fn zindex(self) -> i32 {
        use crate::overlay::Overlay;
        match self {
            Self::WorldLabel => Overlay::WORLD_LABEL_Z,
            Self::Fixed => Overlay::FIXED_HUD_Z,
            Self::Inset => Overlay::PEN_CAM_CORNER_Z,
            Self::Modal => Overlay::SCOREBOARD_Z,
        }
    }
}

fn stamp_hud_layer(mut world: DeferredWorld, HookContext { entity, .. }: HookContext) {
    let layer = *world
        .entity(entity)
        .get::<HudLayer>()
        .expect("HudLayer is present in its own on_add hook");
    world
        .commands()
        .entity(entity)
        .insert(GlobalZIndex(layer.zindex()));
}

/// A fixed panel reserves its actual laid-out box for modal placement.
/// These are root nodes on the player's UI target, with axis-aligned UI transforms. Top is used
/// by status/clock, bottom by crew/drive, and right by diagnostics. Hidden panels reserve nothing.
#[derive(Component, Clone, Copy)]
#[require(HudLayer = HudLayer::Fixed)]
pub(crate) enum HudDock {
    Top,
    Bottom,
    Right,
}

/// The game's damage-model view: the inset's root UI occupies the right dock. This adapter
/// joins the presentation rig to the shared HUD placement without changing its stage entities.
/// DERIVED from `damage_cam::pen_cam::timeline::presence`: the inset scales at most to its layout
/// box, so reserving that box clears the whole entrance/exit animation.
/// Temporary encounter/death cards remain overlay content: the held board deliberately covers
/// them, as specified by `Overlay::SCOREBOARD_Z`; they do not reserve permanent HUD space.
fn dock_corner_ui(
    add: On<Add, crate::damage_cam::pen_cam::CornerRig>,
    nodes: Query<(), With<Node>>,
    mut commands: Commands,
) {
    // A presentation rig also has world meshes, lights and cameras. Only its UI root docks.
    if nodes.contains(add.entity) {
        commands
            .entity(add.entity)
            .insert((HudDock::Right, HudLayer::Inset));
    }
}

/// ASSUMED spacing between fixed panels and modal cards, in logical UI pixels.
const MODAL_GAP_PX: f32 = 8.0;

/// The previous completed layout supplies panel extents for this layout. Neither text length,
/// font size nor a viewport percentage stands in for measured panel geometry. Target propagation
/// has run; the modal's new Node is consumed by this frame's layout. Resizing/text changes settle
/// on the next layout, just as the UI's scroll geometry does.
fn place_modal_hud(
    panels: Query<(
        &HudDock,
        &ComputedNode,
        &UiGlobalTransform,
        &Visibility,
        &ComputedUiTargetCamera,
    )>,
    mut modals: Query<
        (
            &HudLayer,
            &ComputedUiRenderTargetInfo,
            &ComputedUiTargetCamera,
            &mut Node,
        ),
        Without<HudDock>,
    >,
) {
    for (layer, target, camera, mut node) in &mut modals {
        if *layer != HudLayer::Modal {
            continue;
        }
        let size = target.logical_size();
        // The window can be unsized/minimized before its camera publishes a usable viewport.
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        let scale = target.scale_factor();
        let gap = MODAL_GAP_PX.min(size.min_element() * 0.5);
        let screen = Rect::from_corners(Vec2::splat(gap), (size - gap).max(Vec2::splat(gap)));
        let occupied =
            panels
                .iter()
                .filter_map(|(_, panel, transform, visibility, panel_camera)| {
                    if *visibility == Visibility::Hidden || panel_camera != camera {
                        return None; // Hidden panels and other UI targets occupy no space here.
                    }
                    let bounds =
                        Rect::from_center_size(transform.translation / scale, panel.size() / scale);
                    Some(Rect::from_corners(
                        bounds.min - Vec2::splat(gap),
                        bounds.max + Vec2::splat(gap),
                    ))
                });
        let area = largest_clear_region(screen, occupied);
        let placement = Node {
            left: Val::Px(area.min.x),
            top: Val::Px(area.min.y),
            width: Val::Px(area.width()),
            height: Val::Px(area.height()),
            ..node.clone()
        };
        node.set_if_neq(placement);
    }
}

/// Maximal clear rectangles retain space beside a tall panel instead of reserving its entire
/// edge band. Every candidate is contained in the target and outside all occupied panel boxes.
fn largest_clear_region(screen: Rect, occupied: impl Iterator<Item = Rect>) -> Rect {
    let mut free = vec![screen];
    for panel in occupied {
        let mut next = Vec::new();
        for area in free {
            if area.max.x <= panel.min.x
                || area.min.x >= panel.max.x
                || area.max.y <= panel.min.y
                || area.min.y >= panel.max.y
            {
                next.push(area);
                continue;
            }
            for piece in [
                Rect {
                    min: area.min,
                    max: Vec2::new(panel.min.x.min(area.max.x), area.max.y),
                },
                Rect {
                    min: Vec2::new(panel.max.x.max(area.min.x), area.min.y),
                    max: area.max,
                },
                Rect {
                    min: area.min,
                    max: Vec2::new(area.max.x, panel.min.y.min(area.max.y)),
                },
                Rect {
                    min: Vec2::new(area.min.x, panel.max.y.max(area.min.y)),
                    max: area.max,
                },
            ] {
                if piece.min.x < piece.max.x && piece.min.y < piece.max.y {
                    next.push(piece);
                }
            }
        }
        // Contained rectangles cannot beat their containing candidate, including equal duplicates.
        free = next
            .iter()
            .enumerate()
            .filter(|(i, area)| {
                !next.iter().enumerate().any(|(j, other)| {
                    j != *i
                        && other.min.x <= area.min.x
                        && other.min.y <= area.min.y
                        && other.max.x >= area.max.x
                        && other.max.y >= area.max.y
                        && (other != *area || j < *i)
                })
            })
            .map(|(_, area)| *area)
            .collect();
    }
    match free
        .into_iter()
        .max_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
    {
        Some(area) => area,
        // A minimized target or panels covering it completely has no readable modal region.
        None => Rect::from_corners(screen.min, screen.min),
    }
}

/// A pooled tank nameplate: reprojected each frame, its text rewritten only when the name changes.
#[derive(Component)]
struct TankNameplate;

pub fn plugin(app: &mut App) {
    crate::flow::require(app, "hud");
    app.add_observer(dock_corner_ui)
        .add_systems(OnEnter(Flow::InMatch), spawn_labels)
        .add_systems(
            PostUpdate,
            place_modal_hud
                .after(UiSystems::Content)
                .before(UiSystems::Layout)
                .in_set(InMatchSystems),
        )
        .add_systems(
            PostUpdate,
            // Reprojection runs after propagation and gunner-camera placement (`GunnerCameraPlaced`,
            // itself after `Propagate`), so labels use the same frame's transforms as the rendered
            // model. Read any earlier, they see the previous frame's tank and camera globals while the
            // mesh renders from this frame's — a skew that is not constant (Avian re-interpolates the
            // overstep and the orbit camera re-tracks every frame), so the labels jitter against the
            // model. Same read-after-Propagate idiom as the gunner reticles (`sight`) and the trace
            // recorder.
            update_tank_nameplates
                .after(TransformSystems::Propagate)
                .after(GunnerCameraPlaced)
                .in_set(InMatchSystems),
        );
}

fn spawn_labels(mut commands: Commands, fonts: Res<UiFonts>) {
    // Nameplate pool, just the display name, reprojected each frame and hidden while unused. Four
    // plates: tanks past them, in query order, go unnamed (TUNED for the duel; a fuller match
    // wants a bigger pool).
    for _ in 0..4 {
        commands.spawn((
            FlowScoped(Flow::InMatch),
            TankNameplate,
            HudLayer::WorldLabel,
            Text::new(""),
            TextFont {
                // SemiBold: a tank identity chip floated over the world — reads as a label.
                font: fonts.hud.clone().into(),
                font_size: FontSize::Px(15.0),
                ..default()
            },
            TextColor(Color::srgb(0.90, 0.96, 1.0)),
            Node {
                position_type: PositionType::Absolute,
                padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.06, 0.08, 0.55)),
            Visibility::Hidden,
        ));
    }
}

/// Mount the real label pool without its game-state readers for HUD layout tests.
#[cfg(test)]
pub(crate) fn mount_layout_test(app: &mut App) {
    app.add_observer(dock_corner_ui)
        .add_systems(Startup, spawn_labels)
        .add_systems(
            PostUpdate,
            place_modal_hud
                .after(UiSystems::Content)
                .before(UiSystems::Layout),
        );
}

#[cfg(test)]
pub(crate) fn labels_for_test(world: &mut World) -> Vec<Entity> {
    world
        .query_filtered::<Entity, With<TankNameplate>>()
        .iter(world)
        .collect()
}

/// Float a nameplate over each tank the pool reaches, in query order, reprojected to screen; hide
/// the leftover plates. Anchored a little above the hull so it clears the model. The controlled
/// tank's aggregate status is the fixed corner panel's (`crew_ui::update_status_panel`).
fn update_tank_nameplates(
    camera: Single<(&Camera, &GlobalTransform), With<PlayerView>>,
    tanks: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&Name>,
            Option<&TankSpecHandle>,
        ),
        With<Tank>,
    >,
    specs: Res<Assets<TankSpec>>,
    // A bot carries the replicated `NetBot` marker (`Name` doesn't ride the wire), so its nameplate
    // is prefixed `[BOT]`.
    bots: Query<(), With<NetBot>>,
    mut labels: Query<(&mut Node, &mut Text, &mut Visibility), With<TankNameplate>>,
    mut scratch: Local<String>,
) {
    let (camera, cam_transform) = *camera;
    let mut tanks = tanks.iter();
    for (mut node, mut text, mut visibility) in &mut labels {
        let Some((entity, transform, name, spec)) = tanks.next() else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let world_point = transform.translation() + Vec3::Y * 4.4;
        match camera.world_to_viewport(cam_transform, world_point) {
            Ok(screen) => {
                // `Name` does not ride the wire, so a replicated tank's label is its spec sheet's
                // `display_name`; a tank with neither (spec still loading) gets no plate rather than
                // an invented label.
                let label = name.map(Name::as_str).or_else(|| {
                    spec.and_then(|handle| specs.get(&handle.0))
                        .map(|spec| spec.display_name.as_str())
                });
                let Some(label) = label else {
                    visibility.set_if_neq(Visibility::Hidden);
                    continue;
                };
                node.left = Val::Px(screen.x + 12.0);
                node.top = Val::Px(screen.y - 20.0);
                scratch.clear();
                if bots.get(entity).is_ok() {
                    scratch.push_str("[BOT] ");
                }
                scratch.push_str(label);
                set_text(text.reborrow(), &scratch);
                visibility.set_if_neq(Visibility::Visible);
            }
            Err(_) => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::camera::{ComputedCameraValues, RenderTargetInfo};

    use super::*;

    /// Frames in which `Text` was flagged changed, counted by a detector scheduled after the writer.
    #[derive(Resource, Default)]
    struct TextChanges(u32);

    fn count_text_changes(
        changed: Query<(), (With<TankNameplate>, Changed<Text>)>,
        mut counter: ResMut<TextChanges>,
    ) {
        counter.0 += changed.iter().count() as u32;
    }

    /// Invariant: a frame in which no displayed name changes marks no nameplate `Text` changed
    /// (a changed `Text` is re-measured and re-laid-out by `bevy_ui`), while the plate's screen
    /// position still follows the tank every frame. The HUD relies on both: the first is its
    /// layout cost, the second is that a plate never lags the model it labels.
    #[test]
    fn nameplate_text_written_only_on_change_but_position_follows_every_frame() {
        let mut world = World::new();
        world.init_resource::<Assets<TankSpec>>();
        world.init_resource::<TextChanges>();
        world.spawn((
            Camera {
                computed: ComputedCameraValues {
                    clip_from_view: Mat4::perspective_infinite_reverse_rh(1.0, 16.0 / 9.0, 0.1),
                    target_info: Some(RenderTargetInfo {
                        physical_size: UVec2::new(1280, 720),
                        scale_factor: 1.0,
                    }),
                    ..default()
                },
                ..default()
            },
            PlayerView,
            GlobalTransform::default(),
        ));
        let tank = world
            .spawn((
                Tank,
                Name::new("Fixture"),
                GlobalTransform::from_xyz(0.0, 0.0, -30.0),
            ))
            .id();
        let plate = world
            .spawn((
                TankNameplate,
                Text::new(""),
                Node::default(),
                Visibility::Hidden,
            ))
            .id();
        let mut schedule = Schedule::default();
        schedule.add_systems((update_tank_nameplates, count_text_changes).chain());

        schedule.run(&mut world);
        assert_eq!(
            world.resource::<TextChanges>().0,
            1,
            "first sight writes the name"
        );
        assert_eq!(world.get::<Text>(plate).unwrap().0, "Fixture");
        let left = world.get::<Node>(plate).unwrap().left;

        world
            .entity_mut(tank)
            .insert(GlobalTransform::from_xyz(5.0, 0.0, -30.0));
        schedule.run(&mut world);
        assert_eq!(
            world.resource::<TextChanges>().0,
            1,
            "an unchanged name must not mark the text changed"
        );
        assert_ne!(
            world.get::<Node>(plate).unwrap().left,
            left,
            "the plate follows the tank"
        );

        world.entity_mut(tank).insert(Name::new("Renamed"));
        schedule.run(&mut world);
        assert_eq!(
            world.resource::<TextChanges>().0,
            2,
            "a new name is written"
        );
        assert_eq!(world.get::<Text>(plate).unwrap().0, "Renamed");
    }

    /// A tank with neither `Name` nor spec data gets no plate: the label comes from data, never a literal.
    #[test]
    fn unnamed_tank_without_spec_gets_no_plate() {
        let mut world = World::new();
        world.init_resource::<Assets<TankSpec>>();
        world.spawn((
            Camera {
                computed: ComputedCameraValues {
                    clip_from_view: Mat4::perspective_infinite_reverse_rh(1.0, 16.0 / 9.0, 0.1),
                    target_info: Some(RenderTargetInfo {
                        physical_size: UVec2::new(1280, 720),
                        scale_factor: 1.0,
                    }),
                    ..default()
                },
                ..default()
            },
            PlayerView,
            GlobalTransform::default(),
        ));
        world.spawn((Tank, GlobalTransform::from_xyz(0.0, 0.0, -30.0)));
        let plate = world
            .spawn((
                TankNameplate,
                Text::new(""),
                Node::default(),
                Visibility::Visible,
            ))
            .id();
        let mut schedule = Schedule::default();
        schedule.add_systems(update_tank_nameplates);
        schedule.run(&mut world);
        assert_eq!(*world.get::<Visibility>(plate).unwrap(), Visibility::Hidden);
    }

    /// A replicated tank carries no `Name` (it does not ride the wire) but does carry its spec
    /// handle: the plate reads the spec sheet's `display_name`, bots prefixed `[BOT]`. Online
    /// nameplates rely on this.
    #[test]
    fn nameless_tank_shows_its_spec_display_name() {
        let mut world = World::new();
        world.init_resource::<Assets<TankSpec>>();
        world.spawn((
            Camera {
                computed: ComputedCameraValues {
                    clip_from_view: Mat4::perspective_infinite_reverse_rh(1.0, 16.0 / 9.0, 0.1),
                    target_info: Some(RenderTargetInfo {
                        physical_size: UVec2::new(1280, 720),
                        scale_factor: 1.0,
                    }),
                    ..default()
                },
                ..default()
            },
            PlayerView,
            GlobalTransform::default(),
        ));
        let mut spec: TankSpec =
            ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron"))
                .expect("the shipped sheet parses");
        spec.display_name = "Fixture Display".to_string();
        let handle = world.resource_mut::<Assets<TankSpec>>().add(spec);
        world.spawn((
            Tank,
            NetBot,
            TankSpecHandle(handle),
            GlobalTransform::from_xyz(0.0, 0.0, -30.0),
        ));
        let plate = world
            .spawn((
                TankNameplate,
                Text::new(""),
                Node::default(),
                Visibility::Hidden,
            ))
            .id();
        let mut schedule = Schedule::default();
        schedule.add_systems(update_tank_nameplates);
        schedule.run(&mut world);
        assert_eq!(world.get::<Text>(plate).unwrap().0, "[BOT] Fixture Display");
        assert_eq!(
            *world.get::<Visibility>(plate).unwrap(),
            Visibility::Visible
        );
    }
}
