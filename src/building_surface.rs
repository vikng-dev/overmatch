//! Building surfaces, view only: one shared two-lane material. `materials/visuals.ron`
//! declares both palette lanes and their physical repeats; each substance names its visual.
//! Meshes always carry their lane in UV1.x and their hole tag in UV1.y. Graybox face membership
//! and top tangent are prototype data; destruction elements carry an authored surface frame.
//! The material's vertex stage converts face units to metres under the instance transform.
//! A holed building wears its own alpha-masked copy with its hole table bound.
//!
//! Face UVs are `(p dot t, -p dot (n cross t))`, one face unit per cube edge on a graybox,
//! asset metres on an element mesh. The vertex stage multiplies those lanes by the world
//! lengths of the transformed tangent and bitangent, respectively. Each pack's sourced
//! metres per repeat then sets texture scale; anisotropic repeated sampling and OpenGL
//! normal maps use that same frame. The shared material is opaque; a holed copy enables
//! the discard path in both the main pass and the prepass used for shadows.

use bevy::asset::AssetLoadFailedEvent;
use bevy::math::Affine2;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;

use crate::world::{MapEncoding, MapTiling, surface_map};

/// The buildings' material: bevy's `StandardMaterial` carrying the primary palette lane pack and every lighting
/// parameter, extended with [`BuildingSurface`] (the secondary palette lane pack).
pub(crate) type BuildingMaterial = ExtendedMaterial<StandardMaterial, BuildingSurface>;

/// The vertex stage (face units to metres) and the fragment (the roof swap, a hole), one file.
const SHADER: &str = "shaders/building_surface.wgsl";

/// The prepass fragment, which bevy runs only for a [`holed`] material: a hole's discard.
const PREPASS_SHADER: &str = "shaders/building_surface_prepass.wgsl";

/// The two lanes the building shader binds; visuals.ron names their current maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub(crate) enum SurfaceLane {
    Primary,
    Secondary,
}
impl SurfaceLane {
    fn index(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Secondary => 1,
        }
    }
}

/// A declared maps visual and its physical repeat.
struct SurfacePack {
    visual: String,
    maps: String,
    authored_m: f32,
}
impl SurfacePack {
    fn path(&self, map: &str) -> String {
        format!("{}_{map}.ktx2", self.maps)
    }
    fn uv_per_m(&self) -> f32 {
        1.0 / self.authored_m
    }
}
fn palette_of(text: &str) -> Result<[SurfacePack; 2], String> {
    let library = crate::tank_surface::VisualLibrary::parse(text)?;
    let mut lanes = [None, None];
    for (key, visual) in library.iter() {
        if let crate::tank_surface::Visual::Maps {
            maps,
            metres_per_repeat,
            building_lane: Some(lane),
            ..
        } = visual
        {
            let slot = &mut lanes[lane.index()];
            if slot.is_some() {
                return Err(format!(
                    "building_lane {lane:?} is repeated at visual {key:?}; assign each building lane once"
                ));
            }
            *slot = Some(SurfacePack {
                visual: key.to_owned(),
                maps: maps.clone(),
                authored_m: metres_per_repeat.0,
            });
        }
    }
    let [Some(primary), Some(secondary)] = lanes else {
        return Err("building_lane is incomplete; declare one Maps visual for Primary and one for Secondary".to_owned());
    };
    Ok([primary, secondary])
}
fn palette() -> &'static [SurfacePack; 2] {
    static PALETTE: std::sync::LazyLock<[SurfacePack; 2]> = std::sync::LazyLock::new(|| {
        let path = crate::assets::asset_root().join("materials/visuals.ron");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!(
                "{}: {err}; restore the required visual library",
                path.display()
            )
        });
        palette_of(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    });
    &PALETTE
}
pub(crate) fn lane_for_visual(key: &str) -> Result<SurfaceLane, String> {
    for lane in [SurfaceLane::Primary, SurfaceLane::Secondary] {
        if palette()[lane.index()].visual == key {
            return Ok(lane);
        }
    }
    Err(format!(
        "materials/visuals.ron visual {key:?} has no building_lane; declare its palette lane"
    ))
}
pub(crate) fn lane_for_substance(name: &str) -> SurfaceLane {
    let registry = crate::substances::SubstanceRegistry::shipped_static();
    let key = registry.visual(name).unwrap_or_else(|| panic!("materials/materials.ron substance {name:?} has no visual; author a hard substance with its visual"));
    lane_for_visual(key).unwrap_or_else(|err| {
        panic!("materials/materials.ron substance {name:?} field visual: {err}")
    })
}

/// The the secondary palette lane pack's lane of [`BuildingSurface`].
#[derive(ShaderType, Reflect, Debug, Clone, Default)]
pub(crate) struct BuildingSurfaceParams {
    /// `x`: the roof pack's repeats per metre. `yzw`: unused.
    pub roof: Vec4,
}

/// The roof's half of [`BuildingMaterial`]: the secondary palette lane pack's three maps and their scale. Bindings
/// start at 100, bevy's convention for a material extension, clear of the `StandardMaterial` slots.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct BuildingSurface {
    #[uniform(100)]
    pub params: BuildingSurfaceParams,
    #[texture(101)]
    #[sampler(102)]
    pub roof_albedo: Handle<Image>,
    /// NO SAMPLER OF ITS OWN, and neither has the ARM: both read through the albedo's, which
    /// addresses and filters as theirs would. Every sampler a material declares counts against the
    /// 16 a stage gets on Metal and DX12 resource-binding tier 1.
    #[texture(103)]
    pub roof_normal: Handle<Image>,
    #[texture(104)]
    pub roof_arm: Handle<Image>,
    /// The building's hole table ([`crate::buildings::view::hole_table`]), or [`no_holes`] on the
    /// shared material, whose opaque variant never reads it.
    #[storage(105, read_only)]
    pub holes: Handle<ShaderBuffer>,
}

impl MaterialExtension for BuildingSurface {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        PREPASS_SHADER.into()
    }

    fn specialize(
        _pipeline: &bevy::pbr::MaterialExtensionPipeline,
        _descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialExtensionKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        assert!(
            layout.0.contains(Mesh::ATTRIBUTE_UV_1),
            "every building mesh carries its authored palette lane and hole tag in UV1"
        );
        Ok(())
    }
}

/// The table the shared material binds: one empty record, as a storage binding holds at least one.
pub(crate) fn no_holes() -> ShaderBuffer {
    ShaderBuffer::from(vec![Vec4::ZERO])
}

/// `shared` for one building with holes, its `table` bound (module doc). Alpha-masked at a cutoff
/// of 0, which no alpha falls below: the mask only makes bevy build the variant that may discard,
/// in the main pass and in the shadow pass.
pub(crate) fn holed(shared: &BuildingMaterial, table: Handle<ShaderBuffer>) -> BuildingMaterial {
    let mut holed = shared.clone();
    holed.base.alpha_mode = AlphaMode::Mask(0.0);
    holed.extension.holes = table;
    holed
}

/// The building maps whose load must succeed, held by id for [`report_failed_building_map`]. Only
/// inserted where [`material`] ran, which is only where there is a window to draw into.
#[derive(Resource)]
struct BuildingMaps([AssetId<Image>; 6]);

/// The material every building shares: both packs' maps through the surface sampler (repeat,
/// trilinear, anisotropic), the walls' scale on the standard half's `uv_transform`, the roof's on
/// the extension's.
pub(crate) fn material(
    commands: &mut Commands,
    server: &AssetServer,
    materials: &mut Assets<BuildingMaterial>,
    buffers: &mut Assets<ShaderBuffer>,
) -> Handle<BuildingMaterial> {
    let load = |pack: &SurfacePack, map: &str, encoding: MapEncoding| {
        surface_map(server, &pack.path(map), encoding, MapTiling::Repeat)
    };
    let [wall, roof] = palette();
    let wall_albedo = load(wall, "diff", MapEncoding::Srgb);
    let wall_normal = load(wall, "nor_gl", MapEncoding::Linear);
    let wall_arm = load(wall, "arm", MapEncoding::Linear);
    let roof_albedo = load(roof, "diff", MapEncoding::Srgb);
    let roof_normal = load(roof, "nor_gl", MapEncoding::Linear);
    let roof_arm = load(roof, "arm", MapEncoding::Linear);
    commands.insert_resource(BuildingMaps([
        wall_albedo.id(),
        wall_normal.id(),
        wall_arm.id(),
        roof_albedo.id(),
        roof_normal.id(),
        roof_arm.id(),
    ]));
    materials.add(BuildingMaterial {
        base: StandardMaterial {
            base_color_texture: Some(wall_albedo),
            // `nor_gl` is the OpenGL convention bevy reads (+Y green toward the image's top), so
            // no flip; `unit_box`'s tangents are built to the same convention.
            normal_map_texture: Some(wall_normal),
            // glTF ORM packing, which Poly Haven's `arm` is: R ambient occlusion, G roughness,
            // B metallic, one image feeding both slots.
            occlusion_texture: Some(wall_arm.clone()),
            metallic_roughness_texture: Some(wall_arm),
            // DERIVED identity factors: the declared pack's ARM channels supply both values.
            perceptual_roughness: 1.0,
            metallic: 1.0,
            uv_transform: Affine2::from_scale(Vec2::splat(wall.uv_per_m())),
            ..default()
        },
        extension: BuildingSurface {
            params: BuildingSurfaceParams {
                roof: Vec4::new(roof.uv_per_m(), 0.0, 0.0, 0.0),
            },
            roof_albedo,
            roof_normal,
            roof_arm,
            holes: buffers.add(no_holes()),
        },
    })
}

/// The horizontal cube edge a graybox top's tangent runs along (prototype data, `map::BoxSurface`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub(crate) enum BoxAxis {
    X,
    Z,
}
impl BoxAxis {
    pub(crate) fn direction(self) -> Vec3 {
        match self {
            Self::X => Vec3::X,
            Self::Z => Vec3::Z,
        }
    }
}

/// A building's view mesh: the unit cube its block transform scales, the collider's own shape, so
/// the drawn silhouette is the block's. Each face carries its own frame: outward normal `n`, tangent
/// `t` (the image's right, `u` increasing) and image-up `b = n × t` (tangent `w = +1`), with
/// `uv = (p·t, −p·b)` at every corner `p` — one unit per cube edge, `v` increasing down the image.
/// That is the frame bevy's normal mapping reads (`B = w · N × T`, a `nor_gl` map's green toward
/// image-up), and the one the vertex stage rescales to metres (module doc).
///
/// Walls use the upright image convention; the top tangent's cube edge and every face's lane are
/// authored. The corners are `n ± t ± b` halved, so a face is the cube's own only while its frame
/// runs along the cube's edges: [`BoxAxis`] admits no other top tangent.
pub(crate) fn unit_box(top_tangent: BoxAxis, lanes: &[SurfaceLane; 6]) -> Mesh {
    // (outward normal, tangent). A wall's tangent is `Y × n`, which makes its image-up `+Y`.
    let faces = [
        (Vec3::X, Vec3::Y.cross(Vec3::X)),
        (Vec3::NEG_X, Vec3::Y.cross(Vec3::NEG_X)),
        (Vec3::Z, Vec3::Y.cross(Vec3::Z)),
        (Vec3::NEG_Z, Vec3::Y.cross(Vec3::NEG_Z)),
        (Vec3::Y, top_tangent.direction()),
        // The bottom sits in the ground skirt and is never seen; any frame in its plane serves.
        (Vec3::NEG_Y, Vec3::X),
    ];
    let mut packs = Vec::new();
    let (mut positions, mut normals, mut tangents, mut uvs, mut indices) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for ((n, t), lane) in faces.into_iter().zip(lanes) {
        let b = n.cross(t);
        let first = positions.len() as u16;
        // Counter-clockwise seen from outside: bottom-left, bottom-right, top-right, top-left.
        for (along, up) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = 0.5 * (n + along * t + up * b);
            positions.push(p.to_array());
            normals.push(n.to_array());
            tangents.push(t.extend(1.0).to_array());
            uvs.push([p.dot(t), -p.dot(b)]);
            packs.push([lane.index() as f32, 0.0]);
        }
        indices.extend([0, 1, 2, 0, 2, 3].map(|k| first + k));
    }
    Mesh::new(PrimitiveTopology::TriangleList, default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, tangents)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, packs)
        .with_inserted_indices(Indices::U16(indices))
}

/// Surface a failed building-map load (ADR-0011, the stance `world::report_failed_terrain_map`
/// takes). A map that fails to decode leaves the material unprepared, and bevy then draws no
/// building at all while every collider stands: invisible walls. Required, in-repo asset ⇒ panic.
fn report_failed_building_map(
    mut failures: MessageReader<AssetLoadFailedEvent<Image>>,
    required: Option<Res<BuildingMaps>>,
) {
    let Some(required) = required else {
        return;
    };
    for failure in failures.read() {
        if required.0.contains(&failure.id) {
            let (path, err) = (&failure.path, &failure.error);
            error!("required building map {path} failed to load: {err}");
            panic!("required building map {path} failed to load: {err}");
        }
    }
}

/// Mounted by `world` beside the ground's material, on every composition: `world`'s environment
/// spawn takes this material's asset store wherever it runs, and the plugin's render half is behind
/// the render app, which the dedicated server does not have.
pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(MaterialPlugin::<BuildingMaterial>::default())
        .add_systems(Update, report_failed_building_map);
}

#[cfg(test)]
mod tests {
    use bevy::mesh::VertexAttributeValues;

    use super::*;

    fn vec3s(mesh: &Mesh, attribute: bevy::mesh::MeshVertexAttribute) -> Vec<Vec3> {
        match mesh.attribute(attribute) {
            Some(VertexAttributeValues::Float32x3(values)) => {
                values.iter().copied().map(Vec3::from_array).collect()
            }
            other => panic!("{} is not Float32x3: {other:?}", attribute.name),
        }
    }

    /// Every vertex of `unit_box(ridge, &[SurfaceLane::Primary; 6])`: position, normal, tangent (xyz, w) and uv.
    fn corners(ridge: BoxAxis) -> Vec<(Vec3, Vec3, Vec4, Vec2)> {
        let mesh = unit_box(ridge, &[SurfaceLane::Primary; 6]);
        let positions = vec3s(&mesh, Mesh::ATTRIBUTE_POSITION);
        let normals = vec3s(&mesh, Mesh::ATTRIBUTE_NORMAL);
        let Some(VertexAttributeValues::Float32x4(tangents)) =
            mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
        else {
            panic!("the box carries Float32x4 tangents");
        };
        let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("the box carries Float32x2 uvs");
        };
        (0..positions.len())
            .map(|k| {
                (
                    positions[k],
                    normals[k],
                    Vec4::from_array(tangents[k]),
                    Vec2::from_array(uvs[k]),
                )
            })
            .collect()
    }

    /// THE FACE FRAME THE VERTEX STAGE RESCALES. The shader turns a face's UV into metres by
    /// multiplying `u` by the world length of the tangent and `v` by that of `w · n × t`, which is
    /// exact only if every corner's UV IS its position projected on those two axes, `v` downward.
    /// A mesh whose UV ran against its tangent would mirror the texture and turn its normal map's
    /// bumps into dents; one whose UV ran along another axis would stretch a face by the box's
    /// aspect.
    #[test]
    fn every_corner_uv_is_its_position_on_the_face_frame() {
        for ridge in [BoxAxis::X, BoxAxis::Z] {
            for (p, n, t, uv) in corners(ridge) {
                let b = t.w * n.cross(t.truncate());
                assert_eq!(uv, Vec2::new(p.dot(t.truncate()), -p.dot(b)), "corner {p}");
                assert_eq!(t.w, 1.0);
                if n == Vec3::Y {
                    assert_eq!(
                        t.truncate(),
                        ridge.direction(),
                        "the authored top tangent is preserved"
                    );
                }
                assert_eq!(t.truncate().dot(n), 0.0, "the tangent lies in the face");
            }
        }
    }

    /// THE SILHOUETTE IS THE COLLIDER'S. The block transform scales a `Collider::cuboid(1, 1, 1)`;
    /// the mesh it scales must be exactly that cube, every face drawn from outside (counter-clockwise
    /// about its outward normal) — a mesh one face short, or one wound inward, shows the player a
    /// building the simulation does not have.
    #[test]
    fn the_box_is_the_unit_cube_wound_outward() {
        for top in [BoxAxis::X, BoxAxis::Z] {
            let mesh = unit_box(top, &[SurfaceLane::Primary; 6]);
            let positions = vec3s(&mesh, Mesh::ATTRIBUTE_POSITION);
            let normals = vec3s(&mesh, Mesh::ATTRIBUTE_NORMAL);
            assert!(
                positions.iter().all(|p| p.abs() == Vec3::splat(0.5)),
                "every corner is a corner of the unit cube",
            );
            let Some(Indices::U16(indices)) = mesh.indices() else {
                panic!("the box is indexed u16");
            };
            let mut faces = std::collections::BTreeSet::new();
            for triangle in indices.chunks(3) {
                let [a, b, c] = [0, 1, 2].map(|k| usize::from(triangle[k]));
                let n = normals[a];
                let winding = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
                assert!(winding.dot(n) > 0.0, "triangle {triangle:?} faces inward");
                assert!(
                    [a, b, c].iter().all(|&k| positions[k].dot(n) == 0.5),
                    "triangle {triangle:?} lies on its face",
                );
                faces.insert(n.to_array().map(|c| c as i32));
            }
            assert_eq!(
                (faces.len(), indices.len()),
                (6, 36),
                "six faces, two triangles each"
            );
        }
    }

    /// WALLS STAND, THE ROOF RUNS ALONG THE RIDGE. Bricks lie in horizontal courses only if every
    /// wall's image-up is world-up; the top face preserves its authored tangent independently
    /// of the box's proportions.
    #[test]
    fn walls_stand_upright_and_the_roof_runs_along_the_ridge() {
        for ridge in [BoxAxis::X, BoxAxis::Z] {
            for (_, n, t, _) in corners(ridge) {
                let b = t.w * n.cross(t.truncate());
                if n.y == 0.0 {
                    assert_eq!(b, Vec3::Y, "the wall facing {n} stands its image upright");
                } else if n.y > 0.0 {
                    assert_eq!(
                        t.truncate(),
                        ridge.direction(),
                        "the roof's boards run along the ridge"
                    );
                }
            }
        }
    }

    /// EVERY SHIPPED BUILDING MATERIAL HAS A PALETTE LANE. A client resolves a material's lane
    /// when it builds the view mesh, and the dedicated server never reads visuals.ron, so the
    /// shipped data is gated here: a material whose visual has no building_lane would load on the
    /// server and refuse every client at map spawn.
    #[test]
    fn every_shipped_building_material_has_a_lane() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/buildings");
        let mut checked = 0;
        for entry in std::fs::read_dir(&root).expect("assets/buildings is shipped") {
            let dir = entry.expect("a readable directory entry").path();
            let id = dir
                .file_name()
                .expect("a named entry")
                .to_string_lossy()
                .into_owned();
            let file = dir.join(format!("{id}.building.ron"));
            if !file.exists() {
                continue; // A texture pack shares the directory and names no material.
            }
            let text = std::fs::read_to_string(&file).expect("a readable building asset");
            let asset = crate::buildings::schema::BuildingAsset::from_ron(&text)
                .unwrap_or_else(|err| panic!("{}: {err}", file.display()));
            for material in &asset.materials {
                lane_for_substance(material);
                checked += 1;
            }
        }
        assert!(checked > 0, "a building asset is shipped");
    }

    /// The shared shader requires exactly one declared maps visual per lane. A missing or
    /// duplicate assignment is a load refusal, never a material-name or orientation default.
    #[test]
    fn palette_assignments_are_complete_and_unique() {
        let visual = |key: &str, lane: &str| {
            format!(
                "{key:?}: Maps(maps: {key:?}, metres_per_repeat: (1.0, Tuned(\"ASSUMED fixture\")), building_lane: Some({lane}), takes_paint: false),"
            )
        };
        let primary = visual("first", "Primary");
        let secondary = visual("second", "Secondary");
        assert!(palette_of(&format!("(visuals: {{{primary}{secondary}}})")).is_ok());
        for rows in [
            primary.clone(),
            format!("{primary}{}", visual("third", "Primary")),
        ] {
            let error = palette_of(&format!("(visuals: {{{rows}}})"))
                .err()
                .expect("bad palette must refuse");
            assert!(error.contains("building_lane"), "{error}");
        }
    }

    /// THE SHIPPED MAPS ARE WHAT THE MATERIAL BINDS. Each of the six is a real KTX2 (not a Git LFS
    /// pointer) that bevy's own loader transcodes, 1024² with a full mip chain: a map that fails to
    /// load leaves every building undrawn while its collider stands, and one without mips shimmers
    /// at every range a tank sees a house from. Pinned against `CompressedImageFormats::BC`, as the
    /// terrain's maps are: the transcode target follows the caller's flags, not the test machine.
    #[test]
    fn the_shipped_building_maps_are_mipmapped_1k_ktx2() {
        use bevy::image::{CompressedImageFormats, ktx2_buffer_to_image};
        // 1024² ⇒ 11 levels down to 1×1.
        const FULL_MIP_CHAIN: u32 = 11;
        for pack in palette() {
            for (map, is_srgb) in [("diff", true), ("nor_gl", false), ("arm", false)] {
                let path = pack.path(map);
                let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("assets")
                    .join(&path);
                let bytes = std::fs::read(&full).unwrap_or_else(|err| {
                    panic!("building map missing at {}: {err}", full.display())
                });
                assert!(
                    bytes.len() > 4096,
                    "{path} is {} bytes — a Git LFS POINTER, not the map",
                    bytes.len(),
                );
                let image = ktx2_buffer_to_image(&bytes, CompressedImageFormats::BC, is_srgb)
                    .unwrap_or_else(|err| panic!("{path} failed to transcode: {err:?}"));
                let size = image.texture_descriptor.size;
                assert_eq!((size.width, size.height), (1024, 1024), "{path} is 1k");
                assert_eq!(
                    image.texture_descriptor.mip_level_count, FULL_MIP_CHAIN,
                    "{path} carries a full mip chain",
                );
            }
        }
    }
}
