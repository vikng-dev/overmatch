//! Mouse aiming: a screen-center ray commits the shared aim intention into the tank's
//! [`TankCommand`], which every servo then chases (`drive_aim_servos`) — turret, gun, and the
//! hull MG alike. RMB free-look holds the committed point; the HUD shows the center reticle,
//! green bore dot, and amber aim-point dot. The committed intention is hull-local, so it rides
//! with the tank (WW2: no gun stabilization). Storing it in world space instead would be the
//! modern-stabilization split.
//!
//! The servo drive (`drive_aim_servos`) is mode-agnostic and per-tank: it reads each tank's one
//! commanded aim regardless of who wrote it — the gunner optic (`sight::drive_gunner_aim`)
//! commits from its magnified intent instead of commanding the servos itself, and a network
//! peer's command drives its tank through the exact same path.

use avian3d::prelude::{Position, Rotation, SpatialQuery};
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::camera::{CameraKickApplied, GunnerCameraPlaced};
use crate::command::TankCommand;
use crate::damage::ControlledTank;
use crate::firecontrol::{RangeTable, lob};
use crate::sight::in_third_person;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{
    Controlled, Hull, Rig, ServoCommand, ServoRole, Tank, TankRoot, ViewNode, rig_world_pose,
};
use crate::world::ground_distance;

/// Maximum engagement range; rays that hit nothing fall back to a point this far out.
const MAX_RANGE: f32 = 10_000.0;

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
///   person, `sight::drive_gunner_aim` in the optic. Their run conditions (`in_third_person` /
///   `in_gunner`) are mutually exclusive on every frame but the gunner→third-person toggle frame,
///   where both may write once — schedule-ordered (`BeforeFixedMainLoop` then `Update`), never
///   raced, and last-writer-wins lands on the mode the player just entered.
#[derive(Resource, Default)]
pub(crate) struct CommittedAim(Option<(Entity, Vec3)>);

impl CommittedAim {
    /// This tank's committed hull-local sight-line point, or `None` when the memory is empty or
    /// keyed to a DIFFERENT tank (the possession invariant — a stale intention never replays onto a
    /// new tank; a mismatch reads as no commitment).
    pub(crate) fn get(&self, tank: Entity) -> Option<Vec3> {
        self.0.and_then(|(e, p)| (e == tank).then_some(p))
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
            commit_aim
                .run_if(in_third_person)
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
                .after(GunnerCameraPlaced)
                // After the hit-kick has displaced the camera's rendered `GlobalTransform`. The GREEN
                // bore dot (`update_bore_indicator`) reads that kicked pose, so it jolts with the
                // rendered view and the whole sight picture shakes together on a hit — matching the
                // gunner reticles in `sight`. The AMBER intention dot (`update_aim_indicator`)
                // deliberately reads the un-kicked camera `Transform` instead (see its body), so this
                // edge is only load-bearing for the bore dot. Vacuous edge in SP/headless (the kick set
                // is net-client-only, empty there).
                .after(CameraKickApplied),
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

/// Third-person aim commit: a screen-center ray picks the ground point (or a far fallback), stored
/// hull-local as the tank's [`CommittedAim`] and re-authored into its [`TankCommand`]. RMB free-look
/// holds the committed intention by RE-AUTHORING it every frame, never by falling silent (see
/// [`CommittedAim`]'s recirculation invariant — silence lets the net input buffer recirculate a
/// stale sweep). The servos themselves are driven by `drive_aim_servos`, shared with the gunner
/// optic. No commitment yet (first frame, or right after a possession change): author nothing.
fn commit_aim(
    mouse: Res<ButtonInput<MouseButton>>,
    spatial: SpatialQuery,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    window: Single<&Window>,
    controlled: ControlledTank,
    hull: Query<&GlobalTransform, With<Hull>>,
    mut tank_commands: Query<&mut TankCommand>,
    mut committed: ResMut<CommittedAim>,
) {
    let (Some(tank), Some(rig)) = (controlled.entity(), controlled.rig()) else {
        return;
    };

    // Hold RMB to free-look: the camera still pans, but we stop picking NEW aim points — the
    // committed intention is re-authored every frame instead (recirculation invariant). No
    // commitment yet for THIS tank (free-look from the first frame, or right after a possession
    // change — a mismatched entity key reads as `None`): author nothing, exactly the pre-first-commit
    // state.
    if mouse.pressed(MouseButton::Right) {
        if let Some(aim) = committed.get(tank)
            && let Ok(mut command) = tank_commands.get_mut(tank)
            // Same-value writes are skipped so SP (where the hold already sticks) sees no
            // change-detection churn; under netcode the bridge changed it this tick, so this
            // restores the intention before the HUD (PostUpdate) and next tick's input sample read.
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

    // Aim at the ground hit, or a far fallback when nothing is struck (sky / above horizon).
    let point = ray.get_point(ground_distance(&spatial, ray, MAX_RANGE));

    // Stored in the hull's local frame so aim stays correct wherever the tank sits/turns.
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    // Store the raw committed point — the player's aim *intention*. The superelevation lob is added
    // downstream in `drive_aim_servos`, so this stays the intention (what the amber HUD dot shows) and
    // the green bore dot ends up the superelevation above it.
    if let Ok(mut command) = tank_commands.get_mut(tank) {
        let local = hull.affine().inverse().transform_point3(point);
        command.aim = Some(local);
        committed.set(tank, local);
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
fn drive_aim_servos(
    tanks: Query<(Entity, &TankCommand, &Rig, &Position, &Rotation), With<Tank>>,
    tables: Query<&RangeTable>,
    mut servos: Query<(Entity, &mut ServoCommand, &ServoRole, &TankRoot)>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
) {
    for (tank, command, rig, position, rotation) in &tanks {
        let Some(local) = command.aim else {
            continue; // no commitment yet — servos hold
        };
        // A non-finite intention would NaN the servo targets and cascade into the physics state —
        // and under MP the command crosses a trust boundary (a client with a zeroed camera/hull
        // transform, or a hostile one, must not be able to poison the authority's sim). Hold, like
        // no-commitment.
        if !local.is_finite() {
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

        // Lob the raw intention up by the superelevation here (not at commit), so the commanded aim
        // — and its amber HUD dot — stay the intention, while the bore the servos reach is the
        // lobbed point.
        let theta = tables
            .get(rig.muzzle)
            .map_or(0.0, |table| table.superelevation(command.range));
        let hull_affine = Affine3A::from_rotation_translation(hull_rotation, hull_position);
        let point = hull_affine.transform_point3(lob(local, theta));
        let to_local = hull_affine.inverse();
        // Same NaN discipline as the aim check above, for the pose side (a NaN physics pose on a
        // corrupt frame would poison every servo target below).
        if !(to_local.matrix3.is_finite() && to_local.translation.is_finite()) {
            continue;
        }
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
    controlled: Query<&Rig, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    muzzle: Query<&GlobalTransform>,
    mut indicator: Query<(&mut Node, &mut Visibility), With<BoreIndicator>>,
) {
    let (camera, cam_transform) = *camera_query;
    let Ok(rig) = controlled.single() else {
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
    let point = ray.get_point(ground_distance(&spatial, ray, MAX_RANGE));

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
    // The camera's un-kicked `Transform`, NOT its `GlobalTransform`. The amber dot marks the player's
    // committed aim *intention*, which `commit_aim` fixes by projecting screen-centre through the
    // un-kicked (stabilized) camera pose (ADR-0003). `net::hit_feel::apply_camera_kick` displaces only
    // the rendered `GlobalTransform` — a decaying, re-excited-every-hit recoil offset. Reprojecting a
    // FROZEN intention (free-look holds `command.aim`) through that shaking pose makes the marker jitter
    // between two positions and never settle while you are under fire, even though the intention is rock
    // steady — the regression. The camera is parentless, so its `Transform` IS its un-kicked world pose,
    // the exact pose `commit_aim` reads, so the dot stays welded to the point it was committed at while
    // the green bore dot and the gunner reticles still jolt with the kicked view (the sight picture jolt).
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
    let Some(local) = command.aim else {
        *visibility = Visibility::Hidden;
        return;
    };

    // Hull-local -> world, so the dot rides with the hull (unstabilized WW2 behaviour).
    let world = hull.affine().transform_point3(local);

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
    use super::*;

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
