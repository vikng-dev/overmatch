//! View-layer combat feedback for network clients.
//!
//! The player's own replicated [`NetCrew`] drives incoming-hit kick and damage flash. An
//! owner-private, deduplicated [`DamageReceipt`] drives the outgoing hit marker. Neither path writes
//! simulation state or feeds aim: camera kick modifies rendered `GlobalTransform` only, and
//! [`stabilize_camera_pose`] restores it before `Update` readers consume the camera pose.

use bevy::prelude::*;
use lightyear::prelude::client::Remote;

use crate::ShotId;
use crate::ballistics::ComponentHealth;
use crate::camera::CameraKickApplied;
use crate::damage::{CrewStation, TankVolumes};
use crate::hud::HudCamera;
use crate::tank::Controlled;
use crate::ui_font::UiFonts;

use super::protocol::{DamageReceipt, NetCrew, health_bearing_volumes};

// --- Feel dials (all in ONE place) -----------------------------------------------------------
/// Camera-kick impulse per hit before severity scaling.
const KICK_PITCH_RAD: f32 = 0.055;
const KICK_YAW_RAD: f32 = 0.035;
const KICK_ROLL_RAD: f32 = 0.030;
/// Per-axis clamp on the accumulated kick, so a burst of hits jolts hard but never spins the view.
const KICK_MAX_RAD: f32 = 0.22;
/// Kick retention per 60 Hz frame; `powf(dt * 60)` makes decay frame-rate independent.
const KICK_RETAIN: f32 = 0.80;
/// Below this magnitude the kick is spent and zeroed, so it never lingers as denormal dust.
const KICK_ZERO_EPS: f32 = 1e-4;

/// A per-volume health drop smaller than this (in HP) is treated as noise, not a hit — guards against
/// any float churn in the replicated snapshot re-triggering a cue.
const HIT_EPS_HP: f32 = 0.01;

/// Damage-flash and marker retention per 60 Hz frame.
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

/// Hit-confirm intensity ∈ [0, 1]: 1.0 when a discrete server damage confirmation for an authored
/// shot arrives, decaying to 0. Drives the centre hit-marker's alpha.
#[derive(Resource, Default)]
struct HitConfirm(f32);

/// One server-confirmed damaging shot authored by this client. Raised by `net::client` only after the
/// receipt has been accepted idempotently by [`DamageReceipt`]; the view consumes it without
/// inferring shot count or attribution from `NetCrew` state deltas.
#[derive(Event)]
pub(super) struct LocalHitConfirmed {
    pub receipt: DamageReceipt,
    /// Client predicted-present tick at the wire receive boundary.
    pub received_tick: u32,
    /// Authority tick at which this shot first damaged an HP pool.
    pub damage_tick: u32,
}

/// Remembered health and occupant identity. Occupant changes distinguish crew moves from damage.
#[derive(Clone, Copy, PartialEq)]
struct SlotMemory {
    hp: f32,
    occupant: Option<CrewStation>,
}

/// Last observed [`NetCrew`] snapshot, seeded before diffing so a spawn does not read as damage.
#[derive(Component)]
struct HealthMemory(Vec<SlotMemory>);

/// The per-volume [`SlotMemory`] vector out of the atomic [`NetCrew`] snapshot, in the SAME
/// [`health_bearing_volumes`] order the server published — the value hit-feel diffs for drops. Carries
/// each seat's occupant (`crew.home`) alongside HP so a personnel move can be told from damage.
fn slot_memory(net: &NetCrew) -> Vec<SlotMemory> {
    net.volumes
        .iter()
        .map(|v| SlotMemory {
            hp: v.hp,
            occupant: v.crew.map(|c| c.home),
        })
        .collect()
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
        .add_observer(on_local_hit_confirmed)
        .add_systems(Startup, spawn_cue_ui)
        .add_systems(
            Update,
            (
                arm_health_memory,
                detect_health_drops.after(arm_health_memory),
                drive_damage_flash,
                drive_hit_confirm.after(super::client::receive_damage_confirms),
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
            .insert(HealthMemory(slot_memory(net)));
    }
}

/// Diff every changed `NetCrew` against its remembered snapshot and raise the being-hit cue on any
/// per-volume DROP (an increase — a respawn's health reset — raises nothing). The player's own
/// (`Controlled`) tank drives the being-hit cue (camera kick + screen flash). Opponent deltas are state
/// only and raise no marker: the discrete, attributed [`LocalHitConfirmed`] path owns that semantic.
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
) {
    for (root, volumes, net, mut memory, is_own) in &mut tanks {
        // The per-volume snapshot (HP + occupant) in the SAME health-bearing order the server published
        // (index i ↔ volume). `Changed<NetCrew>` also fires each tick a swap countdown ticks AND on the
        // tick a swap COMPLETES — when the two seats' HP transpose. `worst_drop` discounts any slot
        // whose occupant changed, so a completing swap raises no false cue on either the owner (a
        // camera kick + damage flash). Opponent state deltas never own hit-confirm event count.
        let slots = slot_memory(net);
        let bearers = health_bearing_volumes(volumes, |v| health.contains(v));
        // A transient length skew while the rig is still spawning: resync memory, diff nothing.
        if bearers.len() != slots.len() || memory.0.len() != slots.len() {
            memory.0 = slots;
            continue;
        }

        // The worst per-volume drop since last snapshot (`None` if nothing dropped or all deltas are
        // occupancy changes).
        let worst = worst_drop(&memory.0, &slots);
        memory.0 = slots;

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
        }
    }
}

/// Pulse the centre marker from the discrete authoritative fact. This is deliberately separate from
/// health snapshot handling: a latest-state stream can preserve HP but cannot preserve one event per
/// shot under coalescing/loss. The trace is written at this presentation boundary; receipt and dedup
/// tracing remains at the receive boundary.
fn on_local_hit_confirmed(
    hit: On<LocalHitConfirmed>,
    mut confirm: ResMut<HitConfirm>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    confirm.0 = 1.0;
    let shot = ShotId {
        combatant: hit.receipt.combatant,
        weapon: hit.receipt.weapon,
        fire_tick: hit.receipt.fire_tick,
    };
    crate::shot_trace::record(
        &mut shot_trace,
        "marker",
        hit.received_tick,
        shot,
        || serde_json::json!({ "own": true, "dt": hit.damage_tick }),
    );
    info!(
        "hit-feel: receipt {:?} damaged on the authority → hit-confirm",
        hit.receipt
    );
}

#[cfg(test)]
pub(super) fn mount_test_marker_boundary(app: &mut App) {
    app.init_resource::<HitConfirm>()
        .add_observer(on_local_hit_confirmed);
}

/// Scan two same-length slot snapshots for the single largest per-volume DROP, returning its index and
/// magnitude — or `None` if nothing fell by more than [`HIT_EPS_HP`] (an all-increase snapshot, a
/// respawn's health reset, raises nothing). A slot whose OCCUPANT changed between snapshots is skipped:
/// the HP delta there is a personnel move (a crew swap transposing two seats' HP), not damage — this is
/// what stops a swap completion from firing a false hit cue. Pure, so the detection core is
/// unit-testable without the live authoritative hit the spawn-point harness cannot produce. A length
/// mismatch is the caller's to screen (it means the rig is mid-spawn); here mismatched tails are simply
/// not compared.
fn worst_drop(prev: &[SlotMemory], now: &[SlotMemory]) -> Option<(usize, f32)> {
    let mut worst: Option<(usize, f32)> = None;
    for (i, (before, after)) in prev.iter().zip(now).enumerate() {
        // Occupancy changed → the HP delta is a body moving between seats, not a hit.
        if before.occupant != after.occupant {
            continue;
        }
        let drop = before.hp - after.hp;
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

    /// Build a snapshot of module slots (no occupant) with the given HP — the fixture for the plain
    /// HP-diff tests, where occupancy never changes so only the HP delta matters.
    fn modules(hp: &[f32]) -> Vec<SlotMemory> {
        hp.iter()
            .map(|&hp| SlotMemory { hp, occupant: None })
            .collect()
    }

    /// A crew seat slot with a known occupant, for the swap tests.
    fn seat(hp: f32, occupant: CrewStation) -> SlotMemory {
        SlotMemory {
            hp,
            occupant: Some(occupant),
        }
    }

    #[test]
    fn authoritative_damage_confirm_pulses_without_a_health_snapshot_delta() {
        let mut app = App::new();
        app.init_resource::<HitConfirm>()
            .add_observer(on_local_hit_confirmed);
        let receipt = DamageReceipt {
            combatant: crate::CombatantId(1),
            weapon: 1,
            fire_tick: 77,
        };

        app.world_mut().trigger(LocalHitConfirmed {
            receipt,
            received_tick: 77,
            damage_tick: 77,
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<HitConfirm>().0,
            1.0,
            "the discrete authoritative fact, not a NetCrew delta, owns the marker pulse"
        );
    }

    #[test]
    fn no_change_is_no_hit() {
        assert_eq!(
            worst_drop(
                &modules(&[100.0, 50.0, 25.0]),
                &modules(&[100.0, 50.0, 25.0])
            ),
            None
        );
    }

    #[test]
    fn an_increase_raises_nothing() {
        // A respawn resets health UP; that must not read as a hit.
        assert_eq!(
            worst_drop(&modules(&[0.0, 10.0]), &modules(&[100.0, 100.0])),
            None
        );
    }

    #[test]
    fn picks_the_largest_drop_and_its_index() {
        // Volume 0 chips by 5, volume 2 by 40 — the worst is volume 2, and it is what the bearing
        // reads off. Volume 1 rose and is ignored.
        let (index, drop) = worst_drop(
            &modules(&[100.0, 30.0, 80.0]),
            &modules(&[95.0, 60.0, 40.0]),
        )
        .unwrap();
        assert_eq!(index, 2);
        assert!((drop - 40.0).abs() < 1e-4);
    }

    #[test]
    fn a_sub_epsilon_chip_is_noise() {
        // Below HIT_EPS_HP: replication float churn, not a hit.
        assert_eq!(
            worst_drop(&modules(&[100.0]), &modules(&[100.0 - HIT_EPS_HP / 2.0])),
            None
        );
    }

    #[test]
    fn a_crew_swap_transposing_hp_is_not_a_hit() {
        // Snapshot A: the gunner seat is alive+full, the loader seat is dead (0). Snapshot B, the tick
        // a backfill swap COMPLETES: the live body moved, so the seats' HP transpose AND their `home`s
        // swap with them. The full→0 seat is a personnel move, not an own-damage cue.
        let a = [
            seat(100.0, CrewStation::Gunner),
            seat(0.0, CrewStation::Loader),
        ];
        let b = [
            seat(0.0, CrewStation::Loader),
            seat(100.0, CrewStation::Gunner),
        ];
        assert_eq!(
            worst_drop(&a, &b),
            None,
            "a swap's HP transpose (occupant changed) is not damage",
        );
    }

    #[test]
    fn a_genuine_drop_with_unchanged_occupant_still_registers() {
        // Same occupant in the seat, HP fell: real damage, still a hit.
        let a = [seat(100.0, CrewStation::Gunner)];
        let b = [seat(40.0, CrewStation::Gunner)];
        let (index, drop) = worst_drop(&a, &b).unwrap();
        assert_eq!(index, 0);
        assert!((drop - 60.0).abs() < 1e-4);
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
