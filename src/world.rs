//! Battlefield construction and ground queries shared by aiming and the camera.
//! Best practice: simulation comes from map root data; client sky construction belongs to `sky`.
//! See `.agents/docs/design/map-surroundings.md` for map asset ownership.

use avian3d::prelude::{
    Collider, CollisionLayers, LayerMask, RigidBody, SpatialQuery, SpatialQueryFilter,
};
use bevy::prelude::*;

use crate::Layer;

/// The world's construction generation. Streamed environment pieces belong to exactly this
/// generation; consumers must not resolve an older piece against a newly constructed map.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorldGeneration(pub(crate) u32);

/// The generation selected for local construction. `None` before a client receives its authority
/// Battle or between links. Offline presentation starts at the initial generation; authority
/// publication carries its generation separately in a nonempty type.
#[derive(Resource)]
pub(crate) struct CurrentWorld(pub(crate) Option<WorldGeneration>);

impl Default for CurrentWorld {
    fn default() -> Self {
        Self(Some(WorldGeneration::default()))
    }
}

impl Default for WorldGeneration {
    fn default() -> Self {
        Self(1)
    }
}

/// Replace mutable environment entities with the startup construction. Terrain, its immutable
/// height samples and field, lighting and shared asset handles survive. Map roots and released
/// construction bodies own disjoint entity subtrees.
pub(crate) fn reconstruct(world: &mut World) {
    let roots: Vec<Entity> = world
        .query_filtered::<Entity, Or<(
            With<crate::scatter::MapInstance>,
            With<crate::debris::DebrisPiece>,
        )>>()
        .iter(world)
        .collect();
    for root in roots {
        assert!(
            world.despawn(root),
            "environment roots have disjoint ownership"
        );
    }
    if let Some(mut record) = world.get_resource_mut::<crate::construction::breakage::Record>() {
        *record = crate::construction::breakage::Record {
            generation: record.generation.wrapping_add(1),
            ..default()
        };
    }
    world
        .run_system_cached(crate::scatter::reconstruct)
        .expect("the startup scatter recipe remains available for world construction");
    crate::track_prints::reconstruct(world);
    crate::vfx::reconstruct_world(world);
    crate::debris::decor::reconstruct(world);
}

/// The world's static terrain as DATA (track architecture §5): every terrain block in authoring
/// form — a unit cube posed/scaled by its Transform (the Avian collider idiom). Colliders are
/// spawned FROM this list, and the track module's analytic `BlockField` is built from the SAME
/// list, so the two representations cannot drift. `revision` bumps whenever the set changes
/// (map load; future streaming/destruction) so consumers know to rebuild and reseed.
#[derive(Resource)]
pub struct TerrainMap {
    pub revision: u64,
    pub blocks: Vec<Transform>,
}

/// What stands on the map without being terrain: each construction's bounds in the [`TerrainMap`]
/// block encoding (a unit cube posed and scaled; `crate::construction::footprint`). Never part of the
/// track field: belts read its colliders. Spawn avoidance refuses a spawn inside one exactly as inside
/// a standing block (`net::server`).
#[derive(Resource, Default)]
pub struct SpawnObstacles(pub Vec<Transform>);

/// Side length of the (square) ground plane, in metres — the flat fixture world's extent
/// (`terrain_grid::FIXTURE_EXTENT`), which is what the spawn map's UV mapping and the authority's
/// spawn clamp resolve against when no heightmap decoded.
const GROUND_SIZE: f32 = crate::terrain_grid::FIXTURE_EXTENT.world_size_m;
/// Resolution: a nonzero buried thickness keeps the test-course ground a closed collision solid.
const GROUND_THICKNESS: f32 = 1.0;

pub fn plugin(app: &mut App) {
    // Decode the heightmap FIRST (synchronous, ADR-0014), then spawn whichever world it selects:
    // the heightfield when the grid decoded, the flat slab + authored course otherwise.
    app.add_systems(
        Startup,
        (crate::terrain_grid::decode_height_grid, spawn_environment).chain(),
    );
    app.add_systems(Update, report_failed_terrain_map);
    app.add_plugins(crate::sky::plugin);
    // The ground material `spawn_environment` builds above. Mounted here rather than in a client
    // composition because `spawn_environment` writes its asset collection on every composition:
    // the plugin's render half is behind the render app, which the dedicated server does not have.
    app.add_plugins(crate::terrain_blend::plugin);
    // The scatter constructions' material, which `spawn_environment` takes for the same reason.
    app.add_plugins(crate::construction_surface::plugin);
    // The terrain LOD ladder's adaptive half: the levels are generated in `spawn_environment`
    // above, and this keeps their switch distances honest as the optic toggles and the window
    // resizes. Inert until a ladder exists, so the dedicated server pays nothing.
    app.add_plugins(crate::terrain_lod::plugin);
}

/// How a surface map's bytes (a terrain or construction pack's, or a mask's) must be interpreted at LOAD
/// time — the one texture decision that cannot be corrected downstream (the sampler hands the
/// shader whatever the GPU format says). The diffuse carries COLOUR and is authored in sRGB; the
/// normal and ARM maps carry DATA — a tangent-space direction, and three material scalars — and an
/// sRGB transfer applied to those bends every direction and every roughness value.
///
/// Still ours to state even though the maps are KTX2: bevy does NOT read the container's transfer
/// function for a UASTC payload — `is_srgb` is what picks the sRGB variant of the transcode target
/// (`Astc { channel: UnormSrgb }` / `Bc7RgbaUnormSrgb` vs the plain `Unorm` forms).
#[derive(Clone, Copy)]
pub(crate) enum MapEncoding {
    Srgb,
    Linear,
}

/// Resolution: eight taps bound sampling cost while filtering surfaces at grazing angles.
/// Mip chains are required: anisotropy chooses among mip levels rather than adding detail to a
/// single-level texture.
const SURFACE_ANISOTROPY: u16 = 8;

/// How a surface image is addressed outside `0..1`. A surface pack TILES — bevy's default clamps,
/// which would smear the first [`crate::terrain_grid::TEXTURE_TILE_M`] tile across the whole map.
/// A weight mask is STRETCHED over the world square exactly once, so it must clamp instead: with
/// repeat addressing its opposite edges would bleed into each other at the map's border.
#[derive(Clone, Copy)]
pub(crate) enum MapTiling {
    Repeat,
    Stretched,
}

/// The sampler a surface image is read through: [`MapTiling`] addressing, trilinear filtering
/// across the mip chain, and [`SURFACE_ANISOTROPY`] taps.
pub(crate) fn surface_sampler(tiling: MapTiling) -> bevy::image::ImageSamplerDescriptor {
    use bevy::image::{ImageAddressMode, ImageSamplerDescriptor};
    let address_mode = match tiling {
        MapTiling::Repeat => ImageAddressMode::Repeat,
        MapTiling::Stretched => ImageAddressMode::ClampToEdge,
    };
    let mut sampler = ImageSamplerDescriptor {
        address_mode_u: address_mode,
        address_mode_v: address_mode,
        ..ImageSamplerDescriptor::default()
    };
    // Sets the three filters to Linear as well — wgpu REQUIRES that for anisotropy > 1.
    sampler.set_anisotropic_filter(SURFACE_ANISOTROPY);
    sampler
}

/// Load one surface image (a terrain or construction pack's map, or a mask) with the sampler its role
/// needs ([`surface_sampler`]). Async load is fine — this is pure view (ADR-0014), nothing sim-side
/// reads it — but a FAILED load is fatal, so every caller holds its maps' ids for a reporter
/// ([`report_failed_terrain_map`], `construction_surface`'s). The sampler is orthogonal to the
/// container: the same descriptor rides the KTX2 maps exactly as it rode the PNGs.
pub(crate) fn surface_map(
    asset_server: &AssetServer,
    path: &str,
    encoding: MapEncoding,
    tiling: MapTiling,
) -> Handle<Image> {
    use bevy::image::{ImageLoaderSettings, ImageSampler};
    let is_srgb = matches!(encoding, MapEncoding::Srgb);
    let sampler = surface_sampler(tiling);
    asset_server
        .load_builder()
        .with_settings(move |settings: &mut ImageLoaderSettings| {
            settings.is_srgb = is_srgb;
            settings.sampler = ImageSampler::Descriptor(sampler.clone());
        })
        .load(path.to_owned())
}

/// GPU view of the one mask decode. File-row order matches the shader's existing mapping.
fn ground_mask_image(
    field: &crate::ground_field::GroundField,
    rows: crate::terrain_grid::RowOrder,
) -> Image {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::ImageSampler;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let (size, rgba) = field.mask_rgba(rows);
    let mut image = Image::new(
        Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(surface_sampler(MapTiling::Stretched));
    image
}

/// The terrain images whose load must succeed, held by id for [`report_failed_terrain_map`]: the
/// base pack's three maps and the three blend-layer arrays. The weight mask is uploaded from
/// the synchronous ground decode. Only inserted
/// by the heightmap world with a window — the flat slab / authored course and the dedicated server
/// never load them, so they can never fail there.
#[derive(Resource)]
struct TerrainMaps([AssetId<Image>; 6]);

/// Surface a failed terrain-map load instead of swallowing it (ADR-0011, the same stance as
/// `spec::report_failed_spec`). A texture that fails to decode does NOT draw untextured: bevy
/// leaves the material unprepared, so the whole ground silently stops rendering — the exact
/// failure the `jpeg` feature note in `Cargo.toml` records us hitting once already. Required,
/// in-repo asset ⇒ panic in every build.
fn report_failed_terrain_map(
    mut failures: MessageReader<bevy::asset::AssetLoadFailedEvent<Image>>,
    required: Option<Res<TerrainMaps>>,
) {
    let Some(required) = required else {
        return;
    };
    for failure in failures.read() {
        if required.0.contains(&failure.id) {
            let (path, err) = (&failure.path, &failure.error);
            error!("required terrain map {path} failed to load: {err}");
            panic!("required terrain map {path} failed to load: {err}");
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "spawns the environment: six asset stores, terrain, map, window, view profile, loader \
              and the ablation lever"
)]
fn spawn_environment(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut terrain_materials: ResMut<Assets<crate::terrain_blend::TerrainMaterial>>,
    // The scatter constructions' one shared material (`construction_surface`).
    mut construction_materials: ResMut<Assets<crate::construction_surface::ConstructionMaterial>>,
    // Its hole tables' store, which the shared material binds an empty one from: the render
    // plugin's, so absent where nothing draws.
    mut buffers: Option<ResMut<Assets<bevy::render::storage::ShaderBuffer>>>,
    // The track prints' two stores, which the ground's material reads (`track_prints`).
    mut images: ResMut<Assets<Image>>,
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
    ground_field: Option<Res<crate::ground_field::GroundField>>,
    // The one parse of the map's manifest (`terrain_grid::decode_height_grid` publishes it), which
    // the scatter's placement is read out of.
    manifest: Option<Res<crate::map::MapManifest>>,
    windows: Query<&Window>,
    // The tank ladder's own profile, or NONE on the frames before the declared view has been read
    // — which at Startup is always, so the terrain ladder spawns at its finest level and
    // `terrain_lod::adapt_ranges` writes the real thresholds on the first frame with facts.
    view: Option<Res<crate::view::ViewProfile>>,
    asset_server: Res<AssetServer>,
    ablate: Option<Res<crate::render_cost::Ablate>>,
) {
    let mut blocks: Vec<Transform> = Vec::new();
    if !windows.is_empty() {
        crate::sky::spawn_sun(&mut commands);
    }

    // The heightmap world: when the grid decoded, the heightfield IS the world — no flat slab,
    // no authored test course. The oracle's ground term comes from the grid; `TerrainMap` carries
    // only what stands ON it (the scatter's constructions, below), on the same revision semantics
    // `TrackField` rebuilds from.
    if let Some(grid) = grid {
        // The view layer's parry cap must reach across the map the grid actually is (ADR-0011): a
        // world wider than [`VIEW_CAST_MAX_M`]'s diagonal clips aim and camera picks at the cap
        // instead of at the ground, and the clip is invisible — the miss fallback looks like sky.
        assert!(
            VIEW_CAST_MAX_M >= grid.world_size() * std::f32::consts::SQRT_2,
            "VIEW_CAST_MAX_M ({VIEW_CAST_MAX_M} m) must cover the {} m world's diagonal",
            grid.world_size(),
        );
        commands.spawn((
            Transform::IDENTITY,
            RigidBody::Static,
            crate::terrain_grid::heightfield_collider(&grid),
            crate::track::terrain::FieldSurface,
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
            crate::ground_types::GroundTypes::load_shipped().terrain_friction(),
        ));
        // The render mesh is view-only: windowed compositions have a primary window; the
        // dedicated server (and the headless harness) has none and must not pay for it.
        if !windows.is_empty() {
            // The map's manifest is what the mask block and the world extent are read out of, so
            // the blend cannot be built without it. Absent ⇒ no grid either, which is the branch
            // above; present-and-unreadable already panicked in `map::parse`.
            let manifest = manifest
                .as_deref()
                .expect("a decoded height grid comes from a parsed manifest");
            // The BASE ground surface pack (Poly Haven, CC0 — see the pack folder's `cc.txt`),
            // world-space tiled by the mesh's UVs every `terrain_grid::TEXTURE_TILE_M`.
            let diffuse = surface_map(
                &asset_server,
                crate::terrain_grid::TEXTURE_PATH,
                MapEncoding::Srgb,
                MapTiling::Repeat,
            );
            let normal = surface_map(
                &asset_server,
                crate::terrain_grid::NORMAL_PATH,
                MapEncoding::Linear,
                MapTiling::Repeat,
            );
            let arm = surface_map(
                &asset_server,
                crate::terrain_grid::ARM_PATH,
                MapEncoding::Linear,
                MapTiling::Repeat,
            );
            // The three BLENDED packs, one 2D array per map type, plus the map's own weight mask
            // (`terrain_blend` — layer order is the mask's channel order).
            let layer_albedo = surface_map(
                &asset_server,
                crate::terrain_blend::LAYER_TEXTURE_PATH,
                MapEncoding::Srgb,
                MapTiling::Repeat,
            );
            let layer_normal = surface_map(
                &asset_server,
                crate::terrain_blend::LAYER_NORMAL_PATH,
                MapEncoding::Linear,
                MapTiling::Repeat,
            );
            let layer_arm = surface_map(
                &asset_server,
                crate::terrain_blend::LAYER_ARM_PATH,
                MapEncoding::Linear,
                MapTiling::Repeat,
            );
            let field = ground_field
                .as_ref()
                .expect("heightmap rendering requires the ground field decoded with its map");
            let masks = images.add(ground_mask_image(field, manifest.rows));
            commands.insert_resource(TerrainMaps([
                diffuse.id(),
                normal.id(),
                arm.id(),
                layer_albedo.id(),
                layer_normal.id(),
                layer_arm.id(),
            ]));
            // The ground-type table every pack's print response is read from; the prints keep it
            // as a resource, the one the owner tunes live (`track_prints::tuning`).
            let ground = crate::ground_types::GroundTypes::load_shipped();
            let prints = crate::track_prints::images(&mut images);
            let material = terrain_materials.add(crate::terrain_blend::TerrainMaterial {
                base: StandardMaterial {
                    base_color_texture: Some(diffuse),
                    // `nor_gl` is the OpenGL convention (+Y green points UP in tangent space) —
                    // what bevy and glTF expect, so NO flip. The pack's `nor_dx` sibling would
                    // need this `true`; picking the wrong one inverts every bump into a dent under
                    // a low sun, which is why the file name carries the convention.
                    normal_map_texture: Some(normal),
                    // glTF ORM packing — R = ambient occlusion, G = roughness, B = metallic. Poly
                    // Haven's `arm` IS that layout, so ONE image feeds both slots bevy reads it
                    // through (occlusion takes R, metallic-roughness takes G/B).
                    occlusion_texture: Some(arm.clone()),
                    metallic_roughness_texture: Some(arm),
                    // The shader MULTIPLIES these factors into the map (`perceptual_roughness *=
                    // mr.g`, `metallic *= mr.b` — bevy_pbr's `pbr_fragment.wgsl`), so 1.0 passes
                    // the roughness map through unscaled (the old 0.95 would have darkened every
                    // value). Metallic stays 0.0 rather than 1.0: the pack's B channel is all-zero
                    // (checked on import), so the render is identical either way, and hard-zero
                    // keeps a dielectric ground from turning to metal on a pack swap.
                    perceptual_roughness: 1.0,
                    metallic: 0.0,
                    ..default()
                },
                extension: crate::terrain_blend::TerrainBlend {
                    ablate: ablate
                        .as_deref()
                        .map(|ablate| ablate.terrain_ablate())
                        .unwrap_or_default(),
                    params: crate::terrain_blend::params(manifest, &ground),
                    layer_albedo,
                    layer_normal,
                    layer_arm,
                    masks,
                    prints_near: prints.near.clone(),
                    prints_world: prints.world.clone(),
                },
            });
            // The world layer spans the square the material's `print_world` lane maps it over.
            crate::track_prints::install(
                &mut commands,
                prints,
                material.clone(),
                field.as_ref().clone(),
                manifest.extent.world_size_m,
                ground,
            );
            info!(
                "terrain surface: base pack every {} m, masks {} at {:?}, blend layers {}",
                crate::terrain_grid::TEXTURE_TILE_M,
                manifest.masks_asset(),
                manifest.masks_resolution,
                crate::terrain_blend::layer_census(),
            );
            // The ONE-SURFACE invariant (terrain_grid module doc): the drawn ground is built
            // from the grid's own samples with the collider's own cell diagonal — identical
            // geometry, chunked into world-space tiles (positions are absolute, transforms
            // identity) purely so bevy frustum-culls per tile.
            //
            // Each tile ships as a LADDER rather than a single mesh (`terrain_lod`): the exact
            // surface up close, then RTIN levels whose declared deviation is sub-pixel at the
            // distance bevy switches them in. Generated HERE, from the in-memory grid, so there is
            // no build product that can go stale against the surface the sim reads. Tangent
            // generation (ADR-0011, required with no fallback) happens per level inside `spawn`.
            //
            // The same heightfield supplies the baked sun shadow to every receiver. Terrain
            // geometry does not also cast into the cascades.
            crate::terrain_shadow::install(
                &mut commands,
                &mut images,
                &grid,
                crate::sky::toward_sun(),
            );
            crate::terrain_lod::spawn(
                &mut commands,
                &mut meshes,
                &material,
                &grid,
                view.as_deref().copied(),
                crate::render_policy::VisualScope::BAKED_CASTER,
            );
        }
        // The map's object scatter (`scatter`): proxies and bodies posed from the shipped level file
        // onto THIS grid, colliders on every composition and meshes only where there is a window.
        // Construction bounds supply spawn obstacles; belts read their solid colliders.
        // The showcase world excludes scenery in front of the item being inspected.
        let mut obstacles = Vec::new();
        if let Some(manifest) = manifest.filter(|_| !crate::lod_showcase::enabled()) {
            obstacles = crate::scatter::spawn(
                &mut commands,
                &mut meshes,
                &manifest,
                &grid,
                buffers
                    .as_deref_mut()
                    .filter(|_| !windows.is_empty())
                    .map(|buffers| crate::scatter::ViewAssets {
                        server: &asset_server,
                        construction_materials: &mut construction_materials,
                        buffers,
                    }),
            );
        }
        commands.insert_resource(TerrainMap {
            revision: 0,
            blocks,
        });
        commands.insert_resource(SpawnObstacles(obstacles));
        return;
    }

    // The ground: a static slab whose top face sits at y=0 — the same plane the analytic
    // `ground_distance` assumes, so aim/camera are unaffected.
    spawn_block(
        &mut commands,
        &mut blocks,
        meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        materials.add(Color::srgb(0.32, 0.42, 0.28)),
        Transform::from_xyz(0.0, -GROUND_THICKNESS / 2.0, 0.0).with_scale(Vec3::new(
            GROUND_SIZE,
            GROUND_THICKNESS,
            GROUND_SIZE,
        )),
    );

    // The locomotion test course — deliberate, known geometry (not a scenic map) laid out down
    // the −Z lane in front of spawn, so each obstacle isolates one track behaviour and you
    // can tell the *sim* from the *terrain*. All on the Terrain layer, so the wheel rays read it
    // identically to the ground.
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let ramp_mat = materials.add(Color::srgb(0.45, 0.38, 0.28));
    let bump_mat = materials.add(Color::srgb(0.40, 0.33, 0.24));
    spawn_test_course(&mut commands, &mut blocks, &cube, &ramp_mat, &bump_mat);
    commands.insert_resource(TerrainMap {
        revision: 0,
        blocks,
    });
    // The course stands no constructions: every block is terrain.
    commands.insert_resource(SpawnObstacles::default());
}

/// Spawn a static, unit-cube collision block scaled/posed by `transform` (the Avian idiom: a
/// `Collider::cuboid(1,1,1)` that the Transform's scale stretches), on the Terrain layer — and
/// record it in the [`TerrainMap`] block list (the single terrain data source). A block is
/// terrain, so it carries the terrain's friction ([`crate::ground_types::GroundTypes::terrain_friction`]).
fn spawn_block(
    commands: &mut Commands,
    blocks: &mut Vec<Transform>,
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    transform: Transform,
) {
    blocks.push(transform);
    commands.spawn((
        Mesh3d(mesh),
        crate::render_cost::CasterClass::Terrain,
        MeshMaterial3d(material),
        transform,
        RigidBody::Static,
        Collider::cuboid(1.0, 1.0, 1.0),
        crate::track::terrain::FieldSurface,
        CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
        crate::ground_types::GroundTypes::load_shipped().terrain_friction(),
    ));
}

/// The four-obstacle locomotion course. Each obstacle is a static cuboid (or row of them) sized
/// to isolate one thing the belt contact model does. Reuses one unit-cube mesh and two
/// materials, cloned per block.
fn spawn_test_course(
    commands: &mut Commands,
    blocks: &mut Vec<Transform>,
    cube: &Handle<Mesh>,
    ramp_mat: &Handle<StandardMaterial>,
    bump_mat: &Handle<StandardMaterial>,
) {
    // 1. Graduated climbs — ramps at 10°/20°/30°, side by side, to compare pitch and find the
    //    climb limit. (With ~200 kN total thrust vs ~456 kN weight, 20° climbs but 30° stalls —
    //    gravity-along-slope exceeds thrust — so this also shows where it gives out.) Each is a
    //    slab tilted about X and sunk so its low edge's top sits ~1 m under the ground slab: the
    //    upslope crosses y=0 flush (step-free entry), the high edge a crest with a drop beyond.
    //    Low-edge top y = center_y + (thickness/2)·cosθ − (run/2)·sinθ; solve for center_y at −1 m.
    let (run, width, thick) = (10.0_f32, 10.0_f32, 2.0_f32);
    for (i, deg) in [10.0_f32, 20.0, 30.0].into_iter().enumerate() {
        let (sin, cos) = deg.to_radians().sin_cos();
        let center_y = -1.0 - (thick / 2.0) * cos + (run / 2.0) * sin;
        let x = (i as f32 - 1.0) * 14.0; // −14, 0, +14
        spawn_block(
            commands,
            blocks,
            cube.clone(),
            ramp_mat.clone(),
            Transform::from_xyz(x, center_y, -40.0)
                .with_rotation(Quat::from_rotation_x(deg.to_radians()))
                .with_scale(Vec3::new(width, thick, run)),
        );
    }

    // 2. Side-slope — a banked lane tilted about Z, driven ALONG Z so the tank is canted sideways:
    //    shows roll, lateral weight transfer, and whether it holds the face or slides off. Centred
    //    at y=0 so the banked top crosses ground near the lane centre (a roughly flush approach).
    spawn_block(
        commands,
        blocks,
        cube.clone(),
        ramp_mat.clone(),
        Transform::from_xyz(38.0, 0.0, -45.0)
            .with_rotation(Quat::from_rotation_z(18.0_f32.to_radians()))
            .with_scale(Vec3::new(16.0, 2.0, 26.0)),
    );

    // 3. Step / curb — a low box driven over: front wheels lift over the hard edge, then the rear.
    //    Single-wheel articulation against a vertical edge (top at y=0.4).
    spawn_block(
        commands,
        blocks,
        cube.clone(),
        bump_mat.clone(),
        Transform::from_xyz(0.0, 0.2, -70.0).with_scale(Vec3::new(14.0, 0.4, 4.0)),
    );

    // 4. Washboard — a row of low bumps; wheels rise and fall independently while the hull stays
    //    composed (the most legible "terrain following works" demo). Boxes approximate rounded bumps
    //    — a round profile is a later refinement.
    for i in 0..6 {
        let z = -82.0 - i as f32 * 1.6;
        spawn_block(
            commands,
            blocks,
            cube.clone(),
            bump_mat.clone(),
            Transform::from_xyz(0.0, 0.12, z).with_scale(Vec3::new(14.0, 0.25, 0.6)),
        );
    }
}

/// Resolution: cap view-only Parry traversal above the map's full horizontal diagonal.
/// `spawn_environment` checks this against the decoded extent. `aim::MAX_RANGE` separately
/// supplies the sky point when a ray meets no surface; simulation does not read this cap.
pub(crate) const VIEW_CAST_MAX_M: f32 = 2_500.0;

/// Distance along `ray` to the terrain, capped at `max`, falling back to `max` when the ray
/// misses (sky / above the horizon). A world raycast against the `Terrain` layer ONLY — the orbit
/// camera's ground pull-in, which must ignore tanks (a tank crossing behind the player must not
/// yank the camera in). Aim rays use `aim::aim_distance` instead, which adds the `Armor` layer so
/// the aim dots predict what a shell would actually meet, tanks included. The CAST is clamped to
/// [`VIEW_CAST_MAX_M`] (nothing exists beyond the world diagonal); the miss fallback stays `max`.
pub fn ground_distance(spatial: &SpatialQuery, ray: Ray3d, max: f32) -> f32 {
    spatial
        .cast_ray(
            ray.origin,
            ray.direction,
            max.min(VIEW_CAST_MAX_M),
            true,
            &SpatialQueryFilter::from_mask(Layer::Terrain),
        )
        .map(|hit| hit.distance)
        .unwrap_or(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dedicated server shares the world installer but must construct no client sky.
    /// Clients with a window still require the sun.
    #[test]
    fn the_sun_is_constructed_only_for_windowed_worlds() {
        for windowed in [false, true] {
            let mut app = crate::headless_test::headless_shell();
            app.insert_resource(crate::terrain_grid::ForceFlatWorld)
                .add_plugins(plugin);
            if windowed {
                app.world_mut().spawn(Window {
                    visible: false,
                    ..default()
                });
            }
            crate::headless_test::finish_plugins(&mut app);
            app.world_mut().run_schedule(Startup);
            let world = app.world_mut();
            assert_eq!(
                world.query::<&DirectionalLight>().iter(world).count(),
                usize::from(windowed),
            );
        }
    }

    /// Mount the production world installer with a component-only Window and no winit or
    /// GPU. The rendered material and PrintDepths must receive the exact simulation mask.
    /// FIXTURE: a 2x2 asymmetric mask over a small flat grid, without map instances.
    #[test]
    fn world_installation_shares_the_simulation_mask_with_material_and_prints() {
        let mut app = crate::headless_test::headless_shell();
        let mut manifest = crate::map::load(&crate::assets::asset_root());
        manifest.instances.clear();
        manifest.extent.world_size_m = 64.0;
        let rows = manifest.rows;
        let grid = crate::terrain_grid::HeightGrid::new(vec![0.0; 81].into(), 9, manifest.extent);
        let field = crate::ground_field::GroundField::fixture_mask(
            UVec2::splat(2),
            64.0,
            vec![Vec3::X, Vec3::Y, Vec3::Z, Vec3::ZERO],
        );
        let expected = field.clone();
        app.insert_resource(manifest)
            .insert_resource(grid)
            .add_plugins(plugin)
            .add_systems(
                Startup,
                (move |mut commands: Commands| {
                    commands.insert_resource(field.clone());
                })
                .after(crate::terrain_grid::decode_height_grid)
                .before(spawn_environment),
            );
        app.world_mut().spawn(Window {
            visible: false,
            ..default()
        });
        crate::headless_test::finish_plugins(&mut app);
        app.world_mut().run_schedule(Startup);
        let materials = app
            .world()
            .resource::<Assets<crate::terrain_blend::TerrainMaterial>>();
        let (_, material) = materials
            .iter()
            .next()
            .expect("the world installer builds a terrain material");
        let images = app.world().resource::<Assets<Image>>();
        let image = images
            .get(&material.extension.masks)
            .expect("the material's mask image is installed");
        let (size, bytes) = expected.mask_rgba(rows);
        assert_eq!(image.texture_descriptor.size.width, size.x);
        assert_eq!(image.texture_descriptor.size.height, size.y);
        assert_eq!(image.data.as_ref().unwrap(), &bytes);
        let depths = app.world().resource::<crate::track_prints::PrintDepths>();
        for at in [
            Vec2::new(-16.0, -16.0),
            Vec2::new(16.0, -16.0),
            Vec2::new(-16.0, 16.0),
            Vec2::new(16.0, 16.0),
        ] {
            assert_eq!(depths.ground_weights(at), expected.weights_at(at));
        }
    }
}
