//! The sandbox's adapter onto the shared TRACK-LINK render layer ([`crate::track::link_view`]).
//!
//! Everything about the shoe itself — reading the glb's `Link` template, hiding it, the scale
//! contract, the canonical pin frame, the mirrored left-side mesh, the placement math and the
//! phase-rotated entity↔station map — lives in the shared module, because the game instances the
//! SAME shoes from the SAME template and two copies of that is two answers to "what does this tank's
//! track look like".
//!
//! What is genuinely the sandbox's is here and only here: the station source
//! ([`super::ConformedBelts`], rewritten every frame by the wrap view), the LIVE link count (the
//! `;`/`'` knob rebuilds [`RigGeom`] under the running rig, so the pool grows and shrinks with it),
//! and the `links` visibility switch — which writes `Visible`/`Hidden` EXPLICITLY rather than
//! `Inherited`, because hiding the hull model must leave the track on screen (the tooth-mesh view).

use bevy::prelude::*;

use crate::track::link_view::{
    LinkTemplate, TrackLink, place_links as place_shared, spawn_belt, spawn_link,
};

use super::belt::BeltPhase;
use super::rig_geom::RigGeom;
use super::{ConformedBelts, Hull, PerSide, Side, VizLayers};

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<LinkPool>()
        // The template read + the template hide: shared with the game, mounted once per binary.
        .add_plugins(crate::track::link_view::template_plugin)
        .add_systems(
            Update,
            (
                sync_link_pool.run_if(resource_exists::<LinkTemplate>),
                // The stations come from `ConformedBelts`, which the view system rewrites every
                // frame — so the links must land after it, or they would render one frame of lag
                // behind the line drawn through them.
                place_links
                    .run_if(resource_exists::<LinkTemplate>)
                    .after(super::belt::conform_belts_field)
                    .after(sync_link_pool),
            )
                .run_if(resource_exists::<RigGeom>),
        );
}

/// The instanced links, per side, under that side's belt entity.
#[derive(Resource, Default)]
struct LinkPool(PerSide<SideBelt>);

/// One side's belt entity and the shoes hanging from it.
#[derive(Default)]
struct SideBelt {
    /// The `ShoeBelt` entity — spawned once under the hull, and the one that selects the rung every
    /// shoe on this side draws. `None` until the hull exists.
    belt: Option<Entity>,
    links: Vec<Entity>,
}

/// The conform reach the sandbox's belt view probes with — how far below the rest envelope a drawn
/// shoe can be, and therefore part of the belt's own selection radius.
const CONFORM_REACH: f32 = 0.5;

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
        let pool = pool.0.get_mut(side);
        let belt = *pool.belt.get_or_insert_with(|| {
            spawn_belt(
                &mut commands,
                side,
                geom.belt_radius(side, CONFORM_REACH),
                hull,
            )
        });
        if pool.links.len() == want {
            continue;
        }
        for entity in pool.links.drain(want.min(pool.links.len())..) {
            commands.entity(entity).despawn();
        }
        while pool.links.len() < want {
            pool.links
                .push(spawn_link(&mut commands, &template, side, belt));
        }
    }
}

/// Place every link on this frame's belt stations.
///
/// The stations are [`super::ConformedBelts`] — the kinematic-wrap view's joints, resampled at the
/// link pitch this frame — so the layer draws the model on real shoes rather than on a polyline, and
/// the articulation and scroll show up as the track itself. Written in HULL-LOCAL space because the
/// links are children of the hull: the hull's own transform (physics-interpolated) then carries
/// them, so a link can never lag or lead the tank it is bolted to by a frame.
fn place_links(
    template: Res<LinkTemplate>,
    pool: Res<LinkPool>,
    belts: Res<ConformedBelts>,
    phase: Res<BeltPhase>,
    geom: Res<RigGeom>,
    viz: Res<VizLayers>,
    mut links: Query<(&mut Transform, &mut Visibility), With<TrackLink>>,
    // One reusable station buffer: `ConformedBelts` carries world positions alongside the
    // side-plane ones, and the shared placer wants the side-plane points on their own.
    mut stations: Local<Vec<Vec2>>,
) {
    for side in Side::ALL {
        let entities = &pool.0.get(side).links;
        if !viz.links {
            for &entity in entities {
                if let Ok((_, mut visibility)) = links.get_mut(entity) {
                    visibility.set_if_neq(Visibility::Hidden);
                }
            }
            continue;
        }
        stations.clear();
        stations.extend(belts.get(side).iter().map(|s| s.local));
        let frame = template.frame(side);
        place_shared(
            &frame,
            geom.link_center_x(side),
            &stations,
            phase.get(side),
            geom.pitch,
            entities,
            |entity, pose| {
                let Ok((mut transform, mut visibility)) = links.get_mut(entity) else {
                    return;
                };
                match pose {
                    Some(pose) => {
                        *transform = pose;
                        // EXPLICIT, never `Inherited`: the hull layer can hide the model these links
                        // are children of, and the track is exactly what you want left on screen
                        // when you switch the tank's body off (the same override the wheel layer
                        // uses).
                        visibility.set_if_neq(Visibility::Visible);
                    }
                    // A pool that briefly outruns the station list (the frame a link-count bump
                    // spawns entities the belt has not resampled for yet) must not leave a stale
                    // link hanging in the air.
                    None => {
                        visibility.set_if_neq(Visibility::Hidden);
                    }
                }
            },
        );
    }
}
