//! The scatter buildings' surface: brick walls under a timber roof, through ONE
//! [`BuildingMaterial`] every building shares: a graybox building's box ([`crate::scatter`]) and a
//! building of the destruction model's element mesh ([`crate::buildings::view`]), whose faces are
//! framed by the same rule in metres and mark their pack per vertex (see Walls and roof below).
//!
//! VIEW ONLY (ADR-0014). The drawn box is the collider's own unit cube under the block transform,
//! so the silhouette is exactly what the simulation blocks; this module decides only what its faces
//! look like. The dedicated server never builds the material.
//!
//! # Metres on every face, one mesh and one material
//!
//! Every building is a unit cube scaled per instance — its prototype's box times the instance's own
//! scale — so no UV a shared mesh bakes can be in metres. [`unit_box`] bakes each face's UV in FACE
//! UNITS instead: one per cube edge, `u` along the face's tangent and `v` down its image. The
//! material's vertex stage multiplies them by the world length of those two axes under the
//! instance's transform (`assets/shaders/building_surface.wgsl`), so every face reads in metres
//! whatever the box's proportions and the instance's scale, and no instance needs a mesh or a
//! material of its own: the buildings batch as the graybox did.
//!
//! Each pack repeats at its own authored metres ([`SurfacePack::authored_m`], the pack's `cc.txt`):
//! the wall pack through the standard half's `uv_transform`, the roof pack through
//! [`BuildingSurfaceParams::roof`].
//!
//! # Walls and roof
//!
//! The standard half carries the [`WALL`] pack and shades every face. The fragment swaps the
//! [`ROOF`] pack's albedo, normal and ARM in where the face's normal is within 60° of up: the box's
//! top, on any building standing within 60° of upright. The roof's boards run along the box's longer
//! horizontal side ([`ridge_axis`]), as a ridge would. A mesh that carries a second UV channel
//! decides per vertex instead: its `x` is 1 on a timber face and 0 on masonry, so an element mesh's
//! sill tops stay brick and its board edges stay timber.
//!
//! # Holes
//!
//! A building with a hole wears its own copy of the material ([`holed`]): alpha-masked, so bevy
//! compiles and runs the hole path in the main pass and in the prepass the shadow pass draws
//! (`assets/shaders/building_holes.wgsl`), with the building's hole table bound
//! ([`crate::buildings::view::hole_table`]). The shared material binds [`no_holes`] and stays opaque,
//! so every other building draws exactly as it would without the hole code.

use bevy::asset::AssetLoadFailedEvent;
use bevy::math::Affine2;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;

use crate::world::{MapEncoding, MapTiling, surface_map};

/// The buildings' material: bevy's `StandardMaterial` carrying the [`WALL`] pack and every lighting
/// parameter, extended with [`BuildingSurface`] (the [`ROOF`] pack).
pub(crate) type BuildingMaterial = ExtendedMaterial<StandardMaterial, BuildingSurface>;

/// The vertex stage (face units to metres) and the fragment (the roof swap, a hole), one file.
const SHADER: &str = "shaders/building_surface.wgsl";

/// The prepass fragment, which bevy runs only for a [`holed`] material: a hole's discard.
const PREPASS_SHADER: &str = "shaders/building_surface_prepass.wgsl";

/// One surface pack's contract: which pack it is and the real-world metres it was authored at.
pub(crate) struct SurfacePack {
    /// The Poly Haven pack: the folder under `assets/buildings/` whose `cc.txt` carries the
    /// attribution and the authored size below. Names the files; nothing parses it.
    pub(crate) pack: &'static str,
    /// Metres per texture repeat, as the scan declares (`dimensions`, millimetres). Mapping the
    /// pack onto anything else resizes every brick and board in it.
    pub(crate) authored_m: f32,
}

impl SurfacePack {
    /// One of the pack's three maps (`diff`, `nor_gl` or `arm`), as the encode script names it.
    fn path(&self, map: &str) -> String {
        format!("buildings/{0}/{0}_{map}.ktx2", self.pack)
    }

    /// Texture repeats per metre.
    fn uv_per_m(&self) -> f32 {
        1.0 / self.authored_m
    }
}

/// The walls: weathered red brick in lime mortar.
pub(crate) const WALL: SurfacePack = SurfacePack {
    pack: "brick_wall_006",
    authored_m: 3.0,
};

/// The roof: lapped, weathered timber boards.
pub(crate) const ROOF: SurfacePack = SurfacePack {
    pack: "dark_planks",
    authored_m: 2.0,
};

/// The [`ROOF`] pack's lane of [`BuildingSurface`].
#[derive(ShaderType, Reflect, Debug, Clone, Default)]
pub(crate) struct BuildingSurfaceParams {
    /// `x`: the roof pack's repeats per metre. `yzw`: unused.
    pub roof: Vec4,
}

/// The roof's half of [`BuildingMaterial`]: the [`ROOF`] pack's three maps and their scale. Bindings
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
    let wall_albedo = load(&WALL, "diff", MapEncoding::Srgb);
    let wall_normal = load(&WALL, "nor_gl", MapEncoding::Linear);
    let wall_arm = load(&WALL, "arm", MapEncoding::Linear);
    let roof_albedo = load(&ROOF, "diff", MapEncoding::Srgb);
    let roof_normal = load(&ROOF, "nor_gl", MapEncoding::Linear);
    let roof_arm = load(&ROOF, "arm", MapEncoding::Linear);
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
            // Factors the maps are multiplied by: 1.0 passes each roughness map through, and a
            // hard-zero metallic keeps brick and timber dielectric whatever a pack's B holds.
            perceptual_roughness: 1.0,
            metallic: 0.0,
            uv_transform: Affine2::from_scale(Vec2::splat(WALL.uv_per_m())),
            ..default()
        },
        extension: BuildingSurface {
            params: BuildingSurfaceParams {
                roof: Vec4::new(ROOF.uv_per_m(), 0.0, 0.0, 0.0),
            },
            roof_albedo,
            roof_normal,
            roof_arm,
            holes: buffers.add(no_holes()),
        },
    })
}

/// The horizontal axis a box's roof boards run along: its longer horizontal side (`Z` on a tie),
/// in the prototype's local frame. `min`/`max` are the prototype's box.
pub(crate) fn ridge_axis(min: [f32; 3], max: [f32; 3]) -> Vec3 {
    let size = Vec3::from_array(max) - Vec3::from_array(min);
    if size.x > size.z { Vec3::X } else { Vec3::Z }
}

/// A building's view mesh: the unit cube its block transform scales, the collider's own shape, so
/// the drawn silhouette is the block's. Each face carries its own frame: outward normal `n`, tangent
/// `t` (the image's right, `u` increasing) and image-up `b = n × t` (tangent `w = +1`), with
/// `uv = (p·t, −p·b)` at every corner `p` — one unit per cube edge, `v` increasing down the image.
/// That is the frame bevy's normal mapping reads (`B = w · N × T`, a `nor_gl` map's green toward
/// image-up), and the one the vertex stage rescales to metres (module doc).
///
/// Walls stand their image upright (`b = +Y`); the top's `t` is `ridge` ([`ridge_axis`]).
pub(crate) fn unit_box(ridge: Vec3) -> Mesh {
    // (outward normal, tangent). A wall's tangent is `Y × n`, which makes its image-up `+Y`.
    let faces = [
        (Vec3::X, Vec3::Y.cross(Vec3::X)),
        (Vec3::NEG_X, Vec3::Y.cross(Vec3::NEG_X)),
        (Vec3::Z, Vec3::Y.cross(Vec3::Z)),
        (Vec3::NEG_Z, Vec3::Y.cross(Vec3::NEG_Z)),
        (Vec3::Y, ridge),
        // The bottom sits in the ground skirt and is never seen; any frame in its plane serves.
        (Vec3::NEG_Y, Vec3::X),
    ];
    let (mut positions, mut normals, mut tangents, mut uvs, mut indices) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (n, t) in faces {
        let b = n.cross(t);
        let first = positions.len() as u16;
        // Counter-clockwise seen from outside: bottom-left, bottom-right, top-right, top-left.
        for (along, up) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = 0.5 * (n + along * t + up * b);
            positions.push(p.to_array());
            normals.push(n.to_array());
            tangents.push(t.extend(1.0).to_array());
            uvs.push([p.dot(t), -p.dot(b)]);
        }
        indices.extend([0, 1, 2, 0, 2, 3].map(|k| first + k));
    }
    Mesh::new(PrimitiveTopology::TriangleList, default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, tangents)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
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

    /// Every vertex of `unit_box(ridge)`: position, normal, tangent (xyz, w) and uv.
    fn corners(ridge: Vec3) -> Vec<(Vec3, Vec3, Vec4, Vec2)> {
        let mesh = unit_box(ridge);
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
        for ridge in [Vec3::X, Vec3::Z] {
            for (p, n, t, uv) in corners(ridge) {
                let b = t.w * n.cross(t.truncate());
                assert_eq!(uv, Vec2::new(p.dot(t.truncate()), -p.dot(b)), "corner {p}");
                assert_eq!(t.w, 1.0);
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
        let mesh = unit_box(Vec3::Z);
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

    /// WALLS STAND, THE ROOF RUNS ALONG THE RIDGE. Bricks lie in horizontal courses only if every
    /// wall's image-up is world-up; the roof's boards run along the box's longer side only if the
    /// top face's tangent is the ridge axis.
    #[test]
    fn walls_stand_upright_and_the_roof_runs_along_the_ridge() {
        for ridge in [Vec3::X, Vec3::Z] {
            for (_, n, t, _) in corners(ridge) {
                let b = t.w * n.cross(t.truncate());
                if n.y == 0.0 {
                    assert_eq!(b, Vec3::Y, "the wall facing {n} stands its image upright");
                } else if n.y > 0.0 {
                    assert_eq!(t.truncate(), ridge, "the roof's boards run along the ridge");
                }
            }
        }
    }

    /// The ridge is the box's longer horizontal side (house_proxy fixture data: 4 m by 6 m, so Z).
    #[test]
    fn the_ridge_runs_along_the_longer_horizontal_side() {
        assert_eq!(ridge_axis([-2.0, -0.5, -3.0], [2.0, 2.5, 3.0]), Vec3::Z);
        assert_eq!(ridge_axis([-3.0, -0.5, -2.0], [3.0, 2.5, 2.0]), Vec3::X);
        // Height never decides it: a tall, deep tower still ridges along its deeper side.
        assert_eq!(ridge_axis([-1.0, 0.0, -2.0], [1.0, 30.0, 2.0]), Vec3::Z);
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
        for pack in [&WALL, &ROOF] {
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
