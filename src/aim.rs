//! Mouse aiming: a screen-center ray commits the shared aim intention into the tank's
//! [`TankCommand`], which every servo then chases (`drive_aim_servos`) — turret, gun, and the
//! hull MG alike. RMB free-look holds the committed point; the HUD shows the center reticle,
//! green bore dot, and amber aim-point dot.
//!
//! **The intention is HELD hull-locally and TRAVELS in the frame its view is anchored to.**
//! [`CommittedAim`] stores the player's bearing off their own tank, so a hold is hull-rigid and the
//! gun sweeps as the hull turns (ADR-0001, unstabilized WW2 lay). What crosses the wire is an
//! [`AimIntent`] carrying that frame with it: third person is a world-locked orbit camera, so its
//! pick is a WORLD place the delivery gap must not rotate; the gunner optic and free-aim ride the
//! hull, so theirs stays HULL-LOCAL and crosses the gap unchanged (ADR-0038). `drive_aim_servos`
//! names whichever arrives in the hull frame of the tick it lays.
//!
//! The servo drive (`drive_aim_servos`) is mode-agnostic and per-tank: it reads each tank's one
//! commanded aim regardless of who wrote it — the gunner optic (`sight::drive_gunner_aim`)
//! commits from its magnified intent instead of commanding the servos itself, and a network
//! peer's command drives its tank through the exact same path.

use avian3d::prelude::{LayerMask, Position, Rotation, SpatialQuery, SpatialQueryFilter};
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::Layer;
use crate::camera::GunnerCameraPlaced;
use crate::command::{AimIntent, TankCommand};
use crate::damage::{ControlledTank, VolumeOf, hit_ancestor};
use crate::firecontrol::{RangeTable, lob};
use crate::sight::{SightToggled, in_third_person};
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{
    Controlled, Hull, Rig, ServoCommand, ServoRole, Tank, TankRoot, ViewNode, rig_world_pose,
};

/// Maximum engagement range; rays that hit nothing fall back to a point this far out. Shared by
/// every aim ray — the third-person pick, the bore dot, and the optic's resolve
/// (`sight::drive_gunner_aim`) — so "the sky" is one far point in all of them.
pub(crate) const MAX_RANGE: f32 = 10_000.0;

/// Per-axis magnitude a commanded aim point may not exceed, checked where the command meets the sim
/// (`drive_aim_servos`). A legitimate pick is a point inside the world — [`crate::world::VIEW_CAST_MAX_M`]
/// covers its diagonal from any in-world origin — or the [`MAX_RANGE`] sky fallback out from one, so
/// their sum bounds every author with headroom to spare while leaving a poisoned command nowhere near
/// the magnitudes at which the hull composition overflows f32.
pub(crate) const AIM_LIMIT: f32 = crate::world::VIEW_CAST_MAX_M + MAX_RANGE;

/// Distance along an aim ray to the first surface a shell would meet: terrain or a tank's ballistic
/// volumes — the same `Terrain | Armor` mask the live shell marches against
/// (`ballistics::integrate_projectiles`), so the dots predict the shell's truth, tanks included.
/// `own` tank's volumes are excluded: every aim ray legitimately starts inside or behind them (the
/// optic resolve at the gun pivot inside the mantlet, the third-person pick behind own turret, the
/// bore ray at the muzzle inside the barrel's exposed-component volume), and a self-hit would weld
/// the aim to the tank itself. Falls back to `max` on a miss (sky / above the horizon); the CAST
/// itself is clamped to [`crate::world::VIEW_CAST_MAX_M`] — nothing exists between the world
/// diagonal and [`MAX_RANGE`] for a ray to hit, so behavior is identical at a fraction of the
/// parry traversal.
pub(crate) fn aim_distance(
    spatial: &SpatialQuery,
    ray: Ray3d,
    max: f32,
    own: Entity,
    volumes: &Query<&VolumeOf>,
    parents: &Query<&ChildOf>,
) -> f32 {
    // Ownership sits on the hit's ancestry (`hit_ancestor`, the shared hierarchy-resolution rule).
    // No volume in the ancestry ⇒ terrain ⇒ never "own".
    let not_own = |entity: Entity| {
        hit_ancestor(entity, volumes, parents).is_none_or(|(_, owner)| owner.tank() != own)
    };
    spatial
        .cast_ray_predicate(
            ray.origin,
            ray.direction,
            max.min(crate::world::VIEW_CAST_MAX_M),
            true,
            &SpatialQueryFilter::from_mask(
                LayerMask::from(Layer::Terrain) | LayerMask::from(Layer::Armor),
            ),
            &not_own,
        )
        .map(|hit| hit.distance)
        .unwrap_or(max)
}

/// The aim-commit phase: the per-mode input systems that write the command's aim (`commit_aim` in
/// third-person, `sight::drive_gunner_aim` in the optic). Client-side command generation at render
/// rate; the sim (`drive_aim_servos`, fixed clock) consumes whatever intention stands at each tick.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AimCommit;

/// HUD: where the barrel is actually pointing (lags the reticle) — the gun's reality.
#[derive(Component)]
struct BoreIndicator;

/// HUD: the committed aim point — where the gun is *commanded* to point. Shown only during
/// free-look, since otherwise it sits exactly under the center reticle.
#[derive(Component)]
struct AimIndicator;

/// The servo bridge — authority-side: each tick, every tank's servos get targets from its
/// commanded aim.
pub fn sim_plugin(app: &mut App) {
    // In `GameplaySet`, so `drive_servos` (`.after(GameplaySet)`) integrates the fresh targets
    // the same tick. Mode-agnostic and per-tank.
    app.add_systems(FixedUpdate, drive_aim_servos.in_set(GameplaySet));
}

/// **The tank's one committed aim intention** — the single memory both view modes read and write.
/// A hull-local point (ADR-0001: hull-local so it rides with the tank, unstabilized WW2 lay),
/// keyed by tank entity, holding the RAW sight line (pre-superelevation; `drive_aim_servos` adds the
/// lob). This collapses the two former mode-local memories — third-person's free-look hold and the
/// optic's yaw/pitch intent — into the one domain fact they both encoded, so switching modes needs
/// no seeding handoff: whichever mode is active reads this on entry and writes it while active, and
/// the other mode finds the current intention already here (was: seed-on-entry in `toggle_sight` +
/// reseed-on-exit; unified 2026-07-10).
///
/// Three invariants live HERE, at the shared fact, not narrated at each call site:
///
/// - **Recirculation (b206f34):** under net input delay the input bridge
///   (`net::protocol::bridge_action_state_to_tank_command`) rewrites `command.aim` every tick from
///   lightyear's input buffer, so the wire echo can never BE the memory. The active mode must
///   RE-AUTHOR this committed value into `command.aim` every frame — third-person's RMB hold by
///   re-writing it ([`commit_aim`]), the optic by publishing it every frame
///   (`sight::drive_gunner_aim`). Holding is an act, not an omission; fall silent and the buffer
///   recirculates a stale sweep forever (period ≈ D+1 ticks, measured live).
/// - **Possession (entity key):** keyed by tank entity so a possession change (respawn, Tab in SP)
///   can never replay a stale intention onto a new tank — a mismatched key reads as "no
///   commitment" ([`CommittedAim::get`]), exactly the fresh-spawn state.
/// - **Single writer:** exactly one commit system writes this at a time — [`commit_aim`] in third
///   person, `sight::drive_gunner_aim` in the optic. On a toggle frame, only the OUTGOING mode
///   writes: the optic committed in `BeforeFixedMainLoop` while the mode was still Gunner, and
///   `commit_aim` is ordered `.before(SightToggled)` so its run condition is evaluated before the
///   flip (never raced against it — unordered, the executor could run it after a gunner→third
///   flip and commit through the camera's still-gunner `GlobalTransform`). The incoming mode's
///   first write is the NEXT frame, through a camera pose that actually belongs to it.
/// - **Zero-input identity:** a mode transition with zero player input is IDENTITY on this memory.
///   BOTH modes commit RESOLVED WORLD POINTS — third person raycast from the camera, the optic
///   raycast from the gun mount along its sight line (far fallback in the sky) — so the memory
///   holds exactly one domain form and no conversion between a point and a bare direction exists to
///   go wrong (the mount-parallax bug class the 2026-07-10 unification exposed). What keeps the
///   transition identity is the differing RESOLVE ORIGINS: the mount's ray can meet different
///   geometry than the elevated camera's (a crest occludes the lower ray), so the optic must not
///   re-resolve an inherited commitment unprompted. It RESUMES the point into its yaw/pitch working
///   view (measured from the mount), re-authors the ORIGINAL point into `command.aim` (the gun does
///   not move, the reticle does not jump), and leaves this memory untouched until actual optic
///   input (or a fresh tank with no commitment to preserve) re-resolves and re-stores
///   (`sight::resume_commit`).
#[derive(Resource, Default)]
pub(crate) struct CommittedAim(Option<(Entity, Vec3)>);

impl CommittedAim {
    /// This tank's committed hull-local sight-line point, or `None` when the memory is empty or
    /// keyed to a DIFFERENT tank (the possession invariant — a stale intention never replays onto a
    /// new tank; a mismatch reads as no commitment).
    pub(crate) fn get(&self, tank: Entity) -> Option<Vec3> {
        self.0.and_then(|(e, p)| (e == tank).then_some(p))
    }

    /// This tank's held intention as [`commit_aim`] puts it on the wire: the world place the held
    /// bearing stands on under `hull`, the pose of THIS frame.
    ///
    /// **A hold is a bearing, not a place.** The stored value never moves, so the world point it
    /// names sweeps with the hull and the gun rides round with the tank (ADR-0001): no point-hold
    /// can emerge from a player simply stopping picking. Naming it afresh every frame is also why
    /// the value on the wire is never old enough to have been rotated by its own age (ADR-0038).
    ///
    /// `hull` is `None` on a frame whose pose chain will not compose. The bearing then travels as
    /// itself, in the frame it is already measured in: the hold must be re-authored EVERY frame
    /// (recirculation, above), and of the two frames a recirculated value can be stuck in, the
    /// hull-local one rides the tank instead of holding a spot on the ground.
    pub(crate) fn as_intent(&self, tank: Entity, hull: Option<Affine3A>) -> Option<AimIntent> {
        self.get(tank).map(|held| match hull {
            Some(hull) => AimIntent::World(hull.transform_point3(held)),
            None => AimIntent::HullLocal(held),
        })
    }

    /// Commit this tank's intention — the single-writer act performed by the active mode's commit
    /// system. Rekeys to `tank`, so the first write after a possession change adopts the new tank.
    pub(crate) fn set(&mut self, tank: Entity, point: Vec3) {
        self.0 = Some((tank, point));
    }
}

/// The third-person aim commit + HUD dots — client-side: devices → command, and reprojection.
pub fn client_plugin(app: &mut App) {
    app.init_resource::<CommittedAim>()
        .add_systems(Startup, spawn_hud)
        .add_systems(
            Update,
            // Per-mode aim commit: third-person from the screen-center ray; the optic commits
            // from its magnified intent (`sight::drive_gunner_aim`, also in `AimCommit`).
            // `.before(SightToggled)` is the single-writer invariant's ordering half (see
            // [`CommittedAim`]): the run condition is evaluated BEFORE the Lshift flip, so on a
            // toggle frame only the OUTGOING mode's commit runs. Unordered, the executor could run
            // this after the gunner→third-person flip and commit a fresh pick through the camera's
            // still-GUNNER `GlobalTransform` (propagation is PostUpdate; the exit re-aim writes only
            // `Transform`) — a ray from the gun mount along the mid-slew lay, overwriting the
            // player's committed intention with the gun's lag point, intermittently by executor
            // order.
            commit_aim
                .run_if(in_third_person)
                .before(SightToggled)
                .in_set(AimCommit)
                .in_set(PlayerInputSet)
                .in_set(GameplaySet),
        )
        // HUD markers reproject through the camera, so they run after the camera's pose is final
        // for the frame — after propagation and after the gunner camera places itself — or they
        // lag/jitter against the rendered view (worst at the gunner optic's high zoom).
        .add_systems(
            PostUpdate,
            (update_bore_indicator, update_aim_indicator)
                .in_set(GameplaySet)
                .after(TransformSystems::Propagate)
                .after(GunnerCameraPlaced),
        );
}

fn spawn_hud(mut commands: Commands) {
    // Center reticle: a small white dot held at screen center by flexbox.
    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|parent| {
            parent.spawn((
                Node {
                    width: Val::Px(6.0),
                    height: Val::Px(6.0),
                    border_radius: BorderRadius::MAX,
                    ..default()
                },
                BackgroundColor(Color::WHITE),
            ));
        });

    // Green: actual bore. Amber: commanded aim (free-look only). Both hidden until shown.
    commands.spawn((
        BoreIndicator,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(4.0),
            height: Val::Px(4.0),
            border_radius: BorderRadius::MAX,
            ..default()
        },
        BackgroundColor(Color::srgba(0.3, 0.9, 0.4, 0.6)),
        Visibility::Hidden,
    ));
    commands.spawn((
        AimIndicator,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(6.0),
            height: Val::Px(6.0),
            border_radius: BorderRadius::MAX,
            ..default()
        },
        BackgroundColor(Color::srgba(1.0, 0.7, 0.1, 0.7)),
        Visibility::Hidden,
    ));
}

/// The hull's tick-truth pose as an affine — `rig_world_pose` (never `GlobalTransform`: see its
/// doc), composed and checked finite, so a poisoned pose frame reaches neither the shared memory nor
/// the wire. `None` when the chain will not compose or the result is not finite.
fn hull_frame(
    hull: Entity,
    tank: Entity,
    poses: &Query<(&Position, &Rotation)>,
    parents: &Query<&ChildOf>,
    locals: &Query<&Transform>,
) -> Option<Affine3A> {
    let (position, rotation) = poses.get(tank).ok()?;
    let (hull_position, hull_rotation) =
        rig_world_pose(hull, tank, position.0, rotation.0, parents, locals)?;
    let affine = Affine3A::from_rotation_translation(hull_rotation, hull_position);
    (affine.matrix3.is_finite() && affine.translation.is_finite()).then_some(affine)
}

/// Third-person aim commit: a screen-center ray picks the ground point (or a far fallback), held
/// hull-local as the tank's [`CommittedAim`] and authored into its [`TankCommand`] as the WORLD
/// place it is — the orbit camera does not turn with the hull, so what the player picked is a spot
/// on the ground (ADR-0038). RMB free-look holds the committed intention by RE-AUTHORING it every
/// frame, never by falling silent (see [`CommittedAim`]'s recirculation invariant — silence lets the
/// net input buffer recirculate a stale sweep); [`CommittedAim::as_intent`] is what it re-authors.
/// The servos themselves are driven by `drive_aim_servos`, shared with the gunner optic. No
/// commitment yet (first frame, or right after a possession change): author nothing.
fn commit_aim(
    mouse: Res<ButtonInput<MouseButton>>,
    spatial: SpatialQuery,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    window: Single<&Window>,
    controlled: ControlledTank,
    poses: Query<(&Position, &Rotation)>,
    volumes: Query<&VolumeOf>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    mut tank_commands: Query<&mut TankCommand>,
    mut committed: ResMut<CommittedAim>,
) {
    let (Some(tank), Some(rig)) = (controlled.entity(), controlled.rig()) else {
        return;
    };
    let hull = hull_frame(rig.hull, tank, &poses, &parents, &locals);

    // Hold RMB to free-look: the camera still pans, but we stop picking NEW aim points — the
    // committed intention is re-authored every frame instead (recirculation invariant). No
    // commitment yet for THIS tank (free-look from the first frame, or right after a possession
    // change — a mismatched entity key reads as `None`): author nothing, exactly the pre-first-commit
    // state.
    if mouse.pressed(MouseButton::Right) {
        if let Some(aim) = committed.as_intent(tank, hull)
            && let Ok(mut command) = tank_commands.get_mut(tank)
            // Same-value writes are skipped so a still tank sees no change-detection churn; under
            // netcode the bridge changed it this tick, so this restores the intention before the
            // HUD (PostUpdate) and next tick's input sample read.
            && command.aim != Some(aim)
        {
            command.aim = Some(aim);
        }
        return;
    }

    let (camera, cam_transform) = *camera_query;
    let Ok(ray) = camera.viewport_to_world(cam_transform, window.size() / 2.0) else {
        return;
    };

    // Aim at whatever the shell would meet — terrain or another tank — or a far fallback when
    // nothing is struck (sky / above horizon).
    let point = ray.get_point(aim_distance(
        &spatial, ray, MAX_RANGE, tank, &volumes, &parents,
    ));

    // The raw committed point — the player's aim *intention*. The superelevation lob is added
    // downstream in `drive_aim_servos`, so this stays the intention (what the amber HUD dot shows) and
    // the green bore dot ends up the superelevation above it. The memory is a BEARING, so it is
    // stored hull-local; a frame whose hull will not compose still authors the pick, and leaves the
    // memory holding the last bearing it could measure rather than one measured in nothing.
    let aim = Some(AimIntent::World(point));
    if let Ok(mut command) = tank_commands.get_mut(tank)
        // Same-value writes skipped, as in the hold arm above: a parked player looking at a fixed
        // place re-picks the same world point every frame, and must not churn change detection on
        // the replicated input for it.
        && command.aim != aim
    {
        command.aim = aim;
    }
    if let Some(hull) = hull {
        committed.set(tank, hull.inverse().transform_point3(point));
    }
}

/// Drive every servo of every tank at its command's one aim intention — mode-agnostic (the same
/// logic serves third-person and the gunner optic) and per-tank (a network peer's command drives
/// its tank identically). Yaw solves azimuth, Pitch solves elevation, each from its own pose; the
/// hierarchy composes nested mounts, so the turret+gun and the hull MG converge independently with
/// no chain logic here. Whether a mount actually slews is its own gate (`drive_servos`); this just
/// writes the intent. The commanded point is the raw aim *intention*; this bridge lobs it up by
/// the main gun's superelevation for the *commanded* range, so the bore rides above the line of
/// sight while `drive_servos` stays a generic point-chaser. The coax + hull MG ride the gun's lob
/// until per-weapon laying lands.
///
/// The intention arrives carrying the frame it was authored in (ADR-0038) and is named in the hull
/// frame HERE, against the hull pose of the tick being laid: a servo angle is parent-local, so the
/// conversion belongs to whoever drives the servo, at the tick it drives it.
fn drive_aim_servos(
    tanks: Query<(Entity, &TankCommand, &Rig, &Position, &Rotation), With<Tank>>,
    tables: Query<&RangeTable>,
    mut servos: Query<(Entity, &mut ServoCommand, &ServoRole, &TankRoot)>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
) {
    for (tank, command, rig, position, rotation) in &tanks {
        let Some(intent) = command.aim else {
            continue; // no commitment yet — servos hold
        };
        // The command crosses a trust boundary unvalidated (`net::protocol` bridges the action
        // state whole), so this gate is the sim's only guard against a poisoned aim. Finiteness
        // alone does not cover it: a merely LARGE point survives every `is_finite` and then
        // overflows to NaN inside the hull composition below, which reaches the servo targets and
        // the physics state. Bound the magnitude at the same envelope the aim rays already resolve
        // in ([`MAX_RANGE`]) — no legitimate author can name a point outside the one every pick
        // falls back to — with the hull's own world position added, since the point is named in
        // world coordinates and the tank is not at the origin. Hold, like no-commitment.
        let commanded = intent.point();
        if !commanded.is_finite() || commanded.abs().max_element() > AIM_LIMIT {
            continue;
        }
        // Tick-truth hull pose (`rig_world_pose`, never `GlobalTransform` — see its doc): the
        // hull-local aim frame must be the physics state or client and server lay their servos
        // from differently-stale hulls and diverge under maneuver.
        let Some((hull_position, hull_rotation)) =
            rig_world_pose(rig.hull, tank, position.0, rotation.0, &parents, &locals)
        else {
            continue;
        };
        let hull_affine = Affine3A::from_rotation_translation(hull_rotation, hull_position);
        let to_local = hull_affine.inverse();
        // Same NaN discipline as the aim check above, for the pose side (a NaN physics pose on a
        // corrupt frame would poison every servo target below).
        if !(to_local.matrix3.is_finite() && to_local.translation.is_finite()) {
            continue;
        }

        // Lob the raw intention up by the superelevation here (not at commit), so the commanded aim
        // — and its amber HUD dot — stay the intention, while the bore the servos reach is the
        // lobbed point. The lob is a rotation in the hull frame (its plane is spanned by hull up),
        // so the intention is named in that frame first.
        let theta = tables
            .get(rig.muzzle)
            .map_or(0.0, |table| table.superelevation(command.range));
        let point = hull_affine.transform_point3(lob(intent.in_hull(&to_local), theta));
        for (servo, mut servo_command, role, root) in &mut servos {
            if root.0 != tank {
                continue;
            }
            let Some((servo_position, _)) =
                rig_world_pose(servo, tank, position.0, rotation.0, &parents, &locals)
            else {
                continue;
            };
            let dir = to_local.transform_vector3(point - servo_position);
            servo_command.target = match role {
                ServoRole::Yaw => (-dir.x).atan2(-dir.z),
                ServoRole::Pitch => dir.y.atan2((dir.x * dir.x + dir.z * dir.z).sqrt()),
            };
        }
    }
}

/// Project `world_point` to the screen and place a HUD dot there (its top-left offset by
/// `half_size` to centre the dot), hiding it when the point is off-screen or behind the camera.
fn place_indicator(
    node: &mut Node,
    visibility: &mut Visibility,
    camera: &Camera,
    cam_transform: &GlobalTransform,
    world_point: Vec3,
    half_size: f32,
) {
    match camera.world_to_viewport(cam_transform, world_point) {
        Ok(screen) => {
            node.left = Val::Px(screen.x - half_size);
            node.top = Val::Px(screen.y - half_size);
            *visibility = Visibility::Visible;
        }
        Err(_) => *visibility = Visibility::Hidden,
    }
}

fn update_bore_indicator(
    spatial: SpatialQuery,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    muzzle: Query<&GlobalTransform>,
    volumes: Query<&VolumeOf>,
    parents: Query<&ChildOf>,
    mut indicator: Query<(&mut Node, &mut Visibility), With<BoreIndicator>>,
) {
    let (camera, cam_transform) = *camera_query;
    let Ok((tank, rig)) = controlled.single() else {
        return;
    };
    // The VIEW muzzle (design §6C): the bore dot must ride the render-smoothed chain — the sim
    // muzzle steps at tick rate since the sim/view split.
    let Ok(muzzle) = muzzle.get(ViewNode::resolve(
        view_nodes.get(rig.muzzle).ok(),
        rig.muzzle,
    )) else {
        return;
    };
    let Ok((mut node, mut visibility)) = indicator.single_mut() else {
        return;
    };

    // Where the barrel is actually pointing, capped exactly like the aim picker. Fallible
    // direction: for a frame around a networked rig bind (rollback replaying into just-decorated
    // children) the muzzle's GlobalTransform can be zeroed, and `forward()`'s unchecked normalize
    // panics on it — skip the frame instead (measured live, spike step 8).
    let Ok(direction) = Dir3::new(muzzle.rotation() * -Vec3::Z) else {
        return;
    };
    let ray = Ray3d::new(muzzle.translation(), direction);
    let point = ray.get_point(aim_distance(
        &spatial, ray, MAX_RANGE, tank, &volumes, &parents,
    ));

    place_indicator(
        &mut node,
        &mut visibility,
        camera,
        cam_transform,
        point,
        2.0,
    );
}

fn update_aim_indicator(
    mouse: Res<ButtonInput<MouseButton>>,
    // The camera's `Transform`, NOT its `GlobalTransform`. The amber dot marks the player's
    // committed aim *intention*, which `commit_aim` fixes by projecting screen-centre through the
    // camera pose (ADR-0003). The camera is parentless, so its `Transform` IS its world pose,
    // the exact pose `commit_aim` reads, so the dot stays welded to the point it was committed at.
    camera_query: Single<(&Camera, &Transform), With<Camera3d>>,
    controlled: Query<(&Rig, &TankCommand), With<Controlled>>,
    hull: Query<&GlobalTransform, With<Hull>>,
    mut indicator: Query<(&mut Node, &mut Visibility), With<AimIndicator>>,
) {
    let (camera, cam_transform) = *camera_query;
    let cam_transform = GlobalTransform::from(*cam_transform);
    let Ok((mut node, mut visibility)) = indicator.single_mut() else {
        return;
    };

    // Shown only during free-look (RMB held) — otherwise it coincides with the center reticle.
    if !mouse.pressed(MouseButton::Right) {
        *visibility = Visibility::Hidden;
        return;
    }

    let Ok((rig, command)) = controlled.single() else {
        *visibility = Visibility::Hidden;
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };

    // No committed aim yet (before first aim, or free-look from frame one).
    let Some(intent) = command.aim else {
        *visibility = Visibility::Hidden;
        return;
    };

    // The dot marks the intention in whichever frame it was measured; a hull-anchored one rides the
    // RENDERED hull, the pose the player is looking at.
    let world = intent.in_world(&hull.affine());

    place_indicator(
        &mut node,
        &mut visibility,
        camera,
        &cam_transform,
        world,
        3.0,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use avian3d::collider_tree::ColliderTrees;
    use bevy::camera::{ComputedCameraValues, RenderTargetInfo};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::mouse::AccumulatedMouseMotion;

    use super::*;
    use crate::firecontrol::{RangeTable, Ranging};
    use crate::sight::drive_gunner_aim;
    use crate::tank::{ServoIndex, ServoRest, ServoSpec, TankServos, drive_servos};

    /// The fixed clock the servos integrate on.
    const TICK: f32 = 1.0 / 64.0;
    /// The delivery gap an intention crosses between authoring and consumption, in ticks: the
    /// shipping input delay (`net::client::SHIPPING_INPUT_DELAY_TICKS`, 3) plus the render frame the
    /// commit is authored in. The floor of what the authority sees, and what the client sees too —
    /// ADR-0037 gives it no prediction. Named here rather than imported: `aim` is sim code and may
    /// not depend on the netcode layer (`tests/net_boundary.rs`). No assertion below reads it, so a
    /// drift changes only how hard the laws are pushed, never whether one can pass falsely.
    const DELIVERY_TICKS: usize = 3 + 1;
    /// The seed vehicle's authored turret traverse (`assets/tiger_1/tiger_1.tank.ron`): the numbers
    /// are the fixture's, never a law — every assertion below is stated as a formula over them.
    const YAW_SPEED: f32 = 34.0;
    const YAW_ACCEL: f32 = 70.0;
    const PITCH_SPEED: f32 = 23.0;
    const PITCH_ACCEL: f32 = 70.0;

    /// The physics root's world position: far from the origin, so a hull frame composed with
    /// `transform_vector3` where it owes `transform_point3` (or the reverse) moves every number
    /// below instead of cancelling.
    const ROOT: Vec3 = Vec3::new(137.0, 4.0, -512.0);
    /// The hull's offset under the root, so the hull frame's translation is not the root's either.
    const HULL_OFFSET: Vec3 = Vec3::new(0.3, 0.8, -0.4);
    /// Where the third-person camera watches the tank from.
    const EYE: Vec3 = Vec3::new(150.0, 16.0, -496.0);
    /// The superelevation the fixture's muzzle lobs by, at every range (radians). Non-zero so the
    /// lob is never the identity and its frame is under measurement in every law below; large
    /// enough that a lob taken in the wrong frame moves the bore by degrees, not by noise.
    const SUPERELEVATION: f32 = 0.05;

    /// A tank driven by the SHIPPED systems: `commit_aim` or `sight::drive_gunner_aim` authors the
    /// intention, `drive_aim_servos` bridges it to servo targets, `tank::drive_servos` integrates
    /// the real mechanism at the seed vehicle's authored rates. Nothing here re-implements a
    /// transport; the fixture only supplies devices, poses and the delivery gap.
    ///
    /// **The two hulls are the point.** A frame authors against the hull the CLIENT is rendering
    /// (ADR-0037: the own hull is the interpolated stream, which lightyear's clamp freezes and
    /// steps under jitter) and lays against the hull the AUTHORITY actually has. Their difference
    /// is the noise any frame round-trip imports, and it is measurable here.
    ///
    /// The turret, gun and muzzle share one origin, so a converged bore points EXACTLY at the point
    /// it was commanded to and every residual measured is the transport's or the servo's, never rig
    /// parallax. The hull does NOT share the root's, and neither sits at the world origin.
    struct AimRig {
        world: World,
        tank: Entity,
        hull: Entity,
        turret: Entity,
        gun: Entity,
        camera: Entity,
        /// The delivery gap, as the queue it is: what the servos consume this tick is what the
        /// player authored [`DELIVERY_TICKS`] ticks ago.
        in_flight: VecDeque<Option<AimIntent>>,
        /// What the shipped commit system put on the wire this frame, before the gap swallowed it.
        authored: Option<AimIntent>,
    }

    impl AimRig {
        fn new() -> Self {
            let mut world = World::new();
            let mut time = Time::<()>::default();
            time.advance_by(Duration::from_secs_f32(TICK));
            world.insert_resource(time);
            world.init_resource::<CommittedAim>();
            world.init_resource::<Ranging>();
            world.init_resource::<ButtonInput<MouseButton>>();
            world.init_resource::<AccumulatedMouseMotion>();
            // An empty collider set: every aim ray misses, so `aim_distance` returns the shared sky
            // fallback and the picked point is a pure bearing off the camera.
            world.init_resource::<ColliderTrees>();
            world.spawn(Window::default());

            let camera = world
                .spawn((
                    Camera {
                        computed: ComputedCameraValues {
                            clip_from_view: Mat4::perspective_infinite_reverse_rh(
                                0.9,
                                16.0 / 9.0,
                                0.1,
                            ),
                            target_info: Some(RenderTargetInfo {
                                physical_size: UVec2::new(1280, 720),
                                scale_factor: 1.0,
                            }),
                            ..default()
                        },
                        ..default()
                    },
                    GlobalTransform::default(),
                ))
                .id();

            let hull = world.spawn(Transform::from_translation(HULL_OFFSET)).id();
            let turret = world.spawn(Transform::IDENTITY).id();
            let gun = world.spawn(Transform::IDENTITY).id();
            let muzzle = world
                .spawn((Transform::IDENTITY, RangeTable::test_fixed(SUPERELEVATION)))
                .id();
            let tank = world
                .spawn((
                    Tank,
                    Controlled,
                    TankCommand::default(),
                    Position(ROOT),
                    Rotation::default(),
                    Transform::from_translation(ROOT),
                    Rig {
                        hull,
                        turret,
                        gun,
                        muzzle,
                    },
                    TankServos::for_count(2),
                ))
                .id();
            world.entity_mut(hull).insert(ChildOf(tank));
            world.entity_mut(turret).insert(ChildOf(hull));
            world.entity_mut(gun).insert(ChildOf(turret));
            world.entity_mut(muzzle).insert(ChildOf(gun));
            // One ballistic volume, so the root carries the `TankVolumes` the servo's requirement
            // gate resolves through; the authored requirement is empty, so effectiveness is full.
            world.spawn(VolumeOf(tank));

            let mut mount = |servo: Entity, role: ServoRole, slot: usize, speed, accel| {
                world.entity_mut(servo).insert((
                    ServoSpec::test_continuous(role, speed, accel),
                    ServoRest(Quat::IDENTITY),
                    ServoIndex(slot),
                    TankRoot(tank),
                    ServoCommand::default(),
                    role,
                ));
            };
            mount(turret, ServoRole::Yaw, 0, YAW_SPEED, YAW_ACCEL);
            mount(gun, ServoRole::Pitch, 1, PITCH_SPEED, PITCH_ACCEL);

            Self {
                world,
                tank,
                hull,
                turret,
                gun,
                camera,
                in_flight: VecDeque::from(vec![None; DELIVERY_TICKS]),
                authored: None,
            }
        }

        /// Stand the physics root at `yaw_deg`. Both the authoring pass and the laying pass go
        /// through here, with the pose each of them actually sees.
        fn stand_at(&mut self, yaw_deg: f32) {
            self.stand_with(Quat::from_rotation_y(yaw_deg.to_radians()));
        }

        /// Stand the physics root at an arbitrary attitude — a hull off the level, where the hull's
        /// up and the world's up are different axes.
        fn stand_with(&mut self, attitude: Quat) {
            self.world.entity_mut(self.tank).insert(Rotation(attitude));
        }

        /// The hull's world attitude — the frame the servos solve in, and the frame the lob claims.
        fn hull_attitude(&mut self) -> Quat {
            let (tank, hull) = (self.tank, self.hull);
            self.pose_of(hull, tank).1
        }

        /// `rig_world_pose` for one rig node, run as the shipped composer rather than re-derived.
        fn pose_of(&mut self, node: Entity, tank: Entity) -> (Vec3, Quat) {
            self.world
                .run_system_once(
                    move |poses: Query<(&Position, &Rotation)>,
                          parents: Query<&ChildOf>,
                          locals: Query<&Transform>| {
                        let (position, rotation) =
                            poses.get(tank).expect("the root carries a physics pose");
                        rig_world_pose(node, tank, position.0, rotation.0, &parents, &locals)
                            .expect("the node hangs under the root")
                    },
                )
                .expect("the pose probe runs")
        }

        /// Point the third-person camera at `target` — the world-locked orbit view, whose
        /// screen-centre ray is what `commit_aim` picks along.
        fn watch(&mut self, target: Vec3) {
            self.world
                .entity_mut(self.camera)
                .insert(GlobalTransform::from(
                    Transform::from_translation(EYE).looking_at(target, Vec3::Y),
                ));
        }

        fn right_mouse(&mut self, held: bool) {
            let mut mouse = self.world.resource_mut::<ButtonInput<MouseButton>>();
            if held {
                mouse.press(MouseButton::Right);
            } else {
                mouse.release(MouseButton::Right);
            }
        }

        /// One whole frame across the seam: the shipped author writes against the hull the client
        /// renders, the intention crosses the delivery gap, and the shipped servo bridge plus the
        /// real mechanism lay it against the hull the authority has.
        fn frame(&mut self, rendered_yaw: f32, true_yaw: f32, author: Author) {
            self.stand_at(rendered_yaw);
            match author {
                Author::ThirdPerson => self.run(commit_aim),
                Author::Optic => self.run(drive_gunner_aim),
            }

            self.authored = self.command_aim();
            self.in_flight.push_back(self.authored);
            let delivered = self.in_flight.pop_front().expect("the queue is primed");
            self.world
                .entity_mut(self.tank)
                .get_mut::<TankCommand>()
                .expect("the tank carries a command")
                .aim = delivered;

            self.stand_at(true_yaw);
            self.run(drive_aim_servos);
            self.run(drive_servos);
        }

        /// Lay an intention that is already standing against a hull at `true_yaw`, with no fresh
        /// authoring — the authority's half alone, which is also what a starved input buffer does
        /// with the last command it received.
        fn relay(&mut self, intent: AimIntent, true_yaw: f32) {
            self.relay_with(intent, Quat::from_rotation_y(true_yaw.to_radians()));
        }

        /// [`Self::relay`] against an arbitrary hull attitude.
        fn relay_with(&mut self, intent: AimIntent, attitude: Quat) {
            self.world
                .entity_mut(self.tank)
                .get_mut::<TankCommand>()
                .expect("the tank carries a command")
                .aim = Some(intent);
            self.stand_with(attitude);
            self.run(drive_aim_servos);
            self.run(drive_servos);
        }

        fn run<M, S: bevy::ecs::system::IntoSystem<(), (), M> + 'static>(&mut self, system: S) {
            self.world
                .run_system_once(system)
                .expect("the shipped system runs");
        }

        fn command_aim(&self) -> Option<AimIntent> {
            self.world
                .get::<TankCommand>(self.tank)
                .expect("the tank carries a command")
                .aim
        }

        /// The world place the last third-person pick named — read back off what the commit system
        /// authored, so the test never re-implements the camera ray it is measuring.
        fn picked(&self) -> Vec3 {
            match self.authored.expect("a pick was authored") {
                AimIntent::World(point) => point,
                AimIntent::HullLocal(point) => panic!(
                    "the world-locked orbit view must name a place, got the hull-local {point}"
                ),
            }
        }

        /// The bearing off the hull the turret is COMMANDED to: its parent-local target, degrees.
        fn hull_relative_bearing_deg(&self) -> f32 {
            self.world
                .get::<ServoCommand>(self.turret)
                .expect("the turret carries a servo command")
                .target
                .to_degrees()
        }

        /// The world bearing the turret is COMMANDED to — its parent-local target composed with the
        /// hull heading that target was solved against.
        fn commanded_bearing_deg(&self) -> f32 {
            let (hull_yaw, _, _) = self
                .world
                .get::<Rotation>(self.tank)
                .expect("the root carries a physics rotation")
                .to_euler(EulerRot::YXZ);
            self.hull_relative_bearing_deg() + hull_yaw.to_degrees()
        }

        /// How far the bore misses `target`, decomposed IN `frame` into `(azimuth, elevation)`
        /// degrees — the same two angles the servos are commanded in, so a miss reads back as the
        /// mount that owns it.
        ///
        /// Which frame is the question, not a detail. The lay is solved in the HULL's frame and the
        /// superelevation lob is a rotation in that frame, so a converged bore owes zero azimuth and
        /// exactly the superelevation of elevation THERE. Pass `Quat::IDENTITY` to ask the same
        /// question of the world, which answers differently the moment the hull leaves the level.
        ///
        /// Both angles are `atan2`-based, never `acos` of a dot product: near 1 that loses most of
        /// the f32 mantissa, and these are angles that should be zero.
        fn bore_miss_deg(&mut self, target: Vec3, frame: Quat) -> (f32, f32) {
            let (tank, gun) = (self.tank, self.gun);
            let (position, rotation) = self.pose_of(gun, tank);
            let to_frame = frame.inverse();
            let bore = to_frame * (rotation * Vec3::NEG_Z);
            let line = to_frame * (target - position).normalize();
            let azimuth = |v: Vec3| (-v.x).atan2(-v.z);
            let elevation = |v: Vec3| v.y.atan2(v.xz().length());
            (
                (azimuth(bore) - azimuth(line)).to_degrees(),
                (elevation(bore) - elevation(line)).to_degrees(),
            )
        }
    }

    /// Which shipped commit system authors a frame.
    #[derive(Clone, Copy)]
    enum Author {
        /// `commit_aim` — the world-locked orbit view.
        ThirdPerson,
        /// `sight::drive_gunner_aim` — the hull-anchored optic.
        Optic,
    }

    /// **The transport law, third person: a pick is a PLACE.** The orbit camera does not turn with
    /// the hull, so what the player picked is a spot on the ground — and a spot cannot go stale.
    /// Both halves of the seam must say so.
    ///
    /// The author half: the same look, over two different hull headings, must name the same place.
    /// A commit that measured its pick against the hull names two places 20° apart.
    ///
    /// The consumer half: that one place, laid against a hull it never saw, must still put the bore
    /// on it — the case a starved buffer creates, when the authority holds the last command it got
    /// while the hull turns on under it. A hull-local transport re-applies the value to the later
    /// hull and misses by exactly the heading change.
    #[test]
    fn a_third_person_pick_names_a_place_whatever_the_hull_is_doing() {
        const TURN_DEG: f32 = 20.0;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);

        let mut rig = AimRig::new();
        rig.watch(target);
        rig.frame(0.0, 0.0, Author::ThirdPerson);
        let at_rest = rig.picked();
        rig.frame(TURN_DEG, TURN_DEG, Author::ThirdPerson);
        let turned = rig.picked();

        let drift = at_rest.distance(turned);
        assert!(
            drift < 1e-3,
            "one look must name one place: the same camera ray authored {drift:.3} m apart across \
             a {TURN_DEG}° heading change, so the pick was measured against the hull",
        );

        // The value authored before the turn, laid against the turned hull for the whole slew.
        let intent = AimIntent::World(at_rest);
        for _ in 0..256 {
            rig.relay(intent, TURN_DEG);
        }
        let hull = rig.hull_attitude();
        let (azimuth, elevation) = rig.bore_miss_deg(at_rest, hull);
        assert!(
            azimuth.abs() < 0.02,
            "a converged bore must sit on the place the player picked; missing its bearing by \
             {azimuth:.3}° means the intention rode the hull round (a hull-local transport misses \
             by the full {TURN_DEG}° heading change)",
        );
        let lobbed = SUPERELEVATION.to_degrees();
        assert!(
            (elevation - lobbed).abs() < 0.02,
            "the bore must sit exactly the commanded superelevation above the intention — the lob \
             is the only thing entitled to move it off the line of sight — got {elevation:.3}° \
             against {lobbed:.3}°",
        );
    }

    /// **The pivot residual is the servo's, and only the servo's.** The player HOLDS THE CROSSHAIR
    /// on a target — an act, re-picked from the screen-centre ray every frame — while the hull
    /// pivots at ω. The bore must trail by exactly what a rate-limited mount costs and by nothing
    /// else, WHICHEVER WAY the hull turns.
    ///
    /// The mechanism's own floor is its braking envelope's fixed point: `drive_servos` commands
    /// `v = sqrt(2·a·ε)`, so sustaining ω needs a standing error of `ω²/2a`, less the tick the
    /// target moves before the mount answers. That floor is symmetric in ω. A transport that rotates
    /// with the hull is not: it biases the lay by `ω·(delivery gap)` in the direction of the turn,
    /// which partly CANCELS the mount's lag on one side and adds to it on the other — which is why
    /// both directions are run, and why a single-direction measurement can flatter the bug.
    ///
    /// The lay follows the aiming view, and the player owns the view: third person's camera is
    /// world-locked, so while the player is LOOKING at a place the gunner lays on that place,
    /// rate-limited by the mount. That is ratified crew behaviour off live input, not a software
    /// hold — the holds are `a_held_bearing_sweeps_the_gun_with_the_hull` (ADR-0038).
    #[test]
    fn a_steady_pivot_leaves_only_the_mounts_rate_limited_lag() {
        /// Hull yaw rate, degrees/second — a brisk neutral-steer pivot.
        const OMEGA: f32 = 10.0;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);

        // The braking envelope's standing error, one integrated step past the fixed point. It
        // depends on |ω| alone: the mount does not care which way the hull went.
        let lag = OMEGA * OMEGA / (2.0 * YAW_ACCEL) - OMEGA.abs() * TICK;
        let transport = OMEGA * DELIVERY_TICKS as f32 * TICK;

        for omega in [OMEGA, -OMEGA] {
            let mut rig = AimRig::new();
            rig.watch(target);
            rig.frame(0.0, 0.0, Author::ThirdPerson);
            let place = rig.picked();
            for t in 1..512 {
                let yaw = omega * t as f32 * TICK;
                rig.frame(yaw, yaw, Author::ThirdPerson);
            }
            let hull = rig.hull_attitude();
            let residual = rig.bore_miss_deg(place, hull).0.abs();
            assert!(
                (residual - lag).abs() < 0.05,
                "at ω = {omega}°/s the standing lag must be the mount's own ω²/2a − |ω|·dt = \
                 {lag:.3}°, got {residual:.3}°; an intention that rotates in flight biases it by \
                 ±ω·delivery = {transport:.3}°, one way on each side",
            );
        }
    }

    /// **A jittering hull cannot shake a world-anchored bearing.** The interpolated hull does not
    /// turn smoothly — under jitter lightyear's clamp freezes it, then steps it — while the
    /// authority's turns on. An intention that travels hull-local is authored on one stair and
    /// decomposed on another, so the commanded bearing swings by the whole step and back, every
    /// period, while the player holds the crosshair perfectly still. A world place admits no hull
    /// pose, so the commanded bearing is simply where the player put it.
    ///
    /// SCOPE: this is a law about the COMMANDED bearing, which is all the transport owns. The
    /// rendered bore still staircases here, because the hull's step carries the whole tank and the
    /// rate-limited mount can only walk it back — the interpolated hull's smoothness to answer for,
    /// and nothing in ADR-0038 touches it (see "what this does not fix").
    #[test]
    fn a_frozen_then_stepped_hull_does_not_beat_the_third_person_bearing() {
        const OMEGA: f32 = 10.0;
        /// Ticks the interpolated hull holds still before it catches up in one jump.
        const FREEZE_TICKS: u32 = 6;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);

        let mut rig = AimRig::new();
        rig.watch(target);
        rig.frame(0.0, 0.0, Author::ThirdPerson);
        let place = rig.picked();
        let mut swing = (f32::MAX, f32::MIN);
        for t in 0..512u32 {
            // The clamp's staircase: the yaw the client SHOWS is held at the last window boundary
            // until the next arrival steps it on; the authority's runs smoothly through.
            let shown = (t / FREEZE_TICKS * FREEZE_TICKS) as f32;
            rig.frame(
                OMEGA * shown * TICK,
                OMEGA * t as f32 * TICK,
                Author::ThirdPerson,
            );
            // Past the priming ticks, so every sample is of a fully delivered intention.
            if t >= 64 {
                let bearing = rig.commanded_bearing_deg();
                swing = (swing.0.min(bearing), swing.1.max(bearing));
            }
        }

        let peak_to_peak = swing.1 - swing.0;
        let hull_step = OMEGA * FREEZE_TICKS as f32 * TICK;
        // The turret ring is bolted to the hull and the hull is offset from the pivot, so the ring
        // TRANSLATES on a circle of that offset as the tank turns: the bearing from it to a fixed
        // place genuinely shifts. That parallax — not zero — is the floor, and the square wave a
        // hull-local transport beats out is two orders above it.
        let parallax = (2.0 * HULL_OFFSET.xz().length() / (place - ROOT).length())
            .atan()
            .to_degrees();
        assert!(
            peak_to_peak < parallax,
            "the commanded bearing must not move while the intention stands still, whatever the \
             client's rendered hull does — got {peak_to_peak:.4}° of swing against the turret \
             ring's own {parallax:.4}° of parallax, and the {hull_step:.3}° square wave a \
             hull-local transport beats out at the clamp's period",
        );
    }

    /// **The doctrine guard: a hold is a bearing, not a place.** Free-look means looking AWAY from
    /// the gun, and the gun must ride the hull round while the player does it — ADR-0001's
    /// unstabilized lay. The world transport must never make that emerge as stabilization: the
    /// player picks nothing for the whole run, the hull pivots under them, and the gun must sweep
    /// with it rather than counter-rotating to hold the spot it started on.
    ///
    /// A hold's trace is the exact inverse of a point-hold — the world bearing sweeps by the hull's
    /// whole turn, the bearing off the hull stands still — so a memory that held the world point
    /// instead fails both halves.
    #[test]
    fn a_held_bearing_sweeps_the_gun_with_the_hull() {
        const OMEGA: f32 = 10.0;
        /// Ticks skipped before sampling, so every sample is of a fully delivered intention.
        const SETTLE: u32 = 64;
        const RUN: u32 = 512;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);

        let mut rig = AimRig::new();
        rig.watch(target);
        rig.frame(0.0, 0.0, Author::ThirdPerson);
        rig.right_mouse(true);

        let (mut world_span, mut hull_span) = ((f32::MAX, f32::MIN), (f32::MAX, f32::MIN));
        for t in 1..RUN {
            let yaw = OMEGA * t as f32 * TICK;
            rig.frame(yaw, yaw, Author::ThirdPerson);
            if t >= SETTLE {
                let (world, local) = (rig.commanded_bearing_deg(), rig.hull_relative_bearing_deg());
                world_span = (world_span.0.min(world), world_span.1.max(world));
                hull_span = (hull_span.0.min(local), hull_span.1.max(local));
            }
        }

        let swept = world_span.1 - world_span.0;
        let hull_turned = OMEGA * (RUN - 1 - SETTLE) as f32 * TICK;
        assert!(
            (swept - hull_turned).abs() < 0.05,
            "a held bearing must sweep the gun with the hull: the commanded world bearing moved \
             {swept:.3}° while the hull turned {hull_turned:.3}°. Equal means the gun rode round; \
             zero means the aim held a spot on the world and the mount stabilized onto it",
        );
        assert!(
            hull_span.1 - hull_span.0 < 1e-3,
            "the bearing off the hull is what the player is holding, so nothing may move it — got \
             {:.4}° of drift over a {hull_turned:.3}° pivot",
            hull_span.1 - hull_span.0,
        );
    }

    /// **The optic's intention crosses the gap untouched.** The gunner optic is a hull-anchored
    /// view: the camera rides the hull, the working intent is a yaw/pitch off the hull, and the gun
    /// is bolted to it. A bearing off the hull is therefore already invariant across the delivery
    /// gap, and naming it through the CLIENT's interpolated hull only to have the authority
    /// decompose it against the TRUE hull would import the difference between the two as noise in
    /// the fired lay — for nothing.
    ///
    /// So: the player holds the sight still (no mouse motion — the optic's hold path) while the
    /// client's rendered hull freezes and steps under jitter and the authority's turns on smoothly.
    /// The commanded bearing off the hull must not move at all.
    #[test]
    fn the_optic_intent_crosses_the_delivery_gap_untouched() {
        const OMEGA: f32 = 10.0;
        const FREEZE_TICKS: u32 = 6;
        /// The bearing the gunner is holding, in the hull's frame.
        const HELD: Vec3 = Vec3::new(60.0, 0.0, -500.0);

        let mut rig = AimRig::new();
        let tank = rig.tank;
        rig.world.resource_mut::<CommittedAim>().set(tank, HELD);

        let mut span = (f32::MAX, f32::MIN);
        for t in 0..512u32 {
            let shown = (t / FREEZE_TICKS * FREEZE_TICKS) as f32;
            rig.frame(OMEGA * shown * TICK, OMEGA * t as f32 * TICK, Author::Optic);
            assert!(
                matches!(rig.authored, Some(AimIntent::HullLocal(_))),
                "the optic authors in the frame it is anchored to",
            );
            if t >= 64 {
                let bearing = rig.hull_relative_bearing_deg();
                span = (span.0.min(bearing), span.1.max(bearing));
            }
        }

        let peak_to_peak = span.1 - span.0;
        let hull_step = OMEGA * FREEZE_TICKS as f32 * TICK;
        assert!(
            peak_to_peak < 1e-4,
            "a hull-anchored intention must reach the authority exactly as authored — got \
             {peak_to_peak:.4}° of swing in the commanded bearing off the hull, which a round trip \
             through the world imports from the {hull_step:.3}° gap between the client's rendered \
             hull and the authority's",
        );
    }

    /// **A hold names the place it was picked at.** The laws above measure how the lay MOVES, and a
    /// span is blind to the value it moves around: a memory that took the pick through the wrong
    /// transform — `transform_vector3` dropping the hull's translation, or a world point stored
    /// straight into the hull-local slot — holds a CONSTANT bearing, hundreds of metres from
    /// anything the player looked at, and every span law still passes. So pin the value at both ends
    /// of the memory.
    ///
    /// At rest, releasing the pick must change nothing: the re-authored intention is the picked
    /// place itself. Under a pivot it must be that same bearing swept rigidly round the hull — which
    /// is the closed form of ADR-0001, and also what the third-person → optic handover resumes onto
    /// (`sight::resume_commit` reads this memory, and a garbage bearing there is a gun that jumps on
    /// the toggle).
    #[test]
    fn a_held_intention_still_names_the_place_it_was_picked_at() {
        const TURN_DEG: f32 = 20.0;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);
        /// Where the hull's origin stands at a given heading — the point the held bearing is
        /// measured FROM, which is not the physics root and not the world origin.
        fn hull_origin(yaw_deg: f32) -> Vec3 {
            ROOT + Quat::from_rotation_y(yaw_deg.to_radians()) * HULL_OFFSET
        }

        let mut rig = AimRig::new();
        rig.watch(target);
        rig.frame(0.0, 0.0, Author::ThirdPerson);
        let place = rig.picked();

        rig.right_mouse(true);
        rig.frame(0.0, 0.0, Author::ThirdPerson);
        let slip = rig.picked().distance(place);
        assert!(
            slip < 1e-2,
            "letting go of the pick may not move the intention: the hold re-authored {slip:.3} m \
             from the place it was committed at, so the memory took it through the wrong transform",
        );

        rig.frame(TURN_DEG, TURN_DEG, Author::ThirdPerson);
        let swept = rig.picked();
        let rigid = Quat::from_rotation_y(TURN_DEG.to_radians()) * (place - hull_origin(0.0))
            + hull_origin(TURN_DEG);
        let error = swept.distance(rigid);
        assert!(
            error < 1e-2,
            "a hold is a bearing off the hull, so a {TURN_DEG}° pivot must carry it rigidly round: \
             the re-authored place stands {error:.3} m off where that bearing points",
        );
    }

    /// **The lob is a rotation in the HULL's frame.** Superelevation raises the bore above the line
    /// of sight in the gun's own plane of elevation — the plane the pitch mount swings in, spanned
    /// by hull up — so on a tank standing on a slope the lob tilts with the tank.
    ///
    /// Level, the hull's up and the world's up are the same axis and the claim is untestable. Rolled
    /// and pitched, they are not: the same lay taken in the world frame puts the bore somewhere the
    /// mount cannot even reach without traversing, and the two answers separate by degrees.
    #[test]
    fn the_superelevation_lob_rides_the_hull_not_the_world() {
        /// The hull's roll — the whole reason the two frames have different answers. Tilting the
        /// plane of elevation by this projects the lob onto the world's vertical as `cos ROLL`.
        const ROLL: f32 = 0.45;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);
        let attitude = Quat::from_euler(EulerRot::YXZ, 0.35, 0.0, ROLL);

        let mut rig = AimRig::new();
        for _ in 0..512 {
            rig.relay_with(AimIntent::World(target), attitude);
        }

        let lobbed = SUPERELEVATION.to_degrees();
        let hull = rig.hull_attitude();
        let (azimuth, in_hull) = rig.bore_miss_deg(target, hull);
        assert!(
            azimuth.abs() < 0.02 && (in_hull - lobbed).abs() < 0.02,
            "in the hull's frame a converged bore owes zero bearing error and exactly the \
             {lobbed:.3}° lob, got {azimuth:.3}° and {in_hull:.3}°",
        );

        // The tilt's own prediction, halved: the sighting geometry perturbs it, so hold the frames
        // to separating by the order the roll demands rather than to a fitted number.
        let separation = lobbed * (1.0 - ROLL.cos());
        let (_, in_world) = rig.bore_miss_deg(target, Quat::IDENTITY);
        assert!(
            lobbed - in_world > separation * 0.5,
            "the two frames must actually disagree on a hull rolled {ROLL} rad, or the law above \
             proves nothing: the world frame reads {in_world:.3}° against the hull frame's \
             {in_hull:.3}°, a separation of {separation:.3}° short",
        );
    }

    /// **A poisoned aim cannot reach a servo.** `net::protocol`'s bridge copies the action state
    /// into the command whole, unvalidated, so an aim authored by a hostile or broken client arrives
    /// at `drive_aim_servos` exactly as sent — and that gate is the sim's only guard.
    ///
    /// Finiteness is not the whole of it. A merely LARGE point passes every `is_finite` and then
    /// overflows inside the hull composition, so the bound is on magnitude too, and it binds BOTH
    /// frames: the tag is a claim about a frame, never a warrant. What must survive is the last good
    /// lay — the servo target unmoved, and every pose still finite.
    #[test]
    fn a_poisoned_aim_moves_no_servo() {
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);
        let huge = AIM_LIMIT * 1.01;

        for poison in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, huge, -huge] {
            for framed in [AimIntent::World, AimIntent::HullLocal] {
                let mut rig = AimRig::new();
                for _ in 0..256 {
                    rig.relay(AimIntent::World(target), 0.0);
                }
                let good = rig.hull_relative_bearing_deg();

                rig.relay(framed(Vec3::splat(poison)), 0.0);
                assert_eq!(
                    rig.hull_relative_bearing_deg(),
                    good,
                    "a {poison} aim in {:?} moved the turret's target off its last good lay",
                    framed(Vec3::ZERO),
                );
                let (_, rotation) = rig.pose_of(rig.gun, rig.tank);
                assert!(
                    rotation.is_finite(),
                    "a {poison} aim in {:?} poisoned the gun's pose",
                    framed(Vec3::ZERO),
                );
            }
        }
    }

    /// The possession invariant: [`CommittedAim`] is keyed by tank entity, so it hands back a
    /// commitment ONLY for the tank it was set on — a different entity (a possession change:
    /// respawn, Tab) reads as "no commitment", never a stale intention replayed onto the new tank.
    #[test]
    fn committed_aim_is_entity_keyed() {
        // Two distinct entities from a throwaway world (real ids, no `from_raw` guesswork).
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        assert_ne!(a, b);

        let mut committed = CommittedAim::default();
        assert_eq!(committed.get(a), None, "empty memory has no commitment");

        let point = Vec3::new(1.0, 2.0, -3.0);
        committed.set(a, point);
        assert_eq!(
            committed.get(a),
            Some(point),
            "the keyed tank reads its aim"
        );
        assert_eq!(
            committed.get(b),
            None,
            "a different tank reads no commitment (stale intention never replays)"
        );

        // The first write after a possession change rekeys to the new tank.
        let point_b = Vec3::new(-4.0, 0.0, 5.0);
        committed.set(b, point_b);
        assert_eq!(committed.get(b), Some(point_b));
        assert_eq!(
            committed.get(a),
            None,
            "the old tank's key is gone once rekeyed"
        );
    }
}
