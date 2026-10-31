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
//!    ([`crate::ground_types`]); a texel's depth is that ground's `softness × pressure`, capped,
//!    times the texel's pattern and press. The world layer darkens and roughens everywhere; inside
//!    the near store the depth bends the normal, and up to the player's set distance it is drawn
//!    with parallax and the sun's shadow ([`crate::settings::TrackPrintRelief`]).
//! 6. **The belt sink** ([`PrintDepths`]). The drawn belt lowers into its own rut by the depth the
//!    shader draws under a full press (`track::view`).
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
    KPA_PER_UNIT, NEAR_SIDE, NEAR_SPAN_M, NEAR_TEXEL_M, PrintLaw, PrintStores, StampEntry, Store,
    WORLD_SIDE,
};
use upload::{PrintWrites, TileWrite};

/// How far a sole's plane may clear the ground and still rest on it, m. ASSUMED: above the drawn
/// belt's filter error over the oracle. With it a coasting hull lays 1.01 touchdowns per pitch per
/// side (MEASURED, the track-print demo's mud run, 8.6 m of free rolling).
const CONTACT_M: f32 = 0.03;

/// The clearance over which a sole's pattern falls from full depth to none, m. ASSUMED: a 3 cm
/// grouser over its plate prints 20 % deeper than the plate, so the pattern reads inside every
/// print while the whole footprint sinks as one strip; at a 15 cm print the relief is the shoe's
/// own. Under it the fixture shoe's whole underside, 62 mm of relief (MEASURED, the bake's log),
/// still prints.
const PATTERN_RANGE_M: f32 = 0.15;

/// How far inside the near store's window edge its prints fade in, m. ASSUMED.
const NEAR_FADE_M: f32 = 4.0;

/// Screen pixels per near-store texel at which the near store's normal is gone, and at which it is
/// whole: below about a pixel a texel's gradient aliases. ASSUMED.
const NEAR_GONE_PX: f32 = 1.0;
const NEAR_FULL_PX: f32 = 2.0;

/// The share of the relief distance over which parallax fades to lighting only. ASSUMED: a fifth,
/// so the hand-over is a few metres wide at every distance setting.
const RELIEF_FADE_SHARE: f32 = 0.2;

/// Gravity for the ground pressure, m/s².
const G: f32 = 9.81;

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

/// One tank's stamping memory.
#[derive(Default)]
struct TankPrints {
    memory: ShoeMemory,
    /// The root's pose last frame: the teleport detector's previous frame.
    last: Option<Affine3A>,
    seen: bool,
}

/// What the depth a print reaches is read from, CPU-side: the material's print lanes as the shader
/// last received them, and the map's painted mask. The belt sink reads its depth here.
#[derive(Resource)]
pub(crate) struct PrintDepths {
    masks: Handle<Image>,
    /// The mask's weights, row-major RGB, once its image has loaded.
    mask: Option<(UVec2, Vec<Vec3>)>,
    /// The material's parameters as last written ([`sync_material`]).
    params: TerrainBlendParams,
}

impl PrintDepths {
    /// The mask's weights at world XZ `xz`, bilinear and clamped as the material's sampler reads
    /// them; silent before the mask loads.
    fn mask_at(&self, xz: Vec2) -> Vec3 {
        let Some((size, weights)) = &self.mask else {
            return Vec3::ZERO;
        };
        let map = self.params.mask_map;
        let uv = xz * map.truncate().truncate() + Vec2::new(map.z, map.w);
        let texel = uv * size.as_vec2() - 0.5;
        let low = texel.floor();
        let t = texel - low;
        let at = |i: f32, j: f32| {
            let i = (i as i32).clamp(0, size.x as i32 - 1) as u32;
            let j = (j as i32).clamp(0, size.y as i32 - 1) as u32;
            weights[(j * size.x + i) as usize]
        };
        let top = at(low.x, low.y).lerp(at(low.x + 1.0, low.y), t.x);
        let bottom = at(low.x, low.y + 1.0).lerp(at(low.x + 1.0, low.y + 1.0), t.x);
        top.lerp(bottom, t.y)
    }

    /// The depth (m) the shader draws at world XZ `xz` under a full press by a tank of ground
    /// pressure `kpa`: the deepest point of its print there. The pressure is read as the near store
    /// stores it, in whole [`KPA_PER_UNIT`], so this is the shader's own number.
    pub(crate) fn full_press_m(&self, xz: Vec2, kpa: f32) -> f32 {
        let stored = (kpa / KPA_PER_UNIT).round().clamp(1.0, 255.0) * KPA_PER_UNIT;
        crate::terrain_blend::print_depth_m(
            &self.params,
            crate::terrain_blend::print_weights(self.mask_at(xz)),
            stored,
        )
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
/// mask is `masks`, on the ground types of `ground`.
pub(crate) fn install(
    commands: &mut Commands,
    images: PrintImages,
    material: Handle<TerrainMaterial>,
    masks: Handle<Image>,
    world_size_m: f32,
    ground: GroundTypes,
) {
    let stores = PrintStores::new(world_size_m, WORLD_SIDE);
    let law = PrintLaw {
        pattern_range_m: PATTERN_RANGE_M,
        contact_m: CONTACT_M,
    };
    info!(
        "track prints: near store {NEAR_SIDE}² Rg8 over {NEAR_SPAN_M} m ({:.2} cm texels), world \
         layer {WORLD_SIDE}² R8 over {world_size_m} m ({:.1} cm texels, {} levels), {} ground types",
        NEAR_TEXEL_M * 100.0,
        world_size_m / WORLD_SIDE as f32 * 100.0,
        stores.world.levels(),
        ground.types.len(),
    );
    commands.insert_resource(ground);
    commands.insert_resource(PrintDepths {
        masks,
        mask: None,
        params: TerrainBlendParams::default(),
    });
    commands.insert_resource(TrackPrints {
        near_image: images.near,
        world_image: images.world,
        material,
        law,
        shoes: Vec::new(),
        stores,
        tanks: HashMap::new(),
        told: None,
    });
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
        NEAR_FADE_M,
        NEAR_GONE_PX,
        NEAR_FULL_PX,
    )
}

/// The material's `print_relief` lane for the player's `relief` and `range`: `x` the level, `y`
/// the distance (m) relief reaches, `z` the distance over which it fades to lighting only.
pub(crate) fn relief_params(relief: TrackPrintRelief, range: TrackPrintRange) -> Vec4 {
    let reach = range.metres();
    Vec4::new(
        f32::from(relief.level()),
        reach,
        reach * RELIEF_FADE_SHARE,
        0.0,
    )
}

/// A tank's ground pressure, kPa: its weight over the area its two tracks bear on, `m·g / (2·b·L)`
/// with `b` the shoe's width and `L` the length of track in contact. An average over the footprint
/// (no load distribution), which is all the print's depth law reads.
pub(crate) fn ground_pressure_kpa(mass_kg: f32, track_width_m: f32, contact_length_m: f32) -> f32 {
    mass_kg * G / (2.0 * track_width_m * contact_length_m).max(1e-3) / 1000.0
}

/// The ground pressure of the tank whose running gear is `geom`, kPa: its mass from its spec, its
/// shoe width measured off its model, its contact length the span of its road-wheel centres.
pub(crate) fn tank_ground_pressure_kpa(geom: &RigGeom) -> f32 {
    let span = |side: Side| {
        let wheels = geom.road_wheels.get(side);
        let (low, high) = wheels
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), w| {
                (lo.min(w.z), hi.max(w.z))
            });
        (high - low).max(0.0)
    };
    let length = 0.5 * (span(Side::Left) + span(Side::Right));
    ground_pressure_kpa(geom.model.mass, geom.model.width, length)
}

/// Bake the shoe once the template has bound and its mesh is loaded.
fn bake_shoes(
    mut prints: ResMut<TrackPrints>,
    template: Option<Res<LinkTemplate>>,
    geom: Option<Res<RigGeom>>,
    meshes: Res<Assets<Mesh>>,
) {
    let Some(template) = template else {
        return;
    };
    let Some(mesh) = meshes.get(template.shoe_mesh(Side::Right)) else {
        return;
    };
    let frame = template.frame(Side::Right);
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        error_once!("track prints: the shoe mesh has no Float32x3 positions — no prints");
        return;
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
        error_once!("track prints: the shoe's underside covers no cell — no prints");
        return;
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
fn stamp_shoes(
    mut prints: ResMut<TrackPrints>,
    grid: Res<HeightGrid>,
    template: Res<LinkTemplate>,
    geom: Res<RigGeom>,
    tanks: Query<(Entity, &GlobalTransform), With<Tank>>,
    belts: Query<(Entity, &ShoeBelt, &BeltFrames, &ChildOf, &GlobalTransform)>,
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
        tank.last = Some(now);
        tank.seen = true;
    }
    memory.retain(|_, tank| tank.seen);

    let ground = |xz: Vec2| {
        grid.contains_xz(xz.x, xz.y)
            .then(|| grid.height_at(xz.x, xz.y))
    };
    let mut laid = Vec::new();
    for (entity, belt, frames, parent, at) in &belts {
        let Some(tank) = memory.get_mut(&parent.parent()) else {
            continue;
        };
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
    for entry in laid {
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
    if stores.recentre(Vec2::new(eye.x, eye.z)) {
        debug!(
            "track prints: near window recentred at {:?} over {} logged stamps",
            stores.near.origin_m(),
            stores.log.len()
        );
    }
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

/// Copy the painted mask to the CPU for [`PrintDepths`], once its image has loaded.
fn read_mask(mut depths: ResMut<PrintDepths>, images: Res<Assets<Image>>) {
    if depths.mask.is_some() {
        return;
    }
    let Some(image) = images.get(&depths.masks) else {
        return;
    };
    let size = image.size();
    let mut weights = Vec::with_capacity((size.x * size.y) as usize);
    for j in 0..size.y {
        for i in 0..size.x {
            let Ok(color) = image.get_color_at(i, j) else {
                error_once!(
                    "track prints: the mask's {:?} texels are unreadable — the belt sinks into \
                     the base ground's depth",
                    image.texture_descriptor.format
                );
                depths.mask = Some((UVec2::ONE, vec![Vec3::ZERO]));
                return;
            };
            let linear = color.to_linear();
            weights.push(Vec3::new(linear.red, linear.green, linear.blue));
        }
    }
    depths.mask = Some((size, weights));
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
    app.add_plugins(upload::plugin);
    app.add_systems(
        Update,
        (
            bake_shoes.run_if(|prints: Option<Res<TrackPrints>>| {
                prints.is_some_and(|prints| prints.shoes.is_empty())
            }),
            read_mask.run_if(resource_exists::<PrintDepths>),
        ),
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
            masks: Handle::default(),
            mask: None,
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

    /// THE BELT SINK READS THE SAME DEPTH THE SHADER DRAWS. The shader's depth at a texel is the
    /// ground's `softness × pressure`, capped, blended by the painted weights, times the texel's
    /// press (`terrain_blend.wgsl`'s `print_depth`, mirrored by `terrain_blend::print_depth_m`);
    /// the pressure it reads is the byte the store wrote. A fully pressed grouser texel, decoded
    /// from what the store wrote, must give the sink's own number at every mix of grounds: a sink
    /// that read the table rather than the material's lanes, the pressure unquantised, or its own
    /// weights, drifts from the rut it sits in.
    #[test]
    fn the_belt_sink_reads_the_same_depth_the_shader_draws() {
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        let mut params = TerrainBlendParams {
            mask_map: Vec4::new(1.0 / 64.0, -1.0 / 64.0, 0.5, 0.5),
            print_law: law_params(),
            ..default()
        };
        crate::terrain_blend::set_print_lanes(&mut params, &table);
        // A fixture mask, 4 × 4 texels over a 64 m map: every channel and mixes of them.
        let weights: Vec<Vec3> = (0..16)
            .map(|k| {
                let f = k as f32 / 15.0;
                Vec3::new(f * 0.3, (1.0 - f) * 0.8, if k % 3 == 0 { 0.9 } else { 0.0 })
            })
            .collect();
        let depths = PrintDepths {
            masks: Handle::default(),
            mask: Some((UVec2::splat(4), weights)),
            params: params.clone(),
        };
        let shoes = vec![ClearanceStamp::pooled(&stamp_fixture(), NEAR_TEXEL_M)];
        for (k, kpa) in [37.3, 111.4, 180.0].into_iter().enumerate() {
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
            stores.stamp(
                &entry,
                &shoes,
                &PrintLaw {
                    pattern_range_m: PATTERN_RANGE_M,
                    contact_m: CONTACT_M,
                },
            );
            let texel = stores
                .take_uploads()
                .into_iter()
                .filter(|u| u.store == Store::Near)
                .flat_map(|u| u.data.chunks(2).map(|t| [t[0], t[1]]).collect::<Vec<_>>())
                .max()
                .expect("the stamp printed");
            assert_eq!(texel[0], 255, "the sole at full press");
            // The shader: pressure from G, the depth law on the painted weights, times R.
            let kpa_read = f32::from(texel[1]) / 255.0 * params.print_law.x;
            let drawn = crate::terrain_blend::print_depth_m(
                &params,
                crate::terrain_blend::print_weights(depths.mask_at(at)),
                kpa_read,
            ) * f32::from(texel[0])
                / 255.0;
            let sink = depths.full_press_m(at, kpa);
            assert!(
                (sink - drawn).abs() < 1e-6 && sink > 0.0,
                "at {at}, {kpa} kPa: the belt sinks {sink} m, the shader draws {drawn} m"
            );
        }
    }

    /// A flat sole, fixture geometry: 0.6 m across, 0.16 m along, its whole underside its datum,
    /// so every texel inside it prints at full press.
    fn stamp_fixture() -> stamp::ShoeStamp {
        let c = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        let triangles = vec![
            [c(-0.3, -0.08), c(0.3, -0.08), c(0.3, 0.08)],
            [c(-0.3, -0.08), c(0.3, 0.08), c(-0.3, 0.08)],
        ];
        stamp::bake(&triangles).expect("covers")
    }
}
