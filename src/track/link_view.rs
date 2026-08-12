//! The TRACK-LINK RENDER LAYER: the tank's own shoe mesh, instanced onto the belt — shared by the
//! game's [`super::view`] and the sandbox's `track_sandbox::link_view` adapter.
//!
//! Everything upstream of here draws the track as a LINE — the conformed pin line, the reference
//! loop, the cast routes. A line is the right thing to reason about and the wrong thing to look at:
//! you cannot see a shoe overhang a board edge, and you cannot see the belt articulate. This module
//! lays the real shoe — the authored mesh as the tank build certifies it, rung 0 of the `Link#0`
//! chain in `assets/tiger_1/tiger_1.lod.json` — on the same stations the physics already walks,
//! so the model is
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
//! # Distance LOD: extra shoe ENTITIES, not extra pools
//!
//! The shoes are the largest geometry pool a tank owns — the Tiger's MEASURED 97 links per side ×
//! 2 sides = 194 shoes, at 1 661 triangles each, is ~322 k triangles per tank, and a 15v15 frame
//! holds thirty of them. Beyond a few tens of metres none of that detail survives rasterisation, so
//! every shoe carries a CHAIN of lower-detail siblings — machine reductions of the same authored
//! shoe, cut by `scripts/tank/build.py` into the tank's own view glb as rung mesh records and
//! certified in `assets/tiger_1/tiger_1.lod.json`, in the same mesh-local frame and on the same
//! material. At the chain's floor the same belt costs a fraction of that per tank.
//!
//! The switch is bevy's own [`VisibilityRange`] — [`VisibilityRange::abrupt`], deliberately, because
//! a crossfaded range compiles a second dithering permutation of every shoe pipeline for a
//! transition nobody can see. Each reduced level is spawned as a CHILD of the base shoe rather than
//! as another pooled sibling, and three separate mechanisms fall out of that for free:
//!
//!   * **Placement.** [`place_links`] writes ONE transform per shoe and ordinary propagation carries
//!     the children, so the levels can never disagree about where a link is — and there is no
//!     second placement system to keep in step with the first.
//!   * **Rendering policy.** `render_policy` resolves a mesh against its nearest scoped ANCESTOR, so
//!     the children inherit their tank's channel and, once the shadow ribbon lands and the shoe is
//!     silenced with `VisualScope::PROXIED_CASTER`, inherit the silence too. Nothing here mirrors a
//!     layer or a shadow marker by hand; the one path that used to rewrite layers per mesh under the
//!     controlled tank no longer exists.
//!   * **Lifetime.** `despawn` is recursive over `Children`, so a rig rebind or a sandbox pool
//!     shrink takes the reduced shoes with the shoe they belong to.
//!
//! Sharing the material has one obligation the assets do not discharge: [`LINK_MATERIAL`] is
//! NORMAL-MAPPED, and bevy's PBR shader drops normal mapping — silently — on a mesh with no
//! `ATTRIBUTE_TANGENT`. The reduced primitives carry no material of their own, which is exactly the
//! case `bevy_gltf` does NOT auto-generate tangents for, so [`lod_shoe_meshes`] builds them at bind
//! time and refuses the bind if it cannot. Otherwise a swap would change the LIGHTING as well as the
//! silhouette, and the distances below are argued only about the silhouette.
//!
//! The cost of the pattern is entity count: the pool is multiplied by the number of levels, and
//! every one of those entities is visited by `check_visibility_ranges` each frame. That is the trade
//! a measurement sweep has to judge — the triangle win is only worth having if it is not eaten by
//! the visibility walk, and at 194 shoes × 5 levels × 30 tanks the walk is ~29 k entities. The
//! probe scenario can stand its 30-tank block on either side of the swap (`OVERMATCH_PROBE_FAR` —
//! see [`crate::tank::scenario::probe_far`]), which is what makes both halves of that measurable.
//! If the near sweep says the walk costs more than the triangles save, the retreat is NOT editing a
//! number here — there is no number here to edit. It is re-cutting the ladder with a coarser
//! `e1_mm` (or a larger `skip_fraction`) and rebuilding the trio; the bands follow.
//!
//! # `OVERMATCH_LOD_SHOWCASE=1`: judging the switches by eye
//!
//! The pipeline no longer scores a switch by rendering it: ADR 0036 §3 deleted the
//! rendered-difference gate, which had been comparing the shipped meshes under the wrong textures
//! since the corpus was cut. What certifies a level now is purely geometric — a proven bound on its
//! worst-case deviation at the distance it takes over — and the one thing that bound cannot tell
//! anybody is whether a swap LOOKS like a swap. So the eye is the only remaining audit, and the
//! showcase lever ([`crate::lod_showcase`]) is the instrument for it: it flattens the world and
//! stands one PAIR of tanks at each switch distance with their shoes CLAMPED to the two levels that
//! switch is between. That clamp is the only thing in the game that overrides a shoe's range, it is
//! written by a system that does not run unless the variable is set, and it takes the ranges it
//! writes from [`shoe_lod_range`] — so it cannot drift from the chain it is showing off. A swap
//! seen to pop here is the named trigger for re-arming a rendered gate (ADR 0036 §3).

use bevy::camera::visibility::VisibilityRange;
use bevy::mesh::{GenerateTangentsError, Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

use crate::geometry_lod::GeometryLodLevel;

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

// ---------------------------------------------------------------------------------------------
// THE CHAIN COMES FROM THE CERTIFICATE
// ---------------------------------------------------------------------------------------------
//
// Nothing in this module is a measurement any more. `assets/tiger_1/tiger_1.lod.json` carries the
// shoe's bounding radius and its rungs' certified deviations; [`crate::geometry_lod`] derives the
// metres against the live view profile and owns the one writer that keeps them current. The
// hand-transcribed chain table this module used to carry — five constants, a projection and four
// rows of measured millimetres — is deleted with ADR-0035: the certificate is the single seam
// ADR-0033 §8 demanded.

/// The glTF MESH the shoe primitive belongs to. Mesh and node share the name on this model, and
/// the certificate keys a chain on `<meshName>#<primitiveIndex>`, so the shoe's chain is `Link#0`.
const LINK_MESH: &str = "Link";

/// The certificate key of the shoe's chain. [`bind_link_template`] resolves the chain from the
/// bound primitive's MESH ASSET and checks the answer against this, so the name is a claim the
/// bind verifies rather than a lookup it trusts.
pub(crate) fn shoe_chain_key() -> String {
    crate::geometry_lod::chain_key(LINK_MESH, 0)
}

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
    /// The reduced levels, in CERTIFICATE order and always the same length as the chain's rungs
    /// (the bind refuses rather than binding a short chain, which would leave a distance band with
    /// no shoe in it). Per side: the same shoe reduced, mirrored by the same construction. ONE
    /// handle per side per level for the whole session — every link instance clones it, so a
    /// Tiger's 194 shoes at a given level are 194 references to two mesh assets.
    lods: Vec<PerSide<Handle<Mesh>>>,
    /// One material for every link, read off the glb's own shoe primitive — the artist's
    /// [`LINK_MATERIAL`], never a `StandardMaterial` built here. The look is therefore changed by
    /// re-exporting the blend, not by editing this file.
    material: Handle<StandardMaterial>,
    /// Per side: mesh space → the canonical pin frame.
    frame: PerSide<LinkFrame>,
    /// The `geometry_lod` chain index every spawned level records, so the adaptive layer rewrites a
    /// pooled shoe's band by exactly the path it rewrites a scene primitive's — there is no second
    /// threshold writer for the track.
    chain: usize,
    /// The band each level owns under the view profile AT BIND TIME, rung 0 first. A profile move
    /// rewrites the live entities through `geometry_lod::adapt_bands`; this is the seed a fresh
    /// spawn takes.
    bands: Vec<VisibilityRange>,
}

impl LinkTemplate {
    /// This side's mesh→canonical-frame correction, `Copy` so a consumer can capture it at bind and
    /// never touch the template again in its hot loop.
    pub(crate) fn frame(&self, side: Side) -> LinkFrame {
        *self.frame.get(side)
    }

    /// The number of levels the chain has, base INCLUDED — so `0..levels()` is every index
    /// [`Self::band`] and [`Self::tris`] accept.
    pub(crate) fn levels(&self) -> usize {
        self.lods.len() + 1
    }

    /// The range level `level` owns: `0` is the base shoe, `1 + i` is the chain's rung `i`.
    ///
    /// Derived by `geometry_lod` from the certificate, so the levels are COMPLEMENTARY by
    /// construction — each range ends exactly where the next begins.
    pub(crate) fn band(&self, level: usize) -> VisibilityRange {
        self.bands[level].clone()
    }

    /// The chain's levels and thresholds as one log-line phrase — `"4 reduced levels, LOD1 beyond
    /// 56 m, LOD2 beyond 127 m, ..."`. Lives here so a consumer's rig-bound line reports whatever
    /// the chain currently is, rather than naming one threshold a re-cut would silently falsify.
    pub(crate) fn chain_summary(&self) -> String {
        let levels = (1..self.levels())
            .map(|level| {
                format!(
                    "LOD{level} beyond {:.0} m",
                    self.bands[level].start_margin.start
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} reduced {}, {levels}",
            self.lods.len(),
            if self.lods.len() == 1 {
                "level"
            } else {
                "levels"
            },
        )
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
/// EVERY detail level wears it. The consumers only ever reach a link by an entity id they were
/// handed ([`place_links`] calls back with pool entities), so widening the marker costs them
/// nothing — while a reduced child WITHOUT it would be a nameless hull descendant, exactly the case
/// the tagger's exclusion exists for.
#[derive(Component)]
pub(crate) struct TrackLink;

/// WHICH level of the shoe's certified chain a shoe entity is — `0` is the base shoe, `1 + i` is
/// the chain's rung `i`. The same index [`LinkTemplate::band`] and [`LinkTemplate::tris`] take.
///
/// Every level already carries the range that SELECTS it; this says which level it IS. The
/// distinction matters exactly once — for a consumer that wants to OVERRIDE the selection, which is
/// [`crate::lod_showcase`] and nothing else — and it is a tag rather than a lookup because the
/// alternative is matching a spawned entity's `Mesh3d` handle back against `LinkTemplate::lods`,
/// i.e. the same fact recovered by search instead of recorded.
///
/// It is NOT a knob and nothing reads it on the production path: in a process with no showcase this
/// is one index-sized component on entities that already exist.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ShoeLod(pub(crate) usize);

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
    // The reduced shoes are RUNG MESH RECORDS in the tank's own view glb, resolved by
    // `geometry_lod` from the certificate. No sidecar glb, no node to find, nothing to hide — and
    // no measurement to transcribe: the bands come out of the same derivation every scene
    // primitive's do.
    chains: Option<Res<crate::geometry_lod::GeometryLodChains>>,
    view: Res<crate::geometry_lod::ViewProfile>,
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

    // The reduced shoes, tangent-generated and mirrored by [`lod_shoe_meshes`] — every level has to
    // be a reflection of the others for the same reason the two sides do, and reusing
    // `mirrored_mesh` is what keeps that true without a second answer to "how is a shoe mirrored".
    //
    // The CHAIN is resolved from the shoe primitive's own MESH ASSET, which is the identity a
    // certificate chain is per (shared meshes share a chain), and the key the lookup lands on is
    // checked against [`shoe_chain_key`] — a wrong join would otherwise mirror some other part's
    // rungs onto the belt. The whole chain is resolved before ANY of it is bound: a template
    // holding fewer levels than the certificate names would leave a distance band with no shoe
    // drawn in it at all.
    let Some(chains) = chains else {
        return;
    };
    let Some(chain) = chains.of_mesh(source.0.id()) else {
        error_once!(
            "track links: the `{LINK_NODE}` shoe primitive belongs to no certified chain — the \
             certificate names `{}`; refusing to bind",
            shoe_chain_key(),
        );
        return;
    };
    if chain.key() != shoe_chain_key() {
        error_once!(
            "track links: the `{LINK_NODE}` shoe primitive resolves to chain `{}` and the shoe's \
             chain is `{}` — the rungs on the belt would be another part's; refusing to bind",
            chain.key(),
            shoe_chain_key(),
        );
        return;
    }
    let mut lods = Vec::with_capacity(chain.rungs().len());
    let mut tris = vec![triangles];
    for (rung, handle) in chain.chain().rungs.iter().zip(chain.rungs()) {
        let Some(shoe) = meshes.get(handle) else {
            return;
        };
        tris.push(shoe.indices().map_or(0, Indices::len) / 3);
        match lod_shoe_meshes(shoe) {
            // BOTH sides are derived assets here, unlike the base shoe (whose right side is the
            // glTF's own handle): the mirror is built in this process, so neither side is the
            // loaded rung record any more. The rung handles live in `GeometryLodChains`, which is
            // what keeps the records from unloading.
            Ok(pair) => lods.push(pair.map(|shoe| meshes.add(shoe))),
            Err(err) => {
                // Same policy as the missing-material refusal above: a reduced shoe wears the base
                // shoe's NORMAL-MAPPED material, and bevy's PBR shader silently skips normal
                // mapping on a mesh with no tangents. Binding an untangented level would ship a
                // band across the battlefield where every track flattens out.
                error_once!(
                    "track links: the rung `{}` cannot be given tangents ({err}) — it renders \
                     under the normal-mapped `{LINK_MATERIAL}` and would light flat without them; \
                     refusing to bind",
                    rung.mesh,
                );
                return;
            }
        }
    }
    let bands = chain.bands(*view);

    let mesh = PerSide::new(meshes.add(mirrored), source.0.clone());

    // The one lateral datum the markers cannot carry: how far outboard of the PIN PLANE the shoe's
    // centre is authored. `RigGeom` measures both (off `Link_Box` and off the pin markers), so this
    // stays a difference of two measurements rather than a number typed in here — and with the
    // template at unit scale it is already in the mesh's own units.
    let shoe_offset = geom.link_center_x(Side::Right) - geom.plane_x;
    let frame = frames(pin_start, pin_end, shoe_offset);

    info!(
        "track links: template bound - pitch {:.5} m, shoe {:.1} mm outboard of the pin plane, {} \
         triangles/shoe, {} levels on chain `{}`",
        (pin_start - pin_end).with_x(0.0).length(),
        shoe_offset * 1000.0,
        triangles,
        lods.len() + 1,
        chain.key(),
    );
    // The LOD's whole ledger, one line per level: the band it owns, how much geometry it saves
    // against the base shoe, and the two mesh assets every instance of it in the session shares
    // (per side — the left is the mirror).
    //
    // The band is reported WITH THE ARITHMETIC BEHIND IT — the level's certified deviation and the
    // view profile the metres were derived against — so a capture log carries the claim as well as
    // the number, and a stream captured against a re-cut asset says outright whether its thresholds
    // still follow from its meshes.
    for (index, (rung, tier)) in chain.chain().rungs.iter().zip(lods.iter()).enumerate() {
        let band = &bands[index + 1];
        info!(
            "track links: LOD{} bound - `{}`, {} triangles/shoe (−{}%) over [{:.1}, {:.1}) m \
             ({:.3} mm certified deviation + {:.3} m radius, at {:.0} px through the {:.4} rad \
             field on a {:.2} px budget), one mesh per side L {:?} R {:?}",
            index + 1,
            rung.mesh,
            tris[index + 1],
            100 - (tris[index + 1] * 100)
                .checked_div(triangles.max(1))
                .unwrap_or(0),
            band.start_margin.start,
            band.end_margin.end,
            rung.deviation_mm,
            chain.chain().radius_m,
            view.height_px,
            view.vfov_rad,
            view.budget_px,
            tier.get(Side::Left).id(),
            tier.get(Side::Right).id(),
        );
    }

    let index = chain.index();
    commands.insert_resource(LinkTemplate {
        mesh,
        lods,
        material: material.0.clone(),
        frame,
        chain: index,
        bands,
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
    // Tangents, and this branch is LIVE for every level of the shoe (no glb ships them, but the
    // shoe's material is normal-mapped, so the base gets them from `bevy_gltf`'s mikktspace pass at
    // load and the reductions from [`lod_shoe_meshes`]). A tangent is `dP/du` with `w` recording the
    // bitangent's
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

/// One reduced shoe as the two mesh assets the pool binds — `[left, right]` — both carrying
/// TANGENTS. Serves EVERY certified rung: the levels differ in triangle count and in
/// nothing else that matters here, so a second copy of this per level would be a second place for
/// the handedness algebra below to go wrong.
///
/// # The tangents are READ, and the generation is the fallback that must never fire
///
/// The shipped levels now BAKE `TANGENT` (`scripts/lod/generate.py` runs mikktspace before export
/// and the manifest certifies `tangent_default_verts: 0` on the DECODED BYTES), so this function's
/// generation branch is dead on the assets as they ship — which is the point of the branch, not an
/// argument for deleting it.
///
/// It exists because of what `bevy_gltf` 0.19 does NOT do. Its own mikktspace pass runs only when a
/// primitive's OWN material wants tangents (`needs_tangents`: a normal texture, or a clearcoat
/// normal texture), and these primitives carry NO glTF material at all — being bare machine
/// reductions with no look of their own, they resolve to the glTF default material, which wants
/// nothing. So an untangented reduced primitive would arrive with NO `ATTRIBUTE_TANGENT` and bevy
/// would not notice.
///
/// That would be harmless if the shoe were unlit steel, but every reduced instance renders under the
/// base shoe's [`LINK_MATERIAL`], whose three MEASURED maps include a NORMAL map. bevy's PBR shader
/// keys normal mapping on the `VERTEX_TANGENTS` shader def and simply drops the map when the mesh
/// has no tangents — no warning, no error. Every swap in the chain would then change
/// the LIGHTING as well as the silhouette, which is not what the distance was argued from. So an
/// export that stopped baking tangents degrades to a runtime mikktspace pass instead of to flat
/// lighting, and `no_defaulted_tangent_touches_a_triangle_a_player_can_resolve` is what says the
/// shipped bytes are not relying on it.
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
fn lod_shoe_meshes(source: &Mesh) -> Result<PerSide<Mesh>, GenerateTangentsError> {
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
/// together with one child per certified rung. The returned entity is the BASE shoe — the
/// one the pool holds and the one [`place_links`] poses.
///
/// Persistent entities whose transforms are rewritten each frame — never rebuilt meshes and never
/// immediate-mode gizmos: at ~97 links × 2 sides × 1 661 triangles a per-frame rebuild would be
/// ~322 k triangles of CPU work every frame, while identical mesh+material instances batch into two
/// draws. Parked below the world until the first placement writes a real pose (the spawn lands via
/// `Commands`, so the placer only sees it next frame, and an un-posed link must not flash on screen).
///
/// The levels' ranges MEET — `[0, 55.9)`, `[55.9, 126.6)`, ... as the chain ships — so exactly
/// one of them is drawn at every distance, in every view, with no gap and no double-draw. Both the
/// meshes and the ranges come from the chain, so a level added or dropped there stays consistent
/// here without touching this function. Every child carries the same [`TrackLink`] marker as its
/// parent: it is a pooled shoe by every rule that matters to the consumers (the sandbox's mesh
/// tagger excludes the pool by that marker, and would otherwise class a nameless hull descendant as
/// hull geometry and repaint it under x-ray).
pub(crate) fn spawn_link(
    commands: &mut Commands,
    template: &LinkTemplate,
    side: Side,
    parent: Entity,
) -> Entity {
    let mut shoe = commands.spawn((
        TrackLink,
        ShoeLod(0),
        // The chain tag `geometry_lod`'s adaptive layer rewrites bands through — the pooled shoes
        // ride the SAME writer as every scene primitive, so the track cannot drift out of step with
        // the rest of the model when the view profile moves.
        GeometryLodLevel {
            chain: template.chain,
            level: 0,
        },
        Mesh3d(template.mesh.get(side).clone()),
        MeshMaterial3d(template.material.clone()),
        template.band(0),
        Transform::from_xyz(0.0, -1000.0, 0.0),
        ChildOf(parent),
    ));
    for (level, tier) in template.lods.iter().enumerate() {
        shoe.with_child((
            TrackLink,
            ShoeLod(level + 1),
            GeometryLodLevel {
                chain: template.chain,
                level: level + 1,
            },
            Mesh3d(tier.get(side).clone()),
            // The SAME material: the levels differ in triangles and in nothing else, and a second
            // material would put a second batch (and a visible shading seam) on every swap.
            MeshMaterial3d(template.material.clone()),
            template.band(level + 1),
            // IDENTITY, and load-bearing: every reduced mesh is authored in the same mesh-local
            // frame as the full shoe, so riding the parent's transform puts it exactly where the
            // shoe was. It is also what makes the range tests agree — all the levels resolve to the
            // same world origin, so no two of them can be in (or out of) range on the same frame.
            Transform::IDENTITY,
        ));
    }
    shoe.id()
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

    /// The view every certified distance was quoted in when the corpus was cut: the gunner optic
    /// at 4K native, one pixel of budget (`scripts/lod/config.py::REFERENCE_VIEW`).
    fn reference_view() -> crate::geometry_lod::ViewProfile {
        crate::geometry_lod::ViewProfile::new(crate::camera::GUNNER_FOV_FALLBACK, 2160.0, 1.0)
    }

    /// The SHIPPED shoe chain, straight out of the certificate — the same record the bind resolves
    /// through `geometry_lod`.
    fn shoe_chain() -> crate::geometry_lod::Chain {
        let root = crate::assets::asset_root();
        crate::geometry_lod::certificate::load(&root, crate::geometry_lod::TIGER_ID)
            .chains
            .remove(&shoe_chain_key())
            .expect("the shipped certificate names the shoe's chain")
    }

    /// A template whose mesh handles are ALL DISTINCT — two per side for the base plus two per
    /// certified rung — so a test can tell which one landed on which entity. Built from a bare
    /// `Assets<Mesh>` rather than an `AssetPlugin` app: the only thing under test is which handle
    /// `spawn_link` clones where. The BANDS are the shipped chain's, derived exactly as the bind
    /// derives them.
    fn fixture_template(assets: &mut Assets<Mesh>) -> LinkTemplate {
        let mut fresh = || {
            assets.add(Mesh::new(
                PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            ))
        };
        let chain = shoe_chain();
        let mesh = PerSide::new(fresh(), fresh());
        let lods = chain
            .rungs
            .iter()
            .map(|_| PerSide::new(fresh(), fresh()))
            .collect();
        LinkTemplate {
            mesh,
            lods,
            material: Handle::default(),
            frame: tiger_frames(),
            chain: 0,
            bands: chain.bands(reference_view()),
        }
    }

    /// Run `spawn_link` against a real `World` and hand back every level of the shoe, base first:
    /// the pooled entity and one child per chain row, in chain order.
    fn spawn_levels(
        world: &mut World,
        template: &LinkTemplate,
        side: Side,
        parent: Entity,
    ) -> Vec<Entity> {
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        let link = spawn_link(&mut commands, template, side, parent);
        queue.apply(world);
        let children = world
            .entity(link)
            .get::<Children>()
            .expect("a shoe carries its reduced siblings");
        assert_eq!(
            children.len(),
            template.lods.len(),
            "one child per chain rung, never a second pool",
        );
        std::iter::once(link).chain(children.iter()).collect()
    }

    /// The LOD chain as spawned: one entity per level, all DIFFERENT meshes, one material, and
    /// ranges that TILE `[0, ∞)` — every distance owned by exactly one level.
    ///
    /// The handle assertion is the one that matters for cost — a tank's 194 shoes at a level must be
    /// 194 references to that level's two assets, not 194 meshes — and the tiling assertion is what
    /// makes the chain a level-of-detail rather than a gap (the track vanishing in a band) or a
    /// double-draw (two levels submitted at once).
    ///
    /// The tiling is asserted against bevy's REAL contract, read off `bevy_camera` 0.19
    /// `visibility/range.rs`: `is_visible_at_all` is `distance >= start_margin.start &&
    /// distance < end_margin.end`, i.e. HALF-OPEN `[start, end)`, and it is the exact predicate
    /// `check_visibility_ranges` uses to fill `VisibleEntityRanges`. So a boundary distance belongs
    /// to the level BELOW it (350.0 m draws LOD1, not the base) — which is why the sweep below tests
    /// each boundary itself, not just a value either side of it.
    #[test]
    fn every_distance_is_owned_by_exactly_one_shoe_level() {
        let mut assets = Assets::<Mesh>::default();
        let template = fixture_template(&mut assets);
        let mut world = World::new();

        for side in Side::ALL {
            let parent = world.spawn_empty().id();
            let levels = spawn_levels(&mut world, &template, side, parent);
            let mesh_of = |e: Entity| {
                world
                    .entity(e)
                    .get::<Mesh3d>()
                    .expect("a shoe is a mesh")
                    .0
                    .id()
            };
            let material_of = |e: Entity| {
                world
                    .entity(e)
                    .get::<MeshMaterial3d<StandardMaterial>>()
                    .map(|m| m.0.id())
            };
            let range_of = |e: Entity| {
                world
                    .entity(e)
                    .get::<VisibilityRange>()
                    .cloned()
                    .expect("every level is range-gated")
            };

            assert_eq!(mesh_of(levels[0]), template.mesh.get(side).id());
            for (i, tier) in template.lods.iter().enumerate() {
                assert_eq!(mesh_of(levels[i + 1]), tier.get(side).id());
                // The look is the artist's, once: a second material would mean a second batch and a
                // shading seam on the swap.
                assert_eq!(material_of(levels[i + 1]), material_of(levels[0]));
                // Coincident: the children ride their parent's transform, which is what lets one
                // placement write serve every level AND what makes the range tests agree.
                assert_eq!(
                    world.entity(levels[i + 1]).get::<Transform>().copied(),
                    Some(Transform::IDENTITY),
                );
            }
            let meshes: std::collections::HashSet<_> = levels.iter().map(|&e| mesh_of(e)).collect();
            assert_eq!(
                meshes.len(),
                levels.len(),
                "each level must be its own mesh — a shared handle is not an LOD",
            );

            // The ranges themselves, as the chain's thresholds: [0, 55.9), [55.9, 126.6), ... as it
            // ships.
            let ranges: Vec<_> = levels.iter().map(|&e| range_of(e)).collect();
            for range in &ranges {
                assert!(
                    range.is_abrupt(),
                    "abrupt, so no shoe pipeline grows a crossfade permutation for a swap nobody \
                     sees",
                );
            }
            assert_eq!(ranges[0].start_margin.start, 0.0, "the base starts at zero");
            for pair in ranges.windows(2) {
                assert_eq!(
                    pair[0].end_margin.end, pair[1].start_margin.start,
                    "a level must hand over exactly where the next takes over",
                );
            }
            assert!(
                ranges
                    .last()
                    .expect("at least the base level")
                    .end_margin
                    .end
                    .is_infinite(),
                "the last level never ends",
            );
            // The wired ranges carry the CHAIN's thresholds, in order — driven off the chain rather
            // than off a literal list, so adding or dropping a level is still one row there.
            let thresholds: Vec<f32> = ranges[1..].iter().map(|r| r.start_margin.start).collect();
            let chain = shoe_chain();
            let derived: Vec<f32> = chain
                .bands(reference_view())
                .into_iter()
                .skip(1)
                .map(|band| band.start_margin.start)
                .collect();
            assert_eq!(
                thresholds, derived,
                "the spawned bands are the certificate's derivation, in order",
            );
            // ...and the chain as it SHIPS is four reductions. Spelled out so a level added or
            // dropped is a deliberate edit here, not a silent one — the ladder's LENGTH is an
            // output of generation, so it is exactly the thing a re-cut can change without anyone
            // noticing. It has changed four times: 2026-08-07 (four reductions to three against
            // the welded 764-triangle shoe), 2026-08-08 (back to four against the rebuilt
            // 1 520-triangle shoe), 2026-08-12 (the directed search, ADR 0036) and 2026-08-12
            // again (the per-primitive build of ADR 0035, whose rungs are octaves 1/3/4/5). The
            // METRES are not spelled out: they are a function of the view profile now, and the
            // derivation above is what pins them.
            assert_eq!(thresholds.len(), 4);

            // Every level is TAGGED with its own index, which is what lets the showcase override
            // a selection without matching mesh handles back to the template.
            for (i, &e) in levels.iter().enumerate() {
                assert_eq!(world.entity(e).get::<ShoeLod>().copied(), Some(ShoeLod(i)));
            }

            // TILING: exactly one level is drawn at every distance. Each threshold is probed AT the
            // boundary and a hair either side, because `[start, end)` is what decides which level
            // owns the boundary itself — an inclusive-on-both-ends reading would double-draw there,
            // and an exclusive-on-both would leave the track with no shoe at all.
            let mut probes = vec![0.0_f32, 1.0, 120.0, 400.0, 2_500.0, 10_000.0];
            for &t in &thresholds {
                probes.extend([t - 0.01, t, t + 0.01]);
            }
            for d in probes {
                let visible: Vec<usize> = ranges
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.is_visible_at_all(d))
                    .map(|(i, _)| i)
                    .collect();
                assert_eq!(
                    visible.len(),
                    1,
                    "at {d} m exactly one level must be visible, got {visible:?}",
                );
            }
            // ...and the boundary lands on the FARTHER level, which is the half-open contract said
            // as a fact rather than as a count.
            for (i, &t) in thresholds.iter().enumerate() {
                assert!(
                    ranges[i + 1].is_visible_at_all(t) && !ranges[i].is_visible_at_all(t),
                    "at exactly {t} m the level below must take over",
                );
            }
        }
    }

    /// THE TRACK VIEW'S BANDS ARE THE CERTIFICATE'S DERIVATION, pinned against metres worked by
    /// hand from the certificate's own numbers at a fixed view profile.
    ///
    /// This is the test that makes an asset re-cut safe. The rungs are re-cut by
    /// `scripts/tank/build.py`, and a re-cut changes exactly one thing this module depends on per
    /// level: the certified `deviation_mm`. Nothing transcribes it any more — the metres are
    /// derived — so what has to be pinned is the DERIVATION itself, at inputs that do not move.
    #[test]
    fn the_shoe_bands_are_the_certificates_derivation() {
        let chain = shoe_chain();
        let view = reference_view();
        let bands = chain.bands(view);

        // The whole ladder, worked by hand off the certificate at the reference view:
        //
        //     D = dev_mm/1000 x 2160 / (2 * tan(0.06)) + radius_m
        //
        // A derivation that lost its units, dropped the radius slack or took the small-angle
        // shortcut fails here.
        let denominator = 2.0 * (crate::camera::GUNNER_FOV_FALLBACK / 2.0).tan();
        for (level, rung) in chain.rungs.iter().enumerate() {
            let by_hand = (rung.deviation_mm / 1000.0) * 2160.0 / denominator + chain.radius_m;
            let derived = bands[level + 1].start_margin.start;
            assert!(
                (derived - by_hand).abs() < 1e-2,
                "LOD{} ({}) opens at {derived:.4} m and its certified {:.6} mm deviation covers \
                 the 1 px budget only beyond {by_hand:.4} m at 2160 px through the {:.4} rad optic",
                level + 1,
                rung.mesh,
                rung.deviation_mm,
                crate::camera::GUNNER_FOV_FALLBACK,
            );
        }

        // The chain COARSENS with distance, in step: deviations ascend strictly (the certificate's
        // own law) and so, therefore, do the bands. A chain whose rungs were emitted out of order
        // would still tile, and would show a coarser mesh nearer than a finer one.
        for pair in chain.rungs.windows(2) {
            assert!(
                pair[1].deviation_mm > pair[0].deviation_mm,
                "the chain must coarsen with distance",
            );
        }
        for pair in bands.windows(2) {
            assert!(pair[1].start_margin.start > pair[0].start_margin.start);
        }

        // ...and the projection is EXACT, not small-angle: the shortcut reads 0.06 % long at the
        // optic and 5.5 % at the commander field (ADR 0033 section 9).
        let dev_mm = chain.rungs[0].deviation_mm;
        let small_angle =
            (dev_mm / 1000.0) / (crate::camera::GUNNER_FOV_FALLBACK / 2160.0) + chain.radius_m;
        assert!(
            (bands[1].start_margin.start - small_angle).abs() > 1e-3,
            "the small-angle shortcut must NOT be what the bands are derived from",
        );
    }

    /// THE BANDS MOVE WITH THE VIEW, and in the conservative direction.
    ///
    /// The metres are no longer baked at one reference height: a shorter viewport spends fewer
    /// pixels over the same field, so a given deviation goes sub-pixel SOONER and every level takes
    /// over nearer the camera. A wider field and a looser budget do the same. This is what the
    /// deleted transcription could not do, and it is why the certificate ships deviations rather
    /// than metres.
    #[test]
    fn a_shorter_view_switches_sooner_than_the_reference_one() {
        use crate::geometry_lod::ViewProfile;

        let chain = shoe_chain();
        let at = |view| chain.bands(view)[1].start_margin.start;
        let optic = crate::camera::GUNNER_FOV_FALLBACK;
        let reference = at(reference_view());
        assert!(
            at(ViewProfile::new(optic, 1080.0, 1.0)) < reference,
            "half the pixels, half the deviation term",
        );
        assert!(
            at(ViewProfile::new(0.785, 2160.0, 1.0)) < reference,
            "a wider field spends fewer pixels on the same surface",
        );
        assert!(
            at(ViewProfile::new(optic, 2160.0, 2.0)) < reference,
            "a looser budget admits the reduction sooner",
        );
    }

    /// EVERY reduced sibling inherits the CASTER SWAP. When the shadow ribbon lands,
    /// `drive_track_views` writes `VisualScope::PROXIED_CASTER` onto the pooled shoe and onto
    /// nothing else; the children must go quiet with it, or a tank past the first threshold in
    /// the chain's first threshold would cast its whole belt through the shadow map the ribbon was
    /// built to replace.
    ///
    /// Asserted through `render_policy`'s own resolver rather than by reading a marker off the
    /// spawn, because inheritance is exactly the mechanism being relied on: nothing in this module
    /// writes a shadow marker or a layer, and this is the test that says so.
    #[test]
    fn silencing_a_shoe_silences_every_reduced_sibling() {
        use crate::render_policy::{VisualScope, casts_shadow};

        let mut assets = Assets::<Mesh>::default();
        let template = fixture_template(&mut assets);
        let mut app = App::new();
        app.add_plugins(crate::render_policy::plugin);

        let world = app.world_mut();
        let tank = world.spawn(VisualScope::WORLD_SOLID).id();
        let levels = spawn_levels(world, &template, Side::Right, tank);
        app.update();
        assert!(
            levels.iter().all(|&e| casts_shadow(app.world(), e)),
            "before the ribbon exists EVERY level carries the belt's shadow, as one shoe did",
        );

        app.world_mut()
            .entity_mut(levels[0])
            .insert(VisualScope::PROXIED_CASTER);
        app.update();
        assert!(
            !casts_shadow(app.world(), levels[0]),
            "the swap silences the shoe it is written on",
        );
        assert!(
            levels[1..].iter().all(|&e| !casts_shadow(app.world(), e)),
            "and every reduced sibling, which is written on nothing at all",
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

    /// One SHIPPED reduced shoe read straight off disk, EXACTLY as `bevy_gltf` would present it —
    /// every attribute the primitive actually carries, TANGENT included, and nothing invented here.
    ///
    /// The two assertions on the way past are the premises the bind rests on, and they now point in
    /// OPPOSITE directions:
    ///
    ///   * TANGENT must be PRESENT. The generator bakes it (`scripts/lod/generate.py` passes
    ///     `export_tangents`), and NOTHING UPSTREAM CHECKS THAT ANY MORE — ADR 0036 §4 retired the
    ///     lane's tangent gates along with the rendered-difference gate they served, so schema v2
    ///     carries no tangent field and `chain.py --verify` has no opinion. That makes this
    ///     assertion the only remaining guard: a re-export that stopped baking would silently fall
    ///     back to [`lod_shoe_meshes`]' runtime mikktspace — a safety net, not a plan, and
    ///     precisely the path that used to produce defaulted tangents on real geometry. Reading the
    ///     attribute through rather than regenerating it is what makes the gates below charge
    ///     against the BYTES THAT SHIP.
    ///   * MATERIAL must be ABSENT, unchanged: a material-free primitive is what keeps `bevy_gltf`
    ///     from running a mikktspace pass of its own over what the exporter already solved, and it
    ///     is what lets every level share the base shoe's [`LINK_MATERIAL`].
    ///
    /// Returns the mesh those accessors describe, ready to push through [`lod_shoe_meshes`] — the
    /// same call `bind_link_template` makes.
    fn shipped_reduced_shoe(glb: &str) -> Mesh {
        let path = crate::geometry_lod::certificate::member_path(
            &crate::assets::asset_root(),
            crate::geometry_lod::TIGER_ID,
            crate::geometry_lod::TrioMember::View,
        );
        let gltf::Gltf { document, mut blob } =
            gltf::Gltf::open(&path).unwrap_or_else(|e| panic!("{glb} must open: {e}"));
        let buffers = [blob.take().expect("the glb carries its binary chunk")];
        let primitive = document
            .meshes()
            .find(|mesh| mesh.name() == Some(glb))
            .unwrap_or_else(|| panic!("the view glb holds the rung record `{glb}`"))
            .primitives()
            .next()
            .unwrap_or_else(|| panic!("{glb}'s mesh carries one primitive"));
        assert!(
            primitive.get(&gltf::Semantic::Tangents).is_some(),
            "{glb}'s primitive ships NO TANGENT - the build stopped baking them, so the shoe now \
             relies on a runtime mikktspace pass that nothing certified",
        );
        assert!(
            primitive.material().index().is_none(),
            "{glb}'s primitive now carries a material - bevy_gltf may generate its tangents",
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
                .unwrap_or_else(|| panic!("{glb} carries positions"))
                .collect::<Vec<_>>(),
        );
        shoe.insert_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            reader
                .read_normals()
                .unwrap_or_else(|| panic!("{glb} carries normals"))
                .collect::<Vec<_>>(),
        );
        shoe.insert_attribute(
            Mesh::ATTRIBUTE_UV_0,
            reader
                .read_tex_coords(0)
                .unwrap_or_else(|| panic!("{glb} carries TEXCOORD_0"))
                .into_f32()
                .collect::<Vec<_>>(),
        );
        shoe.insert_attribute(
            Mesh::ATTRIBUTE_TANGENT,
            reader
                .read_tangents()
                .unwrap_or_else(|| panic!("{glb} carries TANGENT"))
                .collect::<Vec<_>>(),
        );
        shoe.insert_indices(Indices::U32(
            reader
                .read_indices()
                .unwrap_or_else(|| panic!("{glb} is indexed"))
                .into_u32()
                .collect(),
        ));
        shoe
    }

    /// A bound mesh's tangents, positions and indices — the three attributes the gates below charge
    /// against each other. Panics rather than returns, because every one of them is a bind-time
    /// invariant, not a case to handle.
    fn bound_attributes(mesh: &Mesh, what: &str) -> (Vec<[f32; 4]>, Vec<Vec3>, Vec<u32>) {
        let Some(VertexAttributeValues::Float32x4(tangents)) =
            mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
        else {
            panic!(
                "{what} has no tangents - it renders under the normal-mapped {LINK_MATERIAL} and \
                 would light flat",
            )
        };
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions survive the bind ({what})")
        };
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("the bound mesh is indexed ({what})")
        };
        (
            tangents.clone(),
            positions.iter().copied().map(Vec3::from).collect(),
            indices.clone(),
        )
    }

    /// EVERY SHIPPED rung of the shoe's chain, run through the real bind-time construction: both
    /// final meshes of every level come out carrying one tangent per vertex.
    ///
    /// Driven off the CERTIFICATE rather than off one named file, so a re-cut that adds a level
    /// cannot ship an untangented one: a new rung is covered the moment the certificate names it.
    ///
    /// Whether those tangents are USABLE is the next test's question, not this one's — split so a
    /// red asset names itself as an asset defect instead of hiding inside "the bind works".
    #[test]
    fn every_shipped_reduced_shoe_binds_with_tangents_on_both_sides() {
        for rung in &shoe_chain().rungs {
            let glb = rung.mesh.as_str();
            let shoe = shipped_reduced_shoe(glb);
            let bound =
                lod_shoe_meshes(&shoe).unwrap_or_else(|e| panic!("{glb} must take tangents: {e}"));
            let vertices = shoe.count_vertices();
            for (side, mesh) in bound.iter() {
                let (tangents, ..) = bound_attributes(mesh, &format!("{side:?} {glb}"));
                assert_eq!(
                    tangents.len(),
                    vertices,
                    "one tangent per vertex ({side:?} {glb})"
                );
            }
        }
    }

    /// NO SHIPPED SHOE CARRIES A DEFAULTED TANGENT — the standing gate on what the glbs actually
    /// hold, and on the fact that bevy generates none of it.
    ///
    /// # Why a zeroed tangent is a defect and not a gap
    ///
    /// mikktspace hands back NO tangent frame for a vertex it cannot solve, and `bevy_mesh`'s
    /// `set_tangent` writes its default — `[0, 0, 0, 1]` — in that case, which the shader treats as
    /// a valid frame and lights garbage from. So "the attribute exists" is not the assertion. A
    /// quadric collapse leaves such vertices behind on the slivers its edge collapses produce, and
    /// they light a streak of wrong normal under the normal-mapped [`LINK_MATERIAL`] with no
    /// silhouette change and no error message anywhere.
    ///
    /// # What this gate guards NOW
    ///
    /// Three things, and the first two are the reason the third can be absolute:
    ///
    ///   1. **The shipped bytes CARRY tangents.** `scripts/lod/generate.py` bakes `TANGENT` into
    ///      every level glb (and into `tiger_1.glb`'s own `Link`), and the manifest certifies
    ///      `tangent_default_verts: 0` on the DECODED bytes. [`shipped_reduced_shoe`] refuses a
    ///      primitive without the accessor, so a re-export that stopped baking fails here rather
    ///      than quietly falling back.
    ///   2. **Bevy generates NOTHING.** [`lod_shoe_meshes`]' mikktspace branch is skipped when the
    ///      attribute is present, so the right-hand mesh must be the glb's tangents unchanged and
    ///      the left must be their exact analytic reflection. Asserted below: a runtime pass
    ///      creeping back in is a different mesh being certified from the one that ships.
    ///   3. **Not one defaulted tangent, anywhere.** Not "none big enough to resolve" — NONE. The
    ///      pixel arithmetic is kept in the report as the thing that explains why a future one
    ///      matters, not as the bar it has to clear.
    ///
    /// # It was RED for four asset generations, and what fixed it
    ///
    /// | asset | tris | worst defaulted triangle | switches at | px there |
    /// |---|---|---|---|---|
    /// | glb-route planar 60° + collapse 400 | 386 | 33.87 mm | 350 m | 1.74 |
    /// | `.blend` route, planar 10° + collapse | 477 | 50.13 mm | 350 m | 2.58 |
    /// | pipeline v2 `rung1`, tangents generated at BIND | 855 | 50.16 mm | 55.9 m | **16.15** |
    /// | pipeline v2 `rung1`, tangents BAKED at export | 854 | none | 55.9 m | — |
    ///
    /// Two things had to be true at once, which is why three regenerations did not clear it. The
    /// needle cleanup removed the sliver (one triangle out of each of L1/L2/L3 — the counts moved
    /// 855/581/315 → 854/580/314), and baking the tangents moved the mikktspace pass to where the
    /// mesh can be inspected and certified before it ships. Either alone leaves a runtime generator
    /// deciding, per load, what the lighting looks like.
    ///
    /// The middle row is also the record of a real gap between two gates, worth keeping: the
    /// manifest certified `tangent_default_verts: 0` for that asset and BEVY still defaulted one
    /// vertex, because `scripts/lod/measure.py` counts a vertex whose faces ALL have zero UV area
    /// and none of those faces did. A numeric gate on the source data is not a gate on what the
    /// runtime's own solver returns. That is why this test reads the SHIPPED bytes through the REAL
    /// bind path rather than trusting the manifest, and why it stays that way now that it is green.
    ///
    /// If this ever goes red again the FIX IS AN ASSET (or the generator's cleanup), not a number.
    /// If it has to be parked, park it as `#[ignore = "..."]` naming what it waits on — never by
    /// widening a budget.
    #[test]
    fn no_shipped_shoe_carries_a_defaulted_tangent() {
        // EVERY level is surveyed before anything is asserted, and the whole survey is the failure
        // message. A test that panicked on the first bad level would report one row of a ladder
        // whose other rows are the evidence for what is actually wrong with it.
        let mut report = String::new();
        let mut failed = false;

        let chain = shoe_chain();
        let bands = chain.bands(reference_view());
        for (index, rung) in chain.rungs.iter().enumerate() {
            let glb = rung.mesh.as_str();
            let from_m = bands[index + 1].start_margin.start;
            let shoe = shipped_reduced_shoe(glb);
            let bound =
                lod_shoe_meshes(&shoe).unwrap_or_else(|e| panic!("{glb} must take tangents: {e}"));
            // BEVY GENERATED NOTHING. With TANGENT present `lod_shoe_meshes` clones instead of
            // solving, so the right-hand mesh must carry the glb's own tangents unchanged. If a
            // runtime mikktspace pass ever creeps back in, everything below certifies a mesh that
            // is not the one shipping — and it would do so silently, because a generated frame is
            // usually fine and only sometimes is not.
            let (shipped, ..) = bound_attributes(&shoe, &format!("shipped {glb}"));
            let (right, ..) = bound_attributes(bound.get(Side::Right), &format!("bound {glb}"));
            assert_eq!(
                shipped, right,
                "{glb}: the bind re-solved tangents the export already baked",
            );
            // What the pixel budget covers, in metres, at the distance this level takes over — the
            // SAME exact projection `ViewProfile::switch_distance_m` inverts, less the bounding
            // radius slack it adds (the conservative direction here: a smaller pixel to clear).
            let view = reference_view();
            let pixel_m = (from_m - chain.radius_m) * 2.0 * (view.vfov_rad / 2.0).tan()
                / view.height_px
                * view.budget_px;
            let Some(VertexAttributeValues::Float32x2(uvs)) = shoe.attribute(Mesh::ATTRIBUTE_UV_0)
            else {
                panic!("{glb} carries TEXCOORD_0")
            };

            for (side, mesh) in bound.iter() {
                let what = format!("{side:?} {glb}");
                let (tangents, positions, indices) = bound_attributes(mesh, &what);
                let usable = |v: u32| {
                    let t = tangents[v as usize];
                    Vec3::new(t[0], t[1], t[2]).length() > 0.5 && t[3].abs() > 0.5
                };
                let defaulted_verts = (0..tangents.len() as u32).filter(|&v| !usable(v)).count();
                // The WORST offender, so a failure names the triangle to go and look at rather than
                // whichever one the index order happened to reach first.
                let mut worst: Option<(f32, [u32; 3])> = None;
                let mut touching = 0usize;
                let mut uv_degenerate = 0usize;
                for tri in indices.chunks_exact(3) {
                    if tri.iter().all(|&v| usable(v)) {
                        continue;
                    }
                    touching += 1;
                    // The pipeline's OWN criterion, run here so the two gates can be compared: twice
                    // the signed area of the triangle's UV image. `measure.py` calls a face
                    // tangent-defaulting when this is under `uv_area_eps` (1e-12).
                    let uv = |i: usize| Vec2::from(uvs[tri[i] as usize]);
                    let (a, b) = (uv(1) - uv(0), uv(2) - uv(0));
                    if (a.x * b.y - a.y * b.x).abs() < 1e-12 {
                        uv_degenerate += 1;
                    }
                    let p = |i: usize| positions[tri[i] as usize];
                    let extent = [
                        (p(1) - p(0)).length(),
                        (p(2) - p(1)).length(),
                        (p(0) - p(2)).length(),
                    ]
                    .into_iter()
                    .fold(0.0_f32, f32::max);
                    if worst.is_none_or(|(e, _)| extent > e) {
                        worst = Some((extent, [tri[0], tri[1], tri[2]]));
                    }
                }
                let Some((extent, tri)) = worst else {
                    report.push_str(&format!("  LOD{} {what}: clean\n", index + 1));
                    continue;
                };
                // The tangents themselves, for the worst triangle's bad vertices. `[0, 0, 0, 1]`
                // exactly is `bevy_mesh`'s DEFAULT — mikktspace declined to solve the vertex.
                // Anything else small is a fan whose contributions CANCELLED, which is a different
                // asset defect with the same consequence in the shader, and only this line tells
                // the two apart.
                let bad: Vec<_> = tri
                    .iter()
                    .filter(|&&v| !usable(v))
                    .map(|&v| (v, tangents[v as usize]))
                    .collect();
                // ANY defaulted tangent fails, not just a resolvable one — the levels are
                // certified to have none. The extent is what the report explains it WITH.
                failed = true;
                report.push_str(&format!(
                    "  LOD{level} {what}: {defaulted_verts} defaulted verts on {touching} triangles \
                     ({uv_degenerate} of them UV-degenerate by measure.py's own test); worst is \
                     {tri:?} at {:.2} mm on its longest edge, and one {:.2} px budget at \
                     the {from_m:.1} m this level takes over is {:.2} mm — {:.2} px of garbage \
                     normal frame; bad tangents {bad:?}\n",
                    extent * 1000.0,
                    view.budget_px,
                    pixel_m * 1000.0,
                    extent / pixel_m,
                    level = index + 1,
                ));
            }
        }

        assert!(
            !failed,
            "a SHIPPED shoe carries a defaulted tangent, under the normal-mapped \
             {LINK_MATERIAL}:\n{report}\nFIX THE ASSET (weld or drop the degenerate sliver in the \
             reduction, and bake the tangents at export), never a budget here. The px figures are \
             how visible it is, not the bar it has to clear — the levels are certified to carry \
             NONE. Note also that the build's own UV-area check is not this claim: it counts faces \
             with zero UV AREA, and a mesh has passed that gate while still defaulting a vertex \
             here.",
        );
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
