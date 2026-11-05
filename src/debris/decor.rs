//! The players' decoration, in the game's environment destruction model (game state only): each
//! fragment a crumble shed ([`Shed`]) drawn with its chunks' own mesh and the constructions'
//! material, flying from its event at the shared presentation cursor (ADR-0037). Presentation only:
//! it has no body and no collider, so no hull, round or server body meets it. Its one obstacle is
//! the terrain's height field ([`HeightGrid`]): it rests where a hull corner first reaches the
//! ground closing on it, and shrinks away over the end of its seeded lifetime. The world boundary
//! retires it ([`reconstruct`]).
//!
//! Its flight is a closed form of the event alone (gravity, its velocity and spin), so every
//! player who draws the same event at the same cursor draws the same pose.

use std::collections::VecDeque;

use bevy::prelude::*;

use super::PresentationClock;
use super::crumble::Shed;
use crate::construction::breakage::{Breakage, Piece};
use crate::construction::view::{Look, piece_mesh};
use crate::terrain_grid::HeightGrid;

/// Resolution: the most pieces one player draws at once; each is a mesh entity, so this bounds
/// their draw calls and transform updates. Past it the oldest goes first.
const MOST: usize = 256;

/// Resolution: the step the landing search samples the flight at, s (the sim's 64 Hz tick); a
/// bisection then places the touch within [`TOUCH_M`].
const SEARCH_S: f32 = 1.0 / 64.0;

/// Resolution: how far below the ground a resting piece's lowest corner may sit, m: under a
/// millimetre the bisection has placed the touch finer than the eye can tell.
const TOUCH_M: f32 = 1e-3;

/// A drawn fragment: its flight from the event, its seeded lifetime and its landing.
#[derive(Component, Debug, Clone, PartialEq)]
pub(crate) struct Decoration {
    tick: u32,
    /// The world centre its mesh is drawn about, at the event (m).
    centre: Vec3,
    rotation: Quat,
    velocity: Vec3,
    spin: Vec3,
    /// Seconds after the event it rests from, if it lands within its life.
    lands: Option<f32>,
    life: f32,
}

impl Decoration {
    /// Its pose `t` s after its event, before any fade.
    fn flight(&self, t: f32, gravity: Vec3) -> Transform {
        let t = self.lands.map_or(t, |lands| t.min(lands));
        Transform::from_translation(self.centre + self.velocity * t + 0.5 * gravity * t * t)
            .with_rotation(Quat::from_scaled_axis(self.spin * t) * self.rotation)
    }

    /// Its drawn pose `t` s after its event: its flight, shrunk to nothing over the last `fade`
    /// seconds of its life.
    fn at(&self, t: f32, gravity: Vec3, fade: f32) -> Transform {
        let left = (self.life - t).clamp(0.0, fade);
        self.flight(t, gravity).with_scale(Vec3::splat(left / fade))
    }
}

/// The lowest of `corners` (about the drawn centre) above the ground under it at pose `pose`, m.
fn clearance(corners: &[Vec3], pose: Transform, ground: &impl Fn(Vec3) -> f32) -> f32 {
    corners
        .iter()
        .map(|&corner| {
            let at = pose.transform_point(corner);
            at.y - ground(at)
        })
        .fold(f32::INFINITY, f32::min)
}

/// The first time within `life` its corners reach the ground closing on it (module doc), if they
/// do. A piece that starts on the ground lands there only if its motion closes on it; one moving
/// away or along it (a swept ground-floor patch) flies on until it next closes.
fn landing(
    decoration: &Decoration,
    corners: &[Vec3],
    gravity: Vec3,
    ground: impl Fn(Vec3) -> f32,
) -> Option<f32> {
    let above = |t: f32| clearance(corners, decoration.flight(t, gravity), &ground);
    let start = above(0.0);
    // Its own motion over one search step, gravity aside, tells closing from leaving.
    if start <= 0.0 && clearance(corners, decoration.flight(SEARCH_S, Vec3::ZERO), &ground) < start
    {
        return Some(0.0);
    }
    let steps = (decoration.life / SEARCH_S).ceil() as u32;
    let mut last = start;
    let touch = (1..=steps).find(|&k| {
        let now = above(k as f32 * SEARCH_S);
        let closing = now < last;
        last = now;
        now <= 0.0 && closing
    })?;
    let (mut clear, mut under) = ((touch - 1) as f32 * SEARCH_S, touch as f32 * SEARCH_S);
    if above(clear) <= 0.0 {
        return Some(under); // It never cleared the ground: it rests where it turned to close.
    }
    while above(under) < -TOUCH_M {
        let middle = 0.5 * (clear + under);
        if above(middle) > 0.0 {
            clear = middle;
        } else {
            under = middle;
        }
    }
    Some(under)
}

/// A unit fraction from `seed`, the same on every peer.
fn unit(seed: u32) -> f32 {
    (seed.wrapping_mul(0x9E37_79B9) >> 8) as f32 / (1 << 24) as f32
}

/// The sheds received and not yet due, and the drawn pieces, oldest first.
#[derive(Resource, Default)]
struct Decor {
    due: Vec<Shed>,
    drawn: VecDeque<Entity>,
}

/// What a shed is drawn from: its construction's data, the constructions' material and the ground.
#[derive(bevy::ecs::system::SystemParam)]
struct Sources<'w, 's> {
    constructions: Query<'w, 's, (Entity, &'static Breakage), Without<Piece>>,
    look: Option<Res<'w, Look>>,
    grid: Option<Res<'w, HeightGrid>>,
    meshes: ResMut<'w, Assets<Mesh>>,
}

/// Draw every shed whose tick the cursor has reached, and pose every drawn piece at it.
fn present(
    mut received: MessageReader<Shed>,
    mut decor: ResMut<Decor>,
    clock: Res<PresentationClock>,
    mut sources: Sources,
    mut pieces: Query<(&Decoration, &mut Transform)>,
    mut commands: Commands,
) {
    let Sources {
        constructions,
        look,
        grid,
        meshes,
    } = &mut sources;
    decor.due.extend(received.read().cloned());
    let taste = crate::vfx::construction_decor();
    let gravity = Vec3::NEG_Y * crate::physical_laws::gravity_mps2();
    let tick_s = crate::sim_tick::sim_timestep().as_secs_f64();
    let since = |tick: u32| clock.since(tick).map(|ticks| (ticks * tick_s) as f32);
    // Before a construction is drawn there is nothing to shed from: the look comes with the map.
    if let Some(look) = look.as_deref() {
        let due = std::mem::take(&mut decor.due);
        for shed in due {
            let Some(t) = since(shed.tick).filter(|&t| t >= 0.0) else {
                decor.due.push(shed); // Not due at the cursor yet.
                continue;
            };
            let piece = Piece {
                construction: shed.construction,
                chunks: shed.chunks.clone(),
                born: shed.tick,
                retired: None,
            };
            let admitted = piece
                .admitted(
                    constructions
                        .iter()
                        .find(|(_, breakage)| breakage.construction == shed.construction),
                )
                .expect("a shed is admitted where it enters: the authority's own, or the replica's receive");
            let kind = &admitted.kind;
            let frame = Transform::from_translation(Vec3::from_array(shed.at))
                .with_rotation(Quat::from_array(shed.rotation));
            let corners: Vec<Vec3> = shed
                .chunks
                .iter()
                .flat_map(|&c| kind.layout.blocks[usize::from(c)].points.iter())
                .map(|&p| p - admitted.origin)
                .collect();
            let (low, high) = taste.lifetime_s;
            let mut decoration = Decoration {
                tick: shed.tick,
                centre: frame.transform_point(admitted.origin),
                rotation: frame.rotation,
                velocity: Vec3::from_array(shed.velocity),
                spin: Vec3::from_array(shed.spin),
                lands: None,
                life: low + (high - low) * unit(shed.seed),
            };
            decoration.lands = match grid.as_deref() {
                // A scene with no terrain has no ground to rest on.
                Some(grid) => landing(&decoration, &corners, gravity, |at| {
                    grid.height_at(at.x, at.z)
                }),
                None => None,
            };
            let mesh = meshes.add(piece_mesh(kind, &shed.chunks, admitted.origin));
            let pose = decoration.at(t, gravity, taste.fade_s);
            let entity = commands
                .spawn((
                    Name::new("Decoration"),
                    decoration,
                    pose,
                    Visibility::default(),
                    Mesh3d(mesh),
                    MeshMaterial3d(look.surface.clone()),
                ))
                .id();
            decor.drawn.push_back(entity);
            if decor.drawn.len() > MOST
                && let Some(oldest) = decor.drawn.pop_front()
            {
                commands.entity(oldest).despawn();
            }
        }
    }
    let mut gone = Vec::new();
    for &entity in &decor.drawn {
        let Ok((decoration, mut transform)) = pieces.get_mut(entity) else {
            continue; // Spawned this frame, at its pose.
        };
        let Some(t) = since(decoration.tick) else {
            continue; // The cursor is resyncing; the piece holds its last pose.
        };
        if t >= decoration.life {
            gone.push(entity);
            continue;
        }
        *transform = decoration.at(t.max(0.0), gravity, taste.fade_s);
    }
    decor.drawn.retain(|entity| !gone.contains(entity));
    for entity in gone {
        commands.entity(entity).despawn();
    }
}

/// The world boundary (`crate::world::reconstruct`): every drawn piece goes, and every shed
/// received for the world before, drawn or due, so none stands beside or resolves against the next
/// world's constructions.
pub(crate) fn reconstruct(world: &mut World) {
    if !world.contains_resource::<Decor>() {
        return; // A root that draws no constructions holds no decoration.
    }
    world.insert_resource(Decor::default());
    let drawn: Vec<Entity> = world
        .query_filtered::<Entity, With<Decoration>>()
        .iter(world)
        .collect();
    for entity in drawn {
        assert!(world.despawn(entity), "a drawn piece is its own root");
    }
}

/// Mount the decoration on every root that draws constructions.
pub(crate) fn plugin(app: &mut App) {
    app.add_message::<Shed>()
        .init_resource::<Decor>()
        .add_systems(
            Update,
            present.after(crate::construction::breakage::ReplicaSet),
        );
}

#[cfg(test)]
mod tests;
