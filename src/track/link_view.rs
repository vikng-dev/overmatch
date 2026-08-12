//! The TRACK-LINK RENDER LAYER: the tank's own shoe mesh, instanced onto the belt — shared by the
//! game's [`super::view`] and the sandbox's `track_sandbox::link_view` adapter.
//!
//! Everything upstream of here draws the track as a LINE — the conformed pin line, the reference
//! loop, the cast routes. A line is the right thing to reason about and the wrong thing to look at:
//! you cannot see a shoe overhang a board edge, and you cannot see the belt articulate. This module
//! lays the real shoe — the authored mesh as the LOD pipeline certifies it, a MEASURED 1 661
//! triangles (`assets/lod_manifest.json`, level 0) — on the same stations the physics already walks,
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
//! every shoe carries a CHAIN of lower-detail siblings ([`SHOE_LOD_CHAIN`]) — machine reductions of
//! the same authored shoe, generated beside the tank glb by `scripts/lod/generate.py` and certified
//! into `assets/lod_manifest.json`, in the same mesh-local frame and on the same material. At the
//! chain's floor (194 triangles) the same belt costs ~38 k per tank.
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
//! number here: it is regenerating the ladder with a coarser `e1_mm` (or a larger `skip_fraction`)
//! and re-deriving this table from the manifest it writes — see [`SHOE_LOD_CHAIN`].
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
// PER-ASSET NUMBERS — every one of them TRANSCRIBED from `assets/lod_manifest.json`
// ---------------------------------------------------------------------------------------------
//
// Nothing in this section is a knob, and nothing in it was chosen. `scripts/lod/generate.py`
// enumerates the ladder, certifies each rung against the shipped BYTES and writes the manifest;
// the rows below are that manifest's `assets[tiger_1_link].levels`, retyped as Rust. Regenerating
// the ladder is therefore a MECHANICAL update here — copy the new `tris`, `deviation_mm` and
// `switch_m` per level — and `the_wired_thresholds_are_the_manifest_derivation` fails the build if
// a deviation is updated without its distance, which is the half of the edit that is easy to
// forget and impossible to see. (ADR 0033 §8: the manifest is the single seam; hand-written
// ledgers already drifted once.)

/// The MEASURED triangle count of the base shoe — the manifest's level 0, which is the tank glb's
/// own `Link` primitive. Only ever used to report the reductions as a percentage.
const SHOE_BASE_TRIS: usize = 1520;

/// The main pass's WORST-CASE height in pixels, which is what the sub-pixel arithmetic below is
/// indexed to.
///
/// 2160 is 4K native, and it is a CEILING rather than a guess. The settings render-scale ladder
/// ([`crate::settings::RenderScaleLevel`]) tops out at `Percent100` — native — so no setting can
/// make the main pass TALLER than the display it is presented on, and borderless fullscreen takes
/// the display's own resolution rather than a capped one. A SHORTER view only makes every threshold
/// below more conservative: fewer pixels over the same FOV means each pixel subtends more angle, so
/// a given deviation drops under one sooner. Deriving at 1440 and playing at 2160 would be the
/// error that actually bites — the same deviation is then 1.5× the pixels it was argued to be.
///
/// The manifest's `ladder.reference_view.height_px` is this number; the derivation test is what
/// says so.
const LOD_REF_VIEW_HEIGHT_PX: f32 = 2160.0;

/// The camera the distances are chosen for: the gunner optic's authored vertical FOV. Read off
/// [`crate::camera::GUNNER_FOV_FALLBACK`] rather than retyped, so the two cannot drift — and the
/// manifest's `ladder.reference_view.vfov_rad` cites that same constant as its provenance.
const LOD_REF_FOV_RAD: f32 = crate::camera::GUNNER_FOV_FALLBACK;

/// The pixel budget the whole ladder is generated against: a level is admitted at the distance
/// where its own worst-case deviation covers ONE pixel of the reference view. The manifest's
/// `ladder.reference_view.budget_px`. Named rather than elided so the derivation below reads as the
/// formula it is, and so a future pixel-budget SETTING has an obvious thing to displace.
const LOD_BUDGET_PX: f32 = 1.0;

/// The shoe's bounding radius about its own ORIGIN, metres (the manifest's
/// `source.validity.origin_radius_m`).
///
/// Added to every switch distance, because bevy's [`VisibilityRange`] measures camera-to-ENTITY
/// ORIGIN while the guarantee is about the SURFACE: the near face of a shoe is up to one origin
/// radius closer than the point the runtime actually tested. 0.4 m on a track link is nearly
/// nothing — it is carried anyway because it is the same slack `scripts/lod/config.py` adds, and a
/// derivation that agreed with the manifest only after dropping a term would not be the manifest's
/// derivation (ADR 0033 §9).
const SHOE_ORIGIN_RADIUS_M: f32 = 0.380365;

/// The distance beyond which a worst-case surface deviation of `worst_dev_mm` fits inside the pixel
/// budget of the reference view — THE derivation behind every threshold in [`SHOE_LOD_CHAIN`],
/// written once so a test can re-run it against the wired numbers.
///
/// ```text
/// D = dev_m · height_px / (2 · tan(vfov / 2) · budget_px) + origin_radius_m
/// ```
///
/// EXACT, not small-angle. The `D = dev_m / (vfov / height_px)` shortcut this function used to be is
/// 0.06 % out at the optic and 5.5 % out at the commander FOV, which is precisely the kind of error
/// that survives review while one narrow view is the only one quoted through it (ADR 0033 §9). It is
/// the same expression as `scripts/lod/config.py::switch_distance_m`, and the two are held together
/// by `the_wired_thresholds_are_the_manifest_derivation` re-deriving the manifest's own numbers.
fn sub_pixel_distance_m(worst_dev_mm: f32) -> f32 {
    let budget_rad = 2.0 * (LOD_REF_FOV_RAD / 2.0).tan() * LOD_BUDGET_PX;
    (worst_dev_mm / 1000.0) * LOD_REF_VIEW_HEIGHT_PX / budget_rad + SHOE_ORIGIN_RADIUS_M
}

/// One reduced level: the glb the bind loads, its MEASURED triangle count, the MEASURED deviation
/// that mesh carries, and the distance beyond which it takes over from the level above it.
/// Deviation and distance travel TOGETHER because they are one claim — "this mesh is
/// indistinguishable from here" — and a row that carried only the distance is a row an asset
/// regeneration can silently falsify.
struct ShoeLevel {
    glb: &'static str,
    tris: usize,
    /// The certified UPPER bound on this level's worst-case deviation, millimetres — the larger of
    /// the manifest's `dev_source_mm_upper` (quality against the source) and `pairwise_mm_upper`
    /// (the size of the POP against the level it replaces). The pairwise number is the one that
    /// matters at a switch and it is the larger one at L4, where two levels deviate from the source
    /// in opposite directions (ADR 0033 §4).
    worst_dev_mm: f32,
    from_m: f32,
}

/// The reduced levels below the base shoe, NEAREST FIRST. The base owns `[0, chain[0].from_m)`,
/// level `1 + i` owns `[chain[i].from_m, chain[i + 1].from_m)`, and the last owns everything
/// beyond — see [`shoe_lod_range`], which is the ONE place that arithmetic lives.
///
/// A slice rather than a fixed-size array on purpose: the ladder's LENGTH is an output of
/// generation, so nothing here may name a level count. It has changed three times — four levels
/// deep from the 1 661-triangle pre-weld shoe, three from the 764-triangle welded one, and FOUR
/// from the 1 520-triangle rebuilt one.
///
/// # What the numbers are
///
/// | level | glb | tris | dev (mm) | from (m) |
/// |---|---|---|---|---|
/// | L1 | `rung1` | 678 | 3.338 | 60.39 |
/// | L2 | `rung3` | 390 | 14.955 | 269.24 |
/// | L3 | `rung4` | 248 | 30.032 | 540.30 |
/// | L4 | `rung6` | 146 | 82.654 | 1 486.36 |
///
/// RE-CUT 2026-08-12 by the directed search (ADR 0036), and the numbers moved for TWO independent
/// reasons that are worth keeping apart.
///
/// The GEOMETRY moved once: L3 is CHEAPER, 248 triangles against 254, and the other three levels
/// are byte-identical to the chain the exhaustive search cut. The old 254 existed only because the
/// retired search accepted on a 5 %-tolerance bracket whose upper end could not clear the target
/// for a candidate whose true deviation did; the directed search asks the rung's actual question
/// and proves 248 fits.
///
/// The DEVIATIONS moved on every level, downward, because the certifying bound got tighter — the
/// convex acceptance bound of ADR 0036 §6 applies to certification too. Every level is the same
/// mesh it was, proven to sit CLOSER to the source than the old bound could show, so each takes
/// over slightly nearer the camera. A tighter proof about unchanged bytes is the one direction this
/// table is allowed to move without new geometry.
///
/// The rung numbers are octave indices, not chain indices, so the gaps are real: rungs 2 and 5 were
/// skipped for shedding under the 30 % `SKIP_FRACTION` (512 tris at 24.5 %, 186 at 25.0 %), and the
/// manifest carries both skip records.
///
/// L4's band opens at 1 499.58 m, INSIDE the world: the shipped map is 1 500 m across, so its
/// diagonal — the farthest a camera can be from a surface, and the ladder's right wall — is
/// 2 121.32 m. The level is reachable, in the far corner, which is what it was generated for.
///
/// THE LADDER STOPS HERE FOR A GEOMETRIC REASON, NOT A GEOGRAPHIC ONE. The wall admits rungs 7-12,
/// and the generator considers every one of them; each answers with the decimator's 140-triangle
/// topology floor, 4.1 % below L4's 146 and far under `SKIP_FRACTION`. So the chain's depth is set
/// by how far this shoe can be collapsed, not by how big the map is — and widening the world by
/// half added no level. `config.RATIFICATION_EVIDENCE["skipped_rungs"]` enumerates all eight skips
/// and the manifest carries them.
const SHOE_LOD_CHAIN: &[ShoeLevel] = &[
    ShoeLevel {
        glb: "tiger_1/tiger_1_link.rung1.glb",
        tris: 678,
        worst_dev_mm: 3.337_77,
        from_m: 60.3891,
    },
    ShoeLevel {
        glb: "tiger_1/tiger_1_link.rung3.glb",
        tris: 390,
        worst_dev_mm: 14.954_733,
        from_m: 269.2444,
    },
    ShoeLevel {
        glb: "tiger_1/tiger_1_link.rung4.glb",
        tris: 248,
        // THE PAIRWISE BOUND. L3 sits 30.014 mm from L0 and 30.032 mm from L2, its own parent, and
        // the switch is the distance at which the step from the level ACTUALLY ON SCREEN goes
        // sub-pixel — so the larger owns the threshold. The gap is a third of a millimetre here and
        // it still has to be the pairwise number, because which of the two dominates is an output
        // of generation and not a property of this row.
        worst_dev_mm: 30.031_547,
        from_m: 540.3013,
    },
    ShoeLevel {
        glb: "tiger_1/tiger_1_link.rung6.glb",
        tris: 146,
        // The same rule, and here the two are far apart: L4 sits 76.973 mm from L0 but 82.654 mm
        // from L3, because the two levels deviate from the source in OPPOSITE directions. The
        // manifest records both (`switch_from_source_dev_m` 1384.23, `switch_from_pairwise_m`
        // 1486.36) and takes the max; wiring the source-relative number here would switch 100 m too
        // early against a visible step.
        worst_dev_mm: 82.653_622,
        from_m: 1486.3617,
    },
];

/// The number of levels the chain has, base INCLUDED — so `0..shoe_lod_levels()` is every index
/// [`shoe_lod_range`] and [`shoe_lod_tris`] accept. Named so a consumer can walk the ladder without
/// importing its rows.
pub(crate) fn shoe_lod_levels() -> usize {
    SHOE_LOD_CHAIN.len() + 1
}

/// Level `level`'s MEASURED triangle count — `0` is the base shoe, `1 + i` is `SHOE_LOD_CHAIN[i]`.
/// The manifest's numbers, so a legend that quotes them quotes the manifest.
pub(crate) fn shoe_lod_tris(level: usize) -> usize {
    if level == 0 {
        SHOE_BASE_TRIS
    } else {
        SHOE_LOD_CHAIN[level - 1].tris
    }
}

/// The chain's levels and thresholds as one log-line phrase — `"4 reduced levels, LOD1 beyond 56 m,
/// LOD2 beyond 127 m, ..."`. Lives here so a consumer's rig-bound line reports whatever the chain
/// currently is, rather than naming one threshold that a regeneration would silently falsify.
pub(crate) fn lod_chain_summary() -> String {
    let levels = SHOE_LOD_CHAIN
        .iter()
        .enumerate()
        .map(|(i, level)| format!("LOD{} beyond {:.0} m", i + 1, level.from_m))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} reduced {}, {levels}",
        SHOE_LOD_CHAIN.len(),
        if SHOE_LOD_CHAIN.len() == 1 {
            "level"
        } else {
            "levels"
        },
    )
}

/// The range level `level` owns — `0` is the base shoe, `1 + i` is `SHOE_LOD_CHAIN[i]`.
///
/// Derived from the chain rather than written out per level, which is what makes the levels
/// COMPLEMENTARY by construction: each range ends exactly where the next begins, and
/// `VisibilityRange::is_visible_at_all` is `[start, end)` (bevy_camera 0.19
/// `visibility/range.rs`: `distance >= start_margin.start && distance < end_margin.end`), so every
/// distance in `[0, ∞)` is owned by exactly one level. A hand-written table could gap or overlap;
/// this cannot.
///
/// `pub(crate)` because the probe placement asserts itself against a BAND rather than against a
/// threshold constant (`terrain_grid`'s far-probe test): a scenario sited "inside LOD1" has to keep
/// meaning that when a level is added below or above it.
pub(crate) fn shoe_lod_range(level: usize) -> VisibilityRange {
    let start = if level == 0 {
        0.0
    } else {
        SHOE_LOD_CHAIN[level - 1].from_m
    };
    let end = SHOE_LOD_CHAIN
        .get(level)
        .map_or(f32::INFINITY, |level| level.from_m);
    VisibilityRange::abrupt(start, end)
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
    /// The reduced levels, in [`SHOE_LOD_CHAIN`] order and always the same length as it (the bind
    /// refuses rather than binding a short chain, which would leave a distance band with no shoe in
    /// it). Per side: the same shoe reduced, mirrored by the same construction. ONE handle per side
    /// per level for the whole session — every link instance clones it, so a Tiger's 194 shoes at a
    /// given level are 194 references to two mesh assets.
    lods: Vec<PerSide<Handle<Mesh>>>,
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
/// EVERY detail level wears it. The consumers only ever reach a link by an entity id they were
/// handed ([`place_links`] calls back with pool entities), so widening the marker costs them
/// nothing — while a reduced child WITHOUT it would be a nameless hull descendant, exactly the case
/// the tagger's exclusion exists for.
#[derive(Component)]
pub(crate) struct TrackLink;

/// WHICH level of [`SHOE_LOD_CHAIN`] a shoe entity is — `0` is the base shoe, `1 + i` is
/// `SHOE_LOD_CHAIN[i]`. The same index [`shoe_lod_range`] and [`shoe_lod_tris`] take.
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
    // The reduced shoes are ASSET LOADS rather than scene reads: each comes from its own glb
    // ([`SHOE_LOD_CHAIN`]), so there is no node to find and nothing to hide.
    asset_server: Res<AssetServer>,
    // Held across the bind's retries — handles dropped and re-taken every frame would keep
    // cancelling and restarting the loads they are waiting on. One per chain row, same order.
    mut lod_handles: Local<Vec<Handle<Mesh>>>,
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
    // The whole chain is resolved before ANY of it is bound: a template holding fewer levels than
    // [`SHOE_LOD_CHAIN`] would leave a distance band with no shoe drawn in it at all, so a level
    // still loading starves the bind exactly like the base shoe does.
    if lod_handles.is_empty() {
        lod_handles.extend(SHOE_LOD_CHAIN.iter().map(|level| {
            asset_server.load(
                GltfAssetLabel::Primitive {
                    mesh: 0,
                    primitive: 0,
                }
                .from_asset(level.glb),
            )
        }));
    }
    let mut lods = Vec::with_capacity(SHOE_LOD_CHAIN.len());
    let mut lod_triangles = Vec::with_capacity(SHOE_LOD_CHAIN.len());
    for (level, handle) in SHOE_LOD_CHAIN.iter().zip(lod_handles.iter()) {
        let path = level.glb;
        if let bevy::asset::LoadState::Failed(err) = asset_server.load_state(handle) {
            // Loud rather than a silent starve: without this the whole track — the base shoe
            // included — simply never binds and the tank drives on invisible shoes, which reads as
            // a placement bug.
            error_once!(
                "track links: the reduced shoe `{path}` failed to load ({err}) — refusing to bind"
            );
            return;
        }
        let Some(shoe) = meshes.get(handle) else {
            return;
        };
        let tris = shoe.indices().map_or(0, Indices::len) / 3;
        if tris != level.tris {
            // The chain's rows are the MANIFEST's rows (see [`SHOE_LOD_CHAIN`]), and a triangle
            // count is the cheapest thing on one to check against the bytes that actually loaded.
            // A regenerated glb dropped in beside a stale table would otherwise switch at the OLD
            // level's distance — geometry-only, silent, and exactly what ADR 0033 §8 exists to
            // stop. The deviations cannot be checked this way; the count standing in for them is
            // the point.
            error_once!(
                "track links: `{path}` holds {tris} triangles but SHOE_LOD_CHAIN says {} — the \
                 wiring is stale against the shipped asset, so its switch distance is a claim \
                 about a different mesh; re-transcribe the row from assets/lod_manifest.json. \
                 Refusing to bind",
                level.tris,
            );
            return;
        }
        lod_triangles.push(tris);
        match lod_shoe_meshes(shoe) {
            // BOTH sides are derived assets here, unlike the base shoe (whose right side is the
            // glTF's own handle): the tangents are built in this process, so neither side is the
            // loaded primitive any more. The source handles stay alive in the `Local`, which is
            // what keeps the loads from unloading.
            Ok(pair) => lods.push(pair.map(|shoe| meshes.add(shoe))),
            Err(err) => {
                // Same policy as the missing-material refusal above, and for the same reason: a
                // reduced shoe wears the base shoe's NORMAL-MAPPED material, and bevy's PBR shader
                // silently skips normal mapping on a mesh with no tangents. Binding an untangented
                // level would therefore not fail — it would ship a band across the battlefield
                // where every track flattens out, which is exactly the kind of lighting bug nobody
                // traces back to a vertex attribute.
                error_once!(
                    "track links: the reduced shoe `{path}` cannot be given tangents ({err}) — it \
                     renders under the normal-mapped `{LINK_MATERIAL}` and would light flat \
                     without them; refusing to bind"
                );
                return;
            }
        }
    }

    let mesh = PerSide::new(meshes.add(mirrored), source.0.clone());

    // The one lateral datum the markers cannot carry: how far outboard of the PIN PLANE the shoe's
    // centre is authored. `RigGeom` measures both (off `Link_Box` and off the pin markers), so this
    // stays a difference of two measurements rather than a number typed in here — and with the
    // template at unit scale it is already in the mesh's own units.
    let shoe_offset = geom.link_center_x(Side::Right) - geom.plane_x;
    let frame = frames(pin_start, pin_end, shoe_offset);

    info!(
        "track links: template bound - pitch {:.5} m, shoe {:.1} mm outboard of the pin plane, {} \
         triangles/shoe (assets/lod_manifest.json certifies L0 at {SHOE_BASE_TRIS}), {} levels",
        (pin_start - pin_end).with_x(0.0).length(),
        shoe_offset * 1000.0,
        triangles,
        shoe_lod_levels(),
    );
    // The LOD's whole ledger, one line per level: the band it owns, how much geometry it saves
    // against the base shoe, and the two mesh assets every instance of it in the session shares
    // (per side — the left is the mirror).
    //
    // The band is reported WITH THE ARITHMETIC THAT JUSTIFIES IT — the level's measured deviation
    // and the distance beyond which that deviation is sub-pixel — so a capture log carries the
    // claim as well as the number, and a frame stream captured against a regenerated asset says
    // outright whether its thresholds still follow from its meshes.
    for (index, ((chain, &tier_triangles), tier)) in SHOE_LOD_CHAIN
        .iter()
        .zip(lod_triangles.iter())
        .zip(lods.iter())
        .enumerate()
    {
        let range = shoe_lod_range(index + 1);
        let path = chain.glb;
        info!(
            "track links: LOD{} bound - `{path}`, {tier_triangles} triangles/shoe (−{}%) over \
             [{:.1}, {:.1}) m ({:.3} mm worst deviation, under the {LOD_BUDGET_PX} px budget beyond \
             {:.1} m at {LOD_REF_VIEW_HEIGHT_PX:.0} px through the {LOD_REF_FOV_RAD} rad optic), \
             one mesh per side L {:?} R {:?}",
            index + 1,
            100 - (tier_triangles * 100)
                .checked_div(triangles.max(1))
                .unwrap_or(0),
            range.start_margin.start,
            range.end_margin.end,
            chain.worst_dev_mm,
            sub_pixel_distance_m(chain.worst_dev_mm),
            tier.get(Side::Left).id(),
            tier.get(Side::Right).id(),
        );
    }

    commands.insert_resource(LinkTemplate {
        mesh,
        lods,
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
/// TANGENTS. Serves EVERY row of [`SHOE_LOD_CHAIN`]: the levels differ in triangle count and in
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
/// has no tangents — no warning, no error. Every swap in [`SHOE_LOD_CHAIN`] would then change
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
/// together with one child per [`SHOE_LOD_CHAIN`] row. The returned entity is the BASE shoe — the
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
        Mesh3d(template.mesh.get(side).clone()),
        MeshMaterial3d(template.material.clone()),
        shoe_lod_range(0),
        Transform::from_xyz(0.0, -1000.0, 0.0),
        ChildOf(parent),
    ));
    for (level, tier) in template.lods.iter().enumerate() {
        shoe.with_child((
            TrackLink,
            ShoeLod(level + 1),
            Mesh3d(tier.get(side).clone()),
            // The SAME material: the levels differ in triangles and in nothing else, and a second
            // material would put a second batch (and a visible shading seam) on every swap.
            MeshMaterial3d(template.material.clone()),
            shoe_lod_range(level + 1),
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

    /// A template whose mesh handles are ALL DISTINCT — two per side for the base plus two per
    /// [`SHOE_LOD_CHAIN`] row — so a test can tell which one landed on which entity. Built from a
    /// bare `Assets<Mesh>` rather than an `AssetPlugin` app: the only thing under test is which
    /// handle `spawn_link` clones where.
    fn fixture_template(assets: &mut Assets<Mesh>) -> LinkTemplate {
        let mut fresh = || {
            assets.add(Mesh::new(
                PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            ))
        };
        let mesh = PerSide::new(fresh(), fresh());
        let lods = SHOE_LOD_CHAIN
            .iter()
            .map(|_| PerSide::new(fresh(), fresh()))
            .collect();
        LinkTemplate {
            mesh,
            lods,
            material: Handle::default(),
            frame: tiger_frames(),
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
            SHOE_LOD_CHAIN.len(),
            "one child per chain row, never a second pool",
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
            assert_eq!(
                thresholds,
                SHOE_LOD_CHAIN.iter().map(|l| l.from_m).collect::<Vec<_>>(),
                "the wired thresholds are the chain's, in order",
            );
            // ...and the chain as it SHIPS is the manifest's four reductions. Spelled out so a
            // level added or dropped is a deliberate edit here, not a silent one — the ladder's
            // LENGTH is an output of generation, so it is exactly the thing a regeneration can
            // change without anyone noticing. It has changed three times: the 2026-08-07 re-cut
            // against the welded 764-triangle shoe dropped four reductions to three, the
            // 2026-08-08 re-cut against the rebuilt 1 520-triangle shoe took it back to four, and
            // the 2026-08-12 directed-search re-cut (ADR 0036) kept four while moving every
            // threshold IN — the certifying bound got tighter, so each level is proven closer to
            // the source than before and earns its band a little sooner. This line is what made
            // each of them visible rather than silent.
            assert_eq!(thresholds, vec![60.3891, 269.2444, 540.3013, 1486.3617]);

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

    /// The wired thresholds ARE the manifest's derivation, re-run in Rust: every level's distance
    /// must be the one its OWN measured deviation implies, at [`LOD_REF_VIEW_HEIGHT_PX`] through the
    /// optic, to within transcription slack.
    ///
    /// This is the test that makes an LOD asset regeneration safe. The reduced meshes are
    /// regenerated by `scripts/lod/generate.py`, and a regeneration changes exactly two things this
    /// module depends on per level: `worst_dev_mm` and `from_m`. Update the first alone and the level
    /// now switches in CLOSER than its own faceting is invisible — a silent, subtle, geometry-only
    /// regression that no green suite would ever catch, because everything still binds, tiles and
    /// draws. Update the second alone and the distance is a claim about a mesh that no longer
    /// exists. So the DERIVATION is asserted, not the numbers: half a transcription fails here,
    /// naming the distance to write.
    ///
    /// # Why the tolerance is tiny and TWO-SIDED
    ///
    /// These rows are not rounded, they are TRANSCRIBED — `assets/lod_manifest.json`'s `switch_m`
    /// verbatim, because the manifest already applied this formula (ADR 0033 §8/§9: the manifest is
    /// the seam, and hand-rounded ledgers are what drifted). So the only slack the wiring is allowed
    /// is what f32 costs on the way through, and the upper bound is as load-bearing as the lower: a
    /// distance PAST the derivation is the reduction never being reached, and it is what someone
    /// reaches for to silence the lower bound without re-measuring.
    #[test]
    fn the_wired_thresholds_are_the_manifest_derivation() {
        /// What a verbatim transcription of the manifest, re-derived through `f32`, may differ by.
        /// The worst level (L4, ~1 050 m) lands well inside a centimetre; anything larger is a
        /// number that was edited rather than copied.
        const TRANSCRIPTION_SLACK_M: f32 = 0.01;

        for (i, level) in SHOE_LOD_CHAIN.iter().enumerate() {
            let derived = sub_pixel_distance_m(level.worst_dev_mm);
            assert!(
                (level.from_m - derived).abs() < TRANSCRIPTION_SLACK_M,
                "LOD{} ({}) is wired to switch in at {:.4} m, but its MEASURED {:.6} mm deviation \
                 covers the {LOD_BUDGET_PX} px budget only beyond {derived:.4} m at \
                 {LOD_REF_VIEW_HEIGHT_PX:.0} px through the {LOD_REF_FOV_RAD} rad optic. The row is \
                 half-transcribed: `from_m` and `worst_dev_mm` are ONE fact from \
                 assets/lod_manifest.json and must move together — write {derived:.4}.",
                i + 1,
                level.glb,
                level.from_m,
                level.worst_dev_mm,
            );
        }

        // The chain is ORDERED and its distances STRICTLY INCREASE. Falls out of the ladder's
        // octave grid, and asserting it is what makes `shoe_lod_range`'s tiling meaningful: a
        // chain whose rows were pasted out of order would still tile, and would show a coarser
        // mesh nearer than a finer one.
        for pair in SHOE_LOD_CHAIN.windows(2) {
            assert!(
                pair[1].from_m > pair[0].from_m && pair[1].worst_dev_mm > pair[0].worst_dev_mm,
                "the chain must coarsen with distance, in step",
            );
            assert!(pair[1].tris < pair[0].tris, "and shed triangles doing it");
        }
        assert!(SHOE_LOD_CHAIN[0].tris < SHOE_BASE_TRIS);

        // The arithmetic itself, worked by hand:
        // `0.027841808 m × 2160 / (2 × tan(0.06)) + 0.380365 = 500.9314 m`. Without this the loop
        // above would happily pass a `sub_pixel_distance_m` that had lost its units, as long as the
        // wired distances had lost them the same way.
        //
        // 27.841808 mm was the manifest's L3 deviation when this pin was written. That row is gone
        // (the 2026-08-08 chain is one level deep), and the value stays anyway BECAUSE it is now a
        // pure units probe: what it checks is the formula, and a probe tied to a live row would
        // have to be rewritten every re-cut for no gain. The expected metres moved only by the
        // 19.8 mm the shoe's origin radius did.
        assert!(
            (sub_pixel_distance_m(27.841808) - 500.9314).abs() < 0.01,
            "the derivation is `dev_m × height_px / (2·tan(vfov/2)·budget_px) + radius`, got {}",
            sub_pixel_distance_m(27.841808),
        );
        // ...and it is EXACT, not small-angle: the shortcut this replaced reads 501.4 m here, which
        // is the 0.06 % that hid at the optic and is 5.5 % at the commander FOV (ADR 0033 §9).
        assert!(
            (sub_pixel_distance_m(27.841808)
                - ((27.841808 / 1000.0) / (LOD_REF_FOV_RAD / LOD_REF_VIEW_HEIGHT_PX)
                    + SHOE_ORIGIN_RADIUS_M))
                .abs()
                > 0.1,
            "the small-angle shortcut must NOT be what is wired",
        );
    }

    /// The shipped corpus was cut for a world at least as large as the one that loads.
    ///
    /// The ladder's depth is not a taste call: `scripts/lod/config.py` stops generating rungs once
    /// a level's switch distance passes the RIGHT WALL — the world's diagonal, `world_size·√2`,
    /// beyond which no camera can stand and so no coarser level can ever render. The wall is cut
    /// from the map's own `world_extent_xz`, which is the same declaration `crate::map` builds the
    /// grid from.
    ///
    /// A map that GROWS therefore invalidates the corpus in the one way nothing else notices. Every
    /// glb still binds, every wired row still re-derives, the manifest still verifies against
    /// `config.py` — and the chain's last level now owns a band that ends short of the far corner,
    /// so the shoes past it draw at a detail the ladder never certified for that distance. This is
    /// the tripwire: the map is data and can change without anyone running the twelve-minute
    /// generator, so the two are compared where a map edit and the corpus meet.
    ///
    /// GENERIC by construction — the wall is a property of the WORLD, and the manifest's ladder
    /// block is shared by every asset in the corpus. Nothing here names a vehicle.
    #[test]
    fn the_shipped_corpus_reaches_the_worlds_far_corner() {
        let root = crate::assets::asset_root();
        // The extent the game would run at: the map's when there is one, the flat-slab fallback's
        // otherwise — the same resolution `world::spawn_environment` makes.
        let extent = crate::map::load(&root)
            .map(|manifest| manifest.extent)
            .unwrap_or(crate::terrain_grid::FIXTURE_EXTENT);
        let diagonal_m = extent.world_size_m * std::f32::consts::SQRT_2;

        let text = std::fs::read_to_string(root.join("lod_manifest.json"))
            .expect("the shipped manifest is readable");
        let manifest: serde_json::Value =
            serde_json::from_str(&text).expect("the shipped manifest is JSON");
        let right_wall_m = manifest["ladder"]["right_wall_m"]
            .as_f64()
            .expect("the manifest's ladder declares the right wall it was cut against")
            as f32;

        assert!(
            right_wall_m >= diagonal_m,
            "assets/lod_manifest.json was cut for a {right_wall_m:.1} m right wall and the world \
             that loads is {:.1} m across — a {diagonal_m:.1} m diagonal. The ladder stops at the \
             rung that passes the wall, so every level beyond {right_wall_m:.1} m is one the \
             corpus never generated: regenerate it with `python3 scripts/lod/generate.py` (the \
             wall is re-read from the map manifest) and re-transcribe SHOE_LOD_CHAIN. The \
             manifest's own provenance for that wall: {}",
            extent.world_size_m,
            manifest["ladder"]["right_wall_source"],
        );
    }

    /// EVERY reduced sibling inherits the CASTER SWAP. When the shadow ribbon lands,
    /// `drive_track_views` writes `VisualScope::PROXIED_CASTER` onto the pooled shoe and onto
    /// nothing else; the children must go quiet with it, or a tank past the first threshold in
    /// [`SHOE_LOD_CHAIN`] would cast its whole belt through the shadow map that the ribbon was
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
    ///   * TANGENT must be PRESENT. The generator bakes it (`scripts/lod/generate.py`) and the
    ///     manifest certifies it on the decoded bytes, so a re-export that stopped baking would
    ///     silently fall back to [`lod_shoe_meshes`]' runtime mikktspace — a safety net, not a plan,
    ///     and precisely the path that used to produce defaulted tangents on real geometry. Reading
    ///     the attribute through rather than regenerating it is what makes the gates below charge
    ///     against the BYTES THAT SHIP.
    ///   * MATERIAL must be ABSENT, unchanged: a material-free primitive is what keeps `bevy_gltf`
    ///     from running a mikktspace pass of its own over what the exporter already solved, and it
    ///     is what lets every level share the base shoe's [`LINK_MATERIAL`].
    ///
    /// Returns the mesh those accessors describe, ready to push through [`lod_shoe_meshes`] — the
    /// same call `bind_link_template` makes.
    fn shipped_reduced_shoe(glb: &str) -> Mesh {
        let path = crate::assets::asset_root().join(glb);
        let gltf::Gltf { document, mut blob } =
            gltf::Gltf::open(&path).unwrap_or_else(|e| panic!("{glb} must open: {e}"));
        let buffers = [blob.take().expect("the glb carries its binary chunk")];
        let primitive = document
            .meshes()
            .next()
            .unwrap_or_else(|| panic!("{glb} carries one mesh"))
            .primitives()
            .next()
            .unwrap_or_else(|| panic!("{glb}'s mesh carries one primitive"));
        assert!(
            primitive.get(&gltf::Semantic::Tangents).is_some(),
            "{glb}'s primitive ships NO TANGENT - the export stopped baking them, so the shoe now \
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

    /// EVERY SHIPPED reduced shoe — the whole of [`SHOE_LOD_CHAIN`] — run through the real bind-time
    /// construction: both final meshes of every level come out carrying one tangent per vertex.
    ///
    /// Driven off the chain rather than off one named file, so adding a level cannot ship an
    /// untangented one: a new row is covered the moment it is added.
    ///
    /// Whether those tangents are USABLE is the next test's question, not this one's — split so a
    /// red asset names itself as an asset defect instead of hiding inside "the bind works".
    #[test]
    fn every_shipped_reduced_shoe_binds_with_tangents_on_both_sides() {
        for level in SHOE_LOD_CHAIN {
            let glb = level.glb;
            let shoe = shipped_reduced_shoe(glb);
            // The row is a TRANSCRIPTION of the manifest (see [`SHOE_LOD_CHAIN`]) and this is the
            // suite's half of the bind's own refusal: the shipped bytes must be the mesh the row
            // claims. A regenerated glb dropped in beside a stale table would otherwise carry the
            // OLD level's switch distance.
            assert_eq!(
                shoe.indices().map_or(0, Indices::len) / 3,
                level.tris,
                "{glb} is not the {} triangles SHOE_LOD_CHAIN transcribes for it",
                level.tris,
            );
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

        for (index, level) in SHOE_LOD_CHAIN.iter().enumerate() {
            let glb = level.glb;
            let from_m = level.from_m;
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
            // SAME exact projection [`sub_pixel_distance_m`] inverts, less the origin-radius slack
            // it adds (which is the conservative direction here: a smaller pixel to clear).
            let pixel_m = (from_m - SHOE_ORIGIN_RADIUS_M) * 2.0 * (LOD_REF_FOV_RAD / 2.0).tan()
                / LOD_REF_VIEW_HEIGHT_PX
                * LOD_BUDGET_PX;
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
                     {tri:?} at {:.2} mm on its longest edge, and one {LOD_BUDGET_PX} px budget at \
                     the {from_m:.1} m this level takes over is {:.2} mm — {:.2} px of garbage \
                     normal frame; bad tangents {bad:?}\n",
                    extent * 1000.0,
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
             NONE. Note also that assets/lod_manifest.json's own tangent_default_verts: 0 is not \
             this claim: it counts faces with zero UV AREA, and a mesh has passed that gate while \
             still defaulting a vertex here.",
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
