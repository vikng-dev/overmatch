//! The rubble yard, in the game's tank driving physics (game state only): a pile of loose brick
//! pieces and a standing wall section off the lane's left edge, both cut from the shipped house's
//! own chunks, for the belts to climb and push ([`crate::track::rubble`]). The `R` tour's last spot
//! parks the rig facing the pile; the harness reaches it with `x=`/`z=`.
//!
//! Gizmos, always on (they draw only where a contact presses on a body): a dot on the contact,
//! cyan on a static body and red on a dynamic one, with a line to the body it pressed; the impulse
//! a dynamic body took draws as an orange arrow of its mean force over the tick at
//! [`FORCE_VIZ_SCALE`]. The casts layer (`9`) colours each probe that lands in a body red.

use avian3d::prelude::{ColliderDensity, Position, RigidBody, Rotation};
use bevy::prelude::*;

use crate::buildings::BuildingLibrary;
use crate::buildings::breakage::{BuildingChunk, chunk_collider, piece_layers};
use crate::debris::DebrisPiece;
use crate::track::rubble::{RubbleBodies, RubbleOracle, RubbleQuery, probe_radius};

use super::belt::{EnvelopeLaw, TerrainField};
use super::{CONTACT_PROBE, FORCE_VIZ_SCALE, RigGeom, Side, belt_loop};

/// The building asset the yard is cut from.
const HOUSE: &str = "house_1";
/// Where the pile's base centre stands (world, on the lane's ground at y = 0): on the open flat
/// ground left of the washboards.
pub(super) const PILE_AT: Vec3 = Vec3::new(-13.0, 0.0, -8.0);
/// Where the wall section's chunks' mean stands (xz only; its height is the asset's own).
const WALL_AT: Vec2 = Vec2::new(-18.0, -8.0);
/// How many pieces the pile holds.
const PILE_PIECES: usize = 108;
/// Pieces per layer of the drop grid (a square).
const PILE_ROW: usize = 6;
/// The `R` tour's rubble spot: the rig's hull origin (xz), facing −Z at the pile.
pub(super) const RUBBLE_SPOT: Vec2 = Vec2::new(PILE_AT.x, PILE_AT.z + 9.0);

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

/// The probe sphere about the hull origin for [`RubbleQuery::near`]: [`probe_radius`] over the
/// rig's rest pin line, its widest grip column and the envelope's largest travel.
pub(super) fn probe_sphere(geom: &RigGeom, envelope: &EnvelopeLaw) -> f32 {
    let mut pin_radius = 0.0_f32;
    let mut column = 0.0_f32;
    let mut travel = 0.0_f32;
    for side in Side::ALL {
        let x = side.plane_x(geom.plane_x);
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
    probe_radius(
        pin_radius,
        column,
        geom.model.pin_to_outer,
        travel.max(envelope.free_travel),
        CONTACT_PROBE,
    )
}

/// The rig's oracle this tick: the course field with the rubble `near` the hull folded on, the
/// pieces' velocities read from `bodies`.
pub(super) fn oracle<'a, 'w, 's>(
    field: &'a TerrainField,
    rubble: &'a RubbleQuery<'w, 's>,
    near: &'a [Entity],
    bodies: &'a RubbleBodies,
) -> RubbleOracle<'a, 'w, 's, crate::track::oracle::BlockField> {
    RubbleOracle {
        ground: &field.0,
        rubble,
        near,
        bodies,
    }
}

/// Spawn the yard (module doc).
pub(super) fn spawn_rubble_yard(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let library = BuildingLibrary::load(&root, [HOUSE]);
    let kind = library.get(HOUSE).expect("the shipped house loads").clone();
    let layout = &kind.layout;
    let structure = layout.structure(&kind.asset, 1.0);
    let brick = materials.add(Color::srgb(0.62, 0.33, 0.24));
    let standing = materials.add(Color::srgb(0.55, 0.42, 0.36));

    // The breakable elements, most chunks first: the first is the wall section, the rest feed
    // the pile.
    let mut elements: Vec<usize> = (0..kind.asset.elements.len())
        .filter(|&e| !layout.chunks_of(e).is_empty())
        .collect();
    elements.sort_by_key(|&e| std::cmp::Reverse(layout.chunks_of(e).len()));
    let Some((&wall, rest)) = elements.split_first() else {
        warn!("rubble yard: {HOUSE} has no breakable element");
        return;
    };

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
            Transform::from_xyz(WALL_AT.x - mid.x, 0.0, WALL_AT.y - mid.z).with_rotation(turn),
            RigidBody::Static,
            Mesh3d(meshes.add(crate::buildings::view::piece_mesh(
                &kind.asset,
                layout,
                &wall_chunks.iter().map(|&c| c as u16).collect::<Vec<_>>(),
                Vec3::ZERO,
            ))),
            MeshMaterial3d(standing),
        ))
        .id();
    for &c in &wall_chunks {
        let Some(collider) = chunk_collider(&kind, c, 1.0, Vec3::ZERO, 0.0) else {
            continue;
        };
        commands.spawn((
            ChildOf(wall_root),
            Transform::IDENTITY,
            collider,
            crate::buildings::element_layers(),
            BuildingChunk(c as u16),
        ));
    }

    // The pile: pieces dropped on a square grid, layer over layer, to settle into a heap.
    let pieces: Vec<usize> = rest
        .iter()
        .flat_map(|&e| layout.chunks_of(e))
        .take(PILE_PIECES)
        .collect();
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
    for (i, &c) in pieces.iter().enumerate() {
        let centroid = structure.chunks[c].centroid;
        let Some(collider) = chunk_collider(&kind, c, 1.0, centroid, 0.0) else {
            continue;
        };
        let (row, col, layer) = (
            i % PILE_ROW,
            (i / PILE_ROW) % PILE_ROW,
            i / (PILE_ROW * PILE_ROW),
        );
        let half = (PILE_ROW as f32 - 1.0) / 2.0;
        let at = PILE_AT
            + Vec3::new(
                (row as f32 - half) * spacing * (1.0 - 0.15 * layer as f32),
                spacing * (0.5 + layer as f32),
                (col as f32 - half) * spacing * (1.0 - 0.15 * layer as f32),
            );
        let turn = Quat::from_rotation_y(i as f32 * 0.7);
        commands.spawn((
            Name::new("rubble yard piece"),
            DebrisPiece,
            Transform::from_translation(at).with_rotation(turn),
            Position(at),
            Rotation(turn),
            RigidBody::Dynamic,
            collider,
            ColliderDensity(structure.chunks[c].material.density),
            piece_layers(),
            Mesh3d(meshes.add(crate::buildings::view::piece_mesh(
                &kind.asset,
                layout,
                &[c as u16],
                centroid,
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
    for touch in &touches.0 {
        let dynamic = touch.dynamic;
        let color = if dynamic {
            Color::srgb(1.0, 0.2, 0.2)
        } else {
            Color::srgb(0.2, 0.9, 1.0)
        };
        gizmos.sphere(Isometry3d::from_translation(touch.point), 0.05, color);
        if let Ok(body) = bodies.get(touch.body) {
            gizmos.line(touch.point, body.translation(), color.with_alpha(0.5));
        }
        if dynamic {
            gizmos.arrow(
                touch.point,
                touch.point + touch.reaction * FORCE_VIZ_SCALE,
                Color::srgb(1.0, 0.5, 0.1),
            );
        }
    }
}
