//! The sandbox's TRACK-LINK RENDER LAYER: the Tiger's own shoe mesh, instanced onto the belt.
//!
//! Everything upstream of here draws the track as a LINE — the conformed pin line, the reference
//! loop, the cast routes. A line is the right thing to reason about and the wrong thing to look at:
//! you cannot see a shoe overhang a board edge, you cannot see the belt articulate, and you cannot
//! tell a kinematic wrap from a solved chain by eye. This module lays the real 5 552-triangle shoe
//! on the same stations the physics already walks, so the A/B is done on actual track.
//!
//! # Where the shoe comes from
//!
//! The Tiger glb ships a TEMPLATE link that is not part of the tank: a `Link` node (the shoe mesh,
//! under a 0.8079178 uniform scale) carrying `Pin_Start` / `Pin_End` marker empties and the
//! `Link_Box` volume [`super::model`] measures the shoe's faces from. It sits off to one side of the
//! right sprocket and — because `Link_Box` carries no material — renders as a solid white box beside
//! the tank. [`hide_link_template`] hides both; [`bind_link_template`] reads the mesh and the markers
//! off the same nodes and hands them to the pool.
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
//!     [`super::model`] projects x out of the pitch), so the lateral datum comes from the one place
//!     that measures it: `RigGeom::link_center_x`, the shoe's own centre, ~16.85 mm OUTBOARD of the
//!     pin plane. Anchoring the shoe's centre there reproduces the authored overhang exactly.
//!   * AXES — longitudinal along `Pin_Start - Pin_End`, lateral along model X, the third by cross
//!     product. The template node carries no rotation, so this construction is the identity on the
//!     shipped model: the authored pose IS a valid on-track pose (outer face down, guide horn up),
//!     which is what makes the derivation checkable rather than merely plausible.
//!   * SCALE — `Link`'s own 0.8079178, carried through. Dropping it renders the shoe 23.8 % oversized.
//!
//! Per frame each link spans two consecutive stations of [`super::ConformedBelts`] — the belt line
//! the active view already resampled at the link pitch, so it is the SAME geometry the physics and
//! the drawn line use, under whichever view the `V` toggle selects. Articulation over a washboard and
//! scrolling under drive both fall out for free: the stations articulate, and the resample carries
//! the belt phase.
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

use super::rig_geom::RigGeom;
use super::{ConformedBelts, Hull, PerSide, Side, VizLayers};

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<LinkPool>()
        .add_systems(
            Update,
            (
                // Cheap and always on: an `Added<Name>` scan, so a glb hot-reload re-hides the
                // template it re-instantiates.
                hide_link_template,
                // Latched on its own resource — one read of the template nodes, ever.
                bind_link_template
                    .run_if(resource_exists::<RigGeom>)
                    .run_if(not(resource_exists::<LinkTemplate>)),
            ),
        )
        .add_systems(
            Update,
            (
                sync_link_pool.run_if(resource_exists::<LinkTemplate>),
                // The stations come from `ConformedBelts`, which the ACTIVE view system rewrites
                // every frame — so the links must land after whichever one ran, or they would render
                // one frame of lag behind the line drawn through them.
                place_links
                    .run_if(resource_exists::<LinkTemplate>)
                    .after(super::model4::conform_belts_field)
                    .after(super::model4::conform_belts_field_chain)
                    .after(sync_link_pool),
            )
                .run_if(resource_exists::<RigGeom>),
        );
}

// ---------------------------------------------------------------------------------------------
// The template: one read of the glb's own link
// ---------------------------------------------------------------------------------------------

/// The glb nodes this module reads and hides. `Link` is the shoe (its mesh is still named
/// `Tiger_track`); `Link_Box` is the marker volume `super::model` measures the shoe's faces from —
/// it carries NO material, which is why it renders as a solid white box until it is hidden.
const LINK_NODE: &str = "Link";
const LINK_BOX_NODE: &str = "Link_Box";
/// The pin markers, parented under `Link`: the only meaningful datums it carries.
const PIN_START_NODE: &str = "Pin_Start";
const PIN_END_NODE: &str = "Pin_End";

/// Everything needed to instance one shoe, read once off the template.
#[derive(Resource)]
struct LinkTemplate {
    /// Per side: the authored shoe on the right, its genuine mirror on the left (see the module
    /// doc — a negative-X scale would be a winding flip, not a mirror).
    mesh: PerSide<Handle<Mesh>>,
    /// One material for every link. The template's own mesh has no material in the glb (it would
    /// render default-white), so the shoe gets the same dark steel the game's `track::view` paints
    /// its links with — the two tools should read as one track.
    material: Handle<StandardMaterial>,
    /// Per side: mesh space → the canonical pin frame.
    frame: PerSide<LinkFrame>,
    /// `Link`'s own uniform scale. Carried, not assumed: at 1.0 the shoe renders 23.8 % oversized.
    scale: f32,
}

/// The correction that turns the template's arbitrary origin into a frame you can place: a rotation
/// that maps MESH axes onto the canonical (lateral, inner, longitudinal) = (x, y, z) triple, and the
/// mesh-local point that becomes the frame's origin.
#[derive(Clone, Copy, Debug)]
struct LinkFrame {
    /// Mesh → canonical rotation (the inverse of the canonical basis expressed in mesh space).
    correction: Quat,
    /// The anchor in MESH-LOCAL, PRE-SCALE coordinates: the pin midpoint radially and
    /// longitudinally, the shoe's own lateral centre laterally (the pins cannot answer laterally —
    /// see the module doc).
    origin: Vec3,
}

/// Marker on a pooled link instance, so the placer's `Transform`/`Visibility` query cannot reach
/// anything else in the sandbox.
///
/// `pub(super)` so [`super::mesh_layers`]'s mesh tagger can exclude the shoe pool — the instances are
/// nameless children of the hull, so without this marker they would fall through to the hull layer
/// and fight the `links` switch for their visibility.
#[derive(Component)]
pub(super) struct TrackLink;

/// The instanced links, per side, children of the hull. Persistent entities whose transforms are
/// rewritten each frame — never rebuilt meshes and never immediate-mode gizmos: at ~97 links × 2
/// sides × 5 552 triangles a per-frame rebuild would be ~1.1 M triangles of CPU work every frame,
/// while identical mesh+material instances batch into two draws.
#[derive(Resource, Default)]
struct LinkPool(PerSide<Vec<Entity>>);

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

/// Read the template ONCE: the shoe mesh, `Link`'s scale, and the two pin markers — then build the
/// mirrored mesh, the shared material, and both sides' canonical frames.
///
/// It retries every frame until the glb scene has landed (the scene load is async and the rig build
/// does not wait for it), and latches on inserting [`LinkTemplate`]. The name scan is global rather
/// than a walk from the hull because the sandbox spawns exactly one tank, and `Link` is a SCENE ROOT
/// — a sibling of the hull node, not a descendant of it.
fn bind_link_template(
    mut commands: Commands,
    named: Query<(Entity, &Name, &Transform)>,
    children: Query<&Children>,
    meshes_of: Query<&Mesh3d>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    geom: Res<RigGeom>,
) {
    let (mut link, mut pin_start, mut pin_end) = (None, None, None);
    for (entity, name, transform) in &named {
        match name.as_str() {
            LINK_NODE => link = Some((entity, transform.scale)),
            PIN_START_NODE => pin_start = Some(transform.translation),
            PIN_END_NODE => pin_end = Some(transform.translation),
            _ => {}
        }
    }
    let (Some((link_entity, link_scale)), Some(pin_start), Some(pin_end)) =
        (link, pin_start, pin_end)
    else {
        return;
    };
    // The shoe mesh hangs on a PRIMITIVE child of the node (bevy_gltf always spawns one child per
    // primitive). Direct children only: `Link_Box` is also a child of `Link` and carries its own
    // primitive one level further down, so a descendant search could pick up the marker box instead.
    let Some(source) = children
        .get(link_entity)
        .ok()
        .into_iter()
        .flatten()
        .find_map(|&child| meshes_of.get(child).ok())
    else {
        return;
    };
    let Some(shoe) = meshes.get(&source.0) else {
        return;
    };
    let triangles = shoe.indices().map_or(0, Indices::len) / 3;
    let mirrored = mirrored_mesh(shoe);
    let mesh = PerSide::new(meshes.add(mirrored), source.0.clone());

    // The one lateral datum the markers cannot carry, in the model's own pre-scale units: how far
    // outboard of the PIN PLANE the shoe's centre is authored. `RigGeom` measures both (off
    // `Link_Box` and off the pin markers), so this stays a difference of two measurements rather
    // than a number typed in here.
    let shoe_offset = (geom.link_center_x(Side::Right) - geom.plane_x) / link_scale.x.max(1e-6);
    let frame = frames(pin_start, pin_end, shoe_offset);

    info!(
        "track links: template bound - scale {:.6}, pitch {:.5} m, shoe {:.1} mm outboard of the \
         pin plane, {} triangles/shoe",
        link_scale.x,
        (pin_start - pin_end).with_x(0.0).length() * link_scale.x,
        (geom.link_center_x(Side::Right) - geom.plane_x) * 1000.0,
        triangles,
    );

    commands.insert_resource(LinkTemplate {
        mesh,
        // The game's link material (`track::view`), verbatim: dark steel, rough, part-metallic.
        material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.10, 0.10, 0.11),
            perceptual_roughness: 0.85,
            metallic: 0.4,
            ..default()
        }),
        frame,
        scale: link_scale.x,
    });
}

// ---------------------------------------------------------------------------------------------
// The canonical frame
// ---------------------------------------------------------------------------------------------

/// Reflection across the hull's lateral mid-plane — the ONE operation the left track differs by.
fn mirror(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, v.y, v.z)
}

/// Both sides' frames from the template's markers. `shoe_offset` is the shoe centre's outboard bias
/// from the pin plane in MESH-LOCAL units (i.e. already divided by the link scale).
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
/// (`from_rotation_x(-atan2)` maps local `+z` onto the chord, the same construction the game's
/// `track::view` links use) composed with the template correction, so the whole "where is this mesh's
/// nose" question is answered once at build time and never in the hot loop.
fn link_transform(frame: &LinkFrame, scale: f32, lateral_x: f32, a: Vec2, b: Vec2) -> Transform {
    let chord = b - a;
    let rotation = Quat::from_rotation_x(-chord.y.atan2(chord.x)) * frame.correction;
    let mid = (a + b) * 0.5;
    let anchor = Vec3::new(lateral_x, mid.y, mid.x);
    Transform {
        // The anchor is where the frame's ORIGIN must land, and the mesh carries that origin at
        // `scale · origin` before rotation — so the entity's translation backs it out.
        translation: anchor - rotation * (scale * frame.origin),
        rotation,
        scale: Vec3::splat(scale),
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

/// Keep the instance pool the same size as the material loop.
///
/// The link count is LIVE (`;` / `'` retunes it and rebuilds [`RigGeom`] under the running rig), so
/// the pool grows and shrinks with it rather than being sized once at build. Only the delta is
/// spawned or despawned — the whole point of a pool is that a link entity outlives the frame.
fn sync_link_pool(
    mut commands: Commands,
    template: Res<LinkTemplate>,
    geom: Res<RigGeom>,
    hull: Query<Entity, With<Hull>>,
    mut pool: ResMut<LinkPool>,
) {
    let Ok(hull) = hull.single() else {
        return;
    };
    let want = geom.link_count;
    for side in Side::ALL {
        let links = pool.0.get_mut(side);
        if links.len() == want {
            continue;
        }
        for entity in links.drain(want.min(links.len())..) {
            commands.entity(entity).despawn();
        }
        while links.len() < want {
            links.push(
                commands
                    .spawn((
                        TrackLink,
                        Mesh3d(template.mesh.get(side).clone()),
                        MeshMaterial3d(template.material.clone()),
                        // Parked below the world AND hidden until the first placement writes a real
                        // pose: the pool lands via `Commands`, so `place_links` only sees it next
                        // frame, and an un-posed link must not flash on screen in between.
                        Transform::from_xyz(0.0, -1000.0, 0.0),
                        // EXPLICIT, never `Inherited`: the `1` layer hides the hull these links are
                        // children of, and the track is exactly what you want left on screen when
                        // you switch the tank's body off (the same override the wheel layer uses).
                        Visibility::Hidden,
                        ChildOf(hull),
                    ))
                    .id(),
            );
        }
    }
}

/// Place every link on this frame's belt stations.
///
/// The stations are [`super::ConformedBelts`] — whatever the ACTIVE view (`V`: kinematic wrap vs
/// route chain) resampled at the link pitch this frame — so the layer is a real A/B on shoes rather
/// than on a polyline, and both views' articulation and scroll show up without this system knowing
/// which one ran. Link `i` spans stations `i` and `i+1`, the last wrapping to the first: the loop is
/// closed, and `count` stations carry exactly `count` links.
///
/// The entity↔station binding is FIXED, which is sound only because every shoe is identical. The
/// stations resample at `phase mod pitch`, so material identity shifts by one slot every pitch of
/// travel — the game's `track::view` rotates its mapping by the whole-pitch quotient
/// (`phase_decompose`) so its witness-painted link rides the belt instead of wandering. Add the same
/// rotation here the day a link carries anything of its own (a witness paint, damage, a texture);
/// until then it would be machinery with nothing to carry.
///
/// Written in HULL-LOCAL space because the links are children of the hull: the hull's own transform
/// (physics-interpolated) then carries them, so a link can never lag or lead the tank it is bolted
/// to by a frame.
fn place_links(
    template: Res<LinkTemplate>,
    pool: Res<LinkPool>,
    belts: Res<ConformedBelts>,
    geom: Res<RigGeom>,
    viz: Res<VizLayers>,
    mut links: Query<(&mut Transform, &mut Visibility), With<TrackLink>>,
) {
    for side in Side::ALL {
        let entities = pool.0.get(side);
        if !viz.links {
            for &entity in entities {
                if let Ok((_, mut visibility)) = links.get_mut(entity) {
                    visibility.set_if_neq(Visibility::Hidden);
                }
            }
            continue;
        }
        let stations = belts.get(side);
        let n = stations.len();
        if n < 3 {
            continue;
        }
        let frame = template.frame.get(side);
        let lateral_x = geom.link_center_x(side);
        for (i, &entity) in entities.iter().enumerate().take(n) {
            let (a, b) = (stations[i].local, stations[(i + 1) % n].local);
            if a.distance_squared(b) < 1e-8 {
                continue;
            }
            let Ok((mut transform, mut visibility)) = links.get_mut(entity) else {
                continue;
            };
            *transform = link_transform(frame, template.scale, lateral_x, a, b);
            visibility.set_if_neq(Visibility::Visible);
        }
        // A pool that briefly outruns the station list (the frame a link-count bump spawns entities
        // the belt has not resampled for yet) must not leave a stale link hanging in the air.
        for &entity in entities.iter().skip(n) {
            if let Ok((_, mut visibility)) = links.get_mut(entity) {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A FIXTURE template, taken from the 2026-07-23 audit of `tiger_1.glb`: the two pin markers in
    /// `Link`-local space, the node's uniform scale, and the shoe centre's outboard bias from the pin
    /// plane (world metres — `link_center_x - plane_x`).
    ///
    /// These are INPUTS to the pure placement math, not assertions about the shipped file. A
    /// re-export that moves a marker changes what the sandbox draws, and must not turn this suite
    /// red: the geometry the model actually carries is `model.rs`'s to pin, and every number below
    /// is read live at runtime ([`bind_link_template`]).
    const PIN_START: Vec3 = Vec3::new(0.0220282, -0.0323148, 0.0723815);
    const PIN_END: Vec3 = Vec3::new(0.0223190, -0.0323330, -0.0890539);
    const LINK_SCALE: f32 = 0.8079178;
    const SHOE_OUTBOARD: f32 = 0.016_85;

    fn tiger_frames() -> PerSide<LinkFrame> {
        frames(PIN_START, PIN_END, SHOE_OUTBOARD / LINK_SCALE)
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

        // A level chord one pitch long, running front→rear (+z), at the belly.
        let pitch = (PIN_START - PIN_END).with_x(0.0).length() * LINK_SCALE;
        let (a, b) = (Vec2::new(-pitch / 2.0, 0.4), Vec2::new(pitch / 2.0, 0.4));
        let t = link_transform(&frame, LINK_SCALE, 1.548, a, b);
        assert!(t.rotation.angle_between(Quat::IDENTITY) < 0.01);
        assert!((t.scale - Vec3::splat(LINK_SCALE)).length() < 1e-6);

        // The frame's ORIGIN lands on the anchor: the pin midpoint on the route, the shoe's centre
        // on `link_center_x`.
        let origin_world = t.transform_point(frame.origin);
        assert!(
            (origin_world - Vec3::new(1.548, 0.4, 0.0)).length() < 1e-4,
            "origin landed at {origin_world}",
        );

        // ...and the pin midpoint itself sits 16.85 mm INBOARD of it, which is the authored overhang
        // reproduced rather than re-typed.
        let pin_mid = (PIN_START + PIN_END) * 0.5;
        let pin_world = t.transform_point(pin_mid);
        assert!(
            (pin_world.x - (1.548 - SHOE_OUTBOARD)).abs() < 1e-4,
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
        let t = link_transform(&frame, LINK_SCALE, 1.548, a, b);
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
        let right = link_transform(f.get(Side::Right), LINK_SCALE, 1.548, a, b);
        let left = link_transform(f.get(Side::Left), LINK_SCALE, -1.548, a, b);
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
            assert!(t.scale.x > 0.0 && t.scale.y > 0.0 && t.scale.z > 0.0);
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
}
