//! Track prints in the ground (issue 125): cosmetic terrain marks, the view of where every shoe of
//! every belt has rested. VIEW ONLY (ADR-0014): the ground's geometry, its collider, the oracle
//! and the simulation are untouched, no print crosses the wire, and only a windowed client builds
//! any of this. A late joiner misses the prints laid before it joined.
//!
//! Design, alternatives and the numbers they rest on: `.agents/scratch/impact-feedback-2026-10-28/`
//! (`track-prints-approaches.md` §4, `track-prints-ground-stamping.md` §1 and §4).
//!
//! # The pipeline
//!
//! 1. **The stamp** ([`stamp`]). Once the shoe template binds (`track::link_view`), the authored
//!    shoe's underside is baked at full detail into a height-and-coverage stamp in its canonical
//!    frame, then pooled to the near store's texel. The left shoe is the right's genuine mirror
//!    (`link_view::mirrored_mesh`), so it reads the same stamp mirrored.
//! 2. **The stamper** ([`stamper`], [`stamp_shoes`]). Every frame, after transform propagation,
//!    each shoe of every belt the conformed belt view drew is read at its drawn pose; a grounded
//!    one that has moved half a texel since its last stamp is stamped again. The view runs for
//!    every tank on every windowed client, so every tank prints.
//! 3. **The stores** ([`store`]). Each stamp joins the log and is drawn into the near store (if it
//!    lies in its window) and the world layer. The near store follows the eye ([`follow_eye`]).
//! 4. **The upload** ([`upload`]). Each frame's written tiles go to the two GPU images.
//! 5. **The shader** (`terrain_blend.wgsl`). Each ground pack answers a print its own way
//!    (`terrain_blend::PrintResponse`), through the blend weights it already holds: the world
//!    layer darkens and roughens everywhere, and inside the near store the depth builds a normal.
//!
//! # Entry points
//!
//! [`images`] and [`install`] are called where the ground's material is built
//! (`world::spawn_environment`), which is the windowed heightmap world only; [`plugin`] is mounted
//! by the track view (`track::view_plugin`), which only windowed clients compose. Every system is
//! inert until [`TrackPrints`] exists.

#[cfg(all(feature = "dev_tools", debug_assertions))]
pub(crate) mod demo;
mod stamp;
mod stamper;
mod store;
mod upload;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::math::Affine3A;
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::tank::Tank;
use crate::terrain_blend::TerrainMaterial;
use crate::terrain_grid::HeightGrid;
use crate::track::link_view::{LinkTemplate, ShoeBelt, TrackLink};
use crate::track::side::Side;
use crate::view::PlayerView;

use stamp::ClearanceStamp;
use stamper::ShoeMemory;
use store::{
    NEAR_SIDE, NEAR_SPAN_M, NEAR_TEXEL_M, NearStore, PrintLaw, StampEntry, StampLog, Store,
    WORLD_SIDE, WorldStore,
};
use upload::{PrintWrites, TileWrite};

/// How far a sole's plane may clear the ground and still rest on it, m. ASSUMED: above the drawn
/// belt's filter error over the oracle. With it a coasting hull lays 1.01 stamps per pitch per side
/// (MEASURED, the track-print demo's mud run, 8.6 m of free rolling).
const CONTACT_M: f32 = 0.03;

/// How far inside the near store's window edge its prints fade in, m. ASSUMED.
const NEAR_FADE_M: f32 = 4.0;

/// Screen pixels per near-store texel at which the near store's normal is gone, and at which it is
/// whole: below about a pixel a texel's gradient aliases. ASSUMED.
const NEAR_GONE_PX: f32 = 1.0;
const NEAR_FULL_PX: f32 = 2.0;

/// The prints' CPU half: the baked shoe, the log, both stores and each tank's shoe memory, plus
/// the GPU images and the material they are drawn through.
#[derive(Resource)]
pub(crate) struct TrackPrints {
    near_image: Handle<Image>,
    world_image: Handle<Image>,
    material: Handle<TerrainMaterial>,
    law: PrintLaw,
    /// The baked shoes, pooled to the near store's texel: empty until the template binds.
    shoes: Vec<ClearanceStamp>,
    log: StampLog,
    near: NearStore,
    world: WorldStore,
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

/// Make the two GPU images: the near store (`Rg8Unorm`, press and footprint) and the world layer
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

/// Start printing onto the map of side `world_size_m`, drawn through `material`.
pub(crate) fn install(
    commands: &mut Commands,
    images: PrintImages,
    material: Handle<TerrainMaterial>,
    world_size_m: f32,
) {
    let world = WorldStore::new(world_size_m, WORLD_SIDE);
    let law = PrintLaw {
        range_m: crate::terrain_blend::print_range_m(),
        contact_m: CONTACT_M,
    };
    info!(
        "track prints: near store {NEAR_SIDE}² Rg8 over {NEAR_SPAN_M} m ({:.2} cm texels), world \
         layer {WORLD_SIDE}² R8 over {world_size_m} m ({:.1} cm texels, {} levels), clearance \
         range {:.3} m",
        NEAR_TEXEL_M * 100.0,
        world_size_m / WORLD_SIDE as f32 * 100.0,
        world.levels(),
        law.range_m,
    );
    commands.insert_resource(TrackPrints {
        near_image: images.near,
        world_image: images.world,
        material,
        law,
        shoes: Vec::new(),
        log: StampLog::default(),
        near: NearStore::default(),
        world,
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

/// The material's `print_law` lane: `x` the clearance range the near store's press spans, `y` the
/// window fade, `zw` the pixels per texel at which the near normal is gone and whole.
pub(crate) fn law_params(range_m: f32) -> Vec4 {
    Vec4::new(range_m, NEAR_FADE_M, NEAR_GONE_PX, NEAR_FULL_PX)
}

/// Bake the shoe once the template has bound and its mesh is loaded.
fn bake_shoes(
    mut prints: ResMut<TrackPrints>,
    template: Option<Res<LinkTemplate>>,
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
         MEASURED",
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

/// Stamp every grounded shoe that moved, on every belt, at this frame's drawn pose.
fn stamp_shoes(
    mut prints: ResMut<TrackPrints>,
    grid: Res<HeightGrid>,
    template: Res<LinkTemplate>,
    tanks: Query<(Entity, &GlobalTransform), With<Tank>>,
    belts: Query<(&ShoeBelt, &ChildOf, &Children)>,
    links: Query<&GlobalTransform, With<TrackLink>>,
) {
    let TrackPrints {
        law,
        shoes,
        log,
        near,
        world,
        tanks: memory,
        ..
    } = &mut *prints;
    let Some(shoe) = shoes.first() else {
        return;
    };
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
    for (belt, parent, children) in &belts {
        let Some(tank) = memory.get_mut(&parent.parent()) else {
            continue;
        };
        let side = belt.side();
        let mirrored = side == Side::Left;
        let to_mesh = template.frame(side).canonical_to_mesh();
        let bounds = shoe.bounds(mirrored);
        for &link in children {
            let Ok(global) = links.get(link) else {
                continue;
            };
            let Some(contact) = stamper::contact(
                global.affine() * to_mesh,
                bounds,
                shoe.datum_y(),
                law.contact_m,
                ground,
            ) else {
                continue;
            };
            if tank.memory.observe(link, &contact) {
                laid.push(StampEntry::new(
                    contact.datum,
                    contact.yaw,
                    contact.lift,
                    contact.tilt,
                    0,
                    mirrored,
                ));
            }
        }
    }
    for entry in laid {
        log.push(entry, shoe);
        near.stamp(&entry, shoes, law);
        world.stamp(&entry, shoes, law);
    }
}

/// Keep the near store's window on the player's eye.
fn follow_eye(mut prints: ResMut<TrackPrints>, eye: Query<&GlobalTransform, With<PlayerView>>) {
    let Ok(eye) = eye.single() else {
        return;
    };
    let TrackPrints {
        law,
        shoes,
        log,
        near,
        ..
    } = &mut *prints;
    let eye = eye.translation();
    if near.recentre(Vec2::new(eye.x, eye.z), log, shoes, law) {
        debug!(
            "track prints: near window recentred at {:?} over {} logged stamps",
            near.origin_m(),
            log.len()
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
    let mut tiles = prints.near.take_uploads();
    tiles.extend(prints.world.take_uploads());
    writes.0 = tiles
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
    let origin = prints.near.origin_m();
    if origin != prints.told
        && let Some(origin) = origin
        && let Some(mut material) = materials.get_mut(&prints.material)
    {
        material.extension.params.print_near = near_params(origin);
        prints.told = Some(origin);
    }
}

/// Whether anything of the prints exists in `world`: their stores or their upload queue.
#[cfg(test)]
pub(crate) fn composed(world: &World) -> bool {
    world.contains_resource::<TrackPrints>() || world.contains_resource::<PrintWrites>()
}

/// The system set every prints system runs in, in `PostUpdate` after the drawn poses propagate.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TrackPrintsSet;

/// The prints' main-world systems and their render-world upload. Mounted by the track view.
pub(crate) fn plugin(app: &mut App) {
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
                .run_if(resource_exists::<LinkTemplate>),
            follow_eye,
            flush,
        )
            .chain()
            .in_set(TrackPrintsSet),
    );
}

#[cfg(test)]
mod tests {
    /// ADR-0034, TANKS ARE DATA: the prints' laws take every vehicle's geometry from its shoe
    /// template and nothing else, so no line of their code may name a vehicle. The names are read
    /// off the shipped tank files (`assets/<id>/<id>.tank.ron`, the id and its alphabetic stem), so
    /// a second tank joins the check by existing. Comments are prose and are skipped; a string or
    /// an identifier carrying a name — a branch on one, a per-tank constant — fails.
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
            root.join("assets/shaders/terrain_blend.wgsl"),
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
}
