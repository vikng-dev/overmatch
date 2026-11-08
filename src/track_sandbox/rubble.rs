//! The rubble yard, in the game's tank driving physics (game state only): a pile of loose brick
//! pieces and a standing wall section off the lane's left edge, both cut from the scene's
//! source construction chunks, for the belts to climb and push ([`crate::track::rubble`]). The `R` tour's last spot
//! parks the rig facing the pile; the harness reaches it with `x=`/`z=`.
//!
//! Gizmos, always on (they draw only where a contact presses on a body): a dot on the contact,
//! cyan on a static body and red on a dynamic one, with a line to the body it pressed; the impulse
//! a dynamic body took draws as an orange arrow of its mean force over the tick at
//! [`force_viz_scale`]. The field casts layer (`9`) shows the field; the contact dots show
//! the footprint contacts.

use std::sync::Arc;

use avian3d::prelude::{
    Collider, ColliderDensity, CollisionLayers, Friction, Position, RigidBody, Rotation,
};
use bevy::prelude::*;

use crate::construction::breakage::{ChunkCollider, chunk_collider, piece_layers};
use crate::construction::{ConstructionKind, ConstructionLibrary};
use crate::debris::DebrisPiece;
use crate::track::rubble::footprint_radius;

use super::belt::EnvelopeLaw;
use super::data::scene;
use super::{RigGeom, Side, belt_loop, contact_probe, force_viz_scale};

/// The `R` tour's rubble spot: the rig's hull origin (xz), facing −Z at the pile, the scene's
/// `rubble_spot_ahead_m` short of it.
pub(super) fn rubble_spot() -> Vec2 {
    let [x, _, z] = scene().rubble_pile_at_m.0;
    Vec2::new(x, z + spot_ahead_m())
}

/// How far the `R` tour's rubble spot stands short of the pile's centre (m): the scene's
/// `rubble_spot_ahead_m`.
pub(crate) fn spot_ahead_m() -> f32 {
    scene().rubble_spot_ahead_m.0
}

/// The scene's rubble source construction (`rubble_construction`), read from the shipped assets.
pub(crate) fn yard_source() -> Arc<ConstructionKind> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let source = scene().rubble_construction.0.as_str();
    Arc::clone(
        ConstructionLibrary::load(&root, [source])
            .get(source)
            .expect("the scene's source construction was loaded"),
    )
}

/// A pile piece's sim body (game state only): what makes its chunk a dynamic piece, before any
/// view.
#[derive(Bundle)]
pub(crate) struct PileBody {
    piece: DebrisPiece,
    transform: Transform,
    position: Position,
    rotation: Rotation,
    body: RigidBody,
    collider: Collider,
    density: ColliderDensity,
    layers: CollisionLayers,
    friction: Friction,
}

/// One pile piece: its chunk, the chunk's centroid (the body's origin, construction frame) and
/// its body.
pub(crate) struct PilePiece {
    pub(crate) chunk: usize,
    pub(crate) centroid: Vec3,
    pub(crate) body: PileBody,
}

/// The yard's sim parts, cut from its source construction.
pub(crate) struct Yard {
    /// The breakable element the wall section stands from.
    pub(crate) wall: usize,
    pub(crate) pile: Vec<PilePiece>,
}

/// The yard's wall element and its pile: the scene's piece count dropped on its square grid about
/// `centre` (ground level), layer over layer, tapered and turned by the scene's taper and yaw
/// step, to settle into a heap. The grid's spacing is twice the farthest block point from any
/// piece's centroid.
pub(crate) fn build_yard(kind: &ConstructionKind, centre: Vec3) -> Yard {
    let yard = scene();
    let source = yard.rubble_construction.0.as_str();
    let layout = &kind.layout;
    let (wall, pieces) = yard_geometry(source, layout, yard.rubble_pile_pieces.0)
        .unwrap_or_else(|error| panic!("{error}"));
    let structure = layout.structure(&kind.asset);
    let (row_count, taper) = (yard.rubble_pile_row.0.width(), yard.rubble_pile_taper.0);
    let spacing = pieces
        .iter()
        .map(|&c| {
            let centroid = structure.chunks[c].centroid;
            layout.blocks[c]
                .points
                .iter()
                .map(|p| (*p - centroid).length())
                .fold(0.0_f32, f32::max)
        })
        .fold(0.0_f32, f32::max)
        * 2.0;
    info!(
        "rubble yard: {} pieces on a {spacing:.2} m drop grid",
        pieces.len()
    );
    let pile = pieces
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let centroid = structure.chunks[c].centroid;
            let (row, col, layer) = (
                i % row_count,
                (i / row_count) % row_count,
                i / yard.rubble_pile_row.0.capacity(),
            );
            let half = (row_count as f32 - 1.0) / 2.0;
            let at = centre
                + Vec3::new(
                    (row as f32 - half) * spacing * (1.0 - taper * layer as f32),
                    spacing * (0.5 + layer as f32),
                    (col as f32 - half) * spacing * (1.0 - taper * layer as f32),
                );
            let turn = Quat::from_rotation_y(i as f32 * yard.rubble_pile_yaw_rad.0);
            PilePiece {
                chunk: c,
                centroid,
                body: PileBody {
                    piece: DebrisPiece,
                    transform: Transform::from_translation(at).with_rotation(turn),
                    position: Position(at),
                    rotation: Rotation(turn),
                    body: RigidBody::Dynamic,
                    collider: chunk_collider(kind, c, centroid),
                    density: ColliderDensity(structure.chunks[c].material.density),
                    layers: piece_layers(),
                    friction: kind.chunk_friction(c),
                },
            }
        })
        .collect();
    Yard { wall, pile }
}

/// The contacts of the last fixed tick that pressed on a body, for the gizmos.
#[derive(Resource, Default)]
pub(super) struct RubbleTouches(pub(super) Vec<Touch>);

pub(super) struct Touch {
    pub(super) point: Vec3,
    pub(super) body: Entity,
    /// Whether the body is dynamic, so took an impulse.
    pub(super) dynamic: bool,
    /// The impulse the body took over the tick, as a mean force (N, world); zero for a static
    /// body.
    pub(super) reaction: Vec3,
}

/// The footprint bound about the hull origin for the rubble candidate query: [`footprint_radius`]
/// over the rig's rest pin line, its widest shoe face and the envelope's largest travel.
pub(super) fn footprint_sphere(geom: &RigGeom, envelope: &EnvelopeLaw) -> f32 {
    let mut pin_radius = 0.0_f32;
    let mut column = 0.0_f32;
    let mut travel = 0.0_f32;
    for side in Side::ALL {
        let x = geom.side_plane_x(side);
        for p in belt_loop(geom.rest.get(side)) {
            pin_radius = pin_radius.max(Vec3::new(x, p.y, p.x).length());
        }
        for (offset, _) in geom.grip_column_offsets(side) {
            column = column.max(offset.abs());
        }
        for &(_, t) in envelope.knots.get(side).iter() {
            travel = travel.max(t);
        }
    }
    footprint_radius(
        pin_radius,
        column,
        geom.model.pin_to_outer,
        travel.max(envelope.free_travel),
        contact_probe(),
    )
}

/// Reserve the largest breakable element for the wall, admitting the entire requested pile.
fn yard_geometry(
    source: &str,
    layout: &crate::construction::breakage::Layout,
    requested: usize,
) -> Result<(usize, Vec<usize>), String> {
    let mut elements: Vec<_> = (0..layout.elements.len())
        .filter(|&e| !layout.chunks_of(e).is_empty())
        .collect();
    elements.sort_by_key(|&e| std::cmp::Reverse(layout.chunks_of(e).len()));
    let refuse = || {
        format!(
            "assets/dev/track_sandbox.ron: rubble_construction {source:?}, rubble_pile_pieces {requested}: insufficient source geometry after reserving the largest breakable element for the wall; choose a source with more breakable elements, author more chunks, or reduce the requested pile count"
        )
    };
    let (&wall, rest) = elements.split_first().ok_or_else(refuse)?;
    let pieces: Vec<_> = rest
        .iter()
        .flat_map(|&e| layout.chunks_of(e))
        .take(requested)
        .collect();
    if pieces.len() != requested {
        return Err(refuse());
    }
    Ok((wall, pieces))
}

/// Spawn the yard (module doc).
pub(super) fn spawn_rubble_yard(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let yard = scene();
    let kind = yard_source();
    let Yard { wall, pile } = build_yard(&kind, Vec3::from_array(yard.rubble_pile_at_m.0));
    let layout = &kind.layout;
    let brick = materials.add(Color::srgb_from_array(yard.rubble_piece_srgb.0));
    let standing = materials.add(Color::srgb_from_array(yard.rubble_wall_srgb.0));
    let [wall_x, wall_z] = yard.rubble_wall_at_m.0;

    // The wall section: one static root, its element's chunks standing on it, turned so its long
    // axis runs along the lane (beside the rig, never across its path).
    let wall_chunks: Vec<usize> = layout.chunks_of(wall).collect();
    let (lo, hi) = wall_chunks
        .iter()
        .flat_map(|&c| layout.blocks[c].points.iter())
        .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| {
            (lo.min(*p), hi.max(*p))
        });
    let turn = if hi.x - lo.x > hi.z - lo.z {
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
    } else {
        Quat::IDENTITY
    };
    let mid = turn * ((lo + hi) / 2.0);
    info!(
        "rubble yard: wall section of {} chunks, {:.2} x {:.2} x {:.2} m",
        wall_chunks.len(),
        hi.x - lo.x,
        hi.y - lo.y,
        hi.z - lo.z,
    );
    let wall_root = commands
        .spawn((
            Name::new("rubble yard wall"),
            Transform::from_xyz(wall_x - mid.x, 0.0, wall_z - mid.z).with_rotation(turn),
            RigidBody::Static,
            Mesh3d(meshes.add(crate::construction::view::piece_mesh(
                &kind,
                &wall_chunks.iter().map(|&c| c as u16).collect::<Vec<_>>(),
                Vec3::ZERO,
            ))),
            MeshMaterial3d(standing),
        ))
        .id();
    for &c in &wall_chunks {
        let collider = chunk_collider(&kind, c, Vec3::ZERO);
        commands.spawn((
            ChildOf(wall_root),
            Transform::IDENTITY,
            collider,
            crate::construction::element_layers(),
            kind.chunk_friction(c),
            ChunkCollider(c as u16),
        ));
    }

    // The pile ([`build_yard`]), each piece with its view.
    for piece in pile {
        commands.spawn((
            Name::new("rubble yard piece"),
            piece.body,
            Mesh3d(meshes.add(crate::construction::view::piece_mesh(
                &kind,
                &[piece.chunk as u16],
                piece.centroid,
            ))),
            MeshMaterial3d(brick.clone()),
        ));
    }
}

/// The contact-on-body layer (module doc).
pub(super) fn draw_rubble(
    mut gizmos: Gizmos,
    touches: Res<RubbleTouches>,
    bodies: Query<&GlobalTransform>,
) {
    let yard = scene();
    for touch in &touches.0 {
        let dynamic = touch.dynamic;
        let [r, g, b] = if dynamic {
            yard.rubble_dynamic_contact_srgb.0
        } else {
            yard.rubble_static_contact_srgb.0
        };
        let color = Color::srgb(r, g, b);
        gizmos.sphere(
            Isometry3d::from_translation(touch.point),
            yard.rubble_contact_radius_m.0,
            color,
        );
        // Last tick’s contact telemetry may outlive a piece removed before this render frame.
        if let Ok(body) = bodies.get(touch.body) {
            gizmos.line(
                touch.point,
                body.translation(),
                color.with_alpha(yard.rubble_contact_link_alpha.0),
            );
        }
        if dynamic {
            let [r, g, b] = yard.rubble_reaction_srgb.0;
            gizmos.arrow(
                touch.point,
                touch.point + touch.reaction * force_viz_scale(),
                Color::srgb(r, g, b),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A requested yard cannot silently lose pile pieces when its source is too small. Fixture
    /// element ranges cover empty, single-element, exact-capacity and insufficient-capacity cases.
    #[test]
    fn rubble_source_must_supply_the_wall_and_the_whole_pile() {
        let mut layout = crate::construction::tests::column_construction()
            .layout
            .clone();
        for elements in [vec![], vec![Some((0, 3))], vec![Some((0, 2)), Some((2, 3))]] {
            layout.elements = elements;
            let requested = 2;
            let error = yard_geometry("fixture-source", &layout, requested).unwrap_err();
            for required in [
                "assets/dev/track_sandbox.ron",
                "fixture-source",
                "rubble_pile_pieces 2",
                "choose a source",
            ] {
                assert!(error.contains(required), "{error}");
            }
        }
        layout.elements = vec![Some((0, 2)), Some((2, 3))];
        assert_eq!(
            yard_geometry("fixture-source", &layout, 1).unwrap(),
            (0, vec![2])
        );
    }
}
