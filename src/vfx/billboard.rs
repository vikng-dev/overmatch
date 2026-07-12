//! The shared billboard-sprite machinery every combat effect is assembled from (survey tricks
//! 1/2/4/5/9): camera-facing quads drawing a grayscale flipbook atlas through
//! [`VfxBillboardMaterial`] — a small custom `Material` whose fragment does alpha EROSION (never a
//! uniform fade) and gradient-map coloring (grayscale signal × life fraction → a per-effect color
//! LUT whose alpha channel is a HEAT term that pushes young pixels above 1.0 for bloom).
//!
//! Anti-repetition comes from per-instance randomness: random flipbook START frame
//! (`frame = (start + age·rate) mod count` — [`flipbook_frame`]), random roll/spin, random size.
//! That per-instance mutation is why each billboard clones its own material asset (the same
//! trade the impact puffs already make); the clones are bounded by [`BILLBOARD_CAP`]'s ring, and
//! everything short-lived, so the batching cost stays a handful of draws.
//!
//! Consumers by design: the 88 and MG muzzle dressings ([`super::muzzle`]) and the impact sparks
//! ([`super::impact`]) — nothing in here knows which weapon or surface it is dressing.

use std::collections::VecDeque;

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use bevy::transform::TransformSystems;

/// Live-billboard ring cap — a leak bound, exactly the impact puffs' `PUFF_CAP` shape. Steady state
/// is far below it (an 88 shot spawns ~4 billboards, each sub-second); the cap only bites on
/// pathological refire (rollback-replayed fire seams, spawn storms), evicting oldest-first.
pub(super) const BILLBOARD_CAP: usize = 96;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<BillboardRing>()
        .add_plugins(MaterialPlugin::<VfxBillboardMaterial>::default())
        .add_systems(Update, age_billboards)
        // Facing runs late, after every gameplay writer of camera/billboard transforms, but before
        // propagation bakes `GlobalTransform`s — the same slot the aim/sight readers use.
        .add_systems(
            PostUpdate,
            face_billboards.before(TransformSystems::Propagate),
        );
}

/// The animated uniform block `vfx_billboard.wgsl` reads. Three `Vec4`s, CPU-packed (see the shader
/// for the lane map) — kept dumb so the WGSL and this struct can be compared at a glance.
#[derive(ShaderType, Debug, Clone, Default)]
pub(crate) struct VfxParams {
    /// x: current flipbook frame (wrapped CPU-side by [`flipbook_frame`]), y: atlas cols,
    /// z: atlas rows, w: unused.
    pub frame: Vec4,
    /// x: erosion threshold, y: erosion sharpness, z: life fraction (LUT row), w: overall alpha.
    pub fade: Vec4,
    /// x: emissive boost at LUT heat 1.0.
    pub glow: Vec4,
}

/// The flipbook + erosion + gradient-map billboard material (fragment: `vfx_billboard.wgsl`;
/// vertex: Bevy's default mesh path — facing is done on the entity's `Transform`, not in the
/// shader). Blend discipline is per instance: `AlphaMode::Add` for hot cores/flashes,
/// `AlphaMode::Blend` for smoke mass (survey trick 9). No prepass, no shadows — translucent
/// sprites contribute to neither.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct VfxBillboardMaterial {
    #[uniform(0)]
    pub params: VfxParams,
    #[texture(1)]
    #[sampler(2)]
    pub atlas: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    pub lut: Handle<Image>,
    pub alpha_mode: AlphaMode,
}

impl Material for VfxBillboardMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/vfx_billboard.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    // Translucent one-shot sprites: no depth/normal prepass and no shadow pass — both would be
    // wasted pipeline permutations (and shadow-casting smoke is exactly the artifact
    // `NotShadowCaster` exists to prevent).
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

/// A live billboard's animation state. [`age_billboards`] drives everything visible from `age`:
/// flipbook frame, erosion threshold, LUT row, size ease, drift, spin — and despawns it at
/// `lifetime`.
#[derive(Component)]
pub(crate) struct Billboard {
    pub age: f32,
    pub lifetime: f32,
    /// Where the billboard was born; drift displaces from here (`origin + drift·age`).
    pub origin: Vec3,
    /// World-space drift velocity (smoke rise, muzzle-gas push).
    pub drift: Vec3,
    /// Flipbook playback: `frame = (start_frame + age·frame_rate) mod frames`.
    pub frames: u32,
    pub start_frame: f32,
    pub frame_rate: f32,
    /// Uniform size (m) eased from `start` to `end` over life; `aspect` shapes it per axis
    /// (directional flame planes are taller than wide).
    pub start_size: f32,
    pub end_size: f32,
    pub aspect: Vec3,
    /// Roll around the facing axis (rad) and its rate (rad/s) — only read for `FaceCamera`
    /// billboards; fixed-orientation planes bake their rotation at spawn.
    pub roll: f32,
    pub spin: f32,
    /// Erosion threshold at death: alpha erosion ramps 0 → this over life (1.0 = fully dissolved
    /// exactly at despawn; the flash cluster uses 0 — two frames don't erode).
    pub erosion_end: f32,
}

/// Marks billboards the facing system turns toward the camera each frame. Directional muzzle
/// planes deliberately lack it — they hold the bore-aligned rotation their spawn gave them.
#[derive(Component)]
pub(crate) struct FaceCamera;

/// Live billboards in spawn order — the eviction ring (see [`BILLBOARD_CAP`]).
#[derive(Resource, Default)]
pub(crate) struct BillboardRing(pub VecDeque<Entity>);

/// Everything that varies per billboard spawn, in one bundle-shaped spec so call sites read as
/// data. The MATERIAL template arrives as a value and is cloned into a per-instance asset (the
/// per-instance mutation trade — see the module doc).
pub(crate) struct BillboardSpec {
    pub material: VfxBillboardMaterial,
    pub lifetime: f32,
    pub origin: Vec3,
    pub drift: Vec3,
    pub frames: u32,
    pub start_frame: f32,
    pub frame_rate: f32,
    pub start_size: f32,
    pub end_size: f32,
    pub aspect: Vec3,
    pub roll: f32,
    pub spin: f32,
    pub erosion_end: f32,
    /// `None` = camera-facing ([`FaceCamera`]); `Some(rotation)` = fixed world orientation.
    pub rotation: Option<Quat>,
}

/// Spawn one billboard from a spec: per-instance material clone, ring registration, shadow opt-out.
/// The shared quad `mesh` is the caller's (preloaded once per effect module).
pub(crate) fn spawn_billboard(
    commands: &mut Commands,
    materials: &mut Assets<VfxBillboardMaterial>,
    ring: &mut BillboardRing,
    mesh: Handle<Mesh>,
    spec: BillboardSpec,
) -> Entity {
    let mut material = spec.material;
    // Seed the animated lanes so the first rendered frame is already correct (the ager only
    // catches it NEXT Update).
    material.params.frame.x =
        flipbook_frame(spec.start_frame, 0.0, spec.frame_rate, spec.frames) as f32;
    material.params.fade.z = 0.0;
    let material = materials.add(material);
    let transform = Transform {
        translation: spec.origin,
        rotation: spec.rotation.unwrap_or_default(),
        scale: spec.aspect * spec.start_size,
    };
    let mut entity = commands.spawn((
        Billboard {
            age: 0.0,
            lifetime: spec.lifetime,
            origin: spec.origin,
            drift: spec.drift,
            frames: spec.frames,
            start_frame: spec.start_frame,
            frame_rate: spec.frame_rate,
            start_size: spec.start_size,
            end_size: spec.end_size,
            aspect: spec.aspect,
            roll: spec.roll,
            spin: spec.spin,
            erosion_end: spec.erosion_end,
        },
        Mesh3d(mesh),
        MeshMaterial3d(material),
        transform,
        NotShadowCaster,
        NotShadowReceiver,
    ));
    if spec.rotation.is_none() {
        entity.insert(FaceCamera);
    }
    let id = entity.id();
    ring.0.push_back(id);
    while ring.0.len() > BILLBOARD_CAP {
        if let Some(old) = ring.0.pop_front() {
            commands.entity(old).try_despawn();
        }
    }
    id
}

/// The flipbook frame for a particle of `age` that started at `start` — the random-start-offset
/// anti-repetition trick (survey trick 2): `floor(start + age·rate) mod frames`. This IS the live
/// implementation ([`age_billboards`] writes its result into the material); the shader only does
/// cell-UV arithmetic from the resolved index.
pub(crate) fn flipbook_frame(start: f32, age: f32, rate: f32, frames: u32) -> u32 {
    if frames == 0 {
        return 0;
    }
    let raw = (start + age * rate).floor();
    (raw as i64).rem_euclid(frames as i64) as u32
}

/// Advance every live billboard: flipbook frame, erosion, LUT row, size ease, drift, spin — and
/// despawn at end of life. One pass, view-only, `Update` (render cadence; ages in wall time).
fn age_billboards(
    time: Res<Time>,
    mut materials: ResMut<Assets<VfxBillboardMaterial>>,
    mut billboards: Query<(
        Entity,
        &mut Billboard,
        &mut Transform,
        &MeshMaterial3d<VfxBillboardMaterial>,
    )>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut billboard, mut transform, material) in &mut billboards {
        billboard.age += dt;
        let t = billboard.age / billboard.lifetime;
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        billboard.roll += billboard.spin * dt;
        // Ease-out growth: fast expansion at birth (the gas does its pushing early), settling
        // toward the end size.
        let ease = 1.0 - (1.0 - t) * (1.0 - t);
        let size = billboard.start_size + (billboard.end_size - billboard.start_size) * ease;
        transform.scale = billboard.aspect * size;
        transform.translation = billboard.origin + billboard.drift * billboard.age;
        if let Some(mut mat) = materials.get_mut(&material.0) {
            mat.params.frame.x = flipbook_frame(
                billboard.start_frame,
                billboard.age,
                billboard.frame_rate,
                billboard.frames,
            ) as f32;
            mat.params.fade.x = t * billboard.erosion_end;
            mat.params.fade.z = t;
        }
    }
}

/// Turn every [`FaceCamera`] billboard toward the camera, rolled by its own `roll` — the CPU half
/// of billboarding (the material keeps Bevy's default vertex path). Runs even while paused: a
/// frozen smoke puff should still face a camera the player orbits.
fn face_billboards(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut billboards: Query<(&mut Transform, &Billboard), With<FaceCamera>>,
) {
    let Ok(camera) = camera.single() else {
        return;
    };
    let facing = camera.rotation();
    for (mut transform, billboard) in &mut billboards {
        // Camera rotation aligns the quad's +Z (its normal) with the camera's +Z (toward the
        // viewer); the roll spins it in the screen plane.
        transform.rotation = facing * Quat::from_rotation_z(billboard.roll);
    }
}

/// Build a small 2D gradient LUT as an `Image`: X = grayscale signal (0..1), Y = life fraction
/// (0..1); `stop(x, y)` returns `(linear color, heat)` where heat (stored in alpha) multiplies the
/// color above 1.0 in the shader — the Fallout-4 gradient-map trick with a bloom lane. One
/// grayscale atlas + one LUT per effect = distinct palettes from one sprite set.
pub(crate) fn gradient_lut(
    images: &mut Assets<Image>,
    stop: impl Fn(f32, f32) -> (LinearRgba, f32),
) -> Handle<Image> {
    const W: usize = 64;
    const H: usize = 16;
    let mut data = Vec::with_capacity(W * H * 4);
    for row in 0..H {
        let y = row as f32 / (H - 1) as f32;
        for col in 0..W {
            let x = col as f32 / (W - 1) as f32;
            let (color, heat) = stop(x, y);
            for channel in [color.red, color.green, color.blue, heat] {
                data.push((channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
    }
    images.add(Image::new(
        Extent3d {
            width: W as u32,
            height: H as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        // Non-sRGB: the closure authors LINEAR values; the default sampler (linear filter, clamp)
        // is exactly what a LUT wants.
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    ))
}

/// The shared unit quad for billboards (1×1 m, facing +Z, sprite-up along +Y — `Rectangle`'s UV
/// layout puts image-top at +Y, so the Kenney flame sprites point along +Y).
pub(crate) fn unit_quad(meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
    meshes.add(Rectangle::new(1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anti-repetition frame math: wraps modulo the frame count, honors fractional starts, and
    /// never panics on degenerate inputs.
    #[test]
    fn flipbook_frame_wraps_and_offsets() {
        // 4-frame atlas at 10 fps from frame 0: 0,1,2,3,0,1...
        assert_eq!(flipbook_frame(0.0, 0.0, 10.0, 4), 0);
        assert_eq!(flipbook_frame(0.0, 0.15, 10.0, 4), 1);
        assert_eq!(flipbook_frame(0.0, 0.35, 10.0, 4), 3);
        assert_eq!(
            flipbook_frame(0.0, 0.45, 10.0, 4),
            0,
            "wraps past the last frame"
        );
        // A random start offset shifts the whole sequence — two particles born together differ.
        assert_eq!(flipbook_frame(2.0, 0.0, 10.0, 4), 2);
        assert_eq!(
            flipbook_frame(2.0, 0.25, 10.0, 4),
            0,
            "offset sequence wraps too"
        );
        // Degenerate inputs stay in range instead of panicking.
        assert_eq!(
            flipbook_frame(0.0, 100.0, 1000.0, 4).min(3),
            flipbook_frame(0.0, 100.0, 1000.0, 4)
        );
        assert_eq!(
            flipbook_frame(0.0, 1.0, 1.0, 0),
            0,
            "zero frames is frame 0"
        );
    }

    fn harness() -> App {
        let mut app = App::new();
        app.init_resource::<BillboardRing>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<VfxBillboardMaterial>>()
            .init_resource::<Time>()
            .add_systems(Update, age_billboards);
        app
    }

    fn test_spec(lifetime: f32) -> BillboardSpec {
        BillboardSpec {
            material: VfxBillboardMaterial {
                params: VfxParams::default(),
                atlas: Handle::default(),
                lut: Handle::default(),
                alpha_mode: AlphaMode::Blend,
            },
            lifetime,
            origin: Vec3::ZERO,
            drift: Vec3::Y,
            frames: 4,
            start_frame: 1.0,
            frame_rate: 8.0,
            start_size: 1.0,
            end_size: 3.0,
            aspect: Vec3::ONE,
            roll: 0.0,
            spin: 0.0,
            erosion_end: 1.0,
            rotation: None,
        }
    }

    fn spawn_one(app: &mut App, lifetime: f32) -> Entity {
        let spec = test_spec(lifetime);
        let world = app.world_mut();
        let entity =
            world.resource_scope(|world, mut materials: Mut<Assets<VfxBillboardMaterial>>| {
                world.resource_scope(|world, mut ring: Mut<BillboardRing>| {
                    let mut commands = world.commands();
                    spawn_billboard(
                        &mut commands,
                        &mut materials,
                        &mut ring,
                        Handle::default(),
                        spec,
                    )
                })
            });
        app.world_mut().flush();
        entity
    }

    fn count(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<Billboard>>()
            .iter(app.world())
            .count()
    }

    fn advance(app: &mut App, secs: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(secs));
        app.update();
    }

    /// Billboards live exactly their lifetime, and mid-life the ager has visibly animated them:
    /// grown scale, advanced erosion + LUT row, drifted position.
    #[test]
    fn billboards_animate_then_expire() {
        let mut app = harness();
        let entity = spawn_one(&mut app, 1.0);
        assert_eq!(count(&mut app), 1);

        advance(&mut app, 0.5);
        let world = app.world();
        let transform = world.get::<Transform>(entity).expect("alive mid-life");
        assert!(
            transform.scale.x > 1.0,
            "mid-life billboard must have grown"
        );
        assert!(
            transform.translation.y > 0.0,
            "mid-life billboard must have drifted"
        );
        let material = world
            .get::<MeshMaterial3d<VfxBillboardMaterial>>(entity)
            .expect("material");
        let mat = world
            .resource::<Assets<VfxBillboardMaterial>>()
            .get(&material.0)
            .expect("per-instance material asset");
        assert!(mat.params.fade.x > 0.0, "erosion must ramp with age");
        assert!(mat.params.fade.z > 0.0, "LUT row must track life fraction");

        advance(&mut app, 0.6);
        assert_eq!(count(&mut app), 0, "an expired billboard must despawn");
    }

    /// The eviction ring bounds live billboards at the cap, oldest first — the leak bound under
    /// refire storms (rollback-replayed fire seams).
    #[test]
    fn billboard_ring_caps() {
        let mut app = harness();
        for _ in 0..BILLBOARD_CAP + 9 {
            spawn_one(&mut app, 60.0);
        }
        assert_eq!(count(&mut app), BILLBOARD_CAP);
        assert_eq!(
            app.world().resource::<BillboardRing>().0.len(),
            BILLBOARD_CAP
        );
    }

    /// The gradient LUT builder encodes the closure faithfully: X is the signal axis, Y the life
    /// axis, alpha the heat lane.
    #[test]
    fn gradient_lut_encodes_axes() {
        let mut images = Assets::<Image>::default();
        let handle = gradient_lut(&mut images, |x, y| {
            (LinearRgba::rgb(x, y, 0.25), if y < 0.5 { 1.0 } else { 0.0 })
        });
        let image = images.get(&handle).expect("lut image");
        let data = image.data.as_ref().expect("cpu-side data");
        // Texel (x=last, y=first): red ≈ 255 (x=1), green = 0 (y=0), heat = 255 (young).
        let first_row_last = (64 - 1) * 4;
        assert_eq!(data[first_row_last], 255);
        assert_eq!(data[first_row_last + 1], 0);
        assert_eq!(data[first_row_last + 3], 255);
        // Texel (x=first, y=last): red = 0, green ≈ 255 (y=1), heat = 0 (old).
        let last_row_first = 64 * (16 - 1) * 4;
        assert_eq!(data[last_row_first], 0);
        assert_eq!(data[last_row_first + 1], 255);
        assert_eq!(data[last_row_first + 3], 0);
    }
}
