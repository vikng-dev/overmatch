//! The TRACK-LINK RENDER LAYER: the tank's own shoe mesh, instanced onto the belt — shared by the
//! game's [`super::view`] and the sandbox's `track_sandbox::link_view` adapter.
//!
//! Everything upstream of here draws the track as a LINE — the conformed pin line, the reference
//! loop, the cast routes. A line is the right thing to reason about and the wrong thing to look at:
//! you cannot see a shoe overhang a board edge, and you cannot see the belt articulate. This module
//! lays the real 5 552-triangle shoe on the same stations the physics already walks, so the model is
//! judged on actual track. It is the ONE home for that: the game shipped procedural `Cuboid` boxes
//! until 2026-07-26 while the sandbox instanced the real shoe, which is two answers to "what does
//! this tank's track look like".
//!
//! # Where the shoe comes from
//!
//! The Tiger glb ships a TEMPLATE link that is not part of the tank: a `Link` node (whose mesh is
//! named `Link` too — the name-scan ambiguity the node resolution in [`bind_link_template`] guards
//! against) carrying `Pin_Start` / `Pin_End` marker empties and the `Link_Box` volume
//! [`super::marker_model`] measures the shoe's faces from. It sits off to one side of the right
//! sprocket and — because `Link_Box` carries no material — renders as a solid white box beside the
//! tank. [`hide_link_template`] hides both; [`bind_link_template`] reads the mesh, its MATERIAL and
//! the markers off the same nodes and hands them to the consumers' pools. Both systems run in BOTH
//! binaries: the game used to hide neither, so every game session drew a stray white box and a loose
//! shoe parked beside the hull.
//!
//! # The shoe's LOOK is authored, not coded
//!
//! The shoe primitive carries `Mat_Track_Link` — a tiling worn-steel set of MEASURED three maps
//! (albedo, normal, and a packed metalness/roughness) at MEASURED 512² each, with no
//! `metallicFactor` authored so the glTF default of MEASURED 1.0 leaves the packed map in charge.
//! The tiling is baked into the UV COORDINATES in the blend, and that part is load-bearing:
//! `bevy_gltf` 0.19 honours `KHR_texture_transform` only on `base_color_texture` (bevy #15310), so a
//! Blender Mapping node would tile the albedo and leave the other two at DERIVED 1× (the transform
//! is simply dropped, so those maps keep the authored UVs) — visibly wrong, and awkward to diagnose.
//! Scaling the UVs themselves keeps every map in step and needs no extension at all. This module
//! therefore reads a material and never builds one; re-texturing the track is a blend edit plus a
//! re-export.
//!
//! # The SCALE CONTRACT: the template is authored at 1.0, and that is verified, not carried
//!
//! The link used to ship under a 0.8079178 uniform scale, which every consumer had to carry through
//! its placement math. That is a working-around: a number typed into (or threaded through) the
//! renderer to compensate for the asset, invisible to anyone reading either. The artist re-exported
//! with the scale APPLIED, so [`bind_link_template`] now VERIFIES instead — the composed world scale
//! of `Link` and of `Link_Box` must be 1.0 within [`SCALE_TOLERANCE`] — and [`refuse_scaled`]
//! aborts, naming the node and the measured scale, if it is not. Same policy as
//! [`super::marker_model`]'s marker read: a broken export fails loudly at bind time rather than
//! rendering a 23.8 %-oversized track that looks almost right.
//!
//! # The placement rule: anchor on the PINS, never on the link's origin
//!
//! The `Link` node's own origin is an arbitrary geometric centre — nobody placed it meaningfully, and
//! its Y and Z carry no information at all. What IS meaningful is the link's internal geometry, so
//! the canonical frame is derived from the template's own markers ([`canonical_frame`]):
//!
//!   * ORIGIN — the pin midpoint in the radial/longitudinal axes. That is exactly the point the 2-D
//!     route describes: the route's radius is the PIN-LINE radius, so a link whose pin midpoint rides
//!     the route automatically puts its inner face on the wheel tread and hangs its outer face
//!     `pin_to_outer` below. Laterally the pin markers say NOTHING (a pin is a cylinder spanning the
//!     whole shoe; the artist drops the marker anywhere along the bore — the reason
//!     [`super::marker_model`] projects x out of the pitch), so the lateral datum comes from the one
//!     place that measures it: `RigGeom::link_center_x`, the shoe's own centre, ~16.85 mm OUTBOARD
//!     of the pin plane. Anchoring the shoe's centre there reproduces the authored overhang exactly.
//!   * AXES — longitudinal along `Pin_Start - Pin_End`, lateral along model X, the third by cross
//!     product. The template node carries no rotation, so this construction is the identity on the
//!     shipped model: the authored pose IS a valid on-track pose (outer face down, guide horn up),
//!     which is what makes the derivation checkable rather than merely plausible.
//!
//! Per frame each link spans two consecutive stations of the belt the view resampled at the link
//! pitch — the same stations the drawn line uses, and, everywhere it touches ground, the same
//! geometry the physics walks: the lower run's convex envelope, tangents and wrap arcs come from
//! [`super::route`]'s one shared builder for both. Articulation over a washboard and scrolling under
//! drive both fall out for free: the stations articulate, and the resample carries the belt phase.
//!
//! The RETURN run is the one deliberate exception, and it is worth knowing about before chasing a
//! "the shoes don't match the route" bug that is not one. The drawn drape is pushed out of EVERY
//! circle ([`super::route::SagClip::EveryCircle`]) — including the sprocket and the idler it leaves
//! at a tangent point — and the sim route's drape is not. That is not an oversight in either
//! direction: unclipped, the drawn shoes sink tens of millimetres into the end wheels under any
//! slack at all (very visible on a textured link), while clipping the SIM route lengthens it by
//! millimetres the belt never budgeted, and that phantom strain slides a latched 20° hill hold. The
//! view can absorb the extra length (`wrap::station_params` reads the drawn polyline as a uniform
//! strain); the sim cannot.
//!
//! # The entity↔station map ROTATES with the belt phase
//!
//! Joint slots shift material identity by one every pitch of travel (the wrap resamples at
//! `phase mod pitch`), so a FIXED entity↔slot binding makes any per-link identity — damage, a
//! texture, a witness paint — wander one link per pitch. [`place_links`] rotates the mapping by the
//! whole-pitch quotient ([`phase_decompose`]), so entity `m` always wears material link `m` and
//! anything a shoe carries RIDES the belt. It costs one integer per side per frame and it is the
//! difference between "the shoes are identical so it does not matter" and "it is correct".
//!
//! # The left track is a MIRROR, not a copy
//!
//! The shoe is laterally asymmetric (the guide horn, the outboard bias), so translating the right
//! track across the hull reads visibly wrong. The obvious fix — a negative-X scale — is a
//! NEGATIVE-DETERMINANT transform: it reverses triangle winding, so every face is backwards to the
//! rasteriser (backface culling eats the shoe and lights the inside), and it flips the normals'
//! handedness. So the mirror is baked into a SECOND MESH ASSET instead ([`mirrored_mesh`]): positions
//! and normals negated in x, triangle winding reversed to compensate. Both instances then render
//! under ordinary positive-determinant transforms.
//!
//! # Distance LOD: a second shoe entity, not a second pool
//!
//! The shoes are the largest geometry pool a tank owns — the Tiger's MEASURED 97 links × 2 sides ×
//! 518 triangles is ~100 k triangles per tank, and a 15v15 frame holds thirty of them, so the belt
//! alone is ~3.0 M triangles of the scene. Beyond a few tens of metres none of that detail survives
//! rasterisation, so every shoe carries a LOWER-DETAIL SIBLING: the artist's 194-triangle decimation
//! of the same shoe, in the same mesh-local frame, on the same material — ~1.1 M for the same thirty
//! tanks.
//!
//! The switch is bevy's own [`VisibilityRange`] — [`VisibilityRange::abrupt`], deliberately, because
//! a crossfaded range compiles a second dithering permutation of every shoe pipeline for a
//! transition nobody can see at [`SHOE_LOD1_DISTANCE_M`]. The LOD1 instance is spawned as a CHILD of
//! the LOD0 shoe rather than as a second pooled sibling, and three separate mechanisms fall out of
//! that for free:
//!
//!   * **Placement.** [`place_links`] writes ONE transform per shoe and ordinary propagation carries
//!     the child, so the two levels can never disagree about where a link is — and there is no
//!     second placement system to keep in step with the first.
//!   * **Rendering policy.** `render_policy` resolves a mesh against its nearest scoped ANCESTOR, so
//!     the child inherits its tank's channel and, once the shadow ribbon lands and the shoe is
//!     silenced with `VisualScope::PROXIED_CASTER`, inherits the silence too. Nothing here mirrors a
//!     layer or a shadow marker by hand; the one path that used to rewrite layers per mesh under the
//!     controlled tank no longer exists.
//!   * **Lifetime.** `despawn` is recursive over `Children`, so a rig rebind or a sandbox pool
//!     shrink takes the LOD1 shoe with the shoe it belongs to.
//!
//! Sharing the material has one obligation the asset does not discharge: [`LINK_MATERIAL`] is
//! NORMAL-MAPPED, and bevy's PBR shader drops normal mapping — silently — on a mesh with no
//! `ATTRIBUTE_TANGENT`. The LOD1 primitive carries no material of its own, which is exactly the case
//! `bevy_gltf` does NOT auto-generate tangents for, so [`lod1_shoe_meshes`] builds them at bind time
//! and refuses the bind if it cannot. Otherwise the swap would change the LIGHTING as well as the
//! silhouette, and the distance below is argued only about the silhouette.
//!
//! The cost of the pattern is entity count: the pool doubles, and every one of those entities is
//! visited by `check_visibility_ranges` each frame. That is the trade a measurement sweep has to
//! judge — the triangle win is only worth having if it is not eaten by the visibility walk. The
//! probe scenario can stand its 30-tank block on either side of the swap (`OVERMATCH_PROBE_FAR` —
//! see [`crate::tank::scenario::probe_far`]), which is what makes both halves of that measurable.

use bevy::camera::visibility::VisibilityRange;
use bevy::mesh::{GenerateTangentsError, Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

use super::forces::phase_decompose;
use super::rig_geom::RigGeom;
use super::side::{PerSide, Side};

/// Register the template systems. Mounted by BOTH consumers (the game's `view_plugin` and the
/// sandbox's `link_view` adapter) — one binary each, so the systems never double-register.
pub(crate) fn template_plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            // Cheap and always on: an `Added<Name>` scan, so a glb hot-reload re-hides the template
            // it re-instantiates.
            hide_link_template,
            // Latched on its own resource — one read of the template nodes, ever.
            bind_link_template
                .run_if(resource_exists::<RigGeom>)
                .run_if(not(resource_exists::<LinkTemplate>)),
        ),
    );
}

// ---------------------------------------------------------------------------------------------
// The template: one read of the glb's own link
// ---------------------------------------------------------------------------------------------

/// The glb nodes this module reads and hides. `Link` is the shoe — and its MESH carries the same
/// name, which is the ambiguity a name scan has to resolve structurally rather than by string.
/// `Link_Box` is the marker volume [`super::marker_model`] measures the shoe's faces from — it
/// carries NO material, which is why it renders as a solid white box until it is hidden.
const LINK_NODE: &str = "Link";
const LINK_BOX_NODE: &str = "Link_Box";
/// The pin markers, parented under `Link`: the only meaningful datums it carries.
const PIN_START_NODE: &str = "Pin_Start";
const PIN_END_NODE: &str = "Pin_End";
/// The material the blend assigns to the shoe primitive. Named here only so the refusal below can
/// say what is missing — nothing reads it, because the material arrives as an asset handle on the
/// glb's own primitive rather than by name.
const LINK_MATERIAL: &str = "Mat_Track_Link";

/// The LOW-DETAIL shoe, exported beside the tank as its own glb: one node, one primitive, no
/// material and no markers. It is loaded as a labelled PRIMITIVE rather than a scene, so nothing is
/// instantiated and nothing needs hiding — and the look keeps coming from the template's own
/// [`LINK_MATERIAL`], which is what makes the swap invisible in the first place.
const LINK_LOD1_GLB: &str = "tiger_1/tiger_1_link.lod1.glb";

/// Where a shoe drops to its low-detail sibling, metres from the camera. The ONE number this LOD
/// has, and it is set by the GUNNER OPTIC rather than by the third-person view.
///
/// # Why the optic sets it
///
/// There is exactly one `Camera3d` in the game: the optic is that camera at the tank's authored
/// gunner FOV (0.12 rad on the Tiger) instead of the commander's ~0.785 rad, so it magnifies ~6.5×.
/// bevy's range table IS per-view (`VisibleEntityRanges` is keyed by camera entity), but with one
/// camera there is no second view to give a different range to, and making the range track the
/// current FOV would mean rewriting a `VisibilityRange` on every shoe entity in the world each time
/// the player raises the sight — the exact O(tanks × meshes) view switch `render_policy` exists to
/// have deleted. So one distance serves both views, and it is chosen for the demanding one.
///
/// # Where the number comes from
///
/// MEASURED point-to-surface deviation of the shipped `tiger_1_link.lod1.glb` from the full shoe:
/// median 7.1 mm, p90 26.9 mm, worst 75.0 mm (the guide-horn tip and the outer connector boss). At
/// 500 m through a 0.12 rad optic on a 1440-px-tall view — 8.3e-5 rad/px — those become 0.16 px,
/// 0.65 px and 1.8 px, against a shoe that is itself only ~3 px long there. The swap is therefore
/// below the pixel grid in the view that can see furthest, which is also why an ABRUPT range needs
/// no hysteresis: a tank loitering exactly on the boundary flips between two silhouettes that differ
/// by well under a pixel.
///
/// The commander view would tolerate ~150 m by the same arithmetic (0.785 rad over 1440 px puts the
/// 75 mm worst case at one pixel by 137 m), and a smaller number here would buy a great deal more:
/// most of a battlefield sits inside 500 m. That is a QUALITY decision, not a code one — the lever
/// is this constant and the evidence is the paragraph above.
pub(crate) const SHOE_LOD1_DISTANCE_M: f32 = 500.0;

/// How far the template's composed world scale may sit from 1.0 before the bind refuses. A hair for
/// the `f32` round trip through the export, and four orders of magnitude below the 0.808 the model
/// used to carry — so it catches a re-export that forgets to apply the scale, and nothing else.
const SCALE_TOLERANCE: f32 = 1e-3;

/// Everything needed to instance one shoe, read once off the template.
#[derive(Resource)]
pub(crate) struct LinkTemplate {
    /// Per side: the authored shoe on the right, its genuine mirror on the left (see the module
    /// doc — a negative-X scale would be a winding flip, not a mirror).
    mesh: PerSide<Handle<Mesh>>,
    /// Per side: the same shoe at LOD1, mirrored by the same construction. ONE handle per side for
    /// the whole session — every link instance clones it, so a Tiger's 194 low-detail shoes are 194
    /// references to two mesh assets.
    lod1: PerSide<Handle<Mesh>>,
    /// One material for every link, read off the glb's own shoe primitive — the artist's
    /// [`LINK_MATERIAL`], never a `StandardMaterial` built here. The look is therefore changed by
    /// re-exporting the blend, not by editing this file.
    material: Handle<StandardMaterial>,
    /// Per side: mesh space → the canonical pin frame.
    frame: PerSide<LinkFrame>,
}

impl LinkTemplate {
    /// This side's mesh→canonical-frame correction, `Copy` so a consumer can capture it at bind and
    /// never touch the template again in its hot loop.
    pub(crate) fn frame(&self, side: Side) -> LinkFrame {
        *self.frame.get(side)
    }
}

/// The correction that turns the template's arbitrary origin into a frame you can place: a rotation
/// that maps MESH axes onto the canonical (lateral, inner, longitudinal) = (x, y, z) triple, and the
/// mesh-local point that becomes the frame's origin.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LinkFrame {
    /// Mesh → canonical rotation (the inverse of the canonical basis expressed in mesh space).
    correction: Quat,
    /// The anchor in MESH-LOCAL coordinates: the pin midpoint radially and longitudinally, the
    /// shoe's own lateral centre laterally (the pins cannot answer laterally — see the module doc).
    origin: Vec3,
}

/// Marker on a pooled link instance, so a consumer's `Transform`/`Visibility` query cannot reach
/// anything else — and so the sandbox's `mesh_layers` mesh tagger can exclude the shoe pool (the
/// instances are nameless children of the hull, and without this marker they would fall through to
/// the hull layer and fight the `links` switch for their visibility).
///
/// BOTH detail levels wear it. The consumers only ever reach a link by an entity id they were
/// handed ([`place_links`] calls back with pool entities), so widening the marker costs them
/// nothing — while a LOD1 child WITHOUT it would be a nameless hull descendant, exactly the case
/// the tagger's exclusion exists for.
#[derive(Component)]
pub(crate) struct TrackLink;

/// Hide the glb's template link the moment it appears.
///
/// `Added<Name>` rather than a one-shot latch: hiding is idempotent and costs a filtered scan of
/// newly-named entities, so a hot-reload that re-instantiates the scene re-hides the fresh copy
/// instead of leaving the white marker box floating beside the sprocket again. Hiding `Link` also
/// covers everything under it (`Link_Box` is its child) — the box is named anyway, because "hide the
/// template" should not depend on the artist's parenting.
fn hide_link_template(mut commands: Commands, fresh: Query<(Entity, &Name), Added<Name>>) {
    for (entity, name) in &fresh {
        if matches!(name.as_str(), LINK_NODE | LINK_BOX_NODE) {
            commands.entity(entity).insert(Visibility::Hidden);
        }
    }
}

/// Read the template ONCE: the shoe mesh and the two pin markers — then verify the scale contract,
/// build the mirrored mesh, the shared material, and both sides' canonical frames.
///
/// It retries every frame until the glb scene has landed (the scene load is async and the rig build
/// does not wait for it), and latches on inserting [`LinkTemplate`]. The name scan is global rather
/// than a walk from a root because `Link` is a SCENE ROOT — a sibling of the hull node, not a
/// descendant of it — and both tools spawn exactly one template.
fn bind_link_template(
    mut commands: Commands,
    named: Query<(Entity, &Name)>,
    parents: Query<&ChildOf>,
    transforms: Query<&Transform>,
    children: Query<&Children>,
    meshes_of: Query<&Mesh3d>,
    materials_of: Query<&MeshMaterial3d<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    // The LOD1 shoe is an ASSET LOAD rather than a scene read: it comes from its own glb
    // ([`LINK_LOD1_GLB`]), so there is no node to find and nothing to hide.
    asset_server: Res<AssetServer>,
    // Held across the bind's retries — a handle dropped and re-taken every frame would keep
    // cancelling and restarting the load it is waiting on.
    mut lod1_handle: Local<Option<Handle<Mesh>>>,
    geom: Res<RigGeom>,
) {
    let (mut link_box, mut pin_start, mut pin_end) = (None, None, None);
    for (entity, name) in &named {
        match name.as_str() {
            LINK_BOX_NODE => link_box = Some(entity),
            PIN_START_NODE => pin_start = Some(entity),
            PIN_END_NODE => pin_end = Some(entity),
            _ => {}
        }
    }
    let (Some(pin_start), Some(pin_end)) = (pin_start, pin_end) else {
        return;
    };
    // The `Link` NODE is resolved structurally — the pin markers' nearest ancestor wearing the
    // name — never by the name alone: bevy names a mesh PRIMITIVE child after its node too, so a
    // scene instance carries TWO entities named `Link` (the node, and its Mesh3d child). Which one
    // a global name scan lands on is spawn-order luck; the game composition landed on the mesh
    // child (childless), so the bind starved forever while the sandbox happened to work. Anchoring
    // on the pins also keeps the whole read within ONE instance when several tanks are live.
    let mut link = None;
    let mut current = pin_start;
    while let Ok(child_of) = parents.get(current) {
        current = child_of.parent();
        if named
            .get(current)
            .is_ok_and(|(_, n)| n.as_str() == LINK_NODE)
        {
            link = Some(current);
            break;
        }
    }
    let Some(link_entity) = link else {
        warn_once!(
            "track links: pin markers present but no `{LINK_NODE}` ancestor — template not bound"
        );
        return;
    };
    let (Ok(pin_start), Ok(pin_end)) = (transforms.get(pin_start), transforms.get(pin_end)) else {
        return;
    };
    let (pin_start, pin_end) = (pin_start.translation, pin_end.translation);

    // THE SCALE CONTRACT, checked before anything is built from these nodes: the template must be
    // authored at unit scale, so the placement math below is the geometry and nothing else.
    verify_unit_scale(
        LINK_NODE,
        composed_scale(link_entity, &parents, &transforms),
    );
    if let Some(box_entity) = link_box {
        verify_unit_scale(
            LINK_BOX_NODE,
            composed_scale(box_entity, &parents, &transforms),
        );
    }

    // The shoe mesh hangs on a PRIMITIVE child of the node (bevy_gltf spawns one child per
    // primitive; the game's world-serialized instance names that child `Link` too — see the node
    // resolution above). Direct children only, then the node itself: `Link_Box` is also a child of
    // `Link` and carries its own primitive one level further down, so a descendant search could
    // pick up the marker box instead.
    let Some((shoe_entity, source)) = children
        .get(link_entity)
        .ok()
        .into_iter()
        .flatten()
        .find_map(|&child| meshes_of.get(child).ok().map(|mesh| (child, mesh)))
        .or_else(|| {
            meshes_of
                .get(link_entity)
                .ok()
                .map(|mesh| (link_entity, mesh))
        })
    else {
        return;
    };
    let Some(shoe) = meshes.get(&source.0) else {
        return;
    };

    // The look is the ARTIST'S, read off the same primitive the mesh came from. A shoe that arrives
    // without a material would render default-white on all 194 instances, and a fallback built here
    // would silently re-introduce the code-side steel this module just stopped owning — so a broken
    // export refuses at bind time, the same policy as the scale contract above.
    let Ok(material) = materials_of.get(shoe_entity) else {
        error_once!(
            "track links: the `{LINK_NODE}` shoe primitive carries no material - re-export the glb \
             with `{LINK_MATERIAL}` assigned to it; refusing to bind"
        );
        return;
    };
    let triangles = shoe.indices().map_or(0, Indices::len) / 3;
    let mirrored = mirrored_mesh(shoe);

    // The LOD1 shoe, tangent-generated and mirrored by [`lod1_shoe_meshes`] — the two levels have
    // to be reflections of each other for the same reason the two sides do, and reusing
    // `mirrored_mesh` is what keeps that true without a second answer to "how is a shoe mirrored".
    let lod1_source = lod1_handle
        .get_or_insert_with(|| {
            asset_server.load(
                GltfAssetLabel::Primitive {
                    mesh: 0,
                    primitive: 0,
                }
                .from_asset(LINK_LOD1_GLB),
            )
        })
        .clone();
    if let bevy::asset::LoadState::Failed(err) = asset_server.load_state(&lod1_source) {
        // Loud rather than a silent starve: without this the whole track — LOD0 included — simply
        // never binds and the tank drives on invisible shoes, which reads as a placement bug.
        error_once!(
            "track links: the LOD1 shoe `{LINK_LOD1_GLB}` failed to load ({err}) — refusing to bind"
        );
        return;
    }
    let Some(lod1_shoe) = meshes.get(&lod1_source) else {
        return;
    };
    let lod1_triangles = lod1_shoe.indices().map_or(0, Indices::len) / 3;
    let lod1_pair = match lod1_shoe_meshes(lod1_shoe) {
        Ok(pair) => pair,
        Err(err) => {
            // Same policy as the missing-material refusal above, and for the same reason: the LOD1
            // shoe wears the LOD0 shoe's NORMAL-MAPPED material, and bevy's PBR shader silently
            // skips normal mapping on a mesh with no tangents. Binding an untangented LOD1 would
            // therefore not fail — it would ship a 500 m line across the battlefield where every
            // track flattens out, which is exactly the kind of lighting bug nobody traces back to a
            // vertex attribute.
            error_once!(
                "track links: the LOD1 shoe `{LINK_LOD1_GLB}` cannot be given tangents ({err}) — \
                 it renders under the normal-mapped `{LINK_MATERIAL}` and would light flat without \
                 them; refusing to bind"
            );
            return;
        }
    };

    let mesh = PerSide::new(meshes.add(mirrored), source.0.clone());
    // BOTH sides are derived assets here, unlike LOD0 (whose right side is the glTF's own handle):
    // the tangents are built in this process, so neither side is the loaded primitive any more.
    // `lod1_source` stays alive in the `Local` handle, which is what keeps the load from unloading.
    let lod1 = lod1_pair.map(|shoe| meshes.add(shoe));

    // The one lateral datum the markers cannot carry: how far outboard of the PIN PLANE the shoe's
    // centre is authored. `RigGeom` measures both (off `Link_Box` and off the pin markers), so this
    // stays a difference of two measurements rather than a number typed in here — and with the
    // template at unit scale it is already in the mesh's own units.
    let shoe_offset = geom.link_center_x(Side::Right) - geom.plane_x;
    let frame = frames(pin_start, pin_end, shoe_offset);

    info!(
        "track links: template bound - pitch {:.5} m, shoe {:.1} mm outboard of the pin plane, {} \
         triangles/shoe",
        (pin_start - pin_end).with_x(0.0).length(),
        shoe_offset * 1000.0,
        triangles,
    );
    // The LOD's whole ledger on one line: how much geometry the far shoe saves, and the two mesh
    // assets every low-detail instance in the session shares (per side — the left is the mirror).
    info!(
        "track links: LOD1 bound - {lod1_triangles} triangles/shoe (−{}%) beyond \
         {SHOE_LOD1_DISTANCE_M:.0} m, one mesh per side L {:?} R {:?}",
        100 - (lod1_triangles * 100)
            .checked_div(triangles.max(1))
            .unwrap_or(0),
        lod1.get(Side::Left).id(),
        lod1.get(Side::Right).id(),
    );

    commands.insert_resource(LinkTemplate {
        mesh,
        lod1,
        material: material.0.clone(),
        frame,
    });
}

// ---------------------------------------------------------------------------------------------
// The scale contract
// ---------------------------------------------------------------------------------------------

/// The composed world scale of `entity` — the per-axis length of its full parent-chain affine's
/// basis columns.
///
/// Walked up the `ChildOf` chain rather than read off a `GlobalTransform`: global transforms are
/// propagated at the END of the frame, so on the frame a scene instantiates they are all still
/// identity, and a check that trusted them would pass a scaled node by default.
fn composed_scale(
    entity: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&Transform>,
) -> Vec3 {
    let local = |e: Entity| {
        transforms
            .get(e)
            .map_or(bevy::math::Affine3A::IDENTITY, Transform::compute_affine)
    };
    let mut affine = local(entity);
    let mut current = entity;
    while let Ok(parent) = parents.get(current) {
        current = parent.parent();
        affine = local(current) * affine;
    }
    Vec3::new(
        Vec3::from(affine.matrix3.x_axis).length(),
        Vec3::from(affine.matrix3.y_axis).length(),
        Vec3::from(affine.matrix3.z_axis).length(),
    )
}

/// Enforce the scale contract on one template node, or [`refuse_scaled`].
fn verify_unit_scale(node: &str, scale: Vec3) {
    if (scale - Vec3::ONE).abs().max_element() > SCALE_TOLERANCE {
        refuse_scaled(node, scale);
    }
}

/// Abort because a track-template node is not authored at unit scale.
///
/// **Why a panic.** A carried scale is the shape of bug that hides: the track renders, it looks
/// almost right, and it is 23.8 % oversized — the exact failure the old `LinkTemplate.scale`
/// plumbing existed to paper over, in every consumer, forever. The rule (Yan, 2026-07-26) is that
/// scale is never worked around: either the export applies it or the tool says so, once, with the
/// number and the fix in the message. Same policy as [`super::marker_model`]'s marker read, which
/// already aborts both binaries on a broken export.
fn refuse_scaled(node: &str, scale: Vec3) -> ! {
    panic!(
        "track links: the template node `{node}` carries a composed world scale of \
         ({:.7}, {:.7}, {:.7}) — it must be exactly 1.0 (within {SCALE_TOLERANCE}).\n\
         \n\
         The shoe is instanced straight from this node's mesh and placed on the belt by measured \
         geometry alone, so a scale anywhere in its chain would silently render the whole track at \
         the wrong size. Nothing here compensates for one on purpose: carrying it is how a 23.8 % \
         oversized track shipped looking almost correct.\n\
         \n\
         Fix it in the asset: re-export with the link's scale APPLIED (Blender: select `Link` and \
         its children, Object → Apply → Scale), so `{node}` and everything above it carry unit \
         scale.",
        scale.x, scale.y, scale.z,
    )
}

// ---------------------------------------------------------------------------------------------
// The canonical frame
// ---------------------------------------------------------------------------------------------

/// Reflection across the hull's lateral mid-plane — the ONE operation the left track differs by.
fn mirror(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, v.y, v.z)
}

/// Both sides' frames from the template's markers. `shoe_offset` is the shoe centre's outboard bias
/// from the pin plane, in metres (the template is unit-scaled, so mesh units ARE metres).
///
/// The left frame is built from MIRRORED markers rather than by negating something afterwards: run
/// the identical construction on the mirrored template and the result is the exact conjugate
/// `M·R·M` of the right one, which is what makes the two tracks true mirror images of each other.
fn frames(pin_start: Vec3, pin_end: Vec3, shoe_offset: f32) -> PerSide<LinkFrame> {
    PerSide::new(
        canonical_frame(mirror(pin_start), mirror(pin_end), -shoe_offset),
        canonical_frame(pin_start, pin_end, shoe_offset),
    )
}

/// The canonical link frame of one template: mesh axes → (lateral, inner, longitudinal) = (x, y, z),
/// with the origin on the pin line.
///
/// The axis triple is built so that it is RIGHT-HANDED by construction (`lat × inner = long`), which
/// is what keeps the resulting instance transform a proper rotation. The longitudinal sense is
/// `Pin_Start - Pin_End`, which on the shipped Tiger is +z — i.e. the identity, matching the fact
/// that the template node carries no rotation, so the authored pose is already a legal on-track pose
/// (outer face `-y` down, guide horn `+y` up).
fn canonical_frame(pin_start: Vec3, pin_end: Vec3, shoe_offset: f32) -> LinkFrame {
    let long = (pin_start - pin_end).normalize_or(Vec3::Z);
    // Model X, orthogonalised against the pin axis: the pins are never exactly perpendicular to it
    // (0.24 mm of lateral drift on the Tiger), and an un-orthogonalised basis is not a rotation.
    let lat = (Vec3::X - long * Vec3::X.dot(long)).normalize_or(Vec3::X);
    let inner = long.cross(lat);
    let basis = Mat3::from_cols(lat, inner, long);
    let pin_mid = (pin_start + pin_end) * 0.5;
    LinkFrame {
        correction: Quat::from_mat3(&basis).inverse(),
        // Radially and longitudinally the pin midpoint IS the anchor (the route is the pin line).
        // Laterally it is not an anchor at all — the marker's x is an arbitrary point along the pin
        // bore — so the shoe's own measured centre takes over.
        origin: pin_mid + Vec3::X * shoe_offset,
    }
}

/// Hull-local transform of one link spanning side-plane stations `a -> b`.
///
/// `a`/`b` are side-plane `(z, y)` — the belt line's own coordinates — and `lateral_x` is the shoe
/// centre's hull-local `x` on this side. The rotation is the route's own tangent angle
/// (`from_rotation_x(-atan2)` maps local `+z` onto the chord) composed with the template correction,
/// so the whole "where is this mesh's nose" question is answered once at build time and never in the
/// hot loop.
fn link_transform(frame: &LinkFrame, lateral_x: f32, a: Vec2, b: Vec2) -> Transform {
    let chord = b - a;
    let rotation = Quat::from_rotation_x(-chord.y.atan2(chord.x)) * frame.correction;
    let mid = (a + b) * 0.5;
    let anchor = Vec3::new(lateral_x, mid.y, mid.x);
    Transform {
        // The anchor is where the frame's ORIGIN must land, and the mesh carries that origin
        // unscaled before rotation — so the entity's translation backs it out.
        translation: anchor - rotation * frame.origin,
        rotation,
        scale: Vec3::ONE,
    }
}

/// The genuine mirror of a mesh across x: positions and normals negated, triangle winding reversed.
///
/// Both halves are load-bearing. Negating x alone leaves every triangle wound backwards (a
/// reflection reverses orientation), so backface culling would eat the shoe and lighting would read
/// from the inside; reversing the winding alone would mirror nothing. Doing it in the ASSET is what
/// keeps the instance transform positive-determinant — a `Vec3::new(-1, 1, 1)` scale on the entity
/// would push exactly this winding flip into the renderer, where it cannot be fixed.
fn mirrored_mesh(source: &Mesh) -> Mesh {
    let mut mesh = source.clone();
    for attribute in [Mesh::ATTRIBUTE_POSITION, Mesh::ATTRIBUTE_NORMAL] {
        if let Some(VertexAttributeValues::Float32x3(values)) = mesh.attribute_mut(attribute) {
            for v in values.iter_mut() {
                v[0] = -v[0];
            }
        }
    }
    // Tangents, and this branch is LIVE for both levels of the shoe (the glb ships none, but the
    // shoe's material is normal-mapped, so LOD0 gets them from `bevy_gltf`'s mikktspace pass at load
    // and LOD1 from [`lod1_shoe_meshes`]). A tangent is `dP/du` with `w` recording the bitangent's
    // handedness, so under the reflection `M = diag(-1, 1, 1)` — which leaves the UVs untouched —
    // `T` transforms exactly like a position, `T' = M T`. The `w` flip is what keeps the BITANGENT
    // a reflection too: `B = (N × T) w`, and a determinant-(−1) map obeys `M a × M b = −M (a × b)`,
    // so `(N' × T') w' = −M (N × T) w'` equals `M B` only when `w' = −w`. Miss it and every
    // normal-mapped detail on the left track lights as if lit from the opposite side.
    if let Some(VertexAttributeValues::Float32x4(values)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_TANGENT)
    {
        for v in values.iter_mut() {
            v[0] = -v[0];
            v[3] = -v[3];
        }
    }
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        warn!("track links: mirror skipped - the shoe mesh is not a triangle list");
        return mesh;
    }
    match mesh.indices_mut() {
        Some(Indices::U16(indices)) => {
            for tri in indices.chunks_exact_mut(3) {
                tri.swap(1, 2);
            }
        }
        Some(Indices::U32(indices)) => {
            for tri in indices.chunks_exact_mut(3) {
                tri.swap(1, 2);
            }
        }
        // An un-indexed soup: give it the index buffer that reverses each triangle in place, which
        // is the same reversal without touching (and re-permuting) every vertex attribute.
        None => {
            let count = mesh.count_vertices() as u32;
            mesh.insert_indices(Indices::U32(
                (0..count)
                    .map(|i| i / 3 * 3 + [0, 2, 1][(i % 3) as usize])
                    .collect(),
            ));
        }
    }
    mesh
}

/// The LOD1 shoe as the two mesh assets the pool binds — `[left, right]` — both carrying TANGENTS.
///
/// # Why the tangents are BUILT here rather than read
///
/// The LOD1 primitive ships POSITION, NORMAL and TEXCOORD_0 and nothing else, and — being a bare
/// decimation with no look of its own — it carries NO glTF MATERIAL. `bevy_gltf` 0.19 runs its
/// mikktspace pass only when the primitive's OWN material wants tangents (`needs_tangents`: a normal
/// texture, or a clearcoat normal texture), and a material-free primitive resolves to the glTF
/// default material, which wants nothing. So the loaded LOD1 mesh has no `ATTRIBUTE_TANGENT`.
///
/// That would be harmless if the shoe were unlit steel, but the LOD1 instance renders under the LOD0
/// shoe's [`LINK_MATERIAL`], whose three MEASURED maps include a NORMAL map. bevy's PBR shader keys
/// normal mapping on the `VERTEX_TANGENTS` shader def and simply drops the map when the mesh has no
/// tangents — no warning, no error. The swap at [`SHOE_LOD1_DISTANCE_M`] would then change the
/// LIGHTING as well as the silhouette, which is not what the distance was argued from.
///
/// # Generate BEFORE mirroring, never after
///
/// mikktspace is run ONCE, on the shoe as authored, and [`mirrored_mesh`] carries the result across
/// the reflection analytically (`T' = M T`, `w' = −w` — the algebra is written out there). Doing it
/// the other way round means a second mikktspace run over a reflected, rewound mesh, trusting it to
/// re-derive the handedness the reflection implies; this way the handedness is a two-line identity
/// that a test can check per vertex, it costs half the work, and there is still exactly ONE answer
/// to "how is a shoe mirrored". A re-export that starts shipping tangents (or a material) is honored
/// as-is: the generation is skipped and the artist's tangents are the ones that get mirrored.
fn lod1_shoe_meshes(source: &Mesh) -> Result<PerSide<Mesh>, GenerateTangentsError> {
    let mut right = source.clone();
    if !right.contains_attribute(Mesh::ATTRIBUTE_TANGENT) {
        right.generate_tangents()?;
    }
    let left = mirrored_mesh(&right);
    Ok(PerSide::new(left, right))
}

// ---------------------------------------------------------------------------------------------
// The pool
// ---------------------------------------------------------------------------------------------

/// Spawn one pooled shoe on `side`, parented to `parent` (a hull-local frame in both consumers),
/// together with its LOD1 child. The returned entity is the LOD0 shoe — the one the pool holds and
/// the one [`place_links`] poses.
///
/// Persistent entities whose transforms are rewritten each frame — never rebuilt meshes and never
/// immediate-mode gizmos: at ~97 links × 2 sides × 5 552 triangles a per-frame rebuild would be
/// ~1.1 M triangles of CPU work every frame, while identical mesh+material instances batch into two
/// draws. Parked below the world until the first placement writes a real pose (the spawn lands via
/// `Commands`, so the placer only sees it next frame, and an un-posed link must not flash on screen).
///
/// The two levels' ranges MEET at [`SHOE_LOD1_DISTANCE_M`] — `[0, D)` and `[D, ∞)` — so exactly one
/// of them is drawn at every distance, in every view, with no gap and no double-draw. The child
/// carries the same [`TrackLink`] marker as its parent: it is a pooled shoe by every rule that
/// matters to the consumers (the sandbox's mesh tagger excludes the pool by that marker, and would
/// otherwise class a nameless hull descendant as hull geometry and repaint it under x-ray).
pub(crate) fn spawn_link(
    commands: &mut Commands,
    template: &LinkTemplate,
    side: Side,
    parent: Entity,
) -> Entity {
    commands
        .spawn((
            TrackLink,
            Mesh3d(template.mesh.get(side).clone()),
            MeshMaterial3d(template.material.clone()),
            VisibilityRange::abrupt(0.0, SHOE_LOD1_DISTANCE_M),
            Transform::from_xyz(0.0, -1000.0, 0.0),
            ChildOf(parent),
        ))
        .with_child((
            TrackLink,
            Mesh3d(template.lod1.get(side).clone()),
            // The SAME material: the two levels differ in triangles and in nothing else, and a
            // second material would put a second batch (and a visible shading seam) on the swap.
            MeshMaterial3d(template.material.clone()),
            VisibilityRange::abrupt(SHOE_LOD1_DISTANCE_M, f32::INFINITY),
            // IDENTITY, and load-bearing: the LOD1 mesh is authored in the same mesh-local frame as
            // the full shoe, so riding its parent's transform puts it exactly where the shoe was.
            // It is also what makes the range test agree — both entities resolve to the same world
            // origin, so they can never both be in (or both out of) range on the same frame.
            Transform::IDENTITY,
        ))
        .id()
}

/// Place one side's pool on this frame's belt stations.
///
/// `stations` are the drawn pin joints in side-plane `(z, y)`, in loop order: link `i` spans stations
/// `i` and `i+1`, the last wrapping to the first, so `count` stations carry exactly `count` links.
/// `lateral_x` is this side's `RigGeom::link_center_x`.
///
/// The entity↔station map ROTATES with the belt phase (see the module doc): station `i` is worn by
/// entity `(i − q) mod n`, `q` the whole-pitch quotient of the phase, so a shoe's identity rides the
/// belt instead of wandering one link per pitch.
///
/// `pose` is called once per pooled entity: `Some(transform)` for a slot that has a station this
/// frame, `None` for one that does not (a pool that briefly outruns the station list — the frame a
/// link-count bump spawns entities the belt has not resampled for yet — so the caller can hide the
/// stragglers rather than leave a stale link hanging in the air). A degenerate chord is skipped
/// entirely, leaving that entity's previous pose alone.
pub(crate) fn place_links(
    frame: &LinkFrame,
    lateral_x: f32,
    stations: &[Vec2],
    phase: f64,
    pitch: f32,
    links: &[Entity],
    mut pose: impl FnMut(Entity, Option<Transform>),
) {
    let n = links.len();
    if n == 0 {
        return;
    }
    // The whole-pitch quotient from the canonical decomposition (the offset half is the wrap's own
    // sampling concern) — one home for the phase arithmetic.
    let q = if pitch > 1e-6 {
        phase_decompose(phase, pitch).0
    } else {
        0
    };
    let slot = |i: usize| links[(i as i64 - q).rem_euclid(n as i64) as usize];

    let count = stations.len();
    let m = if count < 3 { 0 } else { count.min(n) };
    for i in 0..m {
        let (a, b) = (stations[i], stations[(i + 1) % count]);
        if a.distance_squared(b) < 1e-8 {
            continue;
        }
        pose(slot(i), Some(link_transform(frame, lateral_x, a, b)));
    }
    // The rotation is a bijection over `0..n`, so the slots with no station this frame are exactly
    // the images of `m..n` — no bookkeeping and no allocation to find them.
    for i in m..n {
        pose(slot(i), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A FIXTURE template, re-measured from `tiger_1.glb` on 2026-07-26 (the export that APPLIED the
    /// link scale): the two pin markers in `Link`-local space — which, the node now carrying unit
    /// scale, are also metres — and the shoe centre's outboard bias from the pin plane
    /// (`link_center_x - plane_x`).
    ///
    /// These are INPUTS to the pure placement math, not assertions about the shipped file. A
    /// re-export that moves a marker changes what the tools draw, and must not turn this suite red:
    /// the geometry the model actually carries is `marker_model`'s to pin, and every number below is
    /// read live at runtime ([`bind_link_template`]).
    const PIN_START: Vec3 = Vec3::new(0.017_796_993, -0.026_107_691, 0.058_478_355);
    const PIN_END: Vec3 = Vec3::new(0.018_031_836, -0.026_122_428, -0.071_948_05);
    const SHOE_OUTBOARD: f32 = 0.016_85;
    /// The Tiger's `link_center_x` — the lateral datum a right-side shoe is anchored on.
    const LATERAL_X: f32 = 1.548;

    fn tiger_frames() -> PerSide<LinkFrame> {
        frames(PIN_START, PIN_END, SHOE_OUTBOARD)
    }

    /// A template whose four mesh handles are all DISTINCT, so a test can tell which one landed on
    /// which entity. Built from a bare `Assets<Mesh>` rather than an `AssetPlugin` app: the only
    /// thing under test is which handle `spawn_link` clones where.
    fn fixture_template(assets: &mut Assets<Mesh>) -> LinkTemplate {
        let mut fresh = || {
            assets.add(Mesh::new(
                PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            ))
        };
        LinkTemplate {
            mesh: PerSide::new(fresh(), fresh()),
            lod1: PerSide::new(fresh(), fresh()),
            material: Handle::default(),
            frame: tiger_frames(),
        }
    }

    /// Run `spawn_link` against a real `World` and hand back the pooled shoe and its child.
    fn spawn_pair(world: &mut World, template: &LinkTemplate, side: Side) -> (Entity, Entity) {
        let parent = world.spawn_empty().id();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        let link = spawn_link(&mut commands, template, side, parent);
        queue.apply(world);
        let children = world
            .entity(link)
            .get::<Children>()
            .expect("a shoe carries its LOD1 sibling");
        assert_eq!(
            children.len(),
            1,
            "exactly one sibling, never a second pool"
        );
        (link, children[0])
    }

    /// The LOD pair: two entities, two DIFFERENT meshes, one material, and ranges that meet.
    ///
    /// The handle assertion is the one that matters for cost — 388 shoes on a tank must be 388
    /// references to the template's two LOD1 assets, not 388 meshes — and the range assertion is
    /// what makes the pair a level-of-detail rather than a double-draw.
    #[test]
    fn a_shoe_and_its_lod1_sibling_split_the_distance_between_them() {
        let mut assets = Assets::<Mesh>::default();
        let template = fixture_template(&mut assets);
        let mut world = World::new();

        for side in Side::ALL {
            let (link, lod1) = spawn_pair(&mut world, &template, side);
            let mesh_of = |e: Entity| {
                world
                    .entity(e)
                    .get::<Mesh3d>()
                    .expect("a shoe is a mesh")
                    .0
                    .id()
            };
            assert_eq!(mesh_of(link), template.mesh.get(side).id());
            assert_eq!(mesh_of(lod1), template.lod1.get(side).id());
            assert_ne!(
                mesh_of(link),
                mesh_of(lod1),
                "the two levels must be two meshes — a shared handle is not an LOD",
            );
            // The look is the artist's, once: a second material would mean a second batch and a
            // shading seam on the swap.
            assert_eq!(
                world
                    .entity(lod1)
                    .get::<MeshMaterial3d<StandardMaterial>>()
                    .map(|m| m.0.id()),
                world
                    .entity(link)
                    .get::<MeshMaterial3d<StandardMaterial>>()
                    .map(|m| m.0.id()),
            );
            // Coincident: the child rides its parent's transform, which is what lets one placement
            // write serve both levels AND what makes the two range tests agree.
            assert_eq!(
                world.entity(lod1).get::<Transform>().copied(),
                Some(Transform::IDENTITY),
            );

            let range = |e: Entity| {
                world
                    .entity(e)
                    .get::<VisibilityRange>()
                    .cloned()
                    .expect("both levels are range-gated")
            };
            let (near, far) = (range(link), range(lod1));
            assert!(
                near.is_abrupt() && far.is_abrupt(),
                "abrupt, so no shoe pipeline grows a crossfade permutation for a swap nobody sees",
            );
            assert_eq!(near.start_margin.start, 0.0);
            assert_eq!(near.end_margin.end, SHOE_LOD1_DISTANCE_M);
            assert_eq!(far.start_margin.start, SHOE_LOD1_DISTANCE_M);
            assert!(far.end_margin.end.is_infinite(), "the far level never ends");

            // COMPLEMENTARY: exactly one level is drawn at every distance — no gap where the track
            // vanishes, no band where both are submitted.
            for d in [
                0.0,
                1.0,
                120.0,
                SHOE_LOD1_DISTANCE_M - 0.01,
                SHOE_LOD1_DISTANCE_M,
                2_500.0,
            ] {
                assert_ne!(
                    near.is_visible_at_all(d),
                    far.is_visible_at_all(d),
                    "at {d} m exactly one level must be visible",
                );
            }
        }
    }

    /// The LOD1 sibling inherits the CASTER SWAP. When the shadow ribbon lands, `drive_track_views`
    /// writes `VisualScope::PROXIED_CASTER` onto the pooled shoe and onto nothing else; the child
    /// must go quiet with it, or a tank beyond [`SHOE_LOD1_DISTANCE_M`] would cast its whole belt
    /// through the shadow map that the ribbon was built to replace.
    ///
    /// Asserted through `render_policy`'s own resolver rather than by reading a marker off the
    /// spawn, because inheritance is exactly the mechanism being relied on: nothing in this module
    /// writes a shadow marker or a layer, and this is the test that says so.
    #[test]
    fn silencing_a_shoe_silences_its_lod1_sibling() {
        use crate::render_policy::{VisualScope, casts_shadow};

        let mut assets = Assets::<Mesh>::default();
        let template = fixture_template(&mut assets);
        let mut app = App::new();
        app.add_plugins(crate::render_policy::plugin);

        let world = app.world_mut();
        let tank = world.spawn(VisualScope::WORLD_SOLID).id();
        let (link, lod1) = {
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            let link = spawn_link(&mut commands, &template, Side::Right, tank);
            queue.apply(world);
            let child = world.entity(link).get::<Children>().expect("a sibling")[0];
            (link, child)
        };
        app.update();
        assert!(
            casts_shadow(app.world(), link) && casts_shadow(app.world(), lod1),
            "before the ribbon exists BOTH levels carry the belt's shadow, exactly as one shoe did",
        );

        app.world_mut()
            .entity_mut(link)
            .insert(VisualScope::PROXIED_CASTER);
        app.update();
        assert!(
            !casts_shadow(app.world(), link),
            "the swap silences the shoe it is written on",
        );
        assert!(
            !casts_shadow(app.world(), lod1),
            "and its LOD1 sibling, which is written on nothing at all",
        );
    }

    /// The template's authored pose is a LEGAL on-track pose: `Link` carries no rotation, so a link
    /// laid on a level, front→rear chord must come out unrotated — outer face down, guide horn up,
    /// lateral axis along the hull's. If the correction were transposed, mis-handed, or built off the
    /// wrong pin end, this is the assertion that would move.
    #[test]
    fn the_level_chord_reproduces_the_authored_pose() {
        let frame = *tiger_frames().get(Side::Right);
        assert!(
            frame.correction.angle_between(Quat::IDENTITY) < 0.01,
            "the template is authored square: {:?}",
            frame.correction,
        );

        // A level chord one pitch long, running front→rear (+z), at the belly. The template is unit
        // scale, so the marker span IS the pitch — no factor anywhere.
        let pitch = (PIN_START - PIN_END).with_x(0.0).length();
        assert!(
            (pitch - 0.130_43).abs() < 1e-4,
            "the unit-scale markers must span the measured pitch, got {pitch}",
        );
        let (a, b) = (Vec2::new(-pitch / 2.0, 0.4), Vec2::new(pitch / 2.0, 0.4));
        let t = link_transform(&frame, LATERAL_X, a, b);
        assert!(t.rotation.angle_between(Quat::IDENTITY) < 0.01);
        assert_eq!(t.scale, Vec3::ONE, "nothing may re-introduce a scale");

        // The frame's ORIGIN lands on the anchor: the pin midpoint on the route, the shoe's centre
        // on `link_center_x`.
        let origin_world = t.transform_point(frame.origin);
        assert!(
            (origin_world - Vec3::new(LATERAL_X, 0.4, 0.0)).length() < 1e-4,
            "origin landed at {origin_world}",
        );

        // ...and the pin midpoint itself sits 16.85 mm INBOARD of it, which is the authored overhang
        // reproduced rather than re-typed.
        let pin_mid = (PIN_START + PIN_END) * 0.5;
        let pin_world = t.transform_point(pin_mid);
        assert!(
            (pin_world.x - (LATERAL_X - SHOE_OUTBOARD)).abs() < 1e-4,
            "pin plane landed at {}",
            pin_world.x,
        );
        // Radially and longitudinally the pin midpoint IS the anchor.
        assert!((pin_world.y - 0.4).abs() < 1e-4 && pin_world.z.abs() < 1e-4);
    }

    /// Articulation: the link follows its chord. A chord climbing at 30° must rotate the shoe by
    /// exactly 30° about the hull's lateral axis — this is the whole reason the belt visibly hinges
    /// over a washboard instead of shearing through it.
    #[test]
    fn the_link_follows_its_chord() {
        let frame = *tiger_frames().get(Side::Right);
        let angle = std::f32::consts::FRAC_PI_6;
        let (a, b) = (
            Vec2::ZERO,
            Vec2::new(angle.cos(), angle.sin()) * 0.13, // (z, y)
        );
        let t = link_transform(&frame, LATERAL_X, a, b);
        // The link's PIN AXIS — not model +z — is what must land on the chord. The two differ by
        // 0.1° on the Tiger (the markers carry 0.24 mm of lateral drift along the pin bore), and
        // asserting on the pin axis is the whole point of deriving the frame from the markers.
        let chord = Vec3::new(0.0, angle.sin(), angle.cos());
        let pin_axis = t.rotation * (PIN_START - PIN_END).normalize();
        assert!(
            (pin_axis - chord).length() < 1e-4,
            "pin axis {pin_axis} is not along the chord {chord}",
        );
        // Model +z lands within that same 0.1° of it — the pins really are near-square to the shoe,
        // which is why the template reads as unrotated in the first place.
        let nose = t.rotation * Vec3::Z;
        assert!(
            nose.angle_between(chord) < 0.01,
            "nose {nose} is more than 0.6 deg off the chord",
        );
        // ...and the outer face (local -y) still points away from the wheels, i.e. below the chord.
        let outer = t.rotation * Vec3::NEG_Y;
        assert!(outer.y < 0.0, "outer face turned upward: {outer}");
    }

    /// The left track is the exact mirror of the right: for EVERY mesh vertex, placing the mirrored
    /// mesh with the left frame must land on the reflection of where the right one landed. This is
    /// the property a naive negative-X scale also has — and the reason it is not usable is the
    /// winding, which the next test covers.
    #[test]
    fn the_left_side_is_the_right_sides_reflection() {
        let f = tiger_frames();
        let (a, b) = (Vec2::new(-0.06, 0.42), Vec2::new(0.07, 0.51));
        let right = link_transform(f.get(Side::Right), LATERAL_X, a, b);
        let left = link_transform(f.get(Side::Left), -LATERAL_X, a, b);
        for v in [
            Vec3::ZERO,
            Vec3::new(0.45, 0.09, 0.10),
            Vec3::new(-0.40, -0.12, -0.11),
            PIN_START,
            PIN_END,
        ] {
            let mirrored = left.transform_point(mirror(v));
            let expected = mirror(right.transform_point(v));
            assert!(
                (mirrored - expected).length() < 1e-4,
                "vertex {v} landed at {mirrored}, expected {expected}",
            );
        }
        // Both instance transforms are PROPER rotations - no hidden reflection anywhere.
        for t in [right, left] {
            assert_eq!(t.scale, Vec3::ONE);
            assert!((t.rotation.length() - 1.0).abs() < 1e-4);
        }
    }

    /// The asset-level mirror: x negated on positions AND normals, and the winding reversed so the
    /// reflected triangles still face outward. The orientation check is the real assertion — a
    /// mirror that forgets the winding renders inside-out, which is exactly the bug this avoids.
    #[test]
    fn the_mirrored_mesh_keeps_its_faces_outward() {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        let positions = vec![[0.2, 0.0, 0.0], [0.9, 0.0, 0.0], [0.5, 1.0, 0.0]];
        // A face wound CCW as seen from +z, so its geometric normal is +z.
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 3]);
        mesh.insert_indices(Indices::U32(vec![0, 1, 2]));

        let mirrored = mirrored_mesh(&mesh);
        let Some(VertexAttributeValues::Float32x3(p)) =
            mirrored.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions survive the mirror")
        };
        for (got, want) in p.iter().zip(&positions) {
            assert_eq!(got[0], -want[0], "x must be negated");
            assert_eq!(got[1], want[1]);
        }
        let Some(VertexAttributeValues::Float32x3(n)) = mirrored.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            panic!("normals survive the mirror")
        };
        assert_eq!(n[0], [0.0, 0.0, 1.0], "a +z normal has no x to negate");

        let Some(Indices::U32(indices)) = mirrored.indices() else {
            panic!("indices survive the mirror")
        };
        assert_eq!(indices, &vec![0, 2, 1], "the winding must reverse");

        // The property those two facts exist to buy: the triangle's GEOMETRIC normal (from the
        // reversed winding, on the reflected positions) still agrees with its vertex normal. Skip
        // either half of the mirror and this flips.
        let v = |i: usize| Vec3::from(p[indices[i] as usize]);
        let geometric = (v(1) - v(0)).cross(v(2) - v(0)).normalize();
        assert!(
            geometric.dot(Vec3::from(n[0])) > 0.99,
            "the mirrored face is inside-out: {geometric}",
        );
    }

    /// A flat quad in the xy plane with a plain UV mapping, indexed as two triangles — the smallest
    /// thing mikktspace will produce a well-defined tangent frame for.
    fn tangent_fixture() -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4]);
        // v runs DOWN the quad (the glTF convention), so the bitangent is genuinely independent of
        // the tangent's sign and `w` is not free to be either.
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_UV_0,
            vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        );
        mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
        mesh
    }

    /// The tangent's HANDEDNESS across the mirror, per vertex, as the identity it is claimed to be:
    /// the reflected bitangent must equal the reflection of the bitangent. `B = (N × T) w` and a
    /// determinant-(−1) map obeys `M a × M b = −M (a × b)`, so negating `T.x` alone would leave the
    /// left track's bitangents pointing the wrong way and every normal-mapped detail lit from the
    /// wrong side — a bug with no silhouette and no error message.
    #[test]
    fn the_mirror_reflects_the_tangent_frame_and_not_just_the_tangent() {
        let mut source = tangent_fixture();
        source
            .generate_tangents()
            .expect("the fixture quad has positions, normals, UVs and indices");
        let mirrored = mirrored_mesh(&source);

        let frame = |mesh: &Mesh| {
            let Some(VertexAttributeValues::Float32x3(n)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
            else {
                panic!("normals")
            };
            let Some(VertexAttributeValues::Float32x4(t)) = mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
            else {
                panic!("tangents survive the mirror")
            };
            (n.clone(), t.clone())
        };
        let (source_n, source_t) = frame(&source);
        let (mirror_n, mirror_t) = frame(&mirrored);

        let reflect = Vec3::new(-1.0, 1.0, 1.0);
        for v in 0..source_t.len() {
            let tangent = |t: &[f32; 4]| Vec3::new(t[0], t[1], t[2]);
            let bitangent = |n: &[f32; 3], t: &[f32; 4]| Vec3::from(*n).cross(tangent(t)) * t[3];
            assert!(
                tangent(&source_t[v]).length() > 0.9,
                "vertex {v} has no usable tangent to mirror",
            );
            assert!(
                (tangent(&mirror_t[v]) - reflect * tangent(&source_t[v])).length() < 1e-5,
                "vertex {v}: the tangent is not the reflected tangent",
            );
            let want = reflect * bitangent(&source_n[v], &source_t[v]);
            let got = bitangent(&mirror_n[v], &mirror_t[v]);
            assert!(
                (got - want).length() < 1e-5,
                "vertex {v}: the mirrored bitangent is {got}, but the reflection of the source's \
                 is {want} — the handedness `w` did not follow the reflection",
            );
        }
    }

    /// The SHIPPED LOD1 shoe, run through the real bind-time construction: both final meshes come
    /// out carrying tangents.
    ///
    /// Asset-backed on purpose, in two halves. First the PREMISE is asserted against the glb — no
    /// TANGENT accessor and no MATERIAL — because that pair is exactly what makes `bevy_gltf` skip
    /// its own mikktspace pass (it runs only when the primitive's own material has a normal
    /// texture). Then the mesh those accessors describe is pushed through [`lod1_shoe_meshes`], the
    /// same call `bind_link_template` makes. If a re-export ever gives the primitive a material or
    /// its own tangents the first half fails loudly and this comment is what explains why — the
    /// generation then becomes redundant rather than wrong.
    #[test]
    fn the_shipped_lod1_shoe_binds_with_tangents_on_both_sides() {
        let path = crate::assets::asset_root().join(LINK_LOD1_GLB);
        let gltf::Gltf { document, mut blob } =
            gltf::Gltf::open(&path).unwrap_or_else(|e| panic!("{LINK_LOD1_GLB} must open: {e}"));
        let buffers = [blob.take().expect("the glb carries its binary chunk")];
        let primitive = document
            .meshes()
            .next()
            .expect("the LOD1 glb carries one mesh")
            .primitives()
            .next()
            .expect("the LOD1 mesh carries one primitive");
        assert!(
            primitive.get(&gltf::Semantic::Tangents).is_none(),
            "the LOD1 primitive now ships TANGENT - the bind-time generation is redundant",
        );
        assert!(
            primitive.material().index().is_none(),
            "the LOD1 primitive now carries a material - bevy_gltf may generate its tangents",
        );

        let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
        let mut shoe = Mesh::new(
            PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        shoe.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            reader
                .read_positions()
                .expect("the LOD1 shoe carries positions")
                .collect::<Vec<_>>(),
        );
        shoe.insert_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            reader
                .read_normals()
                .expect("the LOD1 shoe carries normals")
                .collect::<Vec<_>>(),
        );
        shoe.insert_attribute(
            Mesh::ATTRIBUTE_UV_0,
            reader
                .read_tex_coords(0)
                .expect("the LOD1 shoe carries TEXCOORD_0")
                .into_f32()
                .collect::<Vec<_>>(),
        );
        shoe.insert_indices(Indices::U32(
            reader
                .read_indices()
                .expect("the LOD1 shoe is indexed")
                .into_u32()
                .collect(),
        ));

        let bound = lod1_shoe_meshes(&shoe).expect("the shipped LOD1 shoe must take tangents");
        let vertices = shoe.count_vertices();
        for (side, mesh) in bound.iter() {
            let Some(VertexAttributeValues::Float32x4(tangents)) =
                mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
            else {
                panic!(
                    "the {side:?} LOD1 shoe has no tangents - it renders under the normal-mapped \
                     {LINK_MATERIAL} and would light flat",
                )
            };
            assert_eq!(
                tangents.len(),
                vertices,
                "one tangent per vertex ({side:?})"
            );
            // A zeroed tangent is mikktspace's answer for a degenerate UV island, and the shader
            // treats it as a valid frame - so "the attribute exists" is not the assertion.
            let degenerate = tangents
                .iter()
                .filter(|t| Vec3::new(t[0], t[1], t[2]).length() < 0.5 || t[3].abs() < 0.5)
                .count();
            assert_eq!(
                degenerate, 0,
                "{degenerate} of {vertices} {side:?} LOD1 tangents are degenerate",
            );
        }
    }

    /// The slot rotation, which is the whole reason a shoe's identity rides the belt: driving one
    /// pitch forward must move each material link ONE station along the loop, so the entity that
    /// wore station `i` now wears station `i+1`. A fixed binding (the sandbox's old behaviour) fails
    /// this — every entity would keep the station index it started with.
    #[test]
    fn a_pitch_of_travel_walks_every_shoe_one_station() {
        const N: usize = 8;
        const PITCH: f32 = 0.13;
        let frame = *tiger_frames().get(Side::Right);
        // A closed octagon of stations, all distinct so a mis-map is visible in the pose.
        let stations: Vec<Vec2> = (0..N)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / N as f32;
                Vec2::new(a.cos(), a.sin())
            })
            .collect();
        let mut world = World::new();
        let links: Vec<Entity> = (0..N).map(|_| world.spawn_empty().id()).collect();

        let station_of = |phase: f64| {
            let mut map = vec![usize::MAX; N];
            place_links(
                &frame,
                LATERAL_X,
                &stations,
                phase,
                PITCH,
                &links,
                |entity, transform| {
                    let slot = links.iter().position(|&e| e == entity).expect("pooled");
                    if let Some(t) = transform {
                        // Recover which station this entity landed on from its pose: the anchor's
                        // (z, y) is the chord midpoint.
                        let mid = Vec2::new(t.translation.z, t.translation.y);
                        let best = (0..N)
                            .min_by(|&i, &j| {
                                let m = |k: usize| (stations[k] + stations[(k + 1) % N]) * 0.5;
                                m(i).distance(mid).total_cmp(&m(j).distance(mid))
                            })
                            .expect("a closed loop has stations");
                        map[slot] = best;
                    }
                },
            );
            map
        };

        let at_rest = station_of(0.0);
        // Every entity got exactly one station, and every station exactly one entity.
        let mut seen = at_rest.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..N).collect::<Vec<_>>(), "the map is a bijection");

        let one_pitch = station_of(f64::from(PITCH));
        for slot in 0..N {
            assert_eq!(
                one_pitch[slot],
                (at_rest[slot] + 1) % N,
                "entity {slot} did not walk one station forward",
            );
        }
        // ...and a whole loop of travel returns every entity to where it started (the material loop
        // closes: `count` pitches ≡ 0).
        assert_eq!(station_of(f64::from(PITCH) * N as f64), at_rest);
    }

    /// A pool bigger than the station list must be told about its stragglers rather than leaving
    /// them wherever they last were — one hidden link is a bug you never see, one stale link hanging
    /// in the air is a bug in every screenshot.
    #[test]
    fn a_pool_that_outruns_the_stations_reports_its_stragglers() {
        let frame = *tiger_frames().get(Side::Right);
        let stations: Vec<Vec2> = (0..5).map(|i| Vec2::new(i as f32 * 0.13, 0.4)).collect();
        let mut world = World::new();
        let links: Vec<Entity> = (0..8).map(|_| world.spawn_empty().id()).collect();
        let (mut posed, mut unposed) = (0, 0);
        place_links(
            &frame,
            LATERAL_X,
            &stations,
            0.0,
            0.13,
            &links,
            |_, transform| {
                if transform.is_some() {
                    posed += 1;
                } else {
                    unposed += 1;
                }
            },
        );
        assert_eq!(posed, 5, "one link per station");
        assert_eq!(unposed, 3, "every straggler must be reported");
    }

    /// THE SCALE CONTRACT: a template that is not authored at unit scale aborts, and the message
    /// carries the whole diagnosis — the node, the measured scale, and the fix. Driven through
    /// [`verify_unit_scale`] directly because the shipped glb is correct (the test below asserts
    /// that), and inventing a scaled asset would test the fixture rather than the policy.
    #[test]
    fn a_scaled_template_aborts_and_names_the_number() {
        // The scale the model used to ship with — the exact case the old carried-scale plumbing
        // existed for.
        let payload = std::panic::catch_unwind(|| {
            verify_unit_scale(LINK_NODE, Vec3::splat(0.807_917_8));
        })
        .expect_err("a scaled template must abort");
        let text = payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_else(|| "<non-string panic payload>".into());
        assert!(text.contains(LINK_NODE), "must name the node: {text}");
        assert!(text.contains("0.8079178"), "must print the scale: {text}");
        assert!(text.contains("Apply"), "must say how to fix it: {text}");
        // Unit scale, and a hair either side of it, pass.
        verify_unit_scale(LINK_NODE, Vec3::ONE);
        verify_unit_scale(LINK_BOX_NODE, Vec3::splat(1.0 - SCALE_TOLERANCE * 0.5));
        // A single axis is enough to fail — a non-uniform scale is still a scale.
        assert!(
            std::panic::catch_unwind(|| verify_unit_scale(LINK_NODE, Vec3::new(1.0, 1.0, 1.02)))
                .is_err(),
            "one scaled axis must abort",
        );
    }

    /// ...and the SHIPPED asset honours it. Composed down the glb's own node chain, exactly as
    /// [`composed_scale`] does it on the ECS tree at bind: `Link` is a root-level node and
    /// `Link_Box` its child, and neither carries a scale.
    #[test]
    fn the_shipped_template_is_authored_at_unit_scale() {
        use bevy::math::Mat4;

        let path = crate::assets::asset_root().join(crate::tank::TIGER_GLB_PATH);
        let gltf::Gltf { document, .. } = gltf::Gltf::open(&path).expect("the Tiger glb opens");
        let scene = document.scenes().next().expect("the glb carries a scene");
        let mut stack: Vec<(gltf::Node, Mat4)> =
            scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
        let mut checked = 0;
        while let Some((node, parent)) = stack.pop() {
            let world = parent * crate::track::marker_model::node_matrix(&node);
            if matches!(node.name(), Some(LINK_NODE | LINK_BOX_NODE)) {
                let scale = Vec3::new(
                    world.x_axis.truncate().length(),
                    world.y_axis.truncate().length(),
                    world.z_axis.truncate().length(),
                );
                assert!(
                    (scale - Vec3::ONE).abs().max_element() <= SCALE_TOLERANCE,
                    "{} ships at scale {scale} — the export must APPLY it",
                    node.name().unwrap_or_default(),
                );
                checked += 1;
            }
            for child in node.children() {
                stack.push((child, world));
            }
        }
        assert_eq!(checked, 2, "both template nodes must be in the shipped glb");
    }
}
