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
//! The Tiger glb ships a TEMPLATE link that is not part of the tank: a `Link` node (the shoe mesh,
//! still named `Tiger_track`) carrying `Pin_Start` / `Pin_End` marker empties and the `Link_Box`
//! volume [`super::marker_model`] measures the shoe's faces from. It sits off to one side of the
//! right sprocket and — because `Link_Box` carries no material — renders as a solid white box beside
//! the tank. [`hide_link_template`] hides both; [`bind_link_template`] reads the mesh and the markers
//! off the same nodes and hands them to the consumers' pools. Both systems run in BOTH binaries: the
//! game used to hide neither, so every game session drew a stray white box and a loose shoe parked
//! beside the hull.
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
//! pitch — the SAME geometry the physics and the drawn line use. Articulation over a washboard and
//! scrolling under drive both fall out for free: the stations articulate, and the resample carries
//! the belt phase.
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

use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
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

/// The glb nodes this module reads and hides. `Link` is the shoe (its mesh is still named
/// `Tiger_track`); `Link_Box` is the marker volume [`super::marker_model`] measures the shoe's faces
/// from — it carries NO material, which is why it renders as a solid white box until it is hidden.
const LINK_NODE: &str = "Link";
const LINK_BOX_NODE: &str = "Link_Box";
/// The pin markers, parented under `Link`: the only meaningful datums it carries.
const PIN_START_NODE: &str = "Pin_Start";
const PIN_END_NODE: &str = "Pin_End";

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
    /// One material for every link. The template's own mesh has no material in the glb (it would
    /// render default-white), so every shoe on every tank in both tools gets the same dark steel.
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
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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
    let Some(source) = children
        .get(link_entity)
        .ok()
        .into_iter()
        .flatten()
        .find_map(|&child| meshes_of.get(child).ok())
        .or_else(|| meshes_of.get(link_entity).ok())
    else {
        return;
    };
    let Some(shoe) = meshes.get(&source.0) else {
        return;
    };
    let triangles = shoe.indices().map_or(0, Indices::len) / 3;
    let mirrored = mirrored_mesh(shoe);
    let mesh = PerSide::new(meshes.add(mirrored), source.0.clone());

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

    commands.insert_resource(LinkTemplate {
        mesh,
        material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.10, 0.10, 0.11),
            perceptual_roughness: 0.85,
            metallic: 0.4,
            ..default()
        }),
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
    // Tangents carry a handedness in `w`; mirroring flips it along with the x component. (The Tiger's
    // shoe ships positions + normals only, but a re-export that adds a UV set would add these too.)
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

// ---------------------------------------------------------------------------------------------
// The pool
// ---------------------------------------------------------------------------------------------

/// Spawn one pooled shoe on `side`, parented to `parent` (a hull-local frame in both consumers).
///
/// Persistent entities whose transforms are rewritten each frame — never rebuilt meshes and never
/// immediate-mode gizmos: at ~97 links × 2 sides × 5 552 triangles a per-frame rebuild would be
/// ~1.1 M triangles of CPU work every frame, while identical mesh+material instances batch into two
/// draws. Parked below the world until the first placement writes a real pose (the spawn lands via
/// `Commands`, so the placer only sees it next frame, and an un-posed link must not flash on screen).
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
            Transform::from_xyz(0.0, -1000.0, 0.0),
            ChildOf(parent),
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
            let local = match node.transform() {
                gltf::scene::Transform::Matrix { matrix } => Mat4::from_cols_array_2d(&matrix),
                gltf::scene::Transform::Decomposed {
                    translation,
                    rotation,
                    scale,
                } => Mat4::from_scale_rotation_translation(
                    Vec3::from(scale),
                    Quat::from_array(rotation),
                    Vec3::from(translation),
                ),
            };
            let world = parent * local;
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
