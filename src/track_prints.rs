//! Track prints in the ground (issue 125): cosmetic terrain marks, the view of where every shoe of
//! every belt has rested. VIEW ONLY (ADR-0014): the ground's geometry, its collider, the oracle
//! and the simulation are untouched, no print crosses the wire, and only a windowed client builds
//! any of this. A late joiner misses the prints laid before it joined.
//!
//! Design, alternatives and the numbers they rest on: `.agents/scratch/impact-feedback-2026-10-28/`
//! (`track-prints-approaches.md` §4, `track-prints-ground-stamping.md` §1 and §4, and the
//! slice-2 brief `track-prints-125-slice2-brief.md`).
//!
//! # The pipeline
//!
//! 1. **The stamp** ([`stamp`]). Once the shoe template binds (`track::link_view`), the authored
//!    shoe's underside is baked at full detail into a height-and-coverage stamp in its canonical
//!    frame, then pooled to the near store's texel. The left shoe is the right's genuine mirror
//!    (`link_view::mirrored_mesh`), so it reads the same stamp mirrored.
//! 2. **The stamper** ([`stamper`], [`stamp_shoes`]). Every frame, after transform propagation,
//!    each shoe of every belt the conformed belt view drew is read at its drawn pose; a grounded
//!    one is stamped as its press deepens over one pitch of travel, and again whenever it has slid
//!    half a texel. The view runs for every tank on every windowed client, so every tank prints.
//! 3. **The stores** ([`store`]). Each stamp is rasterised once into the history; the near store's
//!    GPU copy (which follows the eye, [`follow_eye`]) and the world layer are fed from it.
//! 4. **The upload** ([`upload`]). Each frame's written tiles go to the two GPU images.
//! 5. **The shader** (`terrain_blend.wgsl`). Each pack names a ground type
//!    ([`crate::ground_types`]); the sole, the shoe's lowest face, sinks by that ground's
//!    `softness × pressure`, capped, under the share of its tank's pressure the press has put on
//!    it, and a texel prints that sinkage less the shoe's baked clearance over its sole there, so a
//!    print takes the shoe's shape as deep as the ground lets it in. Hard ground takes no depth.
//!    The world layer darkens and roughens everywhere; inside the near store the depth bends the
//!    normal, and up to the player's set distance it is drawn with parallax and the sun's shadow
//!    ([`crate::settings::TrackPrintRelief`]).
//! 6. **The belt sink** ([`PrintDepths`]). The drawn belt lowers its sole by the depth the shader
//!    draws under a full press, onto its print's floor (`track::view`).
//!
//! # Entry points
//!
//! [`images`] and [`install`] are called where the ground's material is built
//! (`world::spawn_environment`), which is the windowed heightmap world only; [`plugin`] is mounted
//! by the track view (`track::view_plugin`), which only windowed clients compose. Every system is
//! inert until [`TrackPrints`] exists.

pub(crate) mod demo;
mod stamp;
mod stamper;
mod store;
#[cfg(feature = "dev_ui")]
pub(crate) mod tuning;
mod upload;

#[cfg(test)]
use crate::track::rig_geom::ground_pressure_kpa;
pub(crate) use crate::track::rig_geom::tank_ground_pressure_kpa;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::math::Affine3A;
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::ground_types::GroundTypes;
use crate::settings::{Settings, TrackPrintRange, TrackPrintRelief};
use crate::tank::Tank;
use crate::terrain_blend::{TerrainBlendParams, TerrainMaterial};
use crate::terrain_grid::HeightGrid;
use crate::track::link_view::{BeltFrames, LinkTemplate, ShoeBelt};
use crate::track::rig_geom::RigGeom;
use crate::track::side::Side;
use crate::view::PlayerView;

use stamp::ClearanceStamp;
use stamper::{ShoeKey, ShoeMemory};
use store::{
    CLEARANCE_UNIT_M, KPA_PER_UNIT, NEAR_SIDE, NEAR_SPAN_M, NEAR_TEXEL_M, PrintLaw, PrintStores,
    StampEntry, Store, WORLD_SIDE,
};
use upload::{PrintWrites, TileWrite};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PrintAppearance {
    contact_m: crate::provenance::Labelled<f32>,
    near_fade_m: crate::provenance::Labelled<f32>,
    near_gone_px: crate::provenance::Labelled<f32>,
    near_full_px: crate::provenance::Labelled<f32>,
    relief_fade_share: crate::provenance::Labelled<f32>,
}

fn print_appearance() -> &'static PrintAppearance {
    static DATA: std::sync::LazyLock<PrintAppearance> = std::sync::LazyLock::new(|| {
        const PATH: &str = "assets/terrain/track_prints.ron";
        let data: PrintAppearance = crate::provenance::from_shipped_str(
            PATH,
            include_str!("../assets/terrain/track_prints.ron"),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        for (field, value) in [
            ("contact_m", &data.contact_m),
            ("near_fade_m", &data.near_fade_m),
            ("relief_fade_share", &data.relief_fade_share),
        ] {
            assert!(
                value.value.is_finite()
                    && value.value > 0.0
                    && matches!(value.label, crate::provenance::Provenance::Taste(_)),
                "{PATH}: {field} must be finite, positive and Taste; state the track-print appearance magnitude"
            );
        }
        for (field, value) in [
            ("near_gone_px", &data.near_gone_px),
            ("near_full_px", &data.near_full_px),
        ] {
            assert!(
                value.value.is_finite()
                    && value.value > 0.0
                    && matches!(value.label, crate::provenance::Provenance::Resolution(_)),
                "{PATH}: {field} must be finite, positive and Resolution; state the texel anti-aliasing bound"
            );
        }
        assert!(
            data.near_full_px.value > data.near_gone_px.value
                && data.relief_fade_share.value <= 1.0,
            "{PATH}: near_full_px must exceed near_gone_px and relief_fade_share must not exceed one; keep ordered fades"
        );
        data
    });
    &DATA
}

/// The prints' CPU half: the baked shoe, the stores and each tank's shoe memory, plus the GPU
/// images and the material they are drawn through.
#[derive(Resource)]
pub(crate) struct TrackPrints {
    near_image: Handle<Image>,
    world_image: Handle<Image>,
    material: Handle<TerrainMaterial>,
    law: PrintLaw,
    /// The baked shoes, pooled to the near store's texel: empty until the template binds.
    shoes: Vec<ClearanceStamp>,
    stores: PrintStores,
    tanks: HashMap<Entity, TankPrints>,
    /// The near window's origin as the material last heard it.
    told: Option<Vec2>,
}

impl TrackPrints {
    fn new(images: PrintImages, material: Handle<TerrainMaterial>, world_size_m: f32) -> Self {
        Self {
            near_image: images.near,
            world_image: images.world,
            material,
            law: PrintLaw {
                contact_m: print_appearance().contact_m.value,
            },
            shoes: Vec::new(),
            stores: PrintStores::new(world_size_m, WORLD_SIDE),
            tanks: HashMap::new(),
            told: None,
        }
    }
}

/// Construct empty print stores through the startup constructor. Fresh image identities also
/// isolate pending render-world uploads: an old tile can only write an old, unbound image.
pub(crate) fn reconstruct(world: &mut World) {
    let Some(old) = world.remove_resource::<TrackPrints>() else {
        return; // A composition without a drawn heightmap has no print stores.
    };
    let extent = world.resource::<HeightGrid>().world_size();
    let fresh_images = images(&mut world.resource_mut::<Assets<Image>>());
    {
        let mut materials = world.resource_mut::<Assets<TerrainMaterial>>();
        let mut terrain = materials
            .get_mut(&old.material)
            .expect("the terrain retains the material its print stores were built for");
        terrain.extension.prints_near = fresh_images.near.clone();
        terrain.extension.prints_world = fresh_images.world.clone();
        terrain.extension.params.print_near = near_params(Vec2::ZERO);
    }
    world.insert_resource(TrackPrints::new(fresh_images, old.material, extent));
    world.resource_mut::<PrintWrites>().0.clear();
}

/// One tank's stamping memory.
#[derive(Default)]
struct TankPrints {
    memory: ShoeMemory,
    /// The root's pose last frame: the teleport detector's previous frame.
    last: Option<Affine3A>,
    /// Whether this frame's pose differs from `last` (a first pose counts): a still root's belts
    /// read nothing new from it.
    moved: bool,
    seen: bool,
}

/// What the depth a print reaches is read from, CPU-side: the material's print lanes as the shader
/// last received them, and the map's painted mask. The belt sink reads its depth here.
#[derive(Resource)]
pub(crate) struct PrintDepths {
    ground: crate::ground_field::GroundField,
    /// The material's parameters as last written ([`sync_material`]).
    params: TerrainBlendParams,
}

impl PrintDepths {
    /// The same decoded mask and sampling law the simulation reads.
    pub(crate) fn ground_weights(&self, xz: Vec2) -> Vec4 {
        self.ground.weights_at(xz)
    }

    /// FIXTURE: an unpainted ground, so the base pack is the whole ground.
    #[cfg(test)]
    pub(crate) fn unpainted() -> Self {
        Self {
            ground: crate::ground_field::GroundField::unpainted(&GroundTypes::load_shipped()),
            params: TerrainBlendParams::default(),
        }
    }

    /// FIXTURE: a loaded one-texel mask painting `mask` everywhere (R recesses, G slopes,
    /// B lowlands).
    #[cfg(test)]
    pub(crate) fn painted_everywhere(mask: Vec3) -> Self {
        Self {
            ground: crate::ground_field::GroundField::fixture_mask(UVec2::ONE, 1.0, vec![mask]),
            params: TerrainBlendParams::default(),
        }
    }

    /// The depth (m) the shader draws at world XZ `xz` under a full press by a tank of ground
    /// pressure `kpa`: the sole's sinkage, the deepest point of its print there. The pressure is
    /// read as the near store stores it, in whole [`KPA_PER_UNIT`], so this is the shader's own
    /// number.
    pub(crate) fn full_press_m(&self, xz: Vec2, kpa: f32) -> f32 {
        let stored = (kpa / KPA_PER_UNIT).round().clamp(1.0, 255.0) * KPA_PER_UNIT;
        crate::terrain_blend::sinkage_m(&self.params, self.ground_weights(xz), stored)
    }
}

/// A dev capture's override of the player's relief setting ([`demo`]); absent in play.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub(crate) struct ReliefOverride(pub(crate) TrackPrintRelief);

/// The two images the ground's material reads the prints from.
pub(crate) struct PrintImages {
    pub(crate) near: Handle<Image>,
    pub(crate) world: Handle<Image>,
}

/// A print store's GPU image: written only by [`upload`], so it is made without data, and zeroed by
/// wgpu, which is the empty store.
fn store_image(side: u32, format: TextureFormat, levels: u32) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = levels;
    image
}

/// Make the two GPU images: the near store (`Rg8Unorm`, press and pressure) and the world layer
/// (`R8Unorm`, coverage, full mip chain).
pub(crate) fn images(images: &mut Assets<Image>) -> PrintImages {
    PrintImages {
        near: images.add(store_image(NEAR_SIDE, TextureFormat::Rg8Unorm, 1)),
        world: images.add(store_image(
            WORLD_SIDE,
            TextureFormat::R8Unorm,
            WORLD_SIDE.ilog2() + 1,
        )),
    }
}

/// Start printing onto the map of side `world_size_m`, drawn through `material`, whose painted
/// mask is shared with `field`, on the ground types of `ground`.
pub(crate) fn install(
    commands: &mut Commands,
    images: PrintImages,
    material: Handle<TerrainMaterial>,
    field: crate::ground_field::GroundField,
    world_size_m: f32,
    ground: GroundTypes,
) {
    let prints = TrackPrints::new(images, material, world_size_m);
    info!(
        "track prints: near store {NEAR_SIDE}² Rg8 over {NEAR_SPAN_M} m ({:.2} cm texels), world \
         layer {WORLD_SIDE}² R8 over {world_size_m} m ({:.1} cm texels, {} levels), {} ground types",
        NEAR_TEXEL_M * 100.0,
        world_size_m / WORLD_SIDE as f32 * 100.0,
        prints.stores.world.levels(),
        ground.types.len(),
    );
    commands.insert_resource(ground);
    commands.insert_resource(PrintDepths {
        ground: field,
        params: TerrainBlendParams::default(),
    });
    commands.insert_resource(prints);
}

/// The material's `print_near` lane for a near window whose low corner is at world XZ
/// `origin_m`: `xy` that corner, `z` the window's side, `w` its texel.
pub(crate) fn near_params(origin_m: Vec2) -> Vec4 {
    Vec4::new(origin_m.x, origin_m.y, NEAR_SPAN_M, NEAR_TEXEL_M)
}

/// The material's `print_world` lane: world XZ to the world layer's UV, `uv = xz · xy + zw`.
pub(crate) fn world_params(world_size_m: f32) -> Vec4 {
    let per_metre = 1.0 / world_size_m;
    Vec4::new(per_metre, per_metre, 0.5, 0.5)
}

/// The material's `print_law` lane: `x` the ground pressure (kPa) a full G channel stands for,
/// `y` the window fade, `zw` the pixels per texel at which the near normal is gone and whole.
pub(crate) fn law_params() -> Vec4 {
    Vec4::new(
        255.0 * KPA_PER_UNIT,
        print_appearance().near_fade_m.value,
        print_appearance().near_gone_px.value,
        print_appearance().near_full_px.value,
    )
}

/// The material's `print_store` lane: `x` the clearance over a sole (m) an empty R channel stands
/// for.
pub(crate) fn store_params() -> Vec4 {
    Vec4::new(255.0 * CLEARANCE_UNIT_M, 0.0, 0.0, 0.0)
}

/// The material's `print_relief` lane for the player's `relief` and `range`: `x` the level, `y`
/// the distance (m) relief reaches, `z` the distance over which it fades to lighting only.
pub(crate) fn relief_params(relief: TrackPrintRelief, range: TrackPrintRange) -> Vec4 {
    let reach = range.metres();
    Vec4::new(
        f32::from(relief.level()),
        reach,
        reach * print_appearance().relief_fade_share.value,
        0.0,
    )
}

/// Bake the shoe once the template has bound. Binding certifies the shoe before it publishes the
/// template (`track::belt_draw::certify_template`): loaded, finite `Float32x3` positions, complete
/// triangles indexing only them. A shoe whose underside covers no print cell is refused here, the
/// first system that reads it, in every build.
fn bake_shoes(
    mut prints: ResMut<TrackPrints>,
    template: Option<Res<LinkTemplate>>,
    geom: Option<Res<RigGeom>>,
    meshes: Res<Assets<Mesh>>,
) {
    // The template has not bound yet: no shoe to bake.
    let Some(template) = template else {
        return;
    };
    let mesh = meshes
        .get(template.shoe_mesh(Side::Right))
        .expect("a published template's shoe was certified loaded, and the template holds it");
    let frame = template.frame(Side::Right);
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        panic!(
            "a certified shoe carries Float32x3 positions (`track::belt_draw::certify_template`)"
        );
    };
    let canonical: Vec<Vec3> = positions
        .iter()
        .map(|&p| frame.mesh_to_canonical(Vec3::from(p)))
        .collect();
    let corners: Vec<usize> = match mesh.indices() {
        Some(Indices::U16(indices)) => indices.iter().map(|&i| usize::from(i)).collect(),
        Some(Indices::U32(indices)) => indices.iter().map(|&i| i as usize).collect(),
        None => (0..canonical.len()).collect(),
    };
    let triangles: Vec<[Vec3; 3]> = corners
        .as_chunks::<3>()
        .0
        .iter()
        .map(|tri| tri.map(|i| canonical[i]))
        .collect();
    let started = Instant::now();
    let Some(baked) = stamp::bake(&triangles) else {
        panic!(
            "{}: track shoe `{}`: its underside covers no print cell; export the shoe with its \
             ground face toward the canonical outside",
            crate::tank::TANK_GLB_PATH,
            crate::track::link_view::LINK_NODE,
        );
    };
    let pooled = ClearanceStamp::pooled(&baked, NEAR_TEXEL_M);
    let elapsed = started.elapsed();
    let relief = baked
        .height
        .iter()
        .copied()
        .max()
        .map_or(0.0, |h| f32::from(h) * baked.height_scale_m);
    let (low, high) = baked.bounds();
    info!(
        "track prints: shoe stamp baked from {} triangles into {}x{} cells of {} mm \
         ({:.3} x {:.3} m, relief {:.1} mm, {:.0} % covered) and pooled to the near texel in {} µs \
         MEASURED; ground pressure {} kPa DERIVED",
        baked.triangles,
        baked.size.x,
        baked.size.y,
        baked.cell_m * 1000.0,
        high.x - low.x,
        high.y - low.y,
        relief * 1000.0,
        baked.coverage.iter().filter(|&&c| c > 0).count() as f32 * 100.0
            / baked.coverage.len() as f32,
        elapsed.as_micros(),
        geom.map_or("unknown".into(), |geom| format!(
            "{:.1}",
            tank_ground_pressure_kpa(&geom)
        )),
    );
    prints.shoes.push(pooled);
}

/// Whether a root moved from `previous` to `now` in a way no frame of motion does: the track
/// view's own snap rule (`track::view`).
fn snapped(previous: Affine3A, now: Affine3A) -> bool {
    use crate::track::view::{SNAP_AXIS, SNAP_TRANSLATION};
    let axis_jump = |axis: Vec3| {
        now.transform_vector3(axis)
            .distance(previous.transform_vector3(axis))
            > SNAP_AXIS
    };
    (now.translation - previous.translation).length() > SNAP_TRANSLATION
        || axis_jump(Vec3::Z)
        || axis_jump(Vec3::Y)
}

/// Stamp every grounded shoe whose press deepened or which moved, on every belt, at this frame's
/// drawn pose: each material link of the belt's table carried through the belt's own
/// `GlobalTransform`, the pose the belt draws it at. No GPU readback; the table is the CPU's.
///
/// A belt is read when one of its inputs moved since the last run: its table, its pose, its
/// tank's pose, or the ground, template or rig. With none moved its shoes' contacts are the ones
/// already observed, and observing a contact again changes no memory and stamps nothing
/// ([`stamper::ShoeMemory::observe`]), so a settled tank's belts are skipped whole.
fn stamp_shoes(
    mut prints: ResMut<TrackPrints>,
    grid: Res<HeightGrid>,
    template: Res<LinkTemplate>,
    geom: Res<RigGeom>,
    tanks: Query<(Entity, &GlobalTransform), With<Tank>>,
    belts: Query<(
        Entity,
        &ShoeBelt,
        Ref<BeltFrames>,
        &ChildOf,
        Ref<GlobalTransform>,
    )>,
    mut laid: Local<Vec<StampEntry>>,
) {
    let TrackPrints {
        law,
        shoes,
        stores,
        tanks: memory,
        ..
    } = &mut *prints;
    let Some(shoe) = shoes.first() else {
        return;
    };
    // One running gear today, every tank's: the spec and model the rig was derived from.
    let (pitch, kpa) = (geom.pitch, tank_ground_pressure_kpa(&geom));
    for tank in memory.values_mut() {
        tank.seen = false;
    }
    for (root, pose) in &tanks {
        let tank = memory.entry(root).or_default();
        let now = pose.affine();
        if tank.last.is_some_and(|last| snapped(last, now)) {
            tank.memory.reset();
        }
        tank.moved = tank.last != Some(now);
        tank.last = Some(now);
        tank.seen = true;
    }
    memory.retain(|_, tank| tank.seen);
    let refit = grid.is_changed() || template.is_changed() || geom.is_changed();

    let ground = |xz: Vec2| {
        grid.contains_xz(xz.x, xz.y)
            .then(|| grid.height_at(xz.x, xz.y))
    };
    for (entity, belt, frames, parent, at) in &belts {
        let Some(tank) = memory.get_mut(&parent.parent()) else {
            continue;
        };
        if !refit && !tank.moved && !frames.is_changed() && !at.is_changed() {
            continue;
        }
        let Some(hull) = tank.last.map(|pose| pose.inverse()) else {
            continue;
        };
        let side = belt.side();
        let mirrored = side == Side::Left;
        let to_mesh = template.frame(side).canonical_to_mesh();
        let bounds = shoe.bounds(mirrored);
        let belt_pose = at.affine();
        for link in 0..frames.count() {
            let key = ShoeKey::new(entity, link);
            // The belt's `GlobalTransform` times the link's table pose: the transform the belt's
            // vertex shader applies to this shoe.
            let global = belt_pose * frames.pose(link);
            let Some(contact) = stamper::contact(
                global * to_mesh,
                bounds,
                shoe.datum_y(),
                law.contact_m,
                ground,
            ) else {
                tank.memory.airborne(key);
                continue;
            };
            let hull_local = hull.transform_point3(global.translation.into());
            if let Some(sweep) = tank.memory.observe(key, &contact, hull_local, pitch) {
                laid.extend(sweep.map(|(contact, press)| {
                    StampEntry::new(contact.placement(0, mirrored), press, kpa)
                }));
            }
        }
    }
    for entry in laid.drain(..) {
        stores.stamp(&entry, shoes, law);
    }
}

/// Keep the near store's window on the player's eye.
fn follow_eye(mut prints: ResMut<TrackPrints>, eye: Query<&GlobalTransform, With<PlayerView>>) {
    let Ok(eye) = eye.single() else {
        return;
    };
    let stores = &mut prints.stores;
    let eye = eye.translation();
    stores.recentre(Vec2::new(eye.x, eye.z));
}

/// Hand this frame's written tiles to the render world, and the near window's place to the
/// material when it moved.
fn flush(
    mut prints: ResMut<TrackPrints>,
    mut writes: ResMut<PrintWrites>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    let prints = &mut *prints;
    writes.0 = prints
        .stores
        .take_uploads()
        .into_iter()
        .map(|tile| TileWrite {
            image: match tile.store {
                Store::Near => prints.near_image.id(),
                Store::World => prints.world_image.id(),
            },
            level: tile.level,
            origin: tile.origin,
            size: tile.size,
            bytes_per_texel: tile.store.bytes_per_texel(),
            data: Arc::from(tile.data),
        })
        .collect();
    let origin = prints.stores.near.origin_m();
    if origin != prints.told
        && let Some(origin) = origin
        && let Some(mut material) = materials.get_mut(&prints.material)
    {
        material.extension.params.print_near = near_params(origin);
        prints.told = Some(origin);
    }
}

/// Write the ground types' print lanes and the relief setting into the material when either
/// changed, and keep [`PrintDepths`] reading the very lanes the shader receives.
fn sync_material(
    prints: Res<TrackPrints>,
    ground: Res<GroundTypes>,
    settings: Option<Res<Settings>>,
    relief_override: Option<Res<ReliefOverride>>,
    mut depths: ResMut<PrintDepths>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut synced: Local<bool>,
) {
    let changed = !*synced
        || ground.is_changed()
        || settings
            .as_ref()
            .is_some_and(|settings| settings.is_changed())
        || relief_override
            .as_ref()
            .is_some_and(|forced| forced.is_changed());
    if !changed {
        return;
    }
    let Some(mut material) = materials.get_mut(&prints.material) else {
        return;
    };
    let settings = settings.map(|settings| *settings).unwrap_or_default();
    let relief = relief_override.map_or(settings.track_prints, |forced| forced.0);
    let params = &mut material.extension.params;
    crate::terrain_blend::set_print_lanes(params, &ground);
    params.print_relief = relief_params(relief, settings.track_print_range);
    depths.params = params.clone();
    *synced = true;
}

/// Whether anything of the prints exists in `world`: their stores, their upload queue, the depth
/// field the belt sinks by, or the ground-type table they are drawn with.
#[cfg(test)]
pub(crate) fn composed(world: &World) -> bool {
    world.contains_resource::<TrackPrints>()
        || world.contains_resource::<PrintWrites>()
        || world.contains_resource::<PrintDepths>()
        || world.contains_resource::<GroundTypes>()
}

/// The system set every prints system runs in, in `PostUpdate` after the drawn poses propagate.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TrackPrintsSet;

/// The prints' main-world systems and their render-world upload. Mounted by the track view.
pub(crate) fn plugin(app: &mut App) {
    print_appearance();
    app.add_plugins(upload::plugin);
    app.add_systems(
        Update,
        bake_shoes.run_if(|prints: Option<Res<TrackPrints>>| {
            prints.is_some_and(|prints| prints.shoes.is_empty())
        }),
    );
    app.configure_sets(
        PostUpdate,
        TrackPrintsSet
            .after(TransformSystems::Propagate)
            .after(crate::camera::GunnerCameraPlaced)
            .run_if(resource_exists::<TrackPrints>),
    );
    app.add_systems(
        PostUpdate,
        (
            stamp_shoes
                .run_if(resource_exists::<HeightGrid>)
                .run_if(resource_exists::<LinkTemplate>)
                .run_if(resource_exists::<RigGeom>),
            follow_eye,
            flush,
            sync_material,
        )
            .chain()
            .in_set(TrackPrintsSet),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new construction has no print history, shoe memory or pending upload from the previous
    /// world. Distinct GPU image identities prevent an extracted old tile resurrecting a rut.
    /// Terrain rendering and belt sink both rely on this boundary.
    #[test]
    fn a_fresh_world_constructs_empty_print_stores_and_new_gpu_images() {
        // ASSUMED fixture dimensions, pressure and camera origin.
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Assets<TerrainMaterial>>();
        world.init_resource::<PrintWrites>();
        world.insert_resource(HeightGrid::new(
            vec![0.0; 9].into(),
            3,
            crate::terrain_grid::FIXTURE_EXTENT,
        ));
        let images = images(&mut world.resource_mut::<Assets<Image>>());
        let material = world
            .resource_mut::<Assets<TerrainMaterial>>()
            .add(TerrainMaterial {
                base: StandardMaterial::default(),
                extension: crate::terrain_blend::TerrainBlend {
                    ablate: default(),
                    params: default(),
                    layer_albedo: default(),
                    layer_normal: default(),
                    layer_arm: default(),
                    masks: default(),
                    prints_near: default(),
                    prints_world: default(),
                },
            });
        let mut prints = TrackPrints::new(
            images,
            material.clone(),
            crate::terrain_grid::FIXTURE_EXTENT.world_size_m,
        );
        let old_images = (prints.near_image.id(), prints.world_image.id());
        prints
            .shoes
            .push(ClearanceStamp::pooled(&stamp_fixture(), NEAR_TEXEL_M));
        prints.stores.recentre(Vec2::ZERO);
        prints.stores.stamp(
            &StampEntry::new(
                store::Placement {
                    datum: Vec2::ZERO,
                    yaw: 0.0,
                    lift: 0.0,
                    tilt: Vec2::ZERO,
                    shoe: 0,
                    mirrored: false,
                },
                1.0,
                100.0,
            ),
            &prints.shoes,
            &prints.law,
        );
        assert!(prints.stores.log.len() > 0);
        let tank = world.spawn_empty().id();
        prints.tanks.insert(tank, TankPrints::default());
        prints.told = Some(Vec2::ONE);
        world.insert_resource(prints);
        world.resource_mut::<PrintWrites>().0.push(TileWrite {
            image: old_images.0,
            level: 0,
            origin: UVec2::ZERO,
            size: UVec2::ONE,
            bytes_per_texel: 2,
            data: Arc::from([255, 255]),
        });
        reconstruct(&mut world);
        let fresh = world.resource::<TrackPrints>();
        assert_eq!(fresh.stores.log.len(), 0);
        assert!(fresh.shoes.is_empty() && fresh.tanks.is_empty() && fresh.told.is_none());
        assert_ne!(old_images, (fresh.near_image.id(), fresh.world_image.id()));
        let terrain = world
            .resource::<Assets<TerrainMaterial>>()
            .get(&material)
            .unwrap();
        assert_eq!(terrain.extension.prints_near, fresh.near_image);
        assert_eq!(terrain.extension.prints_world, fresh.world_image);
        assert!(world.resource::<PrintWrites>().0.is_empty());
    }

    /// ADR-0034, TANKS ARE DATA: the prints' laws take every vehicle's geometry from its shoe
    /// template and its mass and footprint from its data, and nothing else, so no line of their
    /// code may name a vehicle. The names are read off the shipped tank files
    /// (`assets/<id>/<id>.tank.ron`, the id and its alphabetic stem), so a second tank joins the
    /// check by existing. Comments are prose and are skipped; a string or an identifier carrying a
    /// name — a branch on one, a per-tank constant — fails.
    #[test]
    fn no_law_of_the_prints_names_a_tank() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut names: Vec<String> = Vec::new();
        for dir in std::fs::read_dir(root.join("assets"))
            .expect("assets")
            .flatten()
        {
            let id = dir.file_name().to_string_lossy().into_owned();
            if dir.path().join(format!("{id}.tank.ron")).exists() {
                let stem: String = id.chars().take_while(char::is_ascii_alphabetic).collect();
                names.push(id.to_lowercase());
                if stem.len() >= 3 {
                    names.push(stem.to_lowercase());
                }
            }
        }
        assert!(!names.is_empty(), "the shipped tanks are found");
        let mut sources = vec![
            root.join("src/track_prints.rs"),
            root.join("src/ground_types.rs"),
            root.join("src/terrain_blend.rs"),
            root.join("assets/shaders/terrain_blend.wgsl"),
            root.join("assets")
                .join(crate::ground_types::GROUND_TYPES_PATH),
        ];
        for file in std::fs::read_dir(root.join("src/track_prints"))
            .expect("module dir")
            .flatten()
        {
            sources.push(file.path());
        }
        let mut found = Vec::new();
        for path in sources {
            let text = std::fs::read_to_string(&path).expect("readable");
            for (number, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("").to_lowercase();
                for name in &names {
                    if code.contains(name.as_str()) {
                        found.push(format!("{}:{}: {name}", path.display(), number + 1));
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "a tank is named in the prints' code: {found:?}"
        );
    }

    /// HIGHER GROUND PRESSURE PRINTS DEEPER ON THE SAME GROUND: a heavier tank on the same
    /// footprint bears harder on the ground, the store carries its pressure, and the depth the
    /// shader draws from that pressure is deeper until the ground's cap. Fixture masses and
    /// footprints, no vehicle: the law reads mass, width and length and nothing else.
    #[test]
    fn higher_ground_pressure_prints_deeper_on_the_same_ground() {
        let (light, heavy) = (
            ground_pressure_kpa(20_000.0, 0.5, 3.0),
            ground_pressure_kpa(40_000.0, 0.5, 3.0),
        );
        assert!(heavy > light, "{heavy} kPa against {light} kPa");
        assert!(
            ground_pressure_kpa(40_000.0, 1.0, 3.0) < heavy,
            "a wider track bears more lightly"
        );
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        let mut depths = PrintDepths {
            ground: crate::ground_field::GroundField::unpainted(&GroundTypes::load_shipped()),
            params: TerrainBlendParams::default(),
        };
        crate::terrain_blend::set_print_lanes(&mut depths.params, &table);
        // Silent mask: the base pack's ground everywhere.
        let (shallow, deep) = (
            depths.full_press_m(Vec2::ZERO, light),
            depths.full_press_m(Vec2::ZERO, heavy),
        );
        assert!(
            deep > shallow && shallow > 0.0,
            "base ground: {deep} m under {heavy} kPa, {shallow} m under {light} kPa"
        );
    }

    /// THE DRAWN SHOE RESTS ON THE PRINT IT MAKES. The drawn belt lowers its sole by
    /// [`PrintDepths::full_press_m`] (`track::view`); the shader draws each texel at the sole's
    /// sinkage less the shoe's clearance over its sole there (`terrain_blend.wgsl`'s `sunk_at`,
    /// mirrored by `terrain_blend::print_depth_m`). Decoded from what the store wrote under a full
    /// press, the sole's texels must draw exactly the sink, and the recess's the sink less its
    /// height, none where the sole does not sink that far, at every mix of grounds, hard included:
    /// a sink that read the table rather than the material's lanes, the pressure unquantised, or its
    /// own weights, floats the drawn shoe over its print or buries it in the ground.
    #[test]
    fn the_drawn_shoe_rests_on_the_floor_the_shader_draws() {
        let mut table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        // Fixture: the base pack's ground made hard, so the cases below span hard to yielding.
        table
            .types
            .get_mut(crate::terrain_blend::BASE_GROUND)
            .expect("the base pack's ground")
            .print
            .softness_mm_per_kpa = 0.0;
        let mut params = TerrainBlendParams {
            mask_map: Vec4::new(1.0 / 64.0, -1.0 / 64.0, 0.5, 0.5),
            print_law: law_params(),
            print_store: store_params(),
            ..default()
        };
        crate::terrain_blend::set_print_lanes(&mut params, &table);
        // A fixture mask, 4 × 4 texels over a 64 m map: every channel and mixes of them.
        let weights: Vec<Vec3> = (0..16)
            .map(|k| {
                let f = k as f32 / 15.0;
                Vec3::new(f * 0.3, (1.0 - f) * 0.4, if k % 3 == 0 { 0.5 } else { 0.0 })
            })
            .collect();
        let mixed = PrintDepths {
            ground: crate::ground_field::GroundField::fixture_mask(UVec2::splat(4), 64.0, weights),
            params: params.clone(),
        };
        // One ground over the whole map: the bare (hard) base pack, or one layer.
        let only = |mask: Vec3| PrintDepths {
            ground: crate::ground_field::GroundField::fixture_mask(UVec2::ONE, 64.0, vec![mask]),
            params: params.clone(),
        };
        let (hard, red, blue) = (only(Vec3::ZERO), only(Vec3::X), only(Vec3::Z));
        let shoes = vec![ClearanceStamp::pooled(&stamp_fixture(), NEAR_TEXEL_M)];
        let law = PrintLaw {
            contact_m: print_appearance().contact_m.value,
        };
        // Seen: hard ground, a sole that stops above the recess, one that sinks past it.
        let mut seen = [false; 3];
        for (k, (depths, kpa)) in [
            (&mixed, 37.3),
            (&mixed, 111.4),
            (&mixed, 180.0),
            (&hard, 111.4),
            (&red, 5.0),
            (&red, 111.4),
            (&blue, 250.0),
        ]
        .into_iter()
        .enumerate()
        {
            let at = Vec2::new(-20.0 + 13.0 * k as f32, 9.0 - 11.0 * k as f32);
            let mut stores = PrintStores::new(64.0, 64);
            stores.recentre(at);
            let entry = StampEntry::new(
                store::Placement {
                    datum: at,
                    yaw: 0.0,
                    lift: 0.0,
                    tilt: Vec2::ZERO,
                    shoe: 0,
                    mirrored: false,
                },
                1.0,
                kpa,
            );
            stores.stamp(&entry, &shoes, &law);
            let footprint: Vec<[u8; 2]> = stores
                .take_uploads()
                .into_iter()
                .filter(|u| u.store == Store::Near)
                .flat_map(|u| u.data.chunks(2).map(|t| [t[0], t[1]]).collect::<Vec<_>>())
                .filter(|texel| texel[1] > 0)
                .collect();
            let sole = *footprint.iter().max().expect("the stamp printed");
            let recess = *footprint.iter().min().expect("the stamp printed");
            assert_eq!(sole[0], 255, "the sole's own texels reach all the way");
            // The shader: pressure from G, clearance from R, on the painted weights.
            let weights = depths.ground_weights(at);
            let drawn = |texel: [u8; 2]| {
                crate::terrain_blend::print_depth_m(
                    &params,
                    weights,
                    f32::from(texel[1]) / 255.0 * params.print_law.x,
                    (1.0 - f32::from(texel[0]) / 255.0) * params.print_store.x,
                )
            };
            let sink = depths.full_press_m(at, kpa);
            assert!(
                (drawn(sole) - sink).abs() < 1e-6,
                "at {at}, {kpa} kPa: the sole sinks {sink} m, its print's floor is {} m",
                drawn(sole)
            );
            let under_recess = (sink - FIXTURE_RECESS_M).max(0.0);
            assert!(
                (drawn(recess) - under_recess).abs() <= 0.5 * CLEARANCE_UNIT_M + 1e-6,
                "at {at}, {kpa} kPa: the recess sinks {under_recess} m, its print's floor is {} m",
                drawn(recess)
            );
            seen[usize::from(sink > 0.0) + usize::from(sink > FIXTURE_RECESS_M)] = true;
        }
        assert_eq!(seen, [true; 3], "the cases span hard, shallow and deep");
    }

    /// Fixture: how far the recess of [`stamp_fixture`] rides above its sole, m.
    const FIXTURE_RECESS_M: f32 = 0.04;

    /// A shoe with a sole and a recess, fixture geometry: 0.6 m across and 0.3 m along, its sole
    /// over its front half and a recess [`FIXTURE_RECESS_M`] above it over its rear half, each
    /// longer than two near texels so the pooled stamp keeps both whole.
    fn stamp_fixture() -> stamp::ShoeStamp {
        let quad = |y: f32, z0: f32, z1: f32| {
            let c = |x: f32, z: f32| Vec3::new(x, y, z);
            [
                [c(-0.3, z0), c(0.3, z0), c(0.3, z1)],
                [c(-0.3, z0), c(0.3, z1), c(-0.3, z1)],
            ]
        };
        let triangles: Vec<[Vec3; 3]> = quad(FIXTURE_RECESS_M, -0.15, 0.0)
            .into_iter()
            .chain(quad(0.0, 0.0, 0.15))
            .collect();
        stamp::bake(&triangles).expect("covers")
    }
}
