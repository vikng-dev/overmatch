//! The sandbox's adapter onto the shared TRACK-LINK render layer ([`crate::track::link_view`]).
//!
//! Everything about the shoe itself — reading the glb's `Link` template, hiding it, the scale
//! contract, the canonical pin frame, the mirrored left-side mesh, the frame table and its
//! phase-rotated link↔station map, and the belt's GPU draw — lives in the shared modules, because
//! the game draws the SAME shoes from the SAME template and two copies of that is two answers to
//! "what does this tank's track look like".
//!
//! What is genuinely the sandbox's is here and only here: the station source
//! ([`super::ConformedBelts`], rewritten every frame by the wrap view), the LIVE link count (the
//! `;`/`'` knob rebuilds [`RigGeom`] under the running rig, so each belt's table and draw follow
//! it), and the `links` visibility switch — which writes `Visible`/`Hidden` EXPLICITLY rather than
//! `Inherited`, because hiding the hull model must leave the track on screen (the tooth-mesh view).

use bevy::prelude::*;

use crate::track::belt_draw::BeltAssets;
use crate::track::link_view::{BeltFrames, LinkTemplate, ShoeBelt, Stragglers, spawn_belt};

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
                // frame — so the table must land after it, or the shoes would render one frame of
                // lag behind the line drawn through them.
                place_links
                    .after(super::belt::conform_belts_field)
                    .after(sync_link_pool),
            )
                .run_if(resource_exists::<RigGeom>),
        );
}

/// Each side's belt entity, once the hull exists.
#[derive(Resource, Default)]
struct LinkPool(PerSide<Option<Entity>>);

/// Keep each side's belt the size of the material loop.
///
/// The link count is LIVE (`;` / `'` retunes it and rebuilds [`RigGeom`] under the running rig), so
/// a count change resizes the belt's table in place and gives the belt a draw of the new size: the
/// repeated mesh of that count, and a buffer and material sized to the table.
fn sync_link_pool(
    mut commands: Commands,
    template: Res<LinkTemplate>,
    geom: Res<RigGeom>,
    hull: Query<Entity, With<Hull>>,
    mut belts: Query<&mut BeltFrames>,
    mut assets: BeltAssets,
    mut pool: ResMut<LinkPool>,
) {
    let Ok(hull) = hull.single() else {
        return;
    };
    let want = geom.link_count;
    if !assets.ready(&template, want) {
        return;
    }
    for side in Side::ALL {
        let slot = pool.0.get_mut(side);
        match *slot {
            None => {
                let frames = BeltFrames::new(template.frame(side), geom.link_center_x(side), want);
                let Some(draw) = assets.draw(&template, side, &frames) else {
                    continue;
                };
                *slot = Some(spawn_belt(&mut commands, side, hull, frames, draw));
            }
            Some(belt) => {
                let Ok(mut frames) = belts.get_mut(belt) else {
                    continue;
                };
                if frames.count() == want {
                    continue;
                }
                frames.resize(want);
                if let Some(draw) = assets.draw(&template, side, &frames) {
                    commands.entity(belt).insert(draw);
                }
            }
        }
    }
}

/// Lay every belt's table on this frame's stations.
///
/// The stations are [`super::ConformedBelts`] — the kinematic-wrap view's joints, resampled at the
/// link pitch this frame — so the layer draws the model on real shoes rather than on a polyline, and
/// the articulation and scroll show up as the track itself. In HULL-LOCAL space because the belts
/// are children of the hull: the hull's own transform (physics-interpolated) then carries them, so
/// a shoe can never lag or lead the tank it is bolted to by a frame. A link no station reaches
/// (the frame a link-count bump grows the table before the belt resamples) is parked below the
/// belt rather than left hanging in the air.
fn place_links(
    pool: Res<LinkPool>,
    belts: Res<ConformedBelts>,
    phase: Res<BeltPhase>,
    geom: Res<RigGeom>,
    viz: Res<VizLayers>,
    mut tables: Query<(&mut BeltFrames, &mut Visibility), With<ShoeBelt>>,
    // One reusable station buffer: `ConformedBelts` carries world positions alongside the
    // side-plane ones, and the shared fit wants the side-plane points on their own.
    mut stations: Local<Vec<Vec2>>,
) {
    for side in Side::ALL {
        let Some(belt) = *pool.0.get(side) else {
            continue;
        };
        let Ok((mut frames, mut visibility)) = tables.get_mut(belt) else {
            continue;
        };
        // EXPLICIT, never `Inherited`: the hull layer can hide the model the belts are children
        // of, and the track is exactly what you want left on screen when you switch the tank's
        // body off (the same override the wheel layer uses).
        visibility.set_if_neq(if viz.links {
            Visibility::Visible
        } else {
            Visibility::Hidden
        });
        if !viz.links {
            continue;
        }
        stations.clear();
        stations.extend(belts.get(side).iter().map(|s| s.local));
        if frames.bypass_change_detection().fit(
            &stations,
            phase.get(side),
            geom.pitch,
            Stragglers::Park,
        ) {
            frames.set_changed();
        }
    }
}
