//! GUARD for the vendored `bevy_pbr`'s mesh-instance removal path (`OVERMATCH_PATCH.md`, "Mesh
//! instance removals"; issue #173): the render world's mesh instances stay exactly the main world's
//! visible meshes through every lifecycle edge the removal path serves.
//!
//! Each test drives a real `PbrPlugin` render world through one family of edges and, after every
//! frame, checks [`Lab::frame`]'s invariants:
//!
//! - every mesh instance belongs to a live main-world entity that holds a `Mesh3d` and is visible,
//!   and its input slot holds that entity's current translation;
//! - input slots are owned one-to-one: none leaked, none shared, none freed under an instance;
//! - the GPU-culled set is exactly the instances flagged `NO_CPU_CULLING`, and every view's
//!   GPU-culled visible set lies inside it.
//!
//! A test asserts presence where the edge decides it (shown again, material ready, mesh back). A
//! wgpu error fails the test ([`Lab::new`]'s `RenderErrorHandler`): bevy's default stops rendering,
//! which would leave the invariants checking a stale render world. Every family passes on stock
//! 0.19.1 too (MEASURED), so a failure here is a patch's doing.
//!
//! The tests need a GPU adapter, which CI (`ubuntu-latest`) lacks: `RenderPlugin` with no backend
//! builds no render world at all (`bevy_shadow_view_render_layers.rs`). So they are `#[ignore]`d
//! and run by hand on a machine with one; they never open a window:
//! `scripts/serial-cargo test --lib --features dev_ui,bitprobe bevy_mesh_removal -- --ignored`.

use std::any::TypeId;
use std::collections::HashSet;

use bevy::camera::RenderTarget;
use bevy::camera::visibility::NoCpuCulling;
use bevy::pbr::{
    MeshInputUniform, MeshUniform, RenderGpuCulledEntities, RenderMeshInstanceFlags,
    RenderMeshInstanceGpuQueues, RenderMeshInstances, collect_meshes_for_gpu_building,
};
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::BatchedInstanceBuffers;
use bevy::render::error_handler::RenderErrorHandler;
use bevy::render::render_resource::TextureFormat;
use bevy::render::sync_world::MainEntity;
use bevy::render::view::{ExtractedView, NoIndirectDrawing, RenderVisibleEntities};
use bevy::render::{Render, RenderApp, RenderSystems};

/// In front of the default camera (at the origin, looking down -Z).
const IN_VIEW: Vec3 = Vec3::new(0.0, 0.0, -5.0);
/// Behind the camera: frustum-culled, so not visible.
const BEHIND: Vec3 = Vec3::new(0.0, 0.0, 5.0);
/// A step small enough to stay on the same side of the camera.
const NUDGE: Vec3 = Vec3::new(0.01, 0.0, 0.0);
/// Frames a hidden mesh keeps moving in each test.
const MOVING_FRAMES: usize = 5;

/// One frame's queued removals: the main entity, and whether it had an instance to remove.
#[derive(Resource, Default)]
struct QueuedRemovals(Vec<(Entity, bool)>);

/// After extraction, before `collect_meshes_for_gpu_building` applies the removals.
fn record_queued_removals(
    mut queues: ResMut<RenderMeshInstanceGpuQueues>,
    instances: Res<RenderMeshInstances>,
    mut recorded: ResMut<QueuedRemovals>,
) {
    recorded.0.clear();
    recorded.0.extend(
        crate::render_cost::queued_mesh_removals(&mut queues)
            .map(|entity| (entity.id(), instances.mesh_asset_id(entity).is_some())),
    );
}

struct Lab {
    app: App,
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    camera: Entity,
    target: Handle<Image>,
}

impl Lab {
    /// A windowless app on the machine's GPU, its render world built synchronously so each
    /// `update` extracts and prepares that frame, with one camera drawing into an image.
    fn new() -> Self {
        let mut app = App::new();
        app.add_plugins(
            crate::default_plugins()
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .disable::<bevy::winit::WinitPlugin>()
                .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
                .disable::<bevy::log::LogPlugin>()
                .disable::<bevy::audio::AudioPlugin>(),
        );
        // Bevy's default answer to a wgpu error is to stop rendering and request an exit, which
        // would leave every later frame's render world stale under the invariants.
        app.insert_resource(RenderErrorHandler(|error, _, _| {
            panic!(
                "the renderer reported a {:?} error: {}",
                error.ty, error.description
            )
        }));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app.sub_app_mut(RenderApp)
            .init_resource::<QueuedRemovals>()
            .add_systems(
                Render,
                record_queued_removals
                    .in_set(RenderSystems::PrepareMeshes)
                    .before(collect_meshes_for_gpu_building),
            );

        let world = app.world_mut();
        let target = world
            .resource_mut::<Assets<Image>>()
            .add(Image::new_target_texture(
                64,
                64,
                TextureFormat::Rgba8UnormSrgb,
                None,
            ));
        let mesh = world.resource_mut::<Assets<Mesh>>().add(Cuboid::default());
        let material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let camera = world
            .spawn((
                Camera3d::default(),
                RenderTarget::Image(target.clone().into()),
            ))
            .id();
        let mut lab = Lab {
            app,
            mesh,
            material,
            camera,
            target,
        };
        lab.frame();
        assert!(
            matches!(
                lab.app
                    .sub_app(RenderApp)
                    .world()
                    .resource::<RenderMeshInstances>(),
                RenderMeshInstances::GpuBuilding(_)
            ),
            "this adapter builds mesh uniforms on the CPU, so the GPU-building removal path is \
             not what these tests exercise"
        );
        lab
    }

    fn cube(&mut self, at: Vec3) -> Entity {
        let bundle = (
            Mesh3d(self.mesh.clone()),
            MeshMaterial3d(self.material.clone()),
            Transform::from_translation(at),
        );
        self.app.world_mut().spawn(bundle).id()
    }

    /// Replaces the camera with one that culls on the GPU or does not (`NoIndirectDrawing`), which
    /// is what decides the frame's culling mode: GPU culling while any camera lacks the marker.
    /// Inserting or removing the marker on a live camera instead fails wgpu validation in stock
    /// bevy 0.19.1 (a GPU-culling preprocess bind group meets the direct pipeline), a defect of its
    /// own that is outside the removal path.
    fn replace_camera(&mut self, gpu_culling: bool) {
        let camera = self.camera;
        self.app.world_mut().despawn(camera);
        let mut spawned = self.app.world_mut().spawn((
            Camera3d::default(),
            RenderTarget::Image(self.target.clone().into()),
        ));
        if !gpu_culling {
            spawned.insert(NoIndirectDrawing);
        }
        self.camera = spawned.id();
    }

    /// An entity that only carries a pose and a visibility for its children.
    fn parent(&mut self) -> Entity {
        self.app
            .world_mut()
            .spawn((Transform::default(), Visibility::default()))
            .id()
    }

    fn entity(&mut self, entity: Entity) -> EntityWorldMut<'_> {
        self.app.world_mut().entity_mut(entity)
    }

    fn translate(&mut self, entity: Entity, by: Vec3) {
        self.entity(entity)
            .get_mut::<Transform>()
            .expect("a moving entity has a Transform")
            .translation += by;
    }

    /// Hidden, moved for [`MOVING_FRAMES`] frames, shown: absent while hidden, present after.
    #[track_caller]
    fn hide_move_show(&mut self, entity: Entity) {
        self.entity(entity).insert(Visibility::Hidden);
        self.frame();
        for _ in 0..MOVING_FRAMES {
            self.translate(entity, NUDGE);
            self.frame();
        }
        assert!(!self.has_instance(entity));
        self.entity(entity).insert(Visibility::Inherited);
        self.frame();
        assert!(self.has_instance(entity));
    }

    fn global_translation(&self, entity: Entity) -> Vec3 {
        self.app
            .world()
            .get::<GlobalTransform>(entity)
            .expect("a mesh has a GlobalTransform")
            .translation()
    }

    fn has_instance(&self, entity: Entity) -> bool {
        self.app
            .sub_app(RenderApp)
            .world()
            .resource::<RenderMeshInstances>()
            .mesh_asset_id(entity.into())
            .is_some()
    }

    fn in_gpu_culled_set(&self, entity: Entity) -> bool {
        self.app
            .sub_app(RenderApp)
            .world()
            .resource::<RenderGpuCulledEntities>()
            .entities
            .contains_key(&MainEntity::from(entity))
    }

    /// One frame, its invariants checked; returns the removals it queued.
    #[track_caller]
    fn frame(&mut self) -> Vec<(Entity, bool)> {
        self.app.update();
        self.check_invariants();
        self.app
            .sub_app(RenderApp)
            .world()
            .resource::<QueuedRemovals>()
            .0
            .clone()
    }

    #[track_caller]
    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.frame();
        }
    }

    /// Frames until `ready` holds; a material's first preparation may take more than one.
    #[track_caller]
    fn settle(&mut self, what: &str, ready: impl Fn(&Self) -> bool) {
        for _ in 0..30 {
            if ready(self) {
                return;
            }
            self.frame();
        }
        panic!("{what} never settled");
    }

    #[track_caller]
    fn check_invariants(&mut self) {
        let render = self.app.sub_app_mut(RenderApp).world_mut();
        let views: Vec<HashSet<Entity>> = render
            .query_filtered::<&RenderVisibleEntities, With<ExtractedView>>()
            .iter(render)
            .map(|visible| {
                visible
                    .classes
                    .get(&TypeId::of::<Mesh3d>())
                    .map(|class| class.entities_gpu_culling.keys().map(|e| e.id()).collect())
                    .unwrap_or_default()
            })
            .collect();
        let RenderMeshInstances::GpuBuilding(instances) = render.resource::<RenderMeshInstances>()
        else {
            panic!("GPU mesh building switched off mid-test");
        };
        let input = &render
            .resource::<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>()
            .current_input_buffer;
        let mut slots = HashSet::new();
        let mut rows = Vec::new();
        for (main_entity, instance) in instances.iter() {
            let slot = instance.gpu_specific.current_uniform_index();
            assert!(slots.insert(slot), "two instances share input slot {slot}");
            let uniform = input.get(slot).unwrap_or_else(|| {
                panic!("the instance of {main_entity:?} points at freed input slot {slot}")
            });
            let [x, y, z] = uniform.world_from_local.map(|row| row.w);
            let no_cpu_culling = instance
                .flags()
                .contains(RenderMeshInstanceFlags::NO_CPU_CULLING);
            rows.push((main_entity.id(), Vec3::new(x, y, z), no_cpu_culling));
        }
        let live_slots = (0..input.len() as u32)
            .filter(|&slot| input.get(slot).is_some())
            .count();
        assert_eq!(
            live_slots,
            instances.len(),
            "live input slots and mesh instances disagree: a slot leaked or was freed twice"
        );
        let culled = &render.resource::<RenderGpuCulledEntities>().entities;
        let flagged: HashSet<Entity> = rows.iter().filter(|r| r.2).map(|r| r.0).collect();
        let culled_set: HashSet<Entity> = culled.keys().map(|e| e.id()).collect();
        assert_eq!(
            culled_set, flagged,
            "the GPU-culled set is not exactly the instances flagged NO_CPU_CULLING"
        );
        for view in &views {
            assert!(
                view.is_subset(&culled_set),
                "a view still lists GPU-culled entities outside the GPU-culled set: {:?}",
                view.difference(&culled_set).collect::<Vec<_>>()
            );
        }

        let main = self.app.world();
        for (entity, translation, _) in rows {
            let held = main
                .get_entity(entity)
                .unwrap_or_else(|_| panic!("an instance outlived its despawned entity {entity}"));
            assert!(
                held.contains::<Mesh3d>(),
                "{entity} lost its Mesh3d but kept an instance"
            );
            assert!(
                held.get::<ViewVisibility>().is_some_and(|v| v.get()),
                "{entity} is not visible but kept an instance"
            );
            let expected = held
                .get::<GlobalTransform>()
                .expect("a mesh has a GlobalTransform")
                .translation();
            assert!(
                translation.abs_diff_eq(expected, 1e-5),
                "{entity}'s instance holds translation {translation}, the entity is at {expected}"
            );
        }
    }
}

/// Hidden by `Visibility`, by a hidden parent, or by the frustum, and shown again: the instance
/// leaves and comes back with the pose it moved to while hidden.
#[test]
#[ignore = "needs a GPU adapter (module doc); never opens a window"]
fn visibility_transitions_keep_instances_true() {
    let mut lab = Lab::new();
    let plain = lab.cube(IN_VIEW);
    let culled = lab.cube(IN_VIEW + Vec3::X);
    let parent = lab.parent();
    let child = lab.cube(IN_VIEW - Vec3::X);
    lab.entity(child).insert(ChildOf(parent));
    lab.settle("three visible cubes", |lab| {
        [plain, culled, child].iter().all(|&e| lab.has_instance(e))
    });

    lab.entity(plain).insert(Visibility::Hidden);
    lab.entity(parent).insert(Visibility::Hidden);
    lab.translate(culled, BEHIND - IN_VIEW);
    lab.frame();
    for _ in 0..MOVING_FRAMES {
        lab.translate(plain, NUDGE);
        lab.translate(parent, NUDGE);
        lab.translate(culled, NUDGE);
        lab.frame();
    }
    assert!(![plain, culled, child].iter().any(|&e| lab.has_instance(e)));

    lab.entity(plain).insert(Visibility::Inherited);
    lab.entity(parent).insert(Visibility::Inherited);
    lab.translate(culled, IN_VIEW - BEHIND);
    lab.frame();
    assert!([plain, culled, child].iter().all(|&e| lab.has_instance(e)));

    // Hidden for a single frame, moving through both edges.
    lab.entity(plain).insert(Visibility::Hidden);
    lab.translate(plain, NUDGE);
    lab.frame();
    lab.entity(plain).insert(Visibility::Inherited);
    lab.translate(plain, NUDGE);
    lab.frame();
    assert!(lab.has_instance(plain));
}

/// A mesh whose material is not prepared has no instance until it is; hiding and moving it
/// meanwhile must not leave it without one once it is visible and its material ready.
#[test]
#[ignore = "needs a GPU adapter (module doc); never opens a window"]
fn material_readiness_keeps_instances_true() {
    let mut lab = Lab::new();
    let pending: Vec<Handle<StandardMaterial>> = (0..2)
        .map(|_| {
            lab.app
                .world()
                .resource::<Assets<StandardMaterial>>()
                .reserve_handle()
        })
        .collect();
    let waits_hidden = lab.cube(IN_VIEW);
    let waits_visible = lab.cube(IN_VIEW + Vec3::X);
    lab.entity(waits_hidden)
        .insert(MeshMaterial3d(pending[0].clone()));
    lab.entity(waits_visible)
        .insert(MeshMaterial3d(pending[1].clone()));
    lab.frames(3);
    assert!(!lab.has_instance(waits_hidden) && !lab.has_instance(waits_visible));

    lab.entity(waits_hidden).insert(Visibility::Hidden);
    for _ in 0..MOVING_FRAMES {
        lab.translate(waits_hidden, NUDGE);
        lab.frame();
    }
    for handle in &pending {
        lab.app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .insert(handle.id(), StandardMaterial::default())
            .expect("a reserved handle takes its asset");
    }
    lab.translate(waits_hidden, NUDGE);
    lab.frames(3);
    assert!(!lab.has_instance(waits_hidden));
    lab.settle("the material that became ready while visible", |lab| {
        lab.has_instance(waits_visible)
    });

    lab.entity(waits_hidden).insert(Visibility::Inherited);
    lab.settle("the mesh shown after its material became ready", |lab| {
        lab.has_instance(waits_hidden)
    });
    lab.entity(waits_visible).insert(Visibility::Hidden);
    lab.frame();
    assert!(!lab.has_instance(waits_visible));
}

/// `Mesh3d` removed and put back: visible, hidden, moving, and both in one frame.
#[test]
#[ignore = "needs a GPU adapter (module doc); never opens a window"]
fn mesh_removal_and_reinsertion_keep_instances_true() {
    let mut lab = Lab::new();
    let cube = lab.cube(IN_VIEW);
    lab.settle("a visible cube", |lab| lab.has_instance(cube));

    lab.entity(cube).remove::<Mesh3d>();
    lab.frame();
    assert!(!lab.has_instance(cube));
    let mesh = Mesh3d(lab.mesh.clone());
    lab.entity(cube).insert(mesh.clone());
    lab.frame();
    assert!(lab.has_instance(cube));

    // Removed and reinserted between two frames.
    lab.entity(cube).remove::<Mesh3d>().insert(mesh.clone());
    lab.translate(cube, NUDGE);
    lab.frame();
    assert!(lab.has_instance(cube));

    // Hidden, then removed while moving, reinserted while hidden, shown.
    lab.entity(cube).insert(Visibility::Hidden);
    lab.frame();
    lab.entity(cube).remove::<Mesh3d>();
    lab.translate(cube, NUDGE);
    lab.frame();
    lab.translate(cube, NUDGE);
    lab.frame();
    lab.entity(cube).insert(mesh.clone());
    lab.translate(cube, NUDGE);
    lab.frame();
    assert!(!lab.has_instance(cube));
    lab.entity(cube).insert(Visibility::Inherited);
    lab.frame();
    assert!(lab.has_instance(cube));

    // Hidden and removed in one frame, then reinserted and shown in one frame.
    lab.entity(cube)
        .insert(Visibility::Hidden)
        .remove::<Mesh3d>();
    lab.frame();
    assert!(!lab.has_instance(cube));
    lab.entity(cube).insert((Visibility::Inherited, mesh));
    lab.frame();
    assert!(lab.has_instance(cube));
}

/// `NoCpuCulling` toggled per mesh, and the frame's culling mode switched by the cameras
/// ([`Lab::replace_camera`]), with the mesh visible, hidden and moving across each switch.
#[test]
#[ignore = "needs a GPU adapter (module doc); never opens a window"]
fn culling_mode_switches_keep_instances_true() {
    let mut lab = Lab::new();
    let cube = lab.cube(IN_VIEW);
    lab.entity(cube).insert(NoCpuCulling);
    lab.settle("a GPU-culled cube", |lab| {
        lab.has_instance(cube) && lab.in_gpu_culled_set(cube)
    });

    lab.entity(cube).insert(Visibility::Hidden);
    lab.frame();
    assert!(!lab.has_instance(cube) && !lab.in_gpu_culled_set(cube));
    for _ in 0..MOVING_FRAMES {
        lab.translate(cube, NUDGE);
        lab.frame();
    }
    lab.entity(cube).insert(Visibility::Inherited);
    lab.frame();
    assert!(lab.has_instance(cube) && lab.in_gpu_culled_set(cube));

    lab.entity(cube).remove::<NoCpuCulling>();
    lab.frame();
    assert!(lab.has_instance(cube) && !lab.in_gpu_culled_set(cube));
    // Hidden while back on CPU culling, flagged GPU-culled again while hidden, then shown.
    lab.entity(cube).insert(Visibility::Hidden);
    lab.frame();
    lab.entity(cube).insert(NoCpuCulling);
    lab.translate(cube, NUDGE);
    lab.frame();
    assert!(!lab.has_instance(cube) && !lab.in_gpu_culled_set(cube));
    lab.entity(cube).insert(Visibility::Inherited);
    lab.frame();
    assert!(lab.has_instance(cube) && lab.in_gpu_culled_set(cube));

    lab.replace_camera(false);
    lab.frame();
    assert!(lab.has_instance(cube));
    lab.hide_move_show(cube);
    lab.entity(cube).insert(Visibility::Hidden);
    lab.frame();
    lab.replace_camera(true);
    lab.translate(cube, NUDGE);
    lab.frame();
    assert!(!lab.has_instance(cube));
    lab.entity(cube).insert(Visibility::Inherited);
    lab.frame();
    assert!(lab.has_instance(cube) && lab.in_gpu_culled_set(cube));
    lab.hide_move_show(cube);
}

/// Despawned while visible, while hidden, while hidden and moving, while GPU-culled, and with its
/// parent.
#[test]
#[ignore = "needs a GPU adapter (module doc); never opens a window"]
fn despawns_leave_no_instance() {
    let mut lab = Lab::new();
    let cubes: Vec<Entity> = (0..5)
        .map(|i| lab.cube(IN_VIEW + Vec3::X * (i as f32 - 2.0)))
        .collect();
    let [visible, hidden, with_parent, hidden_moving, gpu_culled] = cubes[..] else {
        unreachable!()
    };
    let parent = lab.parent();
    lab.entity(with_parent).insert(ChildOf(parent));
    lab.entity(gpu_culled).insert(NoCpuCulling);
    lab.settle("five visible cubes", |lab| {
        cubes.iter().all(|&e| lab.has_instance(e)) && lab.in_gpu_culled_set(gpu_culled)
    });
    lab.entity(hidden).insert(Visibility::Hidden);
    lab.entity(hidden_moving).insert(Visibility::Hidden);
    lab.frame();
    lab.translate(hidden_moving, NUDGE);
    lab.frame();

    lab.app.world_mut().despawn(visible);
    lab.app.world_mut().despawn(hidden);
    lab.app.world_mut().despawn(parent);
    lab.translate(hidden_moving, NUDGE);
    lab.app.world_mut().despawn(hidden_moving);
    lab.app.world_mut().despawn(gpu_culled);
    lab.frame();
    lab.frame();
    assert!(
        !cubes
            .iter()
            .any(|&e| lab.has_instance(e) || lab.in_gpu_culled_set(e))
    );
}

/// The removal skip (`OVERMATCH_PATCH.md`, "Mesh instance removals"): a mesh that goes invisible
/// queues one removal, for the instance it had, and none while it stays invisible however it moves:
/// on its own, under a moving parent (a vehicle's hidden part), or outside the frustum. A mesh whose
/// material never became ready has no instance and queues none. Without the skip it fails on the
/// frame they go invisible and on every frame they move (MEASURED).
#[test]
#[ignore = "needs a GPU adapter (module doc); never opens a window"]
fn a_hidden_mesh_queues_one_removal_however_long_it_moves() {
    let mut lab = Lab::new();
    let plain = lab.cube(IN_VIEW);
    let culled = lab.cube(IN_VIEW + Vec3::X);
    let parent = lab.parent();
    let child = lab.cube(IN_VIEW - Vec3::X);
    lab.entity(child).insert(ChildOf(parent));
    let pending = lab.cube(IN_VIEW + Vec3::Y);
    let never_ready = lab
        .app
        .world()
        .resource::<Assets<StandardMaterial>>()
        .reserve_handle();
    lab.entity(pending).insert(MeshMaterial3d(never_ready));
    let shown = [plain, culled, child];
    lab.settle("three visible cubes", |lab| {
        shown.iter().all(|&e| lab.has_instance(e))
    });
    assert!(!lab.has_instance(pending));

    lab.entity(plain).insert(Visibility::Hidden);
    lab.entity(parent).insert(Visibility::Hidden);
    lab.entity(pending).insert(Visibility::Hidden);
    lab.translate(culled, BEHIND - IN_VIEW);
    let removals = lab.frame();
    for entity in shown {
        assert_eq!(
            removals.iter().filter(|(e, _)| *e == entity).count(),
            1,
            "{entity} went invisible with an instance: exactly one removal"
        );
    }
    // Every frame is checked before failing, so a failure lists each frame that broke the contract.
    let mut stray = Vec::new();
    let no_instance = |removals: &[(Entity, bool)]| -> Vec<Entity> {
        removals
            .iter()
            .filter(|(_, live)| !live)
            .map(|(e, _)| *e)
            .collect()
    };
    stray.push(("the frame they went invisible", no_instance(&removals)));

    let hidden = [plain, culled, child, pending];
    for _ in 0..MOVING_FRAMES {
        let before = lab.global_translation(child);
        lab.translate(plain, NUDGE);
        lab.translate(parent, NUDGE);
        lab.translate(culled, NUDGE);
        lab.translate(pending, NUDGE);
        let removals = lab.frame();
        assert_ne!(
            lab.global_translation(child),
            before,
            "the hidden child did not move, so this frame proves nothing"
        );
        stray.push((
            "a frame they moved while invisible",
            removals
                .into_iter()
                .filter_map(|(e, _)| hidden.contains(&e).then_some(e))
                .collect(),
        ));
    }
    stray.retain(|(_, entities)| !entities.is_empty());
    assert!(
        stray.is_empty(),
        "removals queued for meshes with no instance (hidden {hidden:?}): {stray:?}"
    );
}
