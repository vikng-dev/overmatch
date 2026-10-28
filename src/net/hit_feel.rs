//! View-layer combat feedback for network clients.
//!
//! The player's own replicated [`NetCrew`] drives the damage flash. An owner-private, deduplicated
//! [`DamageReceipt`] drives the outgoing hit marker. Neither path writes simulation state or feeds
//! aim. Being-hit presents at ARRIVAL (reaction time).

use bevy::prelude::*;

use lightyear::prelude::client::Remote;

use crate::ShotId;
use crate::ballistics::ComponentHealth;
use crate::damage::{CrewStation, TankVolumes};
use crate::flow::{Flow, FlowScoped, InMatchSystems, MatchResourceExt};
use crate::tank::Controlled;
use crate::ui_font::UiFonts;

use super::protocol::{DamageReceipt, NetCrew, health_bearing_volumes};

// --- Feel dials (all in ONE place) -----------------------------------------------------------
/// Damage-flash and marker retention per 60 Hz frame.
const CUE_RETAIN: f32 = 0.86;
/// Below this intensity the flash/marker is fully hidden (and its resource pinned to 0).
const CUE_ZERO_EPS: f32 = 0.02;

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
    /// Client tick at the wire receive boundary.
    pub received_tick: u32,
    /// Authority tick at which this shot first damaged an HP pool.
    pub damage_tick: u32,
}

/// Remembered shown health and occupant identity. Occupant changes distinguish crew moves from
/// damage.
#[derive(Clone, Copy, PartialEq)]
struct SlotMemory {
    /// The HP points the pool shows ([`crate::ballistics::shown_points`]), so the flash follows the
    /// display: a dose the pool keeps short of a whole point raises nothing.
    hp: u32,
    occupant: Option<CrewStation>,
}

/// Last observed [`NetCrew`] snapshot, seeded before diffing so a spawn does not read as damage.
#[derive(Component)]
struct HealthMemory(Vec<SlotMemory>);

/// The per-volume [`SlotMemory`] vector out of the atomic [`NetCrew`] snapshot, in the SAME
/// [`health_bearing_volumes`] order the server published — the shown points hit-feel diffs for
/// drops. Carries each seat's occupant (`crew.home`) alongside HP so a personnel move can be told
/// from damage.
fn slot_memory(net: &NetCrew) -> Vec<SlotMemory> {
    net.volumes
        .iter()
        .map(|v| SlotMemory {
            hp: crate::ballistics::shown_points(v.held),
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
    crate::flow::require(app, "net::hit_feel");
    app.init_match_resource::<DamageFlash>()
        .init_match_resource::<HitConfirm>()
        .add_observer(on_local_hit_confirmed)
        .add_systems(OnEnter(Flow::InMatch), spawn_cue_ui)
        .add_systems(
            Update,
            (
                arm_health_memory,
                detect_health_drops.after(arm_health_memory),
                drive_damage_flash,
                drive_hit_confirm.after(super::client::receive_damage_confirms),
            )
                .in_set(InMatchSystems),
        );
}

/// Arm each replicated tank with a [`HealthMemory`] seeded to its current health, so the frame the
/// component first appears is never diffed as a hit. Polling (not an observer) because replicated
/// markers arrive in no guaranteed order.
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
/// (`Controlled`) tank drives the being-hit cue (damage flash). Opponent deltas are state
/// only and raise no marker: the discrete, attributed [`LocalHitConfirmed`] path owns that semantic.
fn detect_health_drops(
    mut tanks: Query<
        (&TankVolumes, &NetCrew, &mut HealthMemory, Has<Controlled>),
        (With<Remote>, Changed<NetCrew>),
    >,
    health: Query<&ComponentHealth>,
    mut flash: ResMut<DamageFlash>,
) {
    for (volumes, net, mut memory, is_own) in &mut tanks {
        // The per-volume snapshot (HP + occupant) in the SAME health-bearing order the server published
        // (index i ↔ volume). `Changed<NetCrew>` also fires each tick a swap countdown ticks AND on the
        // tick a swap COMPLETES — when the two seats' HP transpose. `worst_drop` discounts any slot
        // whose occupant changed, so a completing swap raises no false cue on the owner. Opponent
        // state deltas never own hit-confirm event count.
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

        let Some((_, drop_hp)) = worst else {
            continue;
        };

        if is_own {
            flash.0 = 1.0;
            info!("hit-feel: OWN tank hit — worst drop {drop_hp} hp → damage flash");
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
/// magnitude — or `None` if nothing fell (an all-increase snapshot, a
/// respawn's health reset, raises nothing). A slot whose OCCUPANT changed between snapshots is skipped:
/// the HP delta there is a personnel move (a crew swap transposing two seats' HP), not damage — this is
/// what stops a swap completion from firing a false hit cue. Pure, so the detection core is
/// unit-testable without the live authoritative hit the spawn-point harness cannot produce. A length
/// mismatch is the caller's to screen (it means the rig is mid-spawn); here mismatched tails are simply
/// not compared.
fn worst_drop(prev: &[SlotMemory], now: &[SlotMemory]) -> Option<(usize, u32)> {
    let mut worst: Option<(usize, u32)> = None;
    for (i, (before, after)) in prev.iter().zip(now).enumerate() {
        // Occupancy changed → the HP delta is a body moving between seats, not a hit.
        if before.occupant != after.occupant {
            continue;
        }
        let drop = before.hp.saturating_sub(after.hp);
        if drop > 0 && worst.is_none_or(|(_, w)| drop > w) {
            worst = Some((i, drop));
        }
    }
    worst
}

/// Spawn the two cue overlays on entering a match. The damage frame is a hollow full-screen red
/// border; the hit-marker is a centred "X" tick. Both start transparent and are driven by their
/// intensity resources. Mirrors the node idiom in `aim::spawn_hud` / `hud::spawn_labels`.
fn spawn_cue_ui(mut commands: Commands, fonts: Res<UiFonts>) {
    commands.spawn((
        FlowScoped(Flow::InMatch),
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
        .spawn((
            FlowScoped(Flow::InMatch),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
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
    fn modules(hp: &[u32]) -> Vec<SlotMemory> {
        hp.iter()
            .map(|&hp| SlotMemory { hp, occupant: None })
            .collect()
    }

    /// A crew seat slot with a known occupant, for the swap tests.
    fn seat(hp: u32, occupant: CrewStation) -> SlotMemory {
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
    fn picks_the_largest_drop_and_its_index() {
        // Volume 0 chips by 5, volume 2 by 40 — the worst is volume 2. Volume 1 rose and is
        // ignored.
        let (index, drop) = worst_drop(&modules(&[100, 30, 80]), &modules(&[95, 60, 40])).unwrap();
        assert_eq!(index, 2);
        assert_eq!(drop, 40);
        assert_eq!(
            worst_drop(&modules(&[100, 50, 25]), &modules(&[100, 50, 25])),
            None,
            "no change is no hit"
        );
        // HP is whole points: the smallest drop there is, one point, is a hit.
        assert_eq!(worst_drop(&modules(&[100]), &modules(&[99])), Some((0, 1)));
    }

    /// The damage flash follows the display: a snapshot whose pool fell short of a whole point
    /// shows the same points and raises nothing, and the snapshot that takes it past one raises a
    /// one-point drop. The player relies on it: a flash with no point shown lost is a hit the HUD
    /// cannot explain. A memory of raw pool units, or one rounded to nearest, fails here. Fixture
    /// pool: 30 HP points.
    #[test]
    fn a_drop_short_of_a_whole_point_raises_no_flash() {
        use crate::ballistics::POOL_UNITS_PER_HP;
        let net = |held: u32| NetCrew {
            volumes: vec![super::super::protocol::VolumeSnapshot { held, crew: None }],
            swap: None,
        };
        let full = slot_memory(&net(30 * POOL_UNITS_PER_HP));
        let short = slot_memory(&net(30 * POOL_UNITS_PER_HP - 600));
        assert_eq!(worst_drop(&full, &short), None, "0.6 points shows full");
        let past = slot_memory(&net(29 * POOL_UNITS_PER_HP - 1));
        assert_eq!(
            worst_drop(&short, &past),
            Some((0, 1)),
            "past a whole point"
        );
    }

    #[test]
    fn a_crew_swap_transposing_hp_is_not_a_hit() {
        // Snapshot A: the gunner seat is alive+full, the loader seat is dead (0). Snapshot B, the tick
        // a backfill swap COMPLETES: the live body moved, so the seats' HP transpose AND their `home`s
        // swap with them. The full→0 seat is a personnel move, not an own-damage cue.
        let a = [seat(100, CrewStation::Gunner), seat(0, CrewStation::Loader)];
        let b = [seat(0, CrewStation::Loader), seat(100, CrewStation::Gunner)];
        assert_eq!(
            worst_drop(&a, &b),
            None,
            "a swap's HP transpose (occupant changed) is not damage",
        );

        // Same occupant in the seat, HP fell: real damage, still a hit.
        let a = [seat(100, CrewStation::Gunner)];
        let b = [seat(40, CrewStation::Gunner)];
        let (index, drop) = worst_drop(&a, &b).unwrap();
        assert_eq!(index, 0);
        assert_eq!(drop, 60);
    }
}
