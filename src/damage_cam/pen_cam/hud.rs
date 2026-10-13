//! The flat half: the corner's frame over the stage camera's picture — a 1 px border, the accent
//! bar in the verdict colour, the verdict glyph, the progress hairline — the sight's reticle, and
//! the demo's own labels.
//!
//! The corner sits top-right, 16 px in, 16:10, 30 % of the window's logical width clamped to
//! 384–640 px, on [`Overlay::PEN_CAM_CORNER_Z`]. The stage camera renders into an image the corner
//! shows, so the corner can fade and scale as a whole; a replacement crossfades from the last frame
//! of the old image while the camera draws the new record into the other one. The corner draws no
//! text: the glyphs are rasterized shapes. The demo's labels use Bevy's default font, ASCII only.

use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::stage::StageCamera;
use super::timeline::{self, CornerKind};
use super::world::MainCamera;
use super::{CornerSetting, DoseScale, Library, PLAYLIST, Run, Settings};
use crate::overlay::Overlay;

const INSET_PX: f32 = 16.0;
const WIDTH_FRACTION: f32 = 0.30;
const WIDTH_PX: (f32, f32) = (384.0, 640.0);
const ASPECT: f32 = 1.6;
const ACCENT_PX: f32 = 3.0;
const HAIRLINE_PX: f32 = 2.0;
const GLYPH_PX: f32 = 20.0;
const GLYPH_INSET_PX: f32 = 10.0;
const GLYPH_TEXELS: u32 = 64;

#[derive(Component)]
pub(super) struct CornerRoot;

/// One of the two images the stage camera renders into.
#[derive(Component)]
pub(super) struct CornerImage(usize);

#[derive(Component)]
pub(super) struct Accent;

#[derive(Component)]
pub(super) struct Hairline;

#[derive(Component)]
pub(super) struct Glyph;

#[derive(Component)]
pub(super) struct Reticle;

#[derive(Component)]
pub(super) struct Label;

/// The two render images, which one the camera draws into, and their physical size.
#[derive(Resource)]
pub(super) struct CornerImages {
    images: [Handle<Image>; 2],
    live: usize,
    size: UVec2,
    scale: f32,
    turn: Option<usize>,
}

impl CornerImages {
    /// The corner image's size (physical px) and scale factor.
    pub(super) fn size_and_scale(&self) -> (UVec2, f32) {
        (self.size, self.scale)
    }
}

/// One rasterized glyph per frame kind.
#[derive(Resource)]
pub(super) struct Glyphs {
    images: Vec<(CornerKind, Handle<Image>)>,
}

pub fn spawn(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let glyphs = Glyphs {
        images: [
            CornerKind::Glass,
            CornerKind::Slice,
            CornerKind::Ricochet,
            CornerKind::Damaged,
            CornerKind::Victim,
        ]
        .into_iter()
        .map(|kind| (kind, images.add(glyph_image(kind))))
        .collect(),
    };
    let blank = images.add(Image::new_target_texture(
        16,
        10,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    commands
        .spawn((
            CornerRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(INSET_PX),
                right: Val::Px(INSET_PX),
                width: Val::Px(WIDTH_PX.0),
                height: Val::Px(WIDTH_PX.0 / ASPECT),
                overflow: Overflow::clip(),
                ..default()
            },
            GlobalZIndex(Overlay::PEN_CAM_CORNER_Z),
            UiTransform::from_scale(Vec2::splat(0.96)),
            Visibility::Hidden,
        ))
        .with_children(|corner| {
            for index in 0..2 {
                corner.spawn((
                    CornerImage(index),
                    ImageNode::new(blank.clone()),
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                ));
            }
            corner.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.12)),
            ));
            corner.spawn((
                Accent,
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(0.0),
                    height: Val::Px(ACCENT_PX),
                    ..default()
                },
                BackgroundColor(Color::WHITE),
            ));
            corner.spawn((
                Glyph,
                ImageNode::new(glyphs.images[0].1.clone()),
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(GLYPH_INSET_PX),
                    left: Val::Px(GLYPH_INSET_PX),
                    width: Val::Px(GLYPH_PX),
                    height: Val::Px(GLYPH_PX),
                    ..default()
                },
            ));
            corner.spawn((
                Hairline,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Px(HAIRLINE_PX),
                    ..default()
                },
                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.55)),
            ));
        });
    // The reticle: four bars around the screen's centre.
    commands
        .spawn((
            Reticle,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn(Node {
                width: Val::Px(64.0),
                height: Val::Px(64.0),
                ..default()
            })
            .with_children(|cross| {
                let bar = |left: f32, top: f32, w: f32, h: f32| {
                    (
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(left),
                            top: Val::Px(top),
                            width: Val::Px(w),
                            height: Val::Px(h),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.05, 0.05, 0.05, 0.85)),
                    )
                };
                cross.spawn(bar(0.0, 31.0, 24.0, 2.0));
                cross.spawn(bar(40.0, 31.0, 24.0, 2.0));
                cross.spawn(bar(31.0, 40.0, 2.0, 24.0));
                cross.spawn(bar(31.0, 0.0, 2.0, 18.0));
            });
        });
    commands.spawn((
        Label,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(13.0),
            ..default()
        },
        TextColor(Color::srgba(0.95, 0.96, 0.98, 0.85)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(14.0),
            bottom: Val::Px(12.0),
            ..default()
        },
    ));
    commands.insert_resource(CornerImages {
        images: [blank.clone(), blank],
        live: 0,
        size: UVec2::ZERO,
        scale: 1.0,
        turn: None,
    });
    commands.insert_resource(glyphs);
}

/// Size the corner from the window's logical width, (re)make its render images at that size in
/// physical pixels, and point the stage camera at the live one; a replacement swaps them.
pub fn layout(
    settings: Res<Settings>,
    corner: Res<super::Corner>,
    main: Query<&Camera, With<MainCamera>>,
    mut stage_camera: Query<&mut RenderTarget, With<StageCamera>>,
    mut root: Query<&mut Node, With<CornerRoot>>,
    mut nodes: Query<(&CornerImage, &mut ImageNode)>,
    mut target: ResMut<CornerImages>,
    mut images: ResMut<Assets<Image>>,
) {
    let Ok(main) = main.single() else {
        return;
    };
    let (Some(window), Some(scale)) = (main.logical_target_size(), main.target_scaling_factor())
    else {
        return;
    };
    let (width, height) = if settings.expanded {
        (window.x - 2.0 * INSET_PX, window.y - 2.0 * INSET_PX)
    } else {
        let width = (window.x * WIDTH_FRACTION).clamp(WIDTH_PX.0, WIDTH_PX.1);
        (width, width / ASPECT)
    };
    if let Ok(mut node) = root.single_mut()
        && (node.width != Val::Px(width) || node.height != Val::Px(height))
    {
        node.width = Val::Px(width);
        node.height = Val::Px(height);
    }
    let size = (Vec2::new(width, height) * scale)
        .round()
        .as_uvec2()
        .max(UVec2::ONE);
    let mut retarget = false;
    if size != target.size || scale != target.scale {
        target.images = [0, 1].map(|_| {
            images.add(Image::new_target_texture(
                size.x,
                size.y,
                TextureFormat::Rgba8UnormSrgb,
                None,
            ))
        });
        target.size = size;
        target.scale = scale;
        retarget = true;
    }
    let turn = corner.showing.map(|showing| showing.turn);
    if turn != target.turn {
        if turn.is_some() && target.turn.is_some() && corner.fading.is_some() {
            target.live = 1 - target.live;
            retarget = true;
        }
        target.turn = turn;
    }
    if retarget {
        if let Ok(mut render_target) = stage_camera.single_mut() {
            *render_target = RenderTarget::Image(bevy::camera::ImageRenderTarget {
                handle: target.images[target.live].clone(),
                scale_factor: scale,
            });
        }
        for (index, mut node) in &mut nodes {
            node.image = target.images[index.0].clone();
        }
    }
}

/// What `R` does on this run, as the legend names it: on a kill run it respawns, the game's
/// binding, and a drag looks while the camera is free.
pub(super) fn legend_r(kill: bool) -> &'static str {
    if kill {
        "R respawn  drag look"
    } else {
        "R replay"
    }
}

/// The corner's presence, accent, glyph and hairline; the reticle; the labels.
pub fn animate(
    corner: Res<super::Corner>,
    run: Res<Run>,
    library: Res<Library>,
    settings: Res<Settings>,
    target: Res<CornerImages>,
    glyphs: Res<Glyphs>,
    mut root: Query<(&mut Visibility, &mut UiTransform), With<CornerRoot>>,
    mut images: Query<(&CornerImage, &mut ImageNode), Without<Glyph>>,
    mut accent: Query<(&mut Node, &mut BackgroundColor), (With<Accent>, Without<Hairline>)>,
    mut hairline: Query<(&mut Node, &mut BackgroundColor), (With<Hairline>, Without<Accent>)>,
    mut glyph: Query<&mut ImageNode, With<Glyph>>,
    mut reticle: Query<&mut Visibility, (With<Reticle>, Without<CornerRoot>)>,
    mut label: Query<&mut Text, With<Label>>,
) {
    let entry = &PLAYLIST[run.entry];
    if let Ok(mut visibility) = reticle.single_mut() {
        visibility.set_if_neq(if entry.victim {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        });
    }
    if let Ok(mut text) = label.single_mut() {
        let verdicts: Vec<String> = run
            .shots
            .iter()
            .map(|shot| format!("{:?}", library.records[shot.record].report.impact.verdict))
            .collect();
        let corner_setting = match settings.corner {
            CornerSetting::All => "all",
            CornerSetting::ThroughOnly => "through only",
            CornerSetting::Off => "off",
        };
        let dose = match settings.dose {
            DoseScale::Absolute => "absolute",
            DoseScale::HpShare => "hp share",
        };
        let line = format!(
            "{}  [{}]\nt {:.2} s  x{}{}{}  corner {}  dose {}  glass {}  volume {}  rim {}  \
             look {}  slice {}{}\n\
             1-9 0 - = scenario  A auto  {}  Space pause  Left/Right scrub  [ ] speed  \
             X dismiss  C hold  V own tank  P setting  F expand  O ortho  G glass  D dose  \
             K volume  J rim  H look  L axis  M reduce motion",
            entry.label,
            verdicts.join(", "),
            run.t,
            super::SPEEDS[run.speed],
            if run.paused { "  paused" } else { "" },
            if run.auto { "  auto" } else { "" },
            corner_setting,
            dose,
            settings.glass,
            settings.volume.label(),
            settings.field_rim.label(),
            settings.look.label(),
            if settings.slice_ortho {
                "ortho"
            } else {
                "perspective"
            },
            if settings.reduce_motion {
                "  reduce motion"
            } else {
                ""
            },
            legend_r(run.kill.is_some()),
        );
        if text.0 != line {
            text.0 = line;
        }
    }
    let Ok((mut visibility, mut transform)) = root.single_mut() else {
        return;
    };
    // The kill cam draws on the world, not in the corner.
    let Some(showing) = corner
        .showing
        .filter(|showing| showing.surface == super::Surface::Corner)
    else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    visibility.set_if_neq(Visibility::Inherited);
    let scale = Vec2::splat(showing.scale);
    if transform.scale != scale {
        transform.scale = scale;
    }
    for (image, mut node) in &mut images {
        let alpha = if image.0 == target.live {
            showing.opacity
        } else {
            corner.fading.unwrap_or(0.0) * showing.opacity
        };
        node.color = Color::WHITE.with_alpha(alpha);
    }
    let color = timeline::verdict_color(showing.kind);
    if let Ok((mut node, mut background)) = accent.single_mut() {
        let drawn = timeline::ease_enter(timeline::progress(showing.c, 0.0, timeline::ACCENT_S));
        node.width = Val::Percent(100.0 * drawn);
        background.0 = color.with_alpha(showing.opacity);
    }
    if let Ok((mut node, mut background)) = hairline.single_mut() {
        node.width = Val::Percent(100.0 * showing.remaining);
        background.0 = Color::srgba(1.0, 1.0, 1.0, 0.55 * showing.opacity);
    }
    if let Ok(mut node) = glyph.single_mut() {
        if let Some((_, handle)) = glyphs.images.iter().find(|(k, _)| *k == showing.kind)
            && node.image != *handle
        {
            node.image = handle.clone();
        }
        node.color = color.with_alpha(showing.opacity);
    }
}

/// A frame kind's glyph, white on clear, for tinting: drawn strokes, not font characters.
///
/// Through: an arrow crossing a bar and continuing. Stopped: an arrow ending in a bar. Ricochet: an
/// arrow bending off a bar. Damaged: an arrow ending in a filled box. Own tank: an empty box.
fn glyph_image(kind: CornerKind) -> Image {
    type Seg = ((f32, f32), (f32, f32));
    let head = |tip: (f32, f32), back: (f32, f32)| -> [Seg; 2] {
        let d = Vec2::new(tip.0 - back.0, tip.1 - back.1).normalize() * 0.2;
        let side = Vec2::new(-d.y, d.x);
        let a = Vec2::from(tip) - d + side * 0.8;
        let b = Vec2::from(tip) - d - side * 0.8;
        [(tip, (a.x, a.y)), (tip, (b.x, b.y))]
    };
    let mut strokes: Vec<Seg> = Vec::new();
    let mut boxes: Vec<(Vec2, Vec2)> = Vec::new();
    match kind {
        CornerKind::Glass => {
            strokes.push(((0.5, 0.12), (0.5, 0.88)));
            strokes.push(((0.06, 0.5), (0.94, 0.5)));
            strokes.extend(head((0.94, 0.5), (0.06, 0.5)));
        }
        CornerKind::Slice => {
            strokes.push(((0.8, 0.12), (0.8, 0.88)));
            strokes.push(((0.06, 0.5), (0.74, 0.5)));
            strokes.extend(head((0.74, 0.5), (0.06, 0.5)));
        }
        CornerKind::Ricochet => {
            strokes.push(((0.06, 0.86), (0.94, 0.86)));
            strokes.push(((0.1, 0.14), (0.5, 0.78)));
            strokes.push(((0.5, 0.78), (0.9, 0.14)));
            strokes.extend(head((0.9, 0.14), (0.5, 0.78)));
        }
        CornerKind::Damaged => {
            strokes.push(((0.06, 0.5), (0.5, 0.5)));
            strokes.extend(head((0.5, 0.5), (0.06, 0.5)));
            boxes.push((Vec2::new(0.6, 0.3), Vec2::new(0.94, 0.7)));
        }
        CornerKind::Victim => {
            strokes.push(((0.14, 0.24), (0.86, 0.24)));
            strokes.push(((0.86, 0.24), (0.86, 0.76)));
            strokes.push(((0.86, 0.76), (0.14, 0.76)));
            strokes.push(((0.14, 0.76), (0.14, 0.24)));
        }
    }
    let half_width = 0.055;
    let texel = 1.0 / GLYPH_TEXELS as f32;
    let mut data = Vec::with_capacity((GLYPH_TEXELS * GLYPH_TEXELS * 4) as usize);
    for y in 0..GLYPH_TEXELS {
        for x in 0..GLYPH_TEXELS {
            let p = Vec2::new((x as f32 + 0.5) * texel, (y as f32 + 0.5) * texel);
            let mut distance = f32::INFINITY;
            for (a, b) in &strokes {
                let (a, b) = (Vec2::from(*a), Vec2::from(*b));
                let ab = b - a;
                let f = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
                distance = distance.min(p.distance(a + ab * f) - half_width);
            }
            for (lo, hi) in &boxes {
                let outside = (*lo - p).max(p - *hi).max(Vec2::ZERO).length();
                let inside = (*lo - p).max(p - *hi).max_element().min(0.0);
                distance = distance.min(outside + inside);
            }
            let coverage = (0.5 - distance / texel).clamp(0.0, 1.0);
            data.extend([255, 255, 255, (coverage * 255.0).round() as u8]);
        }
    }
    Image::new(
        Extent3d {
            width: GLYPH_TEXELS,
            height: GLYPH_TEXELS,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}
