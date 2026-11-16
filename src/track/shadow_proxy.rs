//! The track's SHADOW PROXY: a ~776-triangle closed ribbon that casts the belt's shadow so the
//! 294 880-triangle shoe pool does not have to.
//!
//! # Why
//!
//! A Tiger carries `link_count × 2` real shoes (MEASURED 97 × 2 = 194 entities) at MEASURED 1 520
//! triangles each — 294 880 triangles per tank, and ~1.08 M before the LOD pipeline re-certified the
//! shoe (the cascade numbers below were MEASURED at the old count and
//! are therefore the pessimistic end, not a stale claim about the
//! asset). Every one of them is re-submitted into EVERY shadow
//! cascade, with no detail reduction: MEASURED 2026-07-27, a tank costs 0.364 ms per cascade
//! (`ms_per_cascade = 0.364 × tanks + 0.69`), i.e. 1.36 ms at four cascades against the 0.086 ms the
//! same tank costs with shadows off. A tank's SHADOW is 16× its entire non-shadow cost, and the
//! tracks are almost all of it.
//!
//! None of that geometry can be recorded. At the far cascade's texel size (0.49 m at the shipped
//! 1000 m / 4096 / 3 ladder, and 0.82 m at 1 km on the old one) a single link — MEASURED 0.13 m
//! pitch, 0.726 m wide — is SUB-TEXEL. The shadow map cannot resolve one shoe, let alone the guide
//! horn on it. What it can resolve is the belt's silhouette, and that is a ribbon.
//!
//! # What this is
//!
//! One extra entity per tank side whose mesh is a closed rectangular TUBE swept along the belt's
//! drawn pin polyline — the same `wrap::WrapSideOutput::joints` the shoes are placed on, never a
//! re-derivation (house doctrine: `wrap` is the single geometry authority, and the sim/view
//! `SagClip` split is load-bearing). The real links keep rendering at full detail to the camera and
//! get `NotShadowCaster`; the proxy is invisible to the camera and casts in their place.
//!
//! Triangle budget per side: 4 faces × `count` segments × 2 = MEASURED 776 against ~538 000. The
//! tube is CLOSED (outer, inner and both lateral faces) rather than a zero-thickness sheet, which is
//! what makes it independent of the sun: bevy's shadow pass honours the material's `cull_mode`
//! (`StandardMaterial` defaults to back-face culling), so a single sheet would vanish for every sun
//! direction that sees its back. A closed tube always presents front faces.
//!
//! # What it buys — MEASURED 2026-07-28, against a proxy that casts
//!
//! `OVERMATCH_PROBE_TANKS` 2 vs 8, `M350` / 4096, vsync off, foreground on the built-in panel, the
//! median of a 24 s window, two runs per arm agreeing inside ±0.02 ms at 8 tanks:
//!
//! | 8 tanks              | 2 cascades | 4 cascades |
//! |----------------------|-----------:|-----------:|
//! | shadows off          |   4.51 ms  |   4.51 ms  |
//! | real shoes cast      |  10.55 ms  |  13.15 ms  |
//! | ribbon casts         |   5.34 ms  |   5.88 ms  |
//!
//! Marginal cost of a tank's CASTERS, from the 2 → 8 tank slope with the shadows-off slope removed:
//! MEASURED **0.35 ms per tank per cascade** for the real shoes (which reproduces the 0.364 above)
//! against **0.011 ms at 2 cascades and 0.004 ms at 4** for the ribbon — a 31–70× reduction, and at
//! 4 cascades the ribbon's marginal cost is down at the session's own repeatability. The shipped
//! frame at 8 tanks / 4 cascades goes 13.15 → 5.88 ms.
//!
//! The shadow ladder was tuned against an 18.5× figure (0.0385 ms/tank/cascade) that understated
//! the win, so it is conservative, not optimistic.
//!
//! # How it hides from the camera
//!
//! It declares [`crate::render_policy::VisualScope::SHADOW_PROXY`] and nothing else. That preset
//! means "drawn by no camera, seen by every light", the ribbon carries an ORDINARY OPAQUE material,
//! and the rest of this module has no opinion about rendering at all. `track::view` sets the scope
//! at spawn; `render_policy` owns what it resolves to.
//!
//! ## What it relies on
//!
//! **Each light carries the right mask.** bevy_pbr 0.20.1 copies the light's `RenderLayers` onto
//! every kind of shadow view (bevyengine/bevy#24797; record in
//! `upstream/bevy-shadow-view-ignores-light-render-layers.md`). Without that mask,
//! `queue_shadows` filters every caster against a shadow view that has no mask, so layer 0, and a
//! ribbon off layer 0 casts nothing. `bevy_light`'s `check_dir_light_mesh_visibility` gates
//! casters on the light's mask and marks what it saw visible, so a caster no camera draws
//! survives extraction. The sun declares `LightProfile::BattlefieldSun`, which covers this
//! ribbon's channel, and `render_policy`'s tests pin that the sun reaches it and that no camera
//! does.
//!
//! **Its scope overrides its tank's.** The ribbon is parented under its hull, and scopes are
//! inherited and overridable: a shadow proxy overrides its tank-root ancestor by construction. A
//! sweep that rewrote the `RenderLayers` of every mesh under a tank would put it back on the
//! camera's layers.
//!
//! Hiding the ribbon inside the shoes was measured and rejected: painted magenta, the ribbon shows
//! through the gaps between consecutive shoes all around the belt. A track is a chain of plates,
//! not a surface, and there is nowhere inside it to hide.
//!
//! # The cross-section is the measured shoe, with no fudge
//!
//! [`Section`] puts the ribbon's four faces exactly on the shoe's own measured faces — RADIALLY at
//! `pin_line − pin_to_inner` … `pin_line + pin_to_outer` (MEASURED 24.64 mm and 70.90 mm, the outer
//! down to the sole; the pin does NOT run mid-plate, which is why this is two numbers and not
//! `thickness/2`), LATERALLY across `link_center_x ± width/2` (MEASURED 725.97 mm wide, centred
//! 16.85 mm OUTBOARD of the pin plane — `plane_x ± width/2` about the PIN plane is the wrong
//! construction here, per `rig_geom`'s width rule). All four come from the model's measured shoe
//! (`track::marker_model`), so a new tank's belt needs no new constants.
//!
//! The faces are flush. An inset (12 mm radial, 30 mm lateral) holding the ribbon inside the shoe
//! solid, so a coincident caster cannot stripe the links it stands in for (they RECEIVE shadows:
//! the hull's shadow crossing the return run is visible), is rejected: `Link_Box` is an authored
//! ENVELOPE, not the plate's surface, so an inset from it need not land inside solid material
//! between the grousers, and a caster coincident with its own receiver is the acne case
//! [`crate::world`]'s `SUN_SHADOW_NORMAL_BIAS` owns. Compared by eye (a live `I`-key A/B on one
//! tank, one patch of ground, one sun), flush looks better, and it keeps the 60 mm of shadow width
//! out of 726 (8%) and the 25 mm of drop the inset cost.
//!
//! If acne appears here, the margin is DERIVED — `max(bias × texel size, envelope-to-material
//! gap)`, both measurable — never picked.

use std::ops::DerefMut;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

use super::loop_geom;

/// One side's proxy, held on the side's rig entry.
pub(crate) struct ProxySide {
    /// The ribbon entity — a child of the tank root, identity transform, so its mesh is authored
    /// directly in hull space (the frame the belt joints already live in). It carries no `Mesh3d`
    /// until [`ProxySide::mesh`] exists.
    pub(crate) entity: Entity,
    /// This side's OWN mesh asset, created by the first fit and holding that geometry from its
    /// first frame. Never shared between tanks or sides: two tanks stand on
    /// different ground, and their belts differ every frame.
    ///
    /// No empty ribbon exists, in `Assets<Mesh>` or on an entity. bevy_render 0.20.1 logs every
    /// extraction of a zero-vertex mesh as `Use-after-free … unallocated key`, and a `Mesh3d` on a
    /// mesh with no positions fails the shadow prepass pipeline's validation (vertex input
    /// `Location[0]` not provided), which quits the client.
    ///
    /// `Some` is also the CASTER SWAP's readiness. The shoes are silenced on the frame it is set,
    /// never at spawn: the first fit can legitimately be late (no `TrackField` yet, a replica whose
    /// interpolated `TrackDrive` is not yet visible), and silencing the shoes before it lands is a
    /// tank with NO belt shadow at all, silently.
    pub(crate) mesh: Option<Handle<Mesh>>,
    /// This side's cross-section, resolved from the measured shoe at bind.
    pub(crate) section: Section,
    /// Retained hull-local shape and mesh buffers; independent of the root transform.
    pub(crate) ribbon: RibbonCache,
}

/// What the view layer owes the world after folding one frame's belt into a proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProxyStep {
    /// Nothing to do — the ribbon is already carrying the shadow.
    Idle,
    /// The ribbon became real this frame. Give its entity the mesh and silence the shoes now.
    Silence { mesh: Handle<Mesh> },
}

impl ProxySide {
    /// A freshly bound side: its ribbon entity, spawned without a mesh, and the measured shoe's
    /// section.
    pub(crate) fn new(entity: Entity, section: Section) -> Self {
        Self {
            entity,
            mesh: None,
            section,
            ribbon: RibbonCache::default(),
        }
    }

    /// Fold one frame's drawn pin `joints` into the ribbon and say what the caller must do: the first
    /// fit creates the side's asset and asks for the swap ([`ProxySide::mesh`]); a later fit
    /// rewrites it in place only when the hull-local joints change. `meshes` is dereferenced
    /// mutably only when the ribbon is created or changes, so a settled ribbon leaves a `ResMut`
    /// store unchanged.
    pub(crate) fn fit(
        &mut self,
        mut meshes: impl DerefMut<Target = Assets<Mesh>>,
        joints: &[Vec2],
    ) -> ProxyStep {
        let Some(handle) = &self.mesh else {
            let mesh = meshes.add(self.ribbon.build(joints, self.section));
            self.mesh = Some(mesh.clone());
            return ProxyStep::Silence { mesh };
        };
        if self.ribbon.needs_update(joints) {
            let mut mesh = meshes
                .get_mut(handle)
                .expect("a side's ribbon asset lives while the side holds its handle");
            self.ribbon.write(&mut mesh, joints, self.section);
        }
        ProxyStep::Idle
    }
}

/// The ribbon's cross-section: where its four faces sit relative to the belt's pin line.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Section {
    /// Hull-local x of the ribbon's lateral centre — the SHOE's own centre (`link_center_x`), not
    /// the pin plane.
    pub(crate) center_x: f32,
    /// Half the ribbon's width (m) — the measured shoe's own half-width, no margin.
    pub(crate) half_width: f32,
    /// Outward offset of the ribbon's outer face from the pin line (m, positive = away from the
    /// wheels, the direction the shoe's outer face hangs).
    pub(crate) outer: f32,
    /// Outward offset of the ribbon's inner face from the pin line (m, negative — it is on the wheel
    /// side of the pins).
    pub(crate) inner: f32,
}

impl Section {
    /// Build the cross-section from the shoe's MEASURED faces, flush — no margin of any kind.
    ///
    /// `pin_to_inner`/`pin_to_outer` are the measured plate offsets (never `thickness/2`); `width`
    /// is the measured `Link_Box` lateral extent; `center_x` is this side's `link_center_x`. Every
    /// one of the four comes from the model, so this takes no constants and a new tank needs none.
    /// The model's admission refuses a width or plate offset that is not finite and positive
    /// ([`super::marker_model::DerivedModel`]), so every section has volume.
    pub(crate) fn from_shoe(
        center_x: f32,
        width: f32,
        pin_to_inner: f32,
        pin_to_outer: f32,
    ) -> Self {
        Self {
            center_x,
            half_width: width * 0.5,
            outer: pin_to_outer,
            inner: -pin_to_inner,
        }
    }
}

/// The proxy's material: ORDINARY and OPAQUE.
///
/// The ribbon is hidden by its [`crate::render_policy::VisualScope::SHADOW_PROXY`] scope, not by
/// its material, so the material's only job is to stay out of the way — and opaque is the shape
/// that does that best. `AlphaMode::Opaque` sets no `MeshPipelineKey::MAY_DISCARD` on the shadow
/// item (`vendor/bevy_pbr-0.20.1-overmatch/src/render/light.rs:2585-2591`), so the shadow draw
/// takes the cheapest `is_depth_only` path there is and runs no fragment shader at all.
///
/// # The trick this retires
///
/// It used to be zero alpha and `AlphaMode::Multiply` — an invisible material standing in for a
/// render layer. Worth knowing why, because the FIRST spelling of that trick cast nothing at all
/// and did it silently: `AlphaMode::Blend` at alpha 0 sets `MAY_DISCARD`, which routes the shadow
/// draw through the material's PREPASS fragment shader, and `prepass_alpha_discard`
/// (`pbr_prepass_functions.wesl:22`) discards every fragment under a 0.05 cutoff before it can
/// write shadow depth. Reading only the MAIN pass's `alpha_discard` (`pbr_functions.wesl:98`,
/// which discards for `Mask`/`AlphaToCoverage` only) is what made it look safe.
///
/// MEASURED 2026-07-28 on the shipped scene: the track-shadow lobe read 82.0 mean luma with the
/// real shoes casting and 83.9 with an OPAQUE proxy of this exact mesh, against 95.6 with the
/// `Blend` one — same entity, same geometry, same frame; only the material differed. That
/// measurement is why this is opaque and why the number to watch is the shadow lobe, not the
/// silhouette.
pub(crate) fn proxy_material() -> StandardMaterial {
    StandardMaterial::default()
}

/// A bound ribbon has a fixed station count, loop winding and cross-section. Rebinding the rig
/// constructs a fresh cache. Positions live in the mesh's retained attribute buffer; the joint copy
/// is the equality key.
#[derive(Default)]
pub(crate) struct RibbonCache {
    stations: Vec<Vec2>,
    winding: f32,
}

impl RibbonCache {
    pub(crate) fn needs_update(&self, stations: &[Vec2]) -> bool {
        self.stations != stations
    }

    /// The first fit: the whole ribbon, topology and all. Fixes the station count and winding for
    /// the life of the bound rig. Called only while [`ProxySide::mesh`] is `None`; that field is
    /// the one record of whether the ribbon is built.
    pub(crate) fn build(&mut self, stations: &[Vec2], section: Section) -> Mesh {
        assert!(
            stations.len() >= 3,
            "the wrap resamples to the admitted `track.link_count`, at least three links"
        );
        assert!(self.stations.is_empty(), "a built ribbon never rebuilds");
        self.winding = loop_geom::loop_winding(stations);
        self.stations.extend_from_slice(stations);
        build_ribbon(stations, section, self.winding)
    }

    /// Every later fit: write only a changed hull-local shape into the retained position buffer
    /// of the mesh [`RibbonCache::build`] returned. Callers check `needs_update` before borrowing
    /// the asset mutably, so a settled ribbon emits no asset modification for render extraction.
    pub(crate) fn write(&mut self, mesh: &mut Mesh, stations: &[Vec2], section: Section) {
        assert_eq!(
            stations.len(),
            self.stations.len(),
            "the wrap resamples every frame to the rig's station count"
        );
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("`build_ribbon` gives the ribbon its positions as Float32x3");
        };
        ribbon_vertices(stations, section, self.winding, |i, position, _, _, _| {
            positions[i] = position;
        });
        self.stations.copy_from_slice(stations);
    }
}

/// Visit the ribbon's vertices in ring order, using the mitre of adjacent edge normals to keep
/// faces continuous around end arcs. Inputs and output are hull-local; root motion is irrelevant.
fn ribbon_vertices(
    stations: &[Vec2],
    section: Section,
    winding: f32,
    mut vertex: impl FnMut(usize, [f32; 3], f32, Vec2, f32),
) {
    let n = stations.len();
    let edge_normal =
        |i: usize| loop_geom::outward_normal(stations[i], stations[(i + 1) % n], winding);
    for (i, &s) in stations.iter().enumerate() {
        let normal = (edge_normal((i + n - 1) % n) + edge_normal(i)).normalize_or_zero();
        // Ring order: outer -x, outer +x, inner +x, inner -x. Side-plane (z, y) becomes (x, y, z).
        let ring = [
            (-section.half_width, section.outer),
            (section.half_width, section.outer),
            (section.half_width, section.inner),
            (-section.half_width, section.inner),
        ];
        for (k, (dx, radial)) in ring.into_iter().enumerate() {
            let p = s + normal * radial;
            vertex(
                i * 4 + k,
                [section.center_x + dx, p.y, p.x],
                dx,
                normal,
                radial,
            );
        }
    }
}

/// Build topology and attribute buffers at the first valid fit. The opaque shadow pass reads
/// positions only (`bevy_pbr`'s depth-only prepass); the visible diagnostic uses an unlit material.
/// Normals and UVs retain the common vertex layout but need no subsequent writes.
fn build_ribbon(stations: &[Vec2], section: Section, winding: f32) -> Mesh {
    let n = stations.len();
    let mut positions = Vec::with_capacity(n * 4);
    let mut normals = Vec::with_capacity(n * 4);
    let uvs = vec![[0.0, 0.0]; n * 4];
    ribbon_vertices(
        stations,
        section,
        winding,
        |_, position, dx, normal, radial| {
            positions.push(position);
            normals.push(
                Vec3::new(dx, normal.y * radial, normal.x * radial)
                    .normalize_or_zero()
                    .to_array(),
            );
        },
    );
    let mut indices: Vec<u32> = Vec::with_capacity(n * 24);
    for i in 0..n {
        let a = (i * 4) as u32;
        let b = (((i + 1) % n) * 4) as u32;
        for k in 0..4u32 {
            let k2 = (k + 1) % 4;
            indices.extend_from_slice(&[a + k, b + k, b + k2, a + k, b + k2, a + k2]);
        }
    }
    // Positive signed volume names outward faces: back-face culling must preserve the caster.
    // The wrap keeps this loop's winding and connectivity for the lifetime of the bound rig.
    if signed_volume(&positions, &indices) < 0.0 {
        for tri in indices.as_chunks_mut::<3>().0 {
            tri.swap(1, 2);
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[cfg(test)]
fn ribbon_mesh(stations: &[Vec2], section: Section) -> Mesh {
    build_ribbon(stations, section, loop_geom::loop_winding(stations))
}

/// Six times the signed volume of a closed triangle soup (the divergence-theorem sum). Positive when
/// the triangles are wound outward.
fn signed_volume(positions: &[[f32; 3]], indices: &[u32]) -> f32 {
    indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|tri| {
            let [a, b, c] = [0, 1, 2].map(|k| Vec3::from_array(positions[tri[k] as usize]));
            a.dot(b.cross(c))
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MEASURED Tiger shoe (`marker_model`'s own pins): plate faces, width and the shoe centre.
    /// Fixture inputs to the pure geometry, not assertions about the shipped glb — the file's
    /// numbers are `marker_model`'s to pin.
    const WIDTH: f32 = 0.725_97;
    const PIN_TO_INNER: f32 = 0.025_56;
    const PIN_TO_OUTER: f32 = 0.024_73;
    const CENTER_X: f32 = 1.548;

    fn tiger_section() -> Section {
        Section::from_shoe(CENTER_X, WIDTH, PIN_TO_INNER, PIN_TO_OUTER)
    }

    /// A closed convex belt: a circle of `n` joints in `(z, y)`, radius `r`, centred at the origin.
    fn ring(n: usize, r: f32) -> Vec<Vec2> {
        (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                Vec2::new(r * a.cos(), r * a.sin())
            })
            .collect()
    }

    fn positions(mesh: &Mesh) -> Vec<Vec3> {
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
            .expect("the ribbon always carries positions")
            .iter()
            .map(|p| Vec3::from_array(*p))
            .collect()
    }

    /// Invariant: a bound ribbon builds its topology once and retains all buffers while its shape
    /// deforms. Shadow extraction relies on unchanged connectivity and exact fresh-build positions.
    /// ASSUMED synthetic rings exercise both windings and changing mitres, independently of assets.
    #[test]
    fn a_deforming_ribbon_builds_indices_once_and_retains_its_buffers() {
        for reverse in [false, true] {
            let mut cache = RibbonCache::default();
            let mut stations = ring(48, 2.0);
            if reverse {
                stations.reverse();
            }
            let mut mesh = cache.build(&stations, tiger_section());
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("indexed ribbon")
            };
            let index_ptr = indices.as_ptr();
            let indices = indices.clone();
            let position_ptr = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .unwrap()
                .as_float3()
                .unwrap()
                .as_ptr();
            let normal = mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap().clone();
            let uv = mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap().clone();
            let joint_ptr = cache.stations.as_ptr();
            for frame in 1..64 {
                let shape = stations
                    .iter()
                    .map(|p| Vec2::new(p.x, p.y + (p.x * frame as f32 * 0.01).sin() * 0.1))
                    .collect::<Vec<_>>();
                assert!(cache.needs_update(&shape));
                cache.write(&mut mesh, &shape, tiger_section());
                let reference = ribbon_mesh(&shape, tiger_section());
                assert_eq!(positions(&mesh), positions(&reference));
                let Some(Indices::U32(current)) = mesh.indices() else {
                    panic!("indexed ribbon")
                };
                assert_eq!(current, &indices);
                assert_eq!(current.as_ptr(), index_ptr);
                assert_eq!(mesh.indices(), reference.indices());
                assert_eq!(
                    mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                        .unwrap()
                        .as_float3()
                        .unwrap()
                        .as_ptr(),
                    position_ptr
                );
                assert_eq!(mesh.attribute(Mesh::ATTRIBUTE_NORMAL), Some(&normal));
                assert_eq!(mesh.attribute(Mesh::ATTRIBUTE_UV_0), Some(&uv));
                assert_eq!(cache.stations.as_ptr(), joint_ptr);
                assert!(!cache.needs_update(&shape));
            }
        }
    }

    /// The budget claim the whole exercise rests on: the ribbon is four quads per belt station and
    /// nothing else, so its size scales with the LINK COUNT and not with the shoe's detail.
    #[test]
    fn the_ribbon_costs_eight_triangles_per_link() {
        for count in [12, 97, 240] {
            let mesh = ribbon_mesh(&ring(count, 1.0), tiger_section());
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("the ribbon is indexed");
            };
            assert_eq!(indices.len(), count * 24, "{count} stations");
            assert_eq!(positions(&mesh).len(), count * 4);
        }
        // 97 links/side against the shoe pool's 97 × 1 520.
        assert_eq!(97 * 8, 776);
    }

    /// The ribbon is flush with the measured shoe: every vertex sits within its envelope, on both a
    /// CCW and a CW belt (the wrap is free to emit either winding).
    #[test]
    fn the_ribbon_stays_inside_the_shoe_envelope() {
        let section = tiger_section();
        for reversed in [false, true] {
            let mut stations = ring(64, 1.5);
            if reversed {
                stations.reverse();
            }
            let mesh = ribbon_mesh(&stations, section);
            for p in positions(&mesh) {
                let dx = (p.x - CENTER_X).abs();
                assert!(
                    dx <= WIDTH * 0.5 + 1e-4,
                    "lateral {dx} escapes the shoe (reversed={reversed})"
                );
                // Radial: the belt is a circle about the origin, so the shoe's outer face is at
                // `1.5 + pin_to_outer` and its inner face at `1.5 - pin_to_inner`.
                let radius = Vec2::new(p.z, p.y).length();
                assert!(
                    radius <= 1.5 + PIN_TO_OUTER + 1e-4,
                    "radius {radius} escapes the outer face (reversed={reversed})"
                );
                assert!(
                    radius >= 1.5 - PIN_TO_INNER - 1e-4,
                    "radius {radius} escapes the inner face (reversed={reversed})"
                );
            }
        }
    }

    /// The tube is wound OUTWARD whichever way the belt runs. A back-wound proxy is culled by the
    /// shadow pass and casts nothing — the silent failure the volume check exists to prevent, so it
    /// is checked on both windings.
    #[test]
    fn the_tube_is_wound_outward_on_either_belt_winding() {
        for reversed in [false, true] {
            let mut stations = ring(48, 2.0);
            if reversed {
                stations.reverse();
            }
            let mesh = ribbon_mesh(&stations, tiger_section());
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("the ribbon is indexed");
            };
            let volume = signed_volume(
                &positions(&mesh)
                    .iter()
                    .map(Vec3::to_array)
                    .collect::<Vec<_>>(),
                indices,
            );
            assert!(volume > 0.0, "inside-out tube (reversed={reversed})");
        }
    }

    /// The alpha modes whose SHADOW item bevy tags with `MeshPipelineKey::MAY_DISCARD` — copied by
    /// hand from `vendor/bevy_pbr-0.20.1-overmatch/src/render/light.rs:2585`, the only place that
    /// decides it. A material in this set has its shadow draw routed through the material's PREPASS
    /// fragment shader, and `prepass_alpha_discard`
    /// (`vendor/bevy_pbr-0.20.1-overmatch/src/render/pbr_prepass_functions.wesl:22`) discards
    /// anything whose alpha is under `PREMULTIPLIED_ALPHA_CUTOFF` = 0.05. So a fully transparent
    /// material in this set casts NOTHING.
    ///
    /// Duplicated rather than reached for because bevy does not expose it: `MeshPipelineKey` and
    /// the whole `queue_shadows` path are render-world internals. If a bevy bump moves the list,
    /// the test below is what fails — which is the point, because the failure it replaces was a
    /// track with no shadow that three A/B screenshots called "indistinguishable".
    fn shadow_pass_may_discard(mode: AlphaMode) -> bool {
        matches!(
            mode,
            AlphaMode::Mask(_)
                | AlphaMode::Blend
                | AlphaMode::Premultiplied
                | AlphaMode::Add
                | AlphaMode::AlphaToCoverage
        )
    }

    /// The ribbon's shadow is its opaque depth draw, and the shadow pass discards a material that
    /// may discard before it writes depth; the ribbon's invisibility is its
    /// `VisualScope::SHADOW_PROXY` channel, which `render_policy` pins, never its material.
    #[test]
    fn the_proxy_material_survives_the_shadow_pass() {
        let material = proxy_material();
        assert!(
            !shadow_pass_may_discard(material.alpha_mode),
            "{:?} routes the shadow draw through the prepass fragment shader, which discards any \
             low alpha entirely — the proxy would cast nothing, silently. The ribbon does not need \
             to be transparent: it is hidden by its render_policy channel.",
            material.alpha_mode
        );
        assert_eq!(
            material.alpha_mode,
            AlphaMode::Opaque,
            "ordinary and opaque takes the cheapest depth-only shadow draw there is"
        );
    }

    /// A `ProxySide` with no entity behind it: `fit` never reads the entity.
    fn fresh_side() -> ProxySide {
        ProxySide::new(Entity::PLACEHOLDER, tiger_section())
    }

    /// Invariant: no empty ribbon mesh exists. The first fit creates the asset already holding the
    /// whole ribbon and silences the shoes, exactly once. The renderer relies on the first half: an
    /// empty mesh on a shadow caster fails the prepass pipeline's validation and quits the client.
    /// The caster swap relies on the second: shoes silenced before geometry exists leave a tank
    /// with no belt shadow. ASSUMED synthetic rings, independent of any tank asset.
    #[test]
    fn the_ribbon_asset_is_born_whole_on_the_frame_the_shoes_are_silenced() {
        let mut meshes = Assets::<Mesh>::default();
        let mut side = fresh_side();
        let stations = ring(48, 2.0);
        let ProxyStep::Silence { mesh } = side.fit(&mut meshes, &stations) else {
            panic!("the first closed loop is the swap");
        };
        assert_eq!(side.mesh.as_ref(), Some(&mesh));
        let built = meshes.get(&mesh).expect("the swap names a live asset");
        assert_eq!(positions(built).len(), stations.len() * 4);
        assert_eq!(built.indices().unwrap().len() / 3, stations.len() * 8);
        assert_eq!(meshes.len(), 1);

        let moved: Vec<Vec2> = stations.iter().map(|p| *p * 1.01).collect();
        for joints in [&moved, &stations] {
            assert_eq!(
                side.fit(&mut meshes, joints),
                ProxyStep::Idle,
                "the swap is a one-shot: re-silencing every frame is a per-frame command flush"
            );
        }
        assert_eq!(
            meshes.len(),
            1,
            "a live ribbon rewrites its own asset in place"
        );
        assert_eq!(
            positions(meshes.get(&mesh).unwrap()),
            positions(&ribbon_mesh(&stations, tiger_section())),
            "the ribbon follows the latest joints"
        );
    }

    /// The side holds its ribbon asset's strong handle, so a fit that finds the asset gone is a
    /// broken invariant and stops rather than keeping the last shape unseen.
    #[test]
    #[should_panic(expected = "a side's ribbon asset lives while the side holds its handle")]
    fn a_ribbon_whose_asset_is_gone_stops() {
        let mut meshes = Assets::<Mesh>::default();
        let mut side = fresh_side();
        let stations = ring(48, 2.0);
        side.fit(&mut meshes, &stations);
        meshes.remove(side.mesh.as_ref().unwrap());
        side.fit(&mut meshes, &ring(48, 2.1));
    }

    /// `build_ribbon` gives the mesh its positions, so a write that finds none is a broken
    /// invariant and stops rather than keeping the last shape unseen.
    #[test]
    #[should_panic(expected = "`build_ribbon` gives the ribbon its positions as Float32x3")]
    fn a_ribbon_without_its_positions_stops() {
        let mut cache = RibbonCache::default();
        let mut mesh = cache.build(&ring(48, 2.0), tiger_section());
        mesh.remove_attribute(Mesh::ATTRIBUTE_POSITION);
        cache.write(&mut mesh, &ring(48, 2.1), tiger_section());
    }
}
