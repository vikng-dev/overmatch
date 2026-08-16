//! Mouse aiming: a screen-center ray commits the shared aim intention into the tank's
//! [`TankCommand`], which every servo then chases (`drive_aim_servos`) — turret, gun, and the
//! hull MG alike. RMB free-look holds the committed point; the HUD shows the center reticle,
//! green bore dot, and amber aim-point dot.
//!
//! The intention is a WORLD point (ADR-0038). It crosses the input delay, the wire and the
//! interpolation delay unchanged, so a turning hull cannot rotate it in flight; the hull-local
//! form the servos need is derived by `drive_aim_servos` from the hull pose at the tick it lays
//! them. Free-look therefore holds a spot on the world, not a bearing off the hull.
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
use crate::command::TankCommand;
use crate::damage::{ControlledTank, VolumeOf, hit_ancestor};
use crate::firecontrol::{RangeTable, lob};
use crate::sight::{SightToggled, in_third_person};
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{
    Controlled, Rig, ServoCommand, ServoRole, Tank, TankRoot, ViewNode, rig_world_pose,
};

/// Maximum engagement range; rays that hit nothing fall back to a point this far out. Shared by
/// every aim ray — the third-person pick, the bore dot, and the optic's resolve
/// (`sight::drive_gunner_aim`) — so "the sky" is one far point in all of them.
pub(crate) const MAX_RANGE: f32 = 10_000.0;

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
/// A world point (ADR-0038), keyed by tank entity, holding the RAW sight line
/// (pre-superelevation; `drive_aim_servos` adds the lob). This collapses the two former
/// mode-local memories — third-person's free-look hold and the
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
///   BOTH modes commit POINTS RESOLVED AGAINST THE WORLD — third person raycast from the camera,
///   the optic raycast from the gun mount along its sight line (far fallback in the sky) — so the
///   memory
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
    /// This tank's committed world sight-line point, or `None` when the memory is empty or
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
        // The local view's own-gun lay, on the fixed clock, ahead of the servo bridge that consumes
        // it. `GameplaySet` puts it after the net input bridge, which writes the delayed echo this
        // replaces.
        .add_systems(
            FixedUpdate,
            lay_own_aim_from_the_live_intention
                .before(drive_aim_servos)
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

/// Third-person aim commit: a screen-center ray picks the ground point (or a far fallback), stored
/// as the tank's [`CommittedAim`] and re-authored into its [`TankCommand`]. RMB free-look holds the
/// committed intention by RE-AUTHORING it every frame, never by falling silent (see
/// [`CommittedAim`]'s recirculation invariant — silence lets the net input buffer recirculate a
/// stale sweep); the held point is a spot on the world, so free-look pans the camera and leaves the
/// gun where the player put it. The servos themselves are driven by `drive_aim_servos`, shared with
/// the gunner optic. No commitment yet (first frame, or right after a possession change): author
/// nothing.
fn commit_aim(
    mouse: Res<ButtonInput<MouseButton>>,
    spatial: SpatialQuery,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    window: Single<&Window>,
    controlled: ControlledTank,
    volumes: Query<&VolumeOf>,
    parents: Query<&ChildOf>,
    mut tank_commands: Query<&mut TankCommand>,
    mut committed: ResMut<CommittedAim>,
) {
    let Some(tank) = controlled.entity() else {
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

    // Aim at whatever the shell would meet — terrain or another tank — or a far fallback when
    // nothing is struck (sky / above horizon).
    let point = ray.get_point(aim_distance(
        &spatial, ray, MAX_RANGE, tank, &volumes, &parents,
    ));

    // Store the raw committed point — the player's aim *intention*. The superelevation lob is added
    // downstream in `drive_aim_servos`, so this stays the intention (what the amber HUD dot shows) and
    // the green bore dot ends up the superelevation above it.
    if let Ok(mut command) = tank_commands.get_mut(tank) {
        command.aim = Some(point);
        committed.set(tank, point);
    }
}

/// Lay the player's OWN gun from the intention that stands right now, not from the wire's echo of
/// it. The command a client reads back for its own tank is the value lightyear filed
/// `net::client::SHIPPING_INPUT_DELAY_TICKS` ticks ago and hands back at the tick it was stamped
/// for (`net::protocol::bridge_action_state_to_tank_command`); the server reads it later still.
/// The own turret is the one channel ADR-0037 exempts from that wait — click-immediate client view
/// state — so the local lay reads [`CommittedAim`] directly and only the authority's servos wait.
///
/// No commitment for this tank (fresh spawn, or right after a possession change — the entity-keyed
/// read is `None`): author nothing, exactly the pre-first-commit state. Single-player has no bridge
/// and no delay, so the value is already this one and the write is skipped.
fn lay_own_aim_from_the_live_intention(
    committed: Res<CommittedAim>,
    mut controlled: Query<(Entity, &mut TankCommand), With<Controlled>>,
) {
    for (tank, mut command) in &mut controlled {
        if let Some(aim) = committed.get(tank)
            && command.aim != Some(aim)
        {
            command.aim = Some(aim);
        }
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
/// The intention arrives in world space (ADR-0038) and is dropped into the hull frame HERE, against
/// the hull pose of the tick being laid: a servo angle is parent-local, so the conversion belongs to
/// whoever drives the servo, at the tick it drives it.
fn drive_aim_servos(
    tanks: Query<(Entity, &TankCommand, &Rig, &Position, &Rotation), With<Tank>>,
    tables: Query<&RangeTable>,
    mut servos: Query<(Entity, &mut ServoCommand, &ServoRole, &TankRoot)>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
) {
    for (tank, command, rig, position, rotation) in &tanks {
        let Some(world) = command.aim else {
            continue; // no commitment yet — servos hold
        };
        // A non-finite intention would NaN the servo targets and cascade into the physics state —
        // and under MP the command crosses a trust boundary (a client with a zeroed camera/hull
        // transform, or a hostile one, must not be able to poison the authority's sim). Hold, like
        // no-commitment.
        if !world.is_finite() {
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
        // so the world intention drops into that frame first.
        let theta = tables
            .get(rig.muzzle)
            .map_or(0.0, |table| table.superelevation(command.range));
        let point = hull_affine.transform_point3(lob(to_local.transform_point3(world), theta));
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
    controlled: Query<&TankCommand, With<Controlled>>,
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

    let Ok(command) = controlled.single() else {
        *visibility = Visibility::Hidden;
        return;
    };

    // No committed aim yet (before first aim, or free-look from frame one).
    let Some(world) = command.aim else {
        *visibility = Visibility::Hidden;
        return;
    };

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

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::tank::{ServoIndex, ServoRest, ServoSpec, TankServos, drive_servos};

    /// The fixed clock the servos integrate on.
    const TICK: f32 = 1.0 / 64.0;
    /// The delivery gap the intention crosses between authoring and consumption, in ticks: the
    /// shipping input delay (`net::client::SHIPPING_INPUT_DELAY_TICKS`) plus the render frame the
    /// commit is authored in — the client's own view before the local lay closed it, and the floor
    /// of what the authority sees.
    const DELIVERY_TICKS: usize = 5;
    /// The seed vehicle's authored turret traverse (`assets/tiger_1/tiger_1.tank.ron`): the numbers
    /// are the fixture's, never a law — every assertion below is stated as a formula over them.
    const YAW_SPEED: f32 = 34.0;
    const YAW_ACCEL: f32 = 70.0;
    const PITCH_SPEED: f32 = 23.0;
    const PITCH_ACCEL: f32 = 70.0;

    /// A tank whose servos are the real mechanism at the seed vehicle's authored rates, driven by
    /// the real bridge — [`drive_aim_servos`] writing targets and `tank::drive_servos` integrating
    /// them — across a modelled delivery gap.
    ///
    /// Every local transform is identity, so the yaw mount, the pitch mount and the bore share one
    /// origin: a converged bore then points EXACTLY at the point it was commanded to, and any
    /// residual measured here is a fact about the aim transport or the servo, never rig parallax.
    struct AimRig {
        world: World,
        tank: Entity,
        turret: Entity,
        gun: Entity,
        /// The delivery gap, as the queue it is: what the servos consume this tick is what the
        /// player authored [`DELIVERY_TICKS`] ticks ago.
        in_flight: VecDeque<Vec3>,
    }

    impl AimRig {
        /// `held` primes the delivery queue, so the first tick consumes an intention rather than a
        /// hole.
        fn new(held: Vec3) -> Self {
            let mut world = World::new();
            let mut time = Time::<()>::default();
            time.advance_by(Duration::from_secs_f32(TICK));
            world.insert_resource(time);

            let hull = world.spawn(Transform::IDENTITY).id();
            let turret = world.spawn(Transform::IDENTITY).id();
            let gun = world.spawn(Transform::IDENTITY).id();
            let muzzle = world.spawn(Transform::IDENTITY).id();
            let tank = world
                .spawn((
                    Tank,
                    TankCommand::default(),
                    Position::default(),
                    Rotation::default(),
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
                turret,
                gun,
                in_flight: VecDeque::from(vec![held; DELIVERY_TICKS]),
            }
        }

        /// One fixed tick: the hull stands at `hull_yaw_deg`, the player authors `intent`, and the
        /// servos consume whatever left the player [`DELIVERY_TICKS`] ticks ago.
        fn tick(&mut self, hull_yaw_deg: f32, intent: Vec3) {
            self.world
                .entity_mut(self.tank)
                .insert(Rotation(Quat::from_rotation_y(hull_yaw_deg.to_radians())));
            self.in_flight.push_back(intent);
            let delivered = self.in_flight.pop_front().expect("the queue is primed");
            self.world
                .entity_mut(self.tank)
                .get_mut::<TankCommand>()
                .expect("the tank carries a command")
                .aim = Some(delivered);
            self.world
                .run_system_once(drive_aim_servos)
                .expect("the servo bridge runs");
            self.world
                .run_system_once(drive_servos)
                .expect("the servo mechanism runs");
        }

        /// The world bearing the turret is COMMANDED to — its parent-local target composed with
        /// the hull heading that target was solved against. This is what the player's intention
        /// becomes once it has crossed the delivery gap, before the mount's own lag touches it.
        fn commanded_bearing_deg(&self) -> f32 {
            let target = self
                .world
                .get::<ServoCommand>(self.turret)
                .expect("the turret carries a servo command")
                .target;
            let (hull_yaw, _, _) = self
                .world
                .get::<Rotation>(self.tank)
                .expect("the root carries a physics rotation")
                .to_euler(EulerRot::YXZ);
            (target + hull_yaw).to_degrees()
        }

        /// How far the bore misses `target`, in degrees — the whole angle between where the gun
        /// points and where the target actually is.
        fn bore_error_deg(&mut self, target: Vec3) -> f32 {
            let (tank, gun) = (self.tank, self.gun);
            let (position, rotation) = self
                .world
                .run_system_once(
                    move |poses: Query<(&Position, &Rotation)>,
                          parents: Query<&ChildOf>,
                          locals: Query<&Transform>| {
                        let (position, rotation) =
                            poses.get(tank).expect("the root carries a physics pose");
                        rig_world_pose(gun, tank, position.0, rotation.0, &parents, &locals)
                            .expect("the gun hangs under the root")
                    },
                )
                .expect("the pose probe runs");
            (rotation * Vec3::NEG_Z)
                .angle_between(target - position)
                .to_degrees()
        }
    }

    /// **The transport law.** The player's intention travels in a frame latency cannot rotate: an
    /// aim authored while the hull sat at one heading and consumed while it sits at another must
    /// still name the same spot on the world.
    ///
    /// Authored at hull yaw 0°, where the hull frame and the world frame coincide — so the test
    /// need not know which frame the commit writes in — and consumed at 20°. A hull-local
    /// intention re-applied to the later hull yaws with it: the converged bore misses by exactly
    /// the heading change, 20°.
    #[test]
    fn an_intention_survives_the_hull_turning_under_it() {
        const TARGET: Vec3 = Vec3::new(0.0, 0.0, -500.0);
        const TURN_DEG: f32 = 20.0;

        let mut rig = AimRig::new(TARGET);
        // One tick at the authoring heading, then the hull is round at the new one for the whole
        // slew: the intention crosses the delivery gap and is laid against a hull it never saw.
        rig.tick(0.0, TARGET);
        for _ in 0..256 {
            rig.tick(TURN_DEG, TARGET);
        }

        let error = rig.bore_error_deg(TARGET);
        assert!(
            error < 0.02,
            "a converged bore must sit on the committed world point; missing by {error:.3}° means \
             the intention rode the hull round (a hull-local transport misses by the full \
             {TURN_DEG}° heading change)",
        );
    }

    /// **The pivot residual is the servo's, and only the servo's.** Holding the crosshair on a
    /// fixed spot while the hull pivots at ω, the bore must trail by exactly what a rate-limited
    /// mount costs and by nothing else.
    ///
    /// The mechanism's own floor is its braking envelope's fixed point: `drive_servos` commands
    /// `v = sqrt(2·a·ε)`, so sustaining ω needs a standing error of `ω²/2a`, plus the tick the
    /// target moves before the mount answers. A transport that rotates with the hull adds
    /// `ω·(delivery gap)` on top — of the same order, and pure error.
    #[test]
    fn a_steady_pivot_leaves_only_the_mount_s_rate_limited_lag() {
        const TARGET: Vec3 = Vec3::new(0.0, 0.0, -500.0);
        /// Hull yaw rate, degrees/second — a brisk neutral-steer pivot.
        const OMEGA: f32 = 10.0;

        let mut rig = AimRig::new(TARGET);
        for t in 0..512 {
            rig.tick(OMEGA * t as f32 * TICK, TARGET);
        }
        let residual = rig.bore_error_deg(TARGET);

        // The mount's braking envelope commands `v = sqrt(2·a·ε)`, so holding ω needs a standing
        // error of ω²/2a; the reading is taken one integrated step past that fixed point.
        let lag = OMEGA * OMEGA / (2.0 * YAW_ACCEL) - OMEGA * TICK;
        let transport = OMEGA * DELIVERY_TICKS as f32 * TICK;
        assert!(
            (residual - lag).abs() < 0.05,
            "the standing lag must be the mount's own ω²/2a − ω·dt = {lag:.3}°, got {residual:.3}°; \
             an intention that rotates in flight adds ω·delivery = {transport:.3}° on top of it",
        );
    }

    /// **The comb filter.** An interpolated hull does not turn smoothly — under jitter lightyear's
    /// clamp freezes it, then steps it. A hull-local intention beats against that step: the
    /// authoring hull and the consuming hull sit on different stairs, so the bearing the servos are
    /// commanded to swings by the whole step and back, every period, while the player holds
    /// perfectly still. That swing is what the own turret's bore oscillation is made of.
    ///
    /// The intention is a world point, so the commanded bearing is simply where the player put it:
    /// no hull pose enters it, and no jitter in one can move it. What the mount still cannot undo —
    /// the hull's own step carrying the whole tank with it — is the hull's to answer for, not the
    /// aim transport's.
    #[test]
    fn a_frozen_then_stepped_hull_does_not_beat_the_commanded_bearing() {
        const TARGET: Vec3 = Vec3::new(0.0, 0.0, -500.0);
        const OMEGA: f32 = 10.0;
        /// Ticks the interpolated hull holds still before it catches up in one jump.
        const FREEZE_TICKS: u32 = 6;

        let mut rig = AimRig::new(TARGET);
        let mut swing = (f32::MAX, f32::MIN);
        for t in 0..512u32 {
            // The clamp's staircase: the yaw the hull SHOWS is held at the last window boundary
            // until the next arrival steps it on.
            let held = (t / FREEZE_TICKS * FREEZE_TICKS) as f32;
            rig.tick(OMEGA * held * TICK, TARGET);
            // Past the priming ticks, so every sample is of a fully delivered intention.
            if t >= 64 {
                let bearing = rig.commanded_bearing_deg();
                swing = (swing.0.min(bearing), swing.1.max(bearing));
            }
        }

        let peak_to_peak = swing.1 - swing.0;
        let hull_step = OMEGA * FREEZE_TICKS as f32 * TICK;
        assert!(
            peak_to_peak < 1e-3,
            "the commanded bearing must not move while the intention stands still, whatever the \
             hull's rendered pose does — got {peak_to_peak:.4}° of swing, against the \
             {hull_step:.3}° square wave a hull-local transport beats out at the clamp's period",
        );
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
