//! The view of the hull marks (`hull_marks`, the visual layer of the game's damage model): one
//! `ForwardDecal` per mark a vehicle's replicated [`HullMarks`] holds, hung under the struck
//! part's view node so it rides the part.
//!
//! # The diff
//!
//! [`draw_marks`] keeps each root's decals keyed by mark serial ([`DrawnMarks`]): a mark the record
//! holds and the root has not drawn is drawn, a decal whose mark fell off the record's front is
//! despawned. A decal is a pure function of its mark — nothing in it comes from arrival order, the
//! frame or a view-side random draw — so a late joiner handed the whole record at once draws what a
//! client that watched every mark arrive has drawn. A mark whose part has no view node yet (the glb
//! still loading) waits, and is drawn the frame the view binds.
//!
//! # One decal
//!
//! Two entities under the part's view node. The frame ([`HullMarkDecal`]) sits at the impact point
//! with +Y the face's outward normal and +X the shot's direction across the face, scaled to the
//! kind's footprint at the round's calibre ([`texture::footprint`]) and stretched along +X by
//! [`stretch`]. The surface under it ([`MarkSurface`]) is turned about +Y by the mark's roll, so
//! the roll varies the pattern while the stretch stays on the shot. A scratch is not rolled: its
//! gouge lies along the shot. [`dress_surface`], the render half, gives the surface its
//! `ForwardDecal` and its material, shared per (kind, variant).
//!
//! # What the forward decal cannot do (DERIVED from bevy_pbr 0.19's `forward.rs` and
//! `forward_decal.wgsl`)
//!
//! It is an overlay: `AlphaMode::Blend`, no shadows, depth test `Always`. It is lit with the quad's
//! normal, not the hull's, and cannot cut a real hole (issue 124's step 2 moves the profile into
//! the hull's shader for that). Its alpha fades with the distance between the quad and the surface
//! the depth prepass holds, over [`DEPTH_FADE_M`]: that fade is also what hides a mark behind any
//! surface standing more than a few centimetres in front of it, so it must stay small. Its
//! UV-parallax term divides the tangent by the world-space image of the scale vector, which is
//! exact only for an axis-aligned decal; on a rotated one it is wrong in proportion to the quad's
//! distance from the surface, which the fade keeps small.

pub(super) mod texture;

use std::collections::BTreeMap;

use bevy::camera::visibility::NoFrustumCulling;
use bevy::pbr::decal::{ForwardDecal, ForwardDecalMaterial, ForwardDecalMaterialExt};
use bevy::pbr::{ExtendedMaterial, ParallaxMappingMethod};
use bevy::prelude::*;

use crate::hull_marks::{HullMark, HullMarks, MarkKind, in_plane, parts_in_index_order, stretch};
use crate::tank::{SimParts, ViewNode};
use texture::VARIANTS;

/// How far from the surface a decal fades out, m. ASSUMED: "a few centimetres", against bevy's
/// 8 m default, which would draw a mark on a turret's far face through the turret.
pub(crate) const DEPTH_FADE_M: f32 = 0.04;

/// The material each (kind, variant) shares.
#[derive(Resource)]
pub(super) struct MarkMaterials {
    by_kind: [[Handle<ForwardDecalMaterial<StandardMaterial>>; VARIANTS]; 3],
}

impl MarkMaterials {
    fn get(
        &self,
        kind: MarkKind,
        variant: usize,
    ) -> Handle<ForwardDecalMaterial<StandardMaterial>> {
        self.by_kind[kind_index(kind)][variant % VARIANTS].clone()
    }
}

fn kind_index(kind: MarkKind) -> usize {
    MarkKind::ALL
        .iter()
        .position(|&each| each == kind)
        .expect("ALL lists every kind")
}

/// One drawn mark's frame: at the impact point, +Y the face normal, +X the shot across the face,
/// scaled to the mark's footprint. `serial` is the mark's in its vehicle's record.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HullMarkDecal {
    pub(super) serial: u32,
}

/// The surface of a drawn mark: which material it wears. [`dress_surface`] makes it a decal.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MarkSurface {
    pub(super) kind: MarkKind,
    pub(super) variant: usize,
}

/// A root's drawn marks: its parts in [`HullMark::part`] order, and the frame drawn for each mark
/// serial. View state only.
#[derive(Component, Default)]
pub(super) struct DrawnMarks {
    parts: Vec<Entity>,
    drawn: BTreeMap<u32, Entity>,
    /// A held mark is not drawn because its part's view has not bound yet.
    waiting: bool,
}

/// Where a mark's decal sits in its part's view frame, and how its surface is rolled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct DecalPose {
    pub(super) frame: Transform,
    pub(super) roll: Quat,
}

/// The decal pose a mark gets: its point, its face, its footprint at its calibre stretched along
/// the shot ([`stretch`]), and its roll from its seed.
pub(super) fn pose(mark: &HullMark) -> DecalPose {
    let normal = mark.normal();
    let along = mark.along();
    let across = in_plane(along, normal);
    // Right-handed: X × Y = Z.
    let side = across.cross(normal);
    let rotation = Quat::from_mat3(&Mat3::from_cols(across, normal, side));
    let footprint = texture::footprint(mark.kind) * mark.caliber();
    let frame = Transform {
        translation: mark.at(),
        rotation,
        scale: Vec3::new(
            footprint.x * stretch(along, normal),
            footprint.y,
            footprint.y,
        ),
    };
    let roll = match mark.kind {
        MarkKind::Scratch => Quat::IDENTITY,
        MarkKind::Dent | MarkKind::Hole => Quat::from_rotation_y(roll_angle(mark.seed)),
    };
    DecalPose { frame, roll }
}

/// The roll a seed gives, radians: its low ten bits around the circle.
fn roll_angle(seed: u16) -> f32 {
    f32::from(seed & 0x3FF) / 1024.0 * std::f32::consts::TAU
}

/// The texture variant a seed picks: its high bits.
pub(super) fn variant(seed: u16) -> usize {
    usize::from(seed >> 10) % VARIANTS
}

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, build_materials)
        .add_systems(Update, draw_marks)
        .add_observer(dress_surface);
}

/// Generate every (kind, variant)'s maps and material. The nine sets generate on their own
/// threads; the elapsed time is logged.
pub(super) fn build_materials(
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ForwardDecalMaterial<StandardMaterial>>>,
    mut commands: Commands,
) {
    let started = std::time::Instant::now();
    let sets: Vec<(MarkKind, usize, texture::MarkMaps)> = std::thread::scope(|scope| {
        // Every job is spawned before any is joined, so the sets generate side by side.
        let mut jobs = Vec::new();
        for kind in MarkKind::ALL {
            for variant in 0..VARIANTS {
                jobs.push(scope.spawn(move || (kind, variant, texture::generate(kind, variant))));
            }
        }
        jobs.into_iter()
            .map(|job| job.join().expect("a mark texture job panicked"))
            .collect()
    });
    let elapsed = started.elapsed();
    let mut by_kind: [[Handle<ForwardDecalMaterial<StandardMaterial>>; VARIANTS]; 3] =
        Default::default();
    for (kind, variant, maps) in sets {
        let base = StandardMaterial {
            base_color_texture: Some(images.add(maps.color)),
            normal_map_texture: Some(images.add(maps.normal)),
            metallic_roughness_texture: Some(images.add(maps.metal_rough)),
            // The texture carries both; the scalars pass it through.
            metallic: 1.0,
            perceptual_roughness: 1.0,
            depth_map: Some(images.add(maps.depth)),
            parallax_depth_scale: texture::parallax_depth(kind),
            parallax_mapping_method: ParallaxMappingMethod::Occlusion,
            max_parallax_layer_count: 12.0,
            // None: on a scratch's stretched decal any anisotropy blew the gouge out to white
            // (MEASURED, hidden-window captures at strength 0.4 and 0.6; at 0 it shades).
            anisotropy_strength: 0.0,
            // The extension blends the pipeline, but the shader reads the alpha mode from these
            // flags: left opaque, it writes alpha 1 and the whole quad shows.
            alpha_mode: AlphaMode::Blend,
            ..default()
        };
        by_kind[kind_index(kind)][variant] = materials.add(ExtendedMaterial {
            base,
            extension: ForwardDecalMaterialExt {
                depth_fade_factor: DEPTH_FADE_M,
            },
        });
    }
    info!(
        "hull marks: {} texture sets of {size}x{size} generated in {:.1} ms",
        MarkKind::ALL.len() * VARIANTS,
        elapsed.as_secs_f64() * 1000.0,
        size = texture::SIZE,
    );
    commands.insert_resource(MarkMaterials { by_kind });
}

/// Draw each root's held marks that are not drawn yet, and despawn the decals of marks its record
/// dropped. A root is revisited when its record changes, or while a mark waits for its view.
pub(super) fn draw_marks(
    mut roots: Query<(Entity, Ref<HullMarks>, &SimParts, Option<&mut DrawnMarks>)>,
    views: Query<&ViewNode>,
    mut commands: Commands,
) {
    for (root, record, parts, existing) in &mut roots {
        let mut created = None;
        let drawn = match existing {
            Some(drawn) => {
                if !record.is_changed() && !drawn.waiting {
                    continue;
                }
                drawn.into_inner()
            }
            None => created.insert(DrawnMarks {
                parts: parts_in_index_order(parts),
                ..default()
            }),
        };
        // A record behind what is drawn is not this record's continuation: redraw it whole.
        if drawn
            .drawn
            .last_key_value()
            .is_some_and(|(&serial, _)| serial >= record.recorded())
        {
            for (_, frame) in std::mem::take(&mut drawn.drawn) {
                commands.entity(frame).try_despawn();
            }
        }
        let kept = drawn.drawn.split_off(&record.first_serial());
        for (_, frame) in std::mem::replace(&mut drawn.drawn, kept) {
            commands.entity(frame).try_despawn();
        }
        drawn.waiting = false;
        for (serial, mark) in record.iter() {
            if drawn.drawn.contains_key(&serial) {
                continue;
            }
            let Some(&part) = drawn.parts.get(usize::from(mark.part)) else {
                warn!(
                    "hull marks: mark {serial} names part {} of {}; not drawn",
                    mark.part,
                    drawn.parts.len()
                );
                continue;
            };
            let Ok(view) = views.get(part) else {
                drawn.waiting = true;
                continue;
            };
            drawn
                .drawn
                .insert(serial, spawn_decal(&mut commands, view.0, serial, mark));
        }
        if let Some(created) = created {
            commands.entity(root).try_insert(created);
        }
    }
}

/// Spawn one mark's frame under `anchor`, its surface under the frame; returns the frame.
fn spawn_decal(commands: &mut Commands, anchor: Entity, serial: u32, mark: &HullMark) -> Entity {
    let pose = pose(mark);
    let frame = commands
        .spawn((
            Name::new("HullMark"),
            HullMarkDecal { serial },
            pose.frame,
            Visibility::default(),
            ChildOf(anchor),
        ))
        .id();
    commands.spawn((
        MarkSurface {
            kind: mark.kind,
            variant: variant(mark.seed),
        },
        Transform::from_rotation(pose.roll),
        Visibility::default(),
        ChildOf(frame),
    ));
    frame
}

/// The render half: a mark's surface becomes a forward decal wearing its (kind, variant)'s
/// material, in the flush that spawned it.
fn dress_surface(
    add: On<Add, MarkSurface>,
    surfaces: Query<&MarkSurface>,
    materials: Res<MarkMaterials>,
    mut commands: Commands,
) {
    let Ok(surface) = surfaces.get(add.entity) else {
        return;
    };
    commands.entity(add.entity).insert((
        ForwardDecal,
        MeshMaterial3d(materials.get(surface.kind, surface.variant)),
    ));
}

/// One surface per kind under `rig`, so each kind's pipeline compiles during the startup prewarm
/// rather than on the first hit. Unculled: nothing the camera sees has to be near them.
pub(super) fn spawn_prewarm(commands: &mut Commands, rig: Entity) {
    for kind in MarkKind::ALL {
        commands.spawn((
            MarkSurface { kind, variant: 0 },
            Transform::IDENTITY,
            Visibility::default(),
            NoFrustumCulling,
            ChildOf(rig),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ballistics::{ComponentHealth, Verdict};
    use crate::hull_marks::tests::{Fixture, fixture_in, numbered, plate_in_world, strike};
    use crate::hull_marks::{MARK_CAP, offline_plugin};

    /// What a client shows of a root's marks: per drawn frame, its serial, the view node it hangs
    /// under, its pose and its surface's material and roll. Sorted by serial.
    fn shown(app: &mut App) -> Vec<(u32, Entity, Transform, MarkSurface, Transform)> {
        let world = app.world_mut();
        let mut frames = world.query::<(&HullMarkDecal, &ChildOf, &Transform, &Children)>();
        let mut surfaces = world.query::<(&MarkSurface, &Transform)>();
        let mut shown: Vec<_> = frames
            .iter(world)
            .map(|(decal, parent, frame, children)| {
                let (surface, roll) = surfaces
                    .get(world, children[0])
                    .expect("a frame's child is its surface");
                (decal.serial, parent.parent(), *frame, *surface, *roll)
            })
            .collect();
        shown.sort_by_key(|row| row.0);
        shown
    }

    /// A root with three sim parts, each bound to a view node, and an empty record: the client's
    /// side of a struck vehicle. Returns the root and the view nodes in part order.
    fn client_tank(app: &mut App, bind_views: bool) -> (Entity, Vec<Entity>) {
        let world = app.world_mut();
        let root = world.spawn(HullMarks::default()).id();
        let mut parts = std::collections::HashMap::new();
        let mut views = Vec::new();
        for name in ["Part_A", "Part_B", "Part_C"] {
            let sim = world.spawn((Name::new(name), ChildOf(root))).id();
            let view = world.spawn(Name::new(name)).id();
            if bind_views {
                world.entity_mut(sim).insert(ViewNode(view));
            }
            parts.insert(name.to_owned(), sim);
            views.push(view);
        }
        world.entity_mut(root).insert(SimParts(parts));
        (root, views)
    }

    fn view_app() -> App {
        let mut app = App::new();
        app.add_systems(Update, draw_marks);
        app
    }

    /// A late joiner, handed the whole record at once — and its view bound only after the record
    /// arrived — draws exactly what a client that watched every mark arrive (and saw the oldest
    /// dropped past the cap) has drawn: the same serials, under the same view nodes, at the same
    /// poses, wearing the same materials and rolls. ADR-0039 needs every observer's hull to
    /// match; a mutant that seeds a decal from arrival order or a view-side random draw, or that
    /// leaves a dropped mark's decal standing, breaks it.
    #[test]
    fn a_late_joiner_draws_the_marks_a_watcher_drew() {
        let mut watcher = view_app();
        let (root, _) = client_tank(&mut watcher, true);
        let total = MARK_CAP as u16 + 12;
        for n in 0..total {
            watcher
                .world_mut()
                .get_mut::<HullMarks>(root)
                .unwrap()
                .record(numbered(n));
            watcher.update();
        }
        let record = watcher.world().get::<HullMarks>(root).unwrap().clone();

        let mut joiner = view_app();
        let (late, views) = client_tank(&mut joiner, false);
        *joiner.world_mut().get_mut::<HullMarks>(late).unwrap() = record;
        joiner.update();
        assert!(
            shown(&mut joiner).is_empty(),
            "no view bound, nothing drawn"
        );
        let parts = joiner.world().get::<SimParts>(late).unwrap().0.clone();
        for (name, sim) in parts {
            let view = views
                .iter()
                .copied()
                .find(|&view| joiner.world().get::<Name>(view).unwrap().as_str() == name)
                .unwrap();
            joiner.world_mut().entity_mut(sim).insert(ViewNode(view));
        }
        joiner.update();

        let name = |app: &App, entity: Entity| app.world().get::<Name>(entity).unwrap().to_string();
        let watched: Vec<_> = shown(&mut watcher)
            .into_iter()
            .map(|(serial, view, frame, surface, roll)| {
                (serial, name(&watcher, view), frame, surface, roll)
            })
            .collect();
        let joined: Vec<_> = shown(&mut joiner)
            .into_iter()
            .map(|(serial, view, frame, surface, roll)| {
                (serial, name(&joiner, view), frame, surface, roll)
            })
            .collect();
        assert_eq!(
            watched.len(),
            MARK_CAP,
            "the dropped marks' decals are gone"
        );
        let serials: Vec<u32> = watched.iter().map(|row| row.0).collect();
        let expected: Vec<u32> = (u32::from(total) - MARK_CAP as u32..u32::from(total)).collect();
        assert_eq!(serials, expected);
        assert_eq!(watched, joined);
    }

    /// Recording a strike and drawing its mark leave the struck tank's simulation as it was: no
    /// simulation component changes, and nothing is hung under a sim part — every decal hangs
    /// under a view node (ADR-0014: the sim never reads a mark, and a mark never writes the sim).
    /// A mutant that parents a decal to the sim part, or records by touching the part's pose,
    /// breaks it.
    #[test]
    fn marks_never_touch_the_simulation() {
        let mut app = view_app();
        app.add_plugins(offline_plugin);
        let fixture = fixture_in(app.world_mut());
        let Fixture {
            root,
            turret,
            plate,
            collider,
        } = fixture;
        let at = plate_in_world(app.world(), &fixture).transform_point(Vec3::new(0.1, 0.0, -0.05));
        let world = app.world_mut();
        world.entity_mut(plate).insert(ComponentHealth::full(30));
        let sims: Vec<Entity> = world
            .get::<SimParts>(root)
            .unwrap()
            .0
            .values()
            .copied()
            .collect();
        for &sim in &sims {
            let view = world.spawn(Name::new("view")).id();
            world.entity_mut(sim).insert(ViewNode(view));
        }
        app.update();

        let world = app.world_mut();
        let ticks = |world: &World| {
            let root = world.entity(root);
            let turret = world.entity(turret);
            let plate = world.entity(plate);
            (
                root.get_change_ticks::<avian3d::prelude::Position>()
                    .unwrap()
                    .changed,
                root.get_change_ticks::<avian3d::prelude::Rotation>()
                    .unwrap()
                    .changed,
                turret.get_change_ticks::<Transform>().unwrap().changed,
                plate.get_change_ticks::<Transform>().unwrap().changed,
                plate.get_change_ticks::<ComponentHealth>().unwrap().changed,
                *plate.get::<ComponentHealth>().unwrap(),
            )
        };
        let children = |world: &World| -> Vec<Vec<Entity>> {
            sims.iter()
                .map(|&sim| {
                    world
                        .get::<Children>(sim)
                        .map(|children| children.to_vec())
                        .unwrap_or_default()
                })
                .collect()
        };
        let before = (ticks(world), children(world));
        for verdict in [Verdict::Ricochet, Verdict::Stopped, Verdict::Through] {
            world.trigger(strike(collider, at, verdict));
        }
        app.update();
        app.update();

        let world = app.world_mut();
        assert_eq!(
            world.get::<HullMarks>(root).unwrap().len(),
            3,
            "the strikes were marked"
        );
        let drawn = shown(&mut app);
        assert_eq!(drawn.len(), 3, "and drawn");
        let world = app.world();
        for (_, anchor, ..) in &drawn {
            assert!(
                !sims.contains(anchor),
                "a decal hangs under a view node, never a sim part"
            );
        }
        assert_eq!((ticks(world), children(world)), before);
    }

    /// The decal's frame puts +Y on the face normal and +X on the shot across the face, its
    /// footprint stretched along +X only, by the stretch law. Pins the composition the late-joiner
    /// test treats as opaque: a mutant that stretches across the shot, or along the normal, draws
    /// an oblique hole sideways.
    #[test]
    fn a_decal_lies_on_its_face_stretched_along_the_shot() {
        let theta = 60f32.to_radians();
        let along = Vec3::new(theta.sin(), -theta.cos(), 0.0);
        let mark = HullMark::pack(MarkKind::Hole, 0, Vec3::ZERO, along, Vec3::Y, 0.088, 0).unwrap();
        let DecalPose { frame, .. } = pose(&mark);
        assert!((frame.rotation * Vec3::Y).angle_between(Vec3::Y) < 0.01);
        assert!((frame.rotation * Vec3::X).angle_between(Vec3::X) < 0.01);
        let footprint = texture::footprint(MarkKind::Hole) * mark.caliber();
        assert!(
            (frame.scale.x / footprint.x - 2.0).abs() < 0.02,
            "{:?}",
            frame.scale
        );
        assert!((frame.scale.z - footprint.y).abs() < 1e-6);
    }
}
