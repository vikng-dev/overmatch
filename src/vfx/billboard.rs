//! The shared billboard-sprite machinery every combat effect is assembled from (survey tricks
//! 1/2/4/5/9): camera-facing quads drawing a grayscale flipbook atlas through
//! [`VfxBillboardMaterial`] — a small custom `Material` whose fragment does alpha EROSION (never a
//! uniform fade) and gradient-map coloring (grayscale signal × life fraction → a per-effect color
//! LUT whose alpha channel is a HEAT term that pushes young pixels above 1.0 for bloom).
//!
//! Anti-repetition comes from per-instance randomness: random flipbook START frame
//! ([`Flipbook::Loop`] — [`flipbook_frame`]), random roll/spin, random size.
//!
//! One material per FAMILY — atlas, LUT and alpha mode ([`BillboardMaterials`]) — so a family's
//! sprites share one bind group. Everything that varies per sprite is its
//! [`VfxParams`] lanes, which live on its [`Billboard`] and reach the shader through the LANE TABLE:
//! one [`BILLBOARD_SLOTS`]-entry storage buffer that every family binds and the fragment indexes by
//! the sprite's `MeshTag`, its ring slot ([`BillboardRing`]). [`upload_billboard_lanes`] writes the
//! changed slots at most once per frame; ageing a sprite modifies no material.
//!
//! Each sprite stays its own draw (`NoAutomaticBatching`): Bevy's instanced batch takes its
//! instance slots by GPU `atomicAdd` (`mesh_preprocess.wgsl`), so a batch would draw a family's
//! sprites in an unspecified order, and overlapping alpha-blended ones would composite out of the
//! back-to-front sort the transparent pass gives them.
//!
//! Consumers by design: the 88 and MG muzzle dressings ([`super::muzzle`]) and the impact sparks
//! ([`super::impact`]) — nothing in here knows which weapon or surface it is dressing.

use std::collections::VecDeque;

use bevy::asset::{AssetEventSystems, RenderAssetUsages};
use bevy::ecs::system::SystemParam;
use bevy::image::Image;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::batching::NoAutomaticBatching;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat, encase,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use bevy::transform::TransformSystems;

use crate::view::PlayerView;

/// Live-billboard ring cap — a leak bound over the ONE ring every consumer shares, the impact puffs
/// included: they hold no cap of their own and are evicted against this one. Only the long-lived
/// ground scars are insulated in a ring of their own (`GROUND_MARK_CAP`), so a multi-second scar
/// survives a sub-second billboard storm. The stack per 88 event is 11–14 at the muzzle (blast
/// core + flash core + glow card + 2 flame planes + smoke, plus 5–8 ground-dust puffs) and 12–17 at a terrain
/// impact (contact flash + 8–12 ejecta + 1–2 plumes + shock ring + the 8 s haze); both MGs cycling
/// add ~50/s at sub-second lifetimes. Eviction is FIFO, so the consequence of pressure is that the
/// OLDEST billboard goes first: a sustained storm cuts the multi-second layers (haze, ground dust)
/// short before any of the flash layers that displaced them.
pub(super) const BILLBOARD_CAP: usize = 96;

/// Entries in the lane table: the shared ring's slots `0..BILLBOARD_CAP`, then the ground-mark
/// ring's. Every ring's slot range lies inside it (asserted by [`BillboardRing::new`]).
pub(super) const BILLBOARD_SLOTS: usize = BILLBOARD_CAP + super::impact::GROUND_MARK_CAP;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<BillboardRing>()
        .init_resource::<BillboardFamilies>()
        .add_plugins(MaterialPlugin::<VfxBillboardMaterial>::default())
        .add_systems(Update, age_billboards)
        .add_systems(
            PostUpdate,
            (
                // Facing runs late, after every gameplay writer of camera/billboard transforms, but
                // before propagation bakes `GlobalTransform`s — the same slot the aim/sight readers
                // use.
                face_billboards.before(TransformSystems::Propagate),
                // The table's `Modified` event must flush this frame for this frame's extraction to
                // carry it; a later write would draw one frame stale.
                upload_billboard_lanes.before(AssetEventSystems),
            ),
        );
}

/// The per-sprite lane block `vfx_billboard.wgsl` reads, one lane-table entry per sprite. Three
/// `Vec4`s, CPU-packed (see the shader for the lane map) — kept dumb so the WGSL and this struct
/// can be compared at a glance.
#[derive(ShaderType, Debug, Clone, Default)]
pub(crate) struct VfxParams {
    /// x: current flipbook frame (resolved CPU-side by [`flipbook_frame`]), y: atlas cols,
    /// z: atlas rows, w: unused.
    pub frame: Vec4,
    /// x: erosion threshold, y: erosion sharpness, z: life fraction (LUT row), w: overall alpha.
    pub fade: Vec4,
    /// x: emissive boost at LUT heat 1.0, y: BLEND CONTRACT flag the shader reads — `1.0` = additive
    /// glow (`AlphaMode::Add`), `0.0` = alpha-over mass (`AlphaMode::Blend`). Both alpha modes share
    /// the premultiplied blend state, so the shader must premultiply the additive path itself; this
    /// lane tells it which (see `vfx_billboard.wgsl`). z: soft-particle fade depth in metres
    /// ([`SOFT_PARTICLE_DEPTH`]), `0.0` = hard-edged. w: reserved.
    pub glow: Vec4,
}

/// Bytes per lane-table entry. Every [`VfxParams`] lane is a `vec4` (pinned by
/// `project_vfx_uniforms_agree_with_their_shaders`), so the struct's size is already a multiple of
/// its 16-byte alignment and the WGSL array stride equals it.
fn lane_stride() -> usize {
    VfxParams::min_size().get() as usize
}

/// How far (m) a soft-particle layer fades over as it closes on the geometry behind it — the lane
/// `vfx_billboard.wgsl` reads out of `VfxParams::glow.z`, and the whole cure for a dust puff meeting
/// terrain along the quad's own intersection line.
///
/// Exactly one class carries it: the diffuse MASS volumes (impact dust, dirt, spall; the muzzle's
/// gas smoke and ground dust). Every other layer is 0.0, for three separate mechanisms — additive
/// HOT layers (flashes, sparks, flame licks, the blast core) are airborne, so their approach to a
/// wall is not an intersection to hide; the two layers that LIE on a surface (ground scar, shock
/// ring) have their whole quad inside the band, which scales them out of existence; and ejecta is
/// solid thrown mass whose silhouette is its own edge, not a gas boundary.
pub(crate) const SOFT_PARTICLE_DEPTH: f32 = 0.75;

/// One FAMILY's flipbook + erosion + gradient-map billboard material (fragment:
/// `vfx_billboard.wgsl`; vertex: Bevy's default mesh path — facing is done on the entity's
/// `Transform`, not in the shader). Blend discipline is per family: `AlphaMode::Add` for hot
/// cores/flashes, `AlphaMode::Blend` for smoke mass (survey trick 9). No prepass, no shadows —
/// translucent sprites contribute to neither. The per-sprite lanes are not here: `lanes` is the
/// shared lane table, indexed in the fragment by the sprite's `MeshTag`.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct VfxBillboardMaterial {
    #[storage(0, read_only)]
    pub lanes: Handle<ShaderBuffer>,
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

/// What one billboard draws with: its family (atlas, LUT, alpha mode), shared as one
/// [`VfxBillboardMaterial`], and the starting [`VfxParams`] lanes the sprite carries on its own.
#[derive(Clone)]
pub(crate) struct BillboardLook {
    pub params: VfxParams,
    pub atlas: Handle<Image>,
    pub lut: Handle<Image>,
    pub alpha_mode: AlphaMode,
}

/// The lane table and every family material made so far: one per distinct (atlas, LUT, alpha mode),
/// made on its family's first sprite and kept for the session.
#[derive(Resource)]
pub(crate) struct BillboardFamilies {
    lanes: Handle<ShaderBuffer>,
    made: Vec<(Family, Handle<VfxBillboardMaterial>)>,
}

/// A family's identity: what its one material binds besides the lane table.
type Family = (AssetId<Image>, AssetId<Image>, AlphaMode);

impl FromWorld for BillboardFamilies {
    fn from_world(world: &mut World) -> Self {
        // Main AND render world: the table stays on the CPU to be rewritten in place, and its
        // unchanged size lets the render world rewrite the same GPU buffer
        // (`GpuShaderBuffer::prepare_asset`), which every family's bind group keeps pointing at.
        let table = ShaderBuffer::new(
            &vec![0u8; BILLBOARD_SLOTS * lane_stride()],
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        Self {
            lanes: world.resource_mut::<Assets<ShaderBuffer>>().add(table),
            made: Vec::new(),
        }
    }
}

/// The billboard material store as the spawners use it.
#[derive(SystemParam)]
pub(crate) struct BillboardMaterials<'w> {
    assets: ResMut<'w, Assets<VfxBillboardMaterial>>,
    families: ResMut<'w, BillboardFamilies>,
}

impl BillboardMaterials<'_> {
    /// `look`'s family material, made the first time its family is drawn.
    fn family(&mut self, look: &BillboardLook) -> Handle<VfxBillboardMaterial> {
        let family = (look.atlas.id(), look.lut.id(), look.alpha_mode);
        if let Some((_, material)) = self.families.made.iter().find(|(f, _)| *f == family) {
            return material.clone();
        }
        let material = self.assets.add(VfxBillboardMaterial {
            lanes: self.families.lanes.clone(),
            atlas: look.atlas.clone(),
            lut: look.lut.clone(),
            alpha_mode: look.alpha_mode,
        });
        self.families.made.push((family, material.clone()));
        material
    }
}

/// How a billboard walks its atlas — one axis, two laws, no redundant state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Flipbook {
    /// `floor(start + age·rate) mod frames` — cells picked out of a set, wrapping. A random `start`
    /// is the anti-repetition trick (survey trick 2); `rate` 0 freezes the cell on it.
    Loop { start: f32, rate: f32 },
    /// `min(floor(frames · age/lifetime), frames - 1)` — a SEQUENCE, spread across the whole
    /// lifetime and clamped on its last cell. The law for a sheet that would re-ignite if it
    /// wrapped. Driven by life fraction rather than a rate, so the walk cannot fall behind the
    /// lifetime: one render per cell reaches every cell and closes on the last, and a coarser
    /// cadence drops cells but still never steps backwards nor returns to the opening cell.
    Once,
}

/// A live billboard's animation state. [`age_billboards`] drives everything visible from `age`:
/// flipbook frame, erosion threshold, LUT row, size ease, drift, spin — and despawns it at
/// `lifetime`.
#[derive(Component)]
pub(crate) struct Billboard {
    pub age: f32,
    /// Cleared until [`age_billboards`]'s first visit, which only arms it: every billboard's first
    /// rendered frame is the age-0 state its spawn wrote, and `age` starts advancing the frame
    /// after.
    armed: bool,
    pub lifetime: f32,
    /// Where the billboard was born; drift and accel displace from here
    /// (`origin + drift·age + ½·accel·age²`).
    pub origin: Vec3,
    /// World-space drift velocity (smoke rise, muzzle-gas push).
    pub drift: Vec3,
    /// World-space acceleration (m/s²). `Vec3::ZERO` — every spawn path but
    /// [`spawn_ballistic_billboard`] — leaves the drift a straight line; thrown mass carries gravity
    /// here and arcs.
    pub accel: Vec3,
    /// Atlas cell count and the law that walks them (see [`flipbook_frame`]).
    pub frames: u32,
    pub flipbook: Flipbook,
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
    /// This sprite's lanes as the shader reads them; [`upload_billboard_lanes`] copies them into
    /// the lane table at the slot its `MeshTag` names.
    pub lanes: VfxParams,
}

/// Marks billboards the facing system turns toward the camera each frame. Directional muzzle
/// planes deliberately lack it — they hold the bore-aligned rotation their spawn gave them.
#[derive(Component)]
pub(crate) struct FaceCamera;

/// Live billboards in spawn order, FIFO-evicted past `cap`, each holding a lane-table slot in
/// `first_slot..first_slot + cap`, handed out round-robin.
///
/// Invariant: no two live billboards share a slot. `live` keeps every spawn until it is evicted
/// (an end-of-life despawn leaves its id behind), so it always holds exactly the last `cap` spawns;
/// the spawn that takes a slot is therefore the one whose push evicts the slot's previous holder,
/// and both commands land in the same flush.
#[derive(Resource)]
pub(crate) struct BillboardRing {
    pub(crate) live: VecDeque<Entity>,
    first_slot: usize,
    cap: usize,
    /// The next slot, as an offset from `first_slot`.
    next: usize,
}

impl BillboardRing {
    /// A ring over lane-table slots `first_slot..first_slot + cap`.
    pub(crate) const fn new(first_slot: usize, cap: usize) -> Self {
        assert!(
            cap > 0 && first_slot + cap <= BILLBOARD_SLOTS,
            "a ring's slots must lie inside the lane table"
        );
        Self {
            live: VecDeque::new(),
            first_slot,
            cap,
            next: 0,
        }
    }

    /// Register `entity`, which took [`Self::slot`], and evict the oldest past the cap.
    fn push(&mut self, commands: &mut Commands, entity: Entity) {
        crate::push_capped_entity(commands, &mut self.live, entity, self.cap);
        self.next = (self.next + 1) % self.cap;
    }

    /// The slot the next spawn takes.
    fn slot(&self) -> MeshTag {
        MeshTag((self.first_slot + self.next) as u32)
    }
}

/// The shared ring every sub-second effect spawns into: slots `0..BILLBOARD_CAP`.
impl Default for BillboardRing {
    fn default() -> Self {
        Self::new(0, BILLBOARD_CAP)
    }
}

/// Everything that varies per billboard spawn, in one bundle-shaped spec so call sites read as
/// data. `material` names the sprite's family and seeds its own lanes (see the module doc).
pub(crate) struct BillboardSpec {
    pub material: BillboardLook,
    pub lifetime: f32,
    pub origin: Vec3,
    pub drift: Vec3,
    pub frames: u32,
    pub flipbook: Flipbook,
    pub start_size: f32,
    pub end_size: f32,
    pub aspect: Vec3,
    pub roll: f32,
    pub spin: f32,
    pub erosion_end: f32,
    /// `None` = camera-facing ([`FaceCamera`]); `Some(rotation)` = fixed world orientation.
    pub rotation: Option<Quat>,
}

/// Spawn one billboard from a spec into `ring` — the shared [`BillboardRing`] resource for every
/// sub-second effect, or a ring of its own for a layer that needs an independent leak bound (e.g.
/// long-lived ground marks that an MG storm would otherwise evict early). See
/// [`spawn_ballistic_billboard`] for thrown mass that must arc under gravity.
pub(crate) fn spawn_billboard(
    commands: &mut Commands,
    materials: &mut BillboardMaterials,
    ring: &mut BillboardRing,
    mesh: Handle<Mesh>,
    spec: BillboardSpec,
) -> Entity {
    spawn_with_accel(commands, materials, ring, mesh, spec, Vec3::ZERO)
}

/// Spawn one billboard under a constant `accel` (m/s²): its position follows
/// `origin + drift·age + ½·accel·age²` instead of the straight drift line. The path for thrown mass
/// — soil clods, sparks, spall — which falls; gas and smoke keep the straight drift.
///
/// The acceleration is a spawn argument rather than a [`BillboardSpec`] field: only these few call
/// sites carry one, and the spec stays the flat data bundle every other effect reads as literals.
pub(crate) fn spawn_ballistic_billboard(
    commands: &mut Commands,
    materials: &mut BillboardMaterials,
    ring: &mut BillboardRing,
    mesh: Handle<Mesh>,
    spec: BillboardSpec,
    accel: Vec3,
) -> Entity {
    spawn_with_accel(commands, materials, ring, mesh, spec, accel)
}

/// The one spawn body behind both entry points above: family material, ring slot and
/// registration, batching and shadow opt-outs. The shared quad `mesh` is the caller's (preloaded once per effect
/// module).
fn spawn_with_accel(
    commands: &mut Commands,
    materials: &mut BillboardMaterials,
    ring: &mut BillboardRing,
    mesh: Handle<Mesh>,
    spec: BillboardSpec,
    accel: Vec3,
) -> Entity {
    let material = materials.family(&spec.material);
    // Seed the animated lanes: this is the state that renders until the ager's arming visit hands
    // over (see [`Billboard::armed`]).
    let mut lanes = spec.material.params;
    lanes.frame.x = flipbook_frame(spec.flipbook, 0.0, spec.lifetime, spec.frames) as f32;
    lanes.fade.z = 0.0;
    let transform = Transform {
        translation: spec.origin,
        rotation: spec.rotation.unwrap_or_default(),
        scale: spec.aspect * spec.start_size,
    };
    let mut entity = commands.spawn((
        Billboard {
            age: 0.0,
            armed: false,
            lifetime: spec.lifetime,
            origin: spec.origin,
            drift: spec.drift,
            accel,
            frames: spec.frames,
            flipbook: spec.flipbook,
            start_size: spec.start_size,
            end_size: spec.end_size,
            aspect: spec.aspect,
            roll: spec.roll,
            spin: spec.spin,
            erosion_end: spec.erosion_end,
            lanes,
        },
        Mesh3d(mesh),
        MeshMaterial3d(material),
        ring.slot(),
        NoAutomaticBatching,
        transform,
        NotShadowCaster,
        NotShadowReceiver,
    ));
    if spec.rotation.is_none() {
        entity.insert(FaceCamera);
    }
    let id = entity.id();
    ring.push(commands, id);
    id
}

/// The atlas cell a particle of `age` out of `lifetime` draws, under its [`Flipbook`] law. This IS
/// the live implementation ([`age_billboards`] writes its result into the sprite's lanes); the
/// shader only does cell-UV arithmetic from the resolved index.
pub(crate) fn flipbook_frame(book: Flipbook, age: f32, lifetime: f32, frames: u32) -> u32 {
    let Some(last) = frames.checked_sub(1) else {
        return 0;
    };
    match book {
        Flipbook::Loop { start, rate } => {
            ((start + age * rate).floor() as i64).rem_euclid(frames as i64) as u32
        }
        // Clamped, not wrapped: the caller's contract is that the last cell is the one on screen
        // when the quad despawns, whatever the frame cadence rounded `age` to.
        Flipbook::Once if lifetime > 0.0 => {
            ((frames as f32 * (age / lifetime)).floor().max(0.0) as u32).min(last)
        }
        Flipbook::Once => 0,
    }
}

/// Advance every live billboard: flipbook frame, erosion, LUT row, size ease, drift, spin — and
/// despawn at end of life. One pass, view-only, `Update` (render cadence; ages in wall time).
/// A newborn is only ARMED on this system's first visit — the frame it was spawned in renders its
/// authored age-0 state, whichever side of this system the spawn landed on.
pub(super) fn age_billboards(
    time: Res<Time>,
    mut billboards: Query<(Entity, &mut Billboard, &mut Transform)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut billboard, mut transform) in &mut billboards {
        if !billboard.armed {
            billboard.armed = true;
            continue;
        }
        billboard.age += dt;
        let t = billboard.age / billboard.lifetime;
        if t >= 1.0 {
            // A billboard has two lifetime owners — this end-of-life ager and the [`BillboardRing`]
            // eviction (which already uses `try_despawn`). Both can target the same entity in one
            // frame; if eviction frees it first and the slot is recycled before this despawn lands in
            // the shared command flush, a plain `despawn` would warn on the stale id. `try_despawn`
            // makes the second despawn a silent no-op.
            commands.entity(entity).try_despawn();
            continue;
        }
        billboard.roll += billboard.spin * dt;
        // Ease-out growth: fast expansion at birth (the gas does its pushing early), settling
        // toward the end size.
        let ease = 1.0 - (1.0 - t) * (1.0 - t);
        let size = billboard.start_size + (billboard.end_size - billboard.start_size) * ease;
        transform.scale = billboard.aspect * size;
        let age = billboard.age;
        transform.translation =
            billboard.origin + billboard.drift * age + billboard.accel * (0.5 * age * age);
        let frame = flipbook_frame(
            billboard.flipbook,
            billboard.age,
            billboard.lifetime,
            billboard.frames,
        ) as f32;
        let erosion = t * billboard.erosion_end;
        billboard.lanes.frame.x = frame;
        billboard.lanes.fade.x = erosion;
        billboard.lanes.fade.z = t;
    }
}

/// Copy every changed billboard's lanes into its lane-table slot: one table mutation per frame at
/// most, none while nothing changed. The table keeps its size, so the render world rewrites its GPU
/// buffer in place and no family bind group is rebuilt.
fn upload_billboard_lanes(
    families: Res<BillboardFamilies>,
    mut tables: ResMut<Assets<ShaderBuffer>>,
    changed: Query<(&MeshTag, &Billboard), Changed<Billboard>>,
) {
    if changed.is_empty() {
        return;
    }
    let Some(mut table) = tables.get_mut(&families.lanes) else {
        return;
    };
    let Some(bytes) = table.data.as_mut() else {
        return;
    };
    let stride = lane_stride();
    for (slot, billboard) in &changed {
        let start = slot.0 as usize * stride;
        encase::StorageBuffer::new(&mut bytes[start..start + stride])
            .write(&billboard.lanes)
            .expect("a lane block fills exactly one slot");
    }
}

/// Turn every [`FaceCamera`] billboard toward the camera, rolled by its own `roll` — the CPU half
/// of billboarding (the material keeps Bevy's default vertex path). Runs even while paused: a
/// frozen smoke puff should still face a camera the player orbits.
fn face_billboards(
    camera: Query<&GlobalTransform, With<PlayerView>>,
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

/// Hermite `smoothstep` (the GLSL/WGSL one) — a clamped 0→1 ease over `[e0, e1]`. Used by the
/// procedural LUT floors and the trail's arc-anchored head fade; kept here so the view layer shares
/// one implementation.
pub(crate) fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x >= e1 { 1.0 } else { 0.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The shared unit quad for billboards (1×1 m, facing +Z, sprite-up along +Y — `Rectangle`'s UV
/// layout puts image-top at +Y, so the Kenney flame sprites point along +Y).
pub(crate) fn unit_quad(meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
    meshes.add(Rectangle::new(1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use bevy::app::TaskPoolPlugin;
    use bevy::asset::AssetPlugin;
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// [`Flipbook::Loop`] — the anti-repetition frame math: wraps modulo the frame count, honors
    /// fractional starts, and never panics on degenerate inputs.
    #[test]
    fn a_looping_flipbook_wraps_and_offsets() {
        let at = |start, rate, age| flipbook_frame(Flipbook::Loop { start, rate }, age, 1.0, 4);
        // 4-frame atlas at 10 fps from frame 0: 0,1,2,3,0,1...
        assert_eq!(at(0.0, 10.0, 0.0), 0);
        assert_eq!(at(0.0, 10.0, 0.15), 1);
        assert_eq!(at(0.0, 10.0, 0.35), 3);
        assert_eq!(at(0.0, 10.0, 0.45), 0, "wraps past the last frame");
        // A random start offset shifts the whole sequence — two particles born together differ.
        assert_eq!(at(2.0, 10.0, 0.0), 2);
        assert_eq!(at(2.0, 10.0, 0.25), 0, "offset sequence wraps too");
        // Degenerate inputs stay in range instead of panicking.
        assert!(at(0.0, 1000.0, 100.0) < 4);
        assert_eq!(
            flipbook_frame(
                Flipbook::Loop {
                    start: 0.0,
                    rate: 1.0
                },
                1.0,
                1.0,
                0
            ),
            0,
            "zero frames is frame 0"
        );
    }

    /// [`Flipbook::Once`] over 12 cells in 0.2 s (the 88 blast's shape), with the whole live
    /// sequence pinned at both bracketing cadences: one render per cell (60 Hz) walks every cell
    /// and closes on the last; half that drops every other cell — twelve cells cannot render in six
    /// frames — and still never steps backwards nor returns to cell 0.
    #[test]
    fn a_once_flipbook_ends_on_its_last_cell() {
        const FRAMES: u32 = 12;
        const LIFE: f32 = 0.2;
        // The live ages `age_billboards` visits at `hz`: birth, then a step per frame until the
        // one that reaches `LIFE` and despawns instead.
        fn live(hz: f32) -> Vec<u32> {
            (0..)
                .map(|step| step as f32 / hz)
                .take_while(|age| *age < LIFE)
                .map(|age| flipbook_frame(Flipbook::Once, age, LIFE, FRAMES))
                .collect()
        }
        assert_eq!(live(60.0), (0..FRAMES).collect::<Vec<_>>());
        assert_eq!(live(30.0), vec![0, 2, 4, 6, 8, 10]);

        // Past its life (the ager despawns first) and degenerate inputs stay clamped in range.
        let at = |age| flipbook_frame(Flipbook::Once, age, LIFE, FRAMES);
        assert_eq!(at(LIFE), FRAMES - 1);
        assert_eq!(at(LIFE * 100.0), FRAMES - 1);
        assert_eq!(flipbook_frame(Flipbook::Once, 1.0, 0.0, FRAMES), 0);
        assert_eq!(flipbook_frame(Flipbook::Once, 1.0, 1.0, 0), 0);
    }

    /// The billboard systems over real asset stores: the asset plugin runs the event flush, so a
    /// test can watch which assets a frame modified; `Time` is advanced by hand.
    fn harness() -> App {
        let mut app = App::new();
        app.add_plugins((TaskPoolPlugin::default(), AssetPlugin::default()))
            .init_asset::<VfxBillboardMaterial>()
            .init_asset::<ShaderBuffer>()
            .init_resource::<BillboardRing>()
            .init_resource::<BillboardFamilies>()
            .init_resource::<Time>()
            .add_systems(Update, age_billboards)
            .add_systems(PostUpdate, upload_billboard_lanes.before(AssetEventSystems));
        app
    }

    fn test_spec(lifetime: f32) -> BillboardSpec {
        BillboardSpec {
            material: BillboardLook {
                params: VfxParams::default(),
                atlas: Handle::default(),
                lut: Handle::default(),
                alpha_mode: AlphaMode::Blend,
            },
            lifetime,
            origin: Vec3::ZERO,
            drift: Vec3::Y,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: 1.0,
                rate: 8.0,
            },
            start_size: 1.0,
            end_size: 3.0,
            aspect: Vec3::ONE,
            roll: 0.0,
            spin: 0.0,
            erosion_end: 1.0,
            rotation: None,
        }
    }

    /// Spawn `spec` into the shared ring under `accel`, through the real spawn entry points.
    fn spawn_spec(app: &mut App, spec: BillboardSpec, accel: Vec3) -> Entity {
        let mut spec = Some(spec);
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      mut materials: BillboardMaterials,
                      mut ring: ResMut<BillboardRing>| {
                    let spec = spec.take().expect("one spawn per system");
                    if accel == Vec3::ZERO {
                        spawn_billboard(&mut commands, &mut materials, &mut ring, default(), spec)
                    } else {
                        spawn_ballistic_billboard(
                            &mut commands,
                            &mut materials,
                            &mut ring,
                            default(),
                            spec,
                            accel,
                        )
                    }
                },
            )
            .expect("the spawn system runs")
    }

    fn spawn_one(app: &mut App, lifetime: f32) -> Entity {
        spawn_spec(app, test_spec(lifetime), Vec3::ZERO)
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

    fn lanes(app: &App, entity: Entity) -> &VfxParams {
        &app.world().get::<Billboard>(entity).expect("alive").lanes
    }

    /// A billboard's FIRST rendered frame is its authored age-0 state: the ager's first visit only
    /// arms it, however long that frame was. A 26 ms flash core aged on its spawn frame would first
    /// draw two thirds through its life at 60 fps, and at the 30 fps used here would never draw at
    /// all.
    #[test]
    fn a_newborn_billboard_renders_its_first_frame_unaged() {
        const FRAME: f32 = 1.0 / 30.0;
        let mut app = harness();
        let entity = spawn_one(&mut app, 0.026);
        let born = *app.world().get::<Transform>(entity).expect("spawned");

        advance(&mut app, FRAME);
        let transform = app
            .world()
            .get::<Transform>(entity)
            .expect("alive on frame one");
        assert_eq!(
            (transform.scale, transform.translation),
            (born.scale, born.translation),
            "the first visit must leave a newborn at its authored size and origin"
        );
        assert_eq!(lanes(&app, entity).fade.x, 0.0, "no erosion on frame one");
        assert_eq!(
            lanes(&app, entity).fade.z,
            0.0,
            "LUT row still the birth row"
        );

        // Armed now: the next frame of the same length carries it past its lifetime.
        advance(&mut app, FRAME);
        assert_eq!(count(&mut app), 0, "ageing resumes the frame after arming");
    }

    /// Billboards live exactly their lifetime, and mid-life the ager has visibly animated them:
    /// grown scale, advanced erosion + LUT row, drifted position.
    #[test]
    fn billboards_animate_then_expire() {
        let mut app = harness();
        let entity = spawn_one(&mut app, 1.0);
        assert_eq!(count(&mut app), 1);

        // The arming frame (pinned by `a_newborn_billboard_renders_its_first_frame_unaged`).
        advance(&mut app, 0.0);
        advance(&mut app, 0.5);
        let transform = app
            .world()
            .get::<Transform>(entity)
            .expect("alive mid-life");
        assert!(
            transform.scale.x > 1.0,
            "mid-life billboard must have grown"
        );
        assert!(
            transform.translation.y > 0.0,
            "mid-life billboard must have drifted"
        );
        assert!(
            lanes(&app, entity).fade.x > 0.0,
            "erosion must ramp with age"
        );
        assert!(
            lanes(&app, entity).fade.z > 0.0,
            "LUT row must track life fraction"
        );

        advance(&mut app, 0.6);
        assert_eq!(count(&mut app), 0, "an expired billboard must despawn");
    }

    /// Sprites of one family (atlas, LUT, alpha mode) draw through ONE material however their lanes
    /// differ, and another family gets its own: the sharing that lets a family batch as instances
    /// and keeps the material count at the family count, not the sprite count.
    #[test]
    fn a_family_shares_one_material() {
        let mut app = harness();
        let first = spawn_one(&mut app, 1.0);
        let mut brighter = test_spec(1.0);
        brighter.material.params.fade.w = 0.25;
        brighter.material.params.glow.x = 7.0;
        let second = spawn_spec(&mut app, brighter, Vec3::ZERO);
        let mut additive = test_spec(1.0);
        additive.material.alpha_mode = AlphaMode::Add;
        let other_family = spawn_spec(&mut app, additive, Vec3::ZERO);

        let material = |entity| {
            app.world()
                .get::<MeshMaterial3d<VfxBillboardMaterial>>(entity)
                .expect("material")
                .0
                .clone()
        };
        assert_eq!(
            material(first),
            material(second),
            "one family, one material"
        );
        assert_ne!(
            material(first),
            material(other_family),
            "another alpha mode is another family"
        );
        assert_eq!(
            app.world().resource::<Assets<VfxBillboardMaterial>>().len(),
            2,
            "one material per family, none per sprite"
        );
        assert_ne!(
            lanes(&app, first).glow.x,
            lanes(&app, second).glow.x,
            "a shared material still leaves each sprite its own lanes"
        );
    }

    /// Ageing moves every animated lane through the lane table and modifies no material — a
    /// modified material rebuilds its bind group, which is the per-frame cost the table replaces.
    /// The table then holds each live billboard's own lanes at the slot its `MeshTag` names, the
    /// index the fragment reads.
    #[test]
    fn ageing_billboards_modifies_no_material() {
        #[derive(Resource, Default)]
        struct Modified {
            materials: usize,
            tables: usize,
        }
        fn watch(
            mut materials: MessageReader<AssetEvent<VfxBillboardMaterial>>,
            mut tables: MessageReader<AssetEvent<ShaderBuffer>>,
            mut seen: ResMut<Modified>,
        ) {
            seen.materials += materials
                .read()
                .filter(|event| matches!(event, AssetEvent::Modified { .. }))
                .count();
            seen.tables += tables
                .read()
                .filter(|event| matches!(event, AssetEvent::Modified { .. }))
                .count();
        }

        let mut app = harness();
        app.init_resource::<Modified>().add_systems(Last, watch);
        let billboards = [spawn_one(&mut app, 1.0), spawn_one(&mut app, 2.0)];
        // The arming frame, then a run of ageing frames.
        advance(&mut app, 0.0);
        for _ in 0..5 {
            advance(&mut app, 0.1);
        }

        let seen = app.world().resource::<Modified>();
        assert_eq!(seen.materials, 0, "ageing must modify no material");
        assert_eq!(
            seen.tables, 6,
            "the lanes move through one table write per frame"
        );
        let world = app.world();
        let table = world
            .resource::<Assets<ShaderBuffer>>()
            .get(&world.resource::<BillboardFamilies>().lanes)
            .and_then(|table| table.data.as_deref())
            .expect("the lane table keeps its CPU copy");
        let stride = lane_stride();
        for billboard in billboards {
            let slot = world.get::<MeshTag>(billboard).expect("a slot").0 as usize;
            let lanes = lanes(&app, billboard);
            assert!(lanes.fade.z > 0.0, "the billboard aged");
            let mut expected = encase::StorageBuffer::new(Vec::<u8>::new());
            expected.write(lanes).expect("encodable lanes");
            assert_eq!(
                &table[slot * stride..(slot + 1) * stride],
                expected.as_ref().as_slice(),
                "slot {slot} must hold its own billboard's current lanes"
            );
        }
    }

    /// A ballistic billboard falls exactly `½·g·t²` below the straight drift line its zero-accel
    /// twin holds, and both share the same start point — the arc thrown mass rides.
    #[test]
    fn ballistic_billboards_fall_under_gravity() {
        const G: Vec3 = Vec3::new(0.0, -9.81, 0.0);
        let mut app = harness();
        let straight = spawn_one(&mut app, 2.0);
        let thrown = spawn_spec(&mut app, test_spec(2.0), G);
        // Born together: the arc term is zero at age 0.
        let pos = |app: &App, e: Entity| app.world().get::<Transform>(e).unwrap().translation;
        assert_eq!(pos(&app, straight), pos(&app, thrown));

        let t = 0.5;
        // The arming frame (pinned by `a_newborn_billboard_renders_its_first_frame_unaged`).
        advance(&mut app, 0.0);
        advance(&mut app, t);
        let drop = pos(&app, straight).y - pos(&app, thrown).y;
        assert!(
            (drop - 0.5 * 9.81 * t * t).abs() < 1e-3,
            "a thrown billboard must fall ½·g·t² below the straight line (dropped {drop})"
        );
    }

    /// The eviction ring bounds live billboards at the cap, oldest first — the leak bound under
    /// refire storms (rollback-replayed fire seams) — and no two live billboards share a lane-table
    /// slot, the invariant the lane upload relies on: a shared slot draws one sprite with another's
    /// lanes.
    #[test]
    fn billboard_ring_caps_and_keeps_slots_unique() {
        let mut app = harness();
        for _ in 0..BILLBOARD_CAP + 9 {
            spawn_one(&mut app, 60.0);
        }
        assert_eq!(count(&mut app), BILLBOARD_CAP);
        assert_eq!(
            app.world().resource::<BillboardRing>().live.len(),
            BILLBOARD_CAP
        );
        let slots: std::collections::BTreeSet<u32> = app
            .world_mut()
            .query_filtered::<&MeshTag, With<Billboard>>()
            .iter(app.world())
            .map(|tag| tag.0)
            .collect();
        assert_eq!(
            slots.len(),
            BILLBOARD_CAP,
            "every live billboard holds its own slot"
        );
        assert!(
            slots.iter().all(|&slot| (slot as usize) < BILLBOARD_CAP),
            "the shared ring stays inside its slot range"
        );
    }
}
