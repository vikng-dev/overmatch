//! View-layer combat feedback — "predict what you author, replicate what is authored against you".
//!
//! Getting shot, and landing a shot, are the two halves of the **unfelt hit**
//! (`design/timelines-and-shear.md` §3): the authoritative shove is ~0.14 m/s ≈ 1.1 cm over 80 ms,
//! below `ROLLBACK_POSITION_M` (5 cm), so it never enters the client's sim — and `on_hit_impulse`
//! (`ballistics`) is gated OFF on the whole client anyway (`ClientReplica`). This module delivers the
//! feedback the physics cannot: it watches the SERVER-AUTHORITATIVE per-volume health
//! (`net::protocol::NetHealth`) for drops and answers each with a **presentation-only** cue —
//! - **being hit** (a drop on the player's own `Controlled` tank): a decaying camera kick + a
//!   screen-edge damage flash, the kick's bearing read off the struck volume's world pose;
//! - **confirming your hit** (a drop on an opponent's tank): a brief centre hit-marker.
//!
//! **The sim/view split (ADR-0014), enforced.** Nothing here writes a value the sim or the aim path
//! reads. The camera kick is applied ONLY to the camera's rendered `GlobalTransform`, never to its
//! `Transform` (which `camera::orbit_camera` reads back to derive look yaw/pitch) and never to the
//! gun/servos. `GlobalTransform` is re-derived from truth every frame — by `TransformSystems::Propagate`
//! in commander view, by `camera::gunner_camera` in gunner view — so the offset can never accumulate,
//! exactly as `net::render_error`'s offset rides avian's per-frame `position_to_transform`. The one
//! reader of the camera pose that feeds the sim is `aim::commit_aim` (third-person: a screen-centre ray
//! → committed aim), and it runs in `Update` off the camera's `GlobalTransform`; [`stabilize_camera_pose`]
//! restores `GlobalTransform` to its un-kicked `Transform` at the head of each frame so that reader —
//! and every other `Update`-schedule reader — sees the stable pose while only the rendered image shakes.
//! The gunner optic commits aim from the mouse (`sight::drive_gunner_aim`), never the camera, so the
//! more important gunner-view kick is decoupled by construction.
//!
//! Net-client only, mounted by `NetClientPlugin` (single-player has no authoritative-damage stream to
//! read). The UI-writing systems use `Single`, so they simply don't run on a headless client that
//! never spawned a camera/UI.

use bevy::prelude::*;
use lightyear::prelude::client::Remote;

use crate::ballistics::ComponentHealth;
use crate::camera::CameraKickApplied;
use crate::damage::TankVolumes;
use crate::hud::HudCamera;
use crate::tank::Controlled;
use crate::ui_font::UiFonts;

use super::protocol::{NetCrew, health_bearing_volumes};

// --- Feel dials (all in ONE place) -----------------------------------------------------------
/// Camera-kick impulse added per hit, in radians, before severity scaling. Pitch is the always-up
/// recoil punch; yaw/roll carry the bearing (which side of the hull absorbed the round). Signs are
/// feel dials — the model's local axes decide which way "right" is, and any consistent jolt reads as
/// a hit. ~0.06 rad ≈ 3.4°.
const KICK_PITCH_RAD: f32 = 0.055;
const KICK_YAW_RAD: f32 = 0.035;
const KICK_ROLL_RAD: f32 = 0.030;
/// Per-axis clamp on the accumulated kick, so a burst of hits jolts hard but never spins the view.
const KICK_MAX_RAD: f32 = 0.22;
/// Fraction of the kick RETAINED per 60 Hz frame; `powf(dt*60)` normalizes it to any framerate. 0.80
/// spends the kick in ~0.25 s — a snap out, then a quick settle back to the true (un-kicked) pose.
const KICK_RETAIN: f32 = 0.80;
/// Below this magnitude the kick is spent and zeroed, so it never lingers as denormal dust.
const KICK_ZERO_EPS: f32 = 1e-4;

/// A per-volume health drop smaller than this (in HP) is treated as noise, not a hit — guards against
/// any float churn in the replicated snapshot re-triggering a cue.
const HIT_EPS_HP: f32 = 0.01;

/// Fraction of the damage flash / hit-marker RETAINED per 60 Hz frame (framerate-normalized). ~0.86
/// gives a visible flash that fades over ~0.4 s.
const CUE_RETAIN: f32 = 0.86;
/// Below this intensity the flash/marker is fully hidden (and its resource pinned to 0).
const CUE_ZERO_EPS: f32 = 0.02;

/// The decaying camera-kick offset, in the camera's LOCAL frame: `x` = pitch (up), `y` = yaw, `z` =
/// roll, all radians. Composed as a post-multiplied rotation on the camera's rendered pose and decayed
/// to zero between hits — presentation only, see the module doc's leak analysis.
#[derive(Resource, Default)]
struct CameraKick {
    angular: Vec3,
}

/// Damage-flash intensity ∈ [0, 1]: 1.0 the instant the player's own tank takes a hit, decaying to 0.
/// Drives the screen-edge red frame's alpha.
#[derive(Resource, Default)]
struct DamageFlash(f32);

/// Hit-confirm intensity ∈ [0, 1]: 1.0 the instant an opponent's replicated health drops, decaying to
/// 0. Drives the centre hit-marker's alpha.
#[derive(Resource, Default)]
struct HitConfirm(f32);

/// The last-seen per-volume HP snapshot for a tank, so a change can be diffed into per-volume drops.
/// Armed once (seeded to the live value, so the spawn frame is never read as a hit), updated on every
/// observed change. Read out of the replicated [`NetCrew`] (which subsumes the former `NetHealth`).
#[derive(Component)]
struct HealthMemory(Vec<f32>);

/// The per-volume HP vector out of the atomic [`NetCrew`] snapshot, in the SAME
/// [`health_bearing_volumes`] order the server published — the value hit-feel diffs for drops.
fn net_health(net: &NetCrew) -> Vec<f32> {
    net.volumes.iter().map(|v| v.hp).collect()
}

/// The screen-edge damage frame (own hit). A full-screen node drawn as a thick red border, its alpha
/// driven by [`DamageFlash`]; the hollow centre keeps it from obscuring the fight.
#[derive(Component)]
struct DamageFlashNode;

/// The centre hit-marker (your hit confirmed). A short "X" tick shown briefly, its alpha driven by
/// [`HitConfirm`].
#[derive(Component)]
struct HitConfirmNode;

pub fn plugin(app: &mut App) {
    app.init_resource::<CameraKick>()
        .init_resource::<DamageFlash>()
        .init_resource::<HitConfirm>()
        .add_systems(Startup, spawn_cue_ui)
        .add_systems(
            Update,
            (
                arm_health_memory,
                detect_health_drops.after(arm_health_memory),
                drive_damage_flash,
                drive_hit_confirm,
            ),
        )
        // Restore the camera's rendered pose to its un-kicked truth at the head of the frame, BEFORE
        // any `Update` reader (notably `aim::commit_aim`) turns the camera pose into committed aim.
        .add_systems(PreUpdate, stabilize_camera_pose)
        // Apply the kick to the rendered pose AFTER both camera placements have set it. `.after`
        // both the gunner set (itself after `Propagate`) and `Propagate` directly, because in
        // commander view the gunner set is empty and only the `Propagate` edge is load-bearing.
        .add_systems(
            PostUpdate,
            apply_camera_kick
                .in_set(CameraKickApplied)
                .after(crate::camera::GunnerCameraPlaced)
                .after(TransformSystems::Propagate),
        );
}

/// Arm each replicated tank with a [`HealthMemory`] seeded to its current health, so the frame the
/// component first appears is never diffed as a hit. Polling (not an observer) for the same reason as
/// `net::render_error::arm_render_error`: replicated markers arrive in no guaranteed order.
fn arm_health_memory(
    tanks: Query<(Entity, &NetCrew), (With<Remote>, Without<HealthMemory>)>,
    mut commands: Commands,
) {
    for (entity, net) in &tanks {
        commands
            .entity(entity)
            .insert(HealthMemory(net_health(net)));
    }
}

/// Diff every changed `NetHealth` against its remembered snapshot and raise the matching cue on any
/// per-volume DROP (an increase — a respawn's health reset — raises nothing). The player's own
/// (`Controlled`) tank drives the being-hit cue (camera kick + screen flash); every other replicated
/// tank drives the hit-confirm cue.
///
/// **Attribution for hit-confirm is by NetHealth delta alone, and the ambiguity is named, not
/// plumbed.** A drop on an opponent means SOMEONE's shell connected on the authority; in the 1-v-1
/// (player vs one bot) this slice ships, the only shooter that can damage the opponent is the player,
/// so the confirm is unambiguously yours. With a third shooter (a bot teammate, a 2-v-2) it is not —
/// this cue would fire for a hit you did not land. Tightening it needs server-side shooter attribution
/// on the wire (a protocol change), which is deliberately out of scope here; the honest graybox is the
/// delta, documented.
fn detect_health_drops(
    mut tanks: Query<
        (
            Entity,
            &TankVolumes,
            &NetCrew,
            &mut HealthMemory,
            Has<Controlled>,
        ),
        (With<Remote>, Changed<NetCrew>),
    >,
    health: Query<&ComponentHealth>,
    transforms: Query<&GlobalTransform>,
    mut kick: ResMut<CameraKick>,
    mut flash: ResMut<DamageFlash>,
    mut confirm: ResMut<HitConfirm>,
) {
    for (root, volumes, net, mut memory, is_own) in &mut tanks {
        // The per-volume HP out of the atomic snapshot, in the SAME health-bearing order the server
        // published (index i ↔ volume). `Changed<NetCrew>` also fires each tick a swap countdown
        // ticks; the diff below simply finds no drop then, so that costs a no-op, never a false cue.
        let hp = net_health(net);
        let bearers = health_bearing_volumes(volumes, |v| health.contains(v));
        // A transient length skew while the rig is still spawning: resync memory, diff nothing.
        if bearers.len() != hp.len() || memory.0.len() != hp.len() {
            memory.0 = hp;
            continue;
        }

        // The worst per-volume drop since last snapshot (`None` if nothing dropped).
        let worst = worst_drop(&memory.0, &hp);
        memory.0 = hp;

        let Some((worst_index, drop_hp)) = worst else {
            continue;
        };
        let worst_volume = bearers.get(worst_index).copied();

        if is_own {
            // Severity from the worst volume's share of its own pool; a light chip barely nudges, a
            // heavy penetration jolts hard.
            let severity = worst_volume
                .and_then(|v| health.get(v).ok())
                .map(|hp| (drop_hp / hp.max.max(1.0)).clamp(0.0, 1.0))
                .unwrap_or(0.5);
            let bearing = worst_volume.and_then(|v| hit_bearing(&transforms, root, v));
            add_camera_kick(&mut kick, severity, bearing);
            flash.0 = 1.0;
            info!(
                "hit-feel: OWN tank hit — worst drop {drop_hp:.1} hp (severity {severity:.2}, \
                 bearing {bearing:?}) → camera kick + damage flash"
            );
        } else {
            confirm.0 = 1.0;
            info!("hit-feel: opponent {root} health dropped (worst {drop_hp:.1} hp) → hit-confirm");
        }
    }
}

/// Scan two same-length health snapshots for the single largest per-volume DROP, returning its index
/// and magnitude — or `None` if nothing fell by more than [`HIT_EPS_HP`] (an all-increase snapshot, a
/// respawn's health reset, raises nothing). Pure, so the detection core is unit-testable without the
/// live authoritative hit the spawn-point harness cannot produce. A length mismatch is the caller's to
/// screen (it means the rig is mid-spawn); here mismatched tails are simply not compared.
fn worst_drop(prev: &[f32], now: &[f32]) -> Option<(usize, f32)> {
    let mut worst: Option<(usize, f32)> = None;
    for (i, (&before, &after)) in prev.iter().zip(now).enumerate() {
        let drop = before - after;
        if drop > HIT_EPS_HP && worst.is_none_or(|(_, w)| drop > w) {
            worst = Some((i, drop));
        }
    }
    worst
}

/// The struck volume's lateral position in the tank ROOT's local frame, `+` = struck on the right —
/// the bearing the camera kick leans into. Computed as the volume's world position mapped through the
/// root's inverse world transform, so it is correct whatever way the hull faces. `None` if either
/// pose is unavailable (a kick with no bearing is still a straight pitch-up punch).
fn hit_bearing(transforms: &Query<&GlobalTransform>, root: Entity, volume: Entity) -> Option<f32> {
    let root_gt = transforms.get(root).ok()?;
    let volume_gt = transforms.get(volume).ok()?;
    let local = root_gt
        .affine()
        .inverse()
        .transform_point3(volume_gt.translation());
    Some(local.x)
}

/// Add a hit's kick impulse to the accumulated offset, bearing-aware, clamped per axis. A pitch-up
/// punch always; yaw/roll lean toward the struck side when a bearing is known.
fn add_camera_kick(kick: &mut CameraKick, severity: f32, bearing: Option<f32>) {
    let scale = 0.6 + severity; // never a nothing-kick, harder with severity
    let side = bearing.map(f32::signum).unwrap_or(0.0);
    kick.angular.x = (kick.angular.x + KICK_PITCH_RAD * scale).clamp(-KICK_MAX_RAD, KICK_MAX_RAD);
    kick.angular.y =
        (kick.angular.y + KICK_YAW_RAD * scale * -side).clamp(-KICK_MAX_RAD, KICK_MAX_RAD);
    kick.angular.z =
        (kick.angular.z + KICK_ROLL_RAD * scale * side).clamp(-KICK_MAX_RAD, KICK_MAX_RAD);
}

/// Restore the camera's rendered `GlobalTransform` to its un-kicked `Transform` at the head of the
/// frame — see the module doc. The camera is parentless, so `GlobalTransform == Transform` is the
/// invariant truth; writing it here can only remove last frame's kick, never corrupt the pose. Runs
/// before `Update`, so `aim::commit_aim` and the world-anchored HUD read the stable pose.
fn stabilize_camera_pose(camera: Single<(&Transform, &mut GlobalTransform), With<HudCamera>>) {
    let (transform, mut global) = camera.into_inner();
    *global = GlobalTransform::from(*transform);
}

/// Decay the kick and displace the camera's rendered pose by it. Writes ONLY `GlobalTransform`; the
/// leak analysis is in the module doc. Post-multiplying the offset rotation keeps the kick in the
/// camera's local frame (pitch about local X, etc.).
fn apply_camera_kick(
    time: Res<Time<Real>>,
    mut kick: ResMut<CameraKick>,
    camera: Single<&mut GlobalTransform, With<HudCamera>>,
) {
    let retain = KICK_RETAIN.powf(time.delta_secs() * 60.0);
    kick.angular *= retain;
    if kick.angular.length() <= KICK_ZERO_EPS {
        kick.angular = Vec3::ZERO;
        return; // nothing to apply — the stabilized pose already holds
    }

    let mut global = camera.into_inner();
    let (scale, rotation, translation) = global.to_scale_rotation_translation();
    let offset = Quat::from_euler(
        EulerRot::YXZ,
        kick.angular.y,
        kick.angular.x,
        kick.angular.z,
    );
    *global = GlobalTransform::from(Transform {
        translation,
        rotation: (rotation * offset).normalize(),
        scale,
    });
}

/// Spawn the two cue overlays once. The damage frame is a hollow full-screen red border; the
/// hit-marker is a centred "X" tick. Both start transparent and are driven by their intensity
/// resources. Mirrors the node idiom in `aim::spawn_hud` / `hud::spawn_labels`.
fn spawn_cue_ui(mut commands: Commands, fonts: Res<UiFonts>) {
    commands.spawn((
        DamageFlashNode,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            border: UiRect::all(Val::Px(48.0)),
            ..default()
        },
        BorderColor::all(Color::srgba(0.9, 0.05, 0.05, 0.0)),
    ));
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
                HitConfirmNode,
                Text::new("X"),
                TextFont {
                    // SemiBold: the punchy centre-screen hit marker.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(28.0),
                    ..default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.0)),
            ));
        });
}

/// Fade the screen-edge damage frame toward transparent, framerate-normalized.
fn drive_damage_flash(
    time: Res<Time<Real>>,
    mut flash: ResMut<DamageFlash>,
    frame: Single<&mut BorderColor, With<DamageFlashNode>>,
) {
    flash.0 *= CUE_RETAIN.powf(time.delta_secs() * 60.0);
    if flash.0 <= CUE_ZERO_EPS {
        flash.0 = 0.0;
    }
    frame
        .into_inner()
        .set_all(Color::srgba(0.9, 0.05, 0.05, flash.0));
}

/// Fade the centre hit-marker toward transparent, framerate-normalized.
fn drive_hit_confirm(
    time: Res<Time<Real>>,
    mut confirm: ResMut<HitConfirm>,
    marker: Single<&mut TextColor, With<HitConfirmNode>>,
) {
    confirm.0 *= CUE_RETAIN.powf(time.delta_secs() * 60.0);
    if confirm.0 <= CUE_ZERO_EPS {
        confirm.0 = 0.0;
    }
    *marker.into_inner() = TextColor(Color::srgba(1.0, 1.0, 1.0, confirm.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_change_is_no_hit() {
        assert_eq!(worst_drop(&[100.0, 50.0, 25.0], &[100.0, 50.0, 25.0]), None);
    }

    #[test]
    fn an_increase_raises_nothing() {
        // A respawn resets health UP; that must not read as a hit.
        assert_eq!(worst_drop(&[0.0, 10.0], &[100.0, 100.0]), None);
    }

    #[test]
    fn picks_the_largest_drop_and_its_index() {
        // Volume 0 chips by 5, volume 2 by 40 — the worst is volume 2, and it is what the bearing
        // reads off. Volume 1 rose and is ignored.
        let (index, drop) = worst_drop(&[100.0, 30.0, 80.0], &[95.0, 60.0, 40.0]).unwrap();
        assert_eq!(index, 2);
        assert!((drop - 40.0).abs() < 1e-4);
    }

    #[test]
    fn a_sub_epsilon_chip_is_noise() {
        // Below HIT_EPS_HP: replication float churn, not a hit.
        assert_eq!(worst_drop(&[100.0], &[100.0 - HIT_EPS_HP / 2.0]), None);
    }

    #[test]
    fn a_kick_leans_toward_the_struck_side_and_clamps() {
        // A hit on the right (+bearing) yaws one way; a burst never exceeds the per-axis clamp.
        let mut kick = CameraKick::default();
        add_camera_kick(&mut kick, 1.0, Some(2.0));
        assert!(kick.angular.x > 0.0, "always a pitch-up punch");
        let right_yaw = kick.angular.y;
        let mut left = CameraKick::default();
        add_camera_kick(&mut left, 1.0, Some(-2.0));
        assert!(
            left.angular.y.signum() != right_yaw.signum(),
            "left and right hits yaw opposite ways"
        );
        for _ in 0..50 {
            add_camera_kick(&mut kick, 1.0, Some(2.0));
        }
        assert!(kick.angular.x <= KICK_MAX_RAD + 1e-6, "pitch stays clamped");
        assert!(
            kick.angular.y.abs() <= KICK_MAX_RAD + 1e-6,
            "yaw stays clamped"
        );
    }
}
