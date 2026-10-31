//! `OVERMATCH_HULL_MARKS_DEMO=1`, a dev capture of the hull marks (the visual layer of the game's
//! damage model) on the offline route, with `OVERMATCH_TARGET=ahead`'s placement: once the
//! parked target's view has bound, three rays from the player's line of sight find three points on
//! its armour, and each is struck as the authority's march would report it — a hole (`Through`),
//! a dent (`Stopped`), a scratch (`Ricochet`, from a [`GRAZE_DEG`] graze along the face) — at the
//! player's main-gun calibre. The strikes go through the real recorder and the real view; only the
//! rounds are not flown. The camera is then held on each mark in turn, [`SHOW_EVERY_S`] apart from
//! [`FIRST_SHOW_S`], for `OVERMATCH_CAPTURE` stills.
//!
//! The hole, the dent and the scratch, with a hidden window:
//!
//! ```text
//! SPIKE_SIM_WINDOWED=1 OVERMATCH_TARGET=ahead OVERMATCH_HULL_MARKS_DEMO=1 \
//!   OVERMATCH_CAPTURE=<dir>/hull-mark.png OVERMATCH_CAPTURE_AT=18,21,24 \
//!   OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0 target/debug/overmatch --offline
//! ```
//!
//! Mounted only by the offline composition (`run_offline`), as `tank::scenario`'s target-fires
//! scenario is: nothing it raises can leave the process.

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::prelude::*;

use crate::ballistics::{ShellStrike, Verdict};
use crate::camera::{CameraCaptive, CaptiveCameraPlaced};
use crate::damage::VolumeOf;
use crate::tank::{Controlled, SimParts, Tank, ViewNode, Weapon};
use crate::view::PlayerView;
use crate::{CombatantId, Layer, ShotId};

/// The earliest the strikes land, app seconds: the duel has settled on its suspension (TUNED).
const STRIKE_FROM_S: f32 = 12.0;
/// When the camera first holds on a mark, app seconds, and how long it holds each (TUNED: a
/// capture at the middle of each window, `FIRST_SHOW_S + 1.5 + i × SHOW_EVERY_S`).
const FIRST_SHOW_S: f32 = 16.5;
const SHOW_EVERY_S: f32 = 3.0;
/// How far above the target's origin the strikes are aimed, and their spread along the line of
/// sight's right, m: fixture data, on the hull side above the running gear.
const AIM_RISE_M: f32 = 1.2;
const SPREAD_M: f32 = 0.9;
/// The scratch's obliquity, degrees: a graze shallow enough for a bounce.
const GRAZE_DEG: f32 = 75.0;
/// Where the camera sits off a mark: along its normal, then up, then aside, m — a little oblique,
/// so the lit relief shows.
const VIEW_OFF_M: f32 = 1.1;
const VIEW_RISE_M: f32 = 0.3;
const VIEW_ASIDE_M: f32 = 0.35;
const LOOK_PAST_M: f32 = 0.15;

/// The marks struck, in the order the camera shows them: (point, outward normal), WORLD.
#[derive(Resource)]
struct Struck(Vec<(Vec3, Vec3)>);

pub(crate) fn offline_plugin(app: &mut App) {
    if !crate::env_flag("OVERMATCH_HULL_MARKS_DEMO", false) {
        return;
    }
    info!(
        "hull marks demo: armed, strikes from {STRIKE_FROM_S} s, shown every {SHOW_EVERY_S} s \
         from {FIRST_SHOW_S} s"
    );
    app.add_systems(Update, strike_target.run_if(not(resource_exists::<Struck>)))
        .add_systems(
            PostUpdate,
            hold_camera
                .run_if(resource_exists::<Struck>)
                .in_set(CaptiveCameraPlaced),
        );
}

#[expect(
    clippy::too_many_arguments,
    reason = "a dev scenario reading the camera, both tanks, the struck armour and the gun"
)]
fn strike_target(
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<PlayerView>>,
    player: Query<Entity, (With<Tank>, With<Controlled>)>,
    target: Query<(Entity, &GlobalTransform, &SimParts), (With<Tank>, Without<Controlled>)>,
    bound: Query<(), With<ViewNode>>,
    weapons: Query<(&Weapon, &crate::tank::TankRoot)>,
    volumes: Query<&VolumeOf>,
    parents: Query<&ChildOf>,
    spatial: SpatialQuery,
    mut commands: Commands,
) {
    if time.elapsed_secs() < STRIKE_FROM_S {
        return;
    }
    let (Ok(camera), Ok(player)) = (camera.single(), player.single()) else {
        return;
    };
    let Some((target, pose, parts)) = target.iter().next() else {
        return;
    };
    if !parts.0.values().any(|&part| bound.contains(part)) {
        return;
    }
    let caliber = weapons
        .iter()
        .filter(|(_, root)| root.0 == player)
        .flat_map(|(weapon, _)| weapon.rounds.iter())
        .map(|round| round.round.flying.caliber)
        .fold(0.0_f32, f32::max);
    let aim = pose.translation() + Vec3::Y * AIM_RISE_M;
    let forward = (aim - camera.translation()).normalize();
    let right = forward.cross(Vec3::Y).normalize();
    let filter = SpatialQueryFilter::from_mask(Layer::Armor);
    let mut struck = Vec::new();
    for (index, (offset, verdict)) in [
        (-SPREAD_M, Verdict::Through),
        (0.0, Verdict::Stopped),
        (SPREAD_M, Verdict::Ricochet),
    ]
    .into_iter()
    .enumerate()
    {
        let origin = aim + right * offset - forward * 4.0;
        let Some(hit) = spatial.cast_ray(origin, Dir3::new(forward).unwrap(), 10.0, true, &filter)
        else {
            warn!("hull marks demo: ray {index} met no armour");
            continue;
        };
        let volume = std::iter::once(hit.entity)
            .chain(parents.get(hit.entity).ok().map(ChildOf::parent))
            .find(|&entity| volumes.get(entity).is_ok_and(|of| of.tank() == target));
        let Some(volume) = volume else {
            warn!("hull marks demo: ray {index} met armour that is not the target's");
            continue;
        };
        let at = origin + forward * hit.distance;
        let normal = hit.normal.normalize();
        let incoming = if verdict == Verdict::Ricochet {
            let along = normal.cross(Vec3::Y).normalize();
            let graze = GRAZE_DEG.to_radians();
            along * graze.sin() - normal * graze.cos()
        } else {
            forward
        };
        commands.trigger(ShellStrike {
            shot: ShotId {
                combatant: CombatantId(u64::MAX),
                weapon: 0,
                fire_tick: index as u32,
            },
            sequence: 0,
            struck: volume,
            at,
            normal,
            incoming,
            caliber,
            verdict,
        });
        info!("hull marks demo: {verdict:?} at {at:.3}, normal {normal:.3}, calibre {caliber} m");
        struck.push((at, normal));
    }
    commands.insert_resource(Struck(struck));
    commands.insert_resource(CameraCaptive(true));
}

/// Hold the player's camera on the mark whose window the clock is in.
fn hold_camera(
    time: Res<Time>,
    struck: Res<Struck>,
    mut camera: Query<&mut Transform, With<PlayerView>>,
) {
    let (Ok(mut camera), false) = (camera.single_mut(), struck.0.is_empty()) else {
        return;
    };
    let since = (time.elapsed_secs() - FIRST_SHOW_S).max(0.0);
    let index = (since / SHOW_EVERY_S) as usize % struck.0.len();
    let (at, normal) = struck.0[index];
    let aside = normal.cross(Vec3::Y).normalize_or_zero();
    let eye = at + normal * VIEW_OFF_M + Vec3::Y * VIEW_RISE_M + aside * VIEW_ASIDE_M;
    // Looking a little past the mark keeps the aim marker at the screen's centre off it.
    *camera = Transform::from_translation(eye).looking_at(at - aside * LOOK_PAST_M, Vec3::Y);
}
