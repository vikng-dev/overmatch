//! The player's gun control: fire per each weapon's [`FireMode`] (a `Single`'s click → one shell
//! plus a crew-gated reload; an `Automatic`'s held trigger → cyclic fire off a finite belt, with a
//! crew-gated belt swap when it runs dry), raising a `ballistics::FireShell` per round, and recoil
//! the barrel. The trajectory itself lives in `ballistics` — this module owns only what makes it
//! the *player's* gun. The armor sandbox drives the same `FireShell` from its free-fly camera
//! instead.

use avian3d::prelude::{Forces, Position, Rotation, WriteRigidBodyForces};
use bevy::prelude::*;

use crate::ballistics::{FireShell, ShotSource};
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::damage::{TankVolumes, VolumeFacets, requirement_met};
use crate::spec::{FireMode, Trigger};
use crate::state::GameplaySet;
use crate::tank::{Muzzle, Tank, TankRoot, TankSim, Weapon, WeaponIndex, rig_world_pose};

/// Feel multiplier on the hull recoil impulse (1.0 = physical momentum). On a 57 t hull true momentum
/// is a gentle rock by design; bump this if the firing kick should read more dramatically.
const RECOIL_FEEL: f32 = 1.0;

/// Procedural barrel recoil CONFIG: the damped-spring tuning + the barrel's rest (battery)
/// position, built during complete tank construction from the weapon's `recoil` spec and barrel
/// node's authored translation — spawn-time data, not a bind-time transform capture. The recoil
/// STATE (offset/velocity) is sim truth — the muzzle rides the barrel — and lives root-resident
/// in `TankSim::weapons` (see `TankSim`), keyed by the barrel's `WeaponIndex`. The translational
/// cousin of `Servo`.
#[derive(Component)]
pub(crate) struct RecoilParams {
    pub(crate) rest: Vec3,
    pub(crate) stiffness: f32,
    pub(crate) damping: f32,
}

/// THE single expression of how a fired weapon kicks its barrel: add `weapon`'s `recoil.kick` to
/// slot `slot`'s recoil velocity in `sim`. Owns the WHOLE decision — a weapon with no `recoil` spec
/// (a coax), no `barrel` node, or a slot absent from `sim.weapons` (a rig still spawning, a bad slot
/// off the wire) kicks nothing. Both the local shooter ([`fire`], this module) and the remote-recoil
/// applier (`net::client::apply_pending_recoil_kicks`) pass `(sim, slot, weapon)` and are IDENTICAL
/// by construction: "how a shot recoils the gun" is one implementation, not two that agree today
/// (the derive-the-consequence doctrine, ADR-0016 — a derive that branches differently per end is
/// two implementations).
///
/// The `barrel` gate lives HERE, not at the call sites, and is load-bearing: `apply_recoil` only
/// steps slots that have `RecoilParams`, which tank construction installs on the barrel node — so a
/// kick on a barrel-less slot would land in `recoil_velocity` and NEVER decay, accumulating without
/// bound in rollback-tracked `TankSim` state, shot after shot. Gating in one place makes that
/// unreachable on both ends.
pub(crate) fn kick_recoil(sim: &mut TankSim, slot: usize, weapon: &Weapon) {
    // No barrel node to recoil (no `RecoilParams`, so `apply_recoil` never springs it back), or a
    // recoil-less weapon — either way, no kick.
    let (Some(_), Some(recoil)) = (weapon.barrel, weapon.recoil.as_ref()) else {
        return;
    };
    if let Some(state) = sim.weapons.get_mut(slot) {
        state.recoil_velocity += recoil.kick;
    }
}

/// Whether the `rounds_fired`-th round down a belt with cadence `tracer_every` is a tracer. Pure
/// belt arithmetic (see [`crate::spec::FireMode::Automatic`]'s `tracer_every` — only the
/// `Automatic` arm of [`fire`] calls this; a `Single`'s round always traces): every
/// `tracer_every`-th round traces, counting the belt from the first round (index 0). `1` = every
/// round; `5` = rounds 0, 5, 10, … (one-in-five); `0` = a tracerless belt, never. Both the server
/// and the predicted client call this with a counter they each walk from 0, so they agree on every
/// round's tracer-ness.
pub(crate) fn tracer_round(rounds_fired: u32, tracer_every: u32) -> bool {
    // The `!= 0` guard both encodes the "never" belt and short-circuits before `is_multiple_of(0)`.
    tracer_every != 0 && rounds_fired.is_multiple_of(tracer_every)
}

pub fn plugin(app: &mut App) {
    // The gun is sim: reload and firing run on the fixed clock, driven by each tank's `TankCommand`
    // — `fire` consumes the click edge, so it must precede the command layer's edge clear.
    //
    // `apply_recoil.after(fire)` is DETERMINISM-LOAD-BEARING, not a preference: both systems take
    // `&mut TankSim`, and without an explicit edge Bevy's executor may serialize them in either
    // order — an order that measurably differed between client and server processes (2026-07-10,
    // divergence instrument): on the fire tick one end integrated the spring before the kick and
    // the other after, a one-tick recoil phase offset that read as a 33-tick `hrec` divergence
    // window per shot. The canonical order is kick-then-integrate — a shot's kick springs the
    // barrel the SAME tick, matching what the remote-fire path already promises
    // (`net::client::apply_pending_recoil_kicks` runs `.before(GameplaySet)` for exactly that).
    //
    // The remaining unordered `&mut TankSim` neighbors (driving.rs's suspension/drive chain) write
    // DISJOINT TankSim fields (anchors, never weapons), so their order against these systems
    // cannot change values today. If a shooting system ever touches anchors — or a driving system
    // touches weapons — that pair must be ordered explicitly too.
    app.add_systems(
        FixedUpdate,
        (
            (tick_reload, fire).chain().before(ConsumeCommandEdges),
            apply_recoil.after(fire),
        )
            .in_set(GameplaySet),
    );
}

/// Tick every weapon's fire timer down, with the crew gate applied per [`FireMode`] — this is
/// where "what does the crew actually do" splits by mechanism:
///
/// * `Single`: the timer is the reload, and the whole reload is gated by the weapon's `load`
///   requirement (Loader staffed + Breech intact). A dead Loader or broken Breech freezes the
///   reload partway through; a backfilled Loader (slice 2) would resume it.
/// * `Automatic`, belt has rounds: the timer is the cyclic interval (60/rpm), pure mechanism —
///   it ticks UNGATED. Crew-gating it was the old single-path latent trap: a dead loader would
///   have frozen an MG's rate of fire mid-belt, which no crew casualty physically does.
/// * `Automatic`, belt dry: the timer is the belt swap, and the SWAP is what `load` gates (the
///   human act, same machinery as the 88's reload). When it reaches 0 the belt refills — inside
///   the gated tick, so a crew-dead tank never completes a swap.
///
/// Per-tank, not controlled-only: a tank keeps loading whether you're in it or it's a network
/// peer's.
fn tick_reload(
    time: Res<Time>,
    tanks: Query<Option<&TankVolumes>, With<Tank>>,
    volumes: Query<VolumeFacets>,
    weapons: Query<(&Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
    mut sims: Query<&mut TankSim>,
) {
    for (weapon, slot, root) in &weapons {
        let Ok(tank_volumes) = tanks.get(root.0) else {
            continue;
        };
        let Ok(mut sim) = sims.get_mut(root.0) else {
            continue;
        };
        let Some(state) = sim.weapons.get_mut(slot.0) else {
            continue;
        };
        if state.reload_remaining <= 0.0 {
            continue;
        }
        match weapon.fire_mode {
            // Single-shot reload: crew-gated, unchanged.
            FireMode::Single { .. } => {
                if requirement_met(tank_volumes, &weapon.load, &volumes) {
                    state.reload_remaining = (state.reload_remaining - time.delta_secs()).max(0.0);
                }
            }
            FireMode::Automatic { belt_size, .. } => {
                if state.belt_remaining > 0 {
                    // Cyclic interval: mechanism, never crew-gated.
                    state.reload_remaining = (state.reload_remaining - time.delta_secs()).max(0.0);
                } else if requirement_met(tank_volumes, &weapon.load, &volumes) {
                    // Belt swap: the crew-gated act. The refill lives INSIDE the gated tick, on
                    // the exact tick the timer bottoms out — so completion is impossible while
                    // the gate is unmet, and happens exactly once per swap.
                    state.reload_remaining = (state.reload_remaining - time.delta_secs()).max(0.0);
                    if state.reload_remaining <= 0.0 {
                        state.belt_remaining = belt_size;
                    }
                }
            }
        }
    }
}

/// Fire each tank's weapons whose trigger its command holds this tick — THE one fire system, a
/// `match` on [`FireMode`] per weapon, never per-mechanism systems (the schedule edges above are
/// determinism-load-bearing and must stay singular). `Trigger` is pure input ROUTING (which
/// command field the weapon reads); the input *semantics* come from the mode: a `Single` weapon
/// consumes a latched click edge (`fire_primary`-style, one tick per click), an `Automatic` reads
/// a held level and cycles at 60/rpm from a finite belt. Each weapon fires from its *own* muzzle
/// and ballistics, gated by its `fire` requirement + fire timer (+ belt for an `Automatic`) — the
/// gate lives here in the sim, where the server will enforce it, not in the input path.
fn fire(
    tanks: Query<(&TankCommand, Option<&TankVolumes>, &Position, &Rotation), With<Tank>>,
    volumes: Query<VolumeFacets>,
    weapons: Query<(Entity, &Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
    mut sims: Query<&mut TankSim>,
    mut bodies: Query<Forces, With<Tank>>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    // F1: `true` only while a net client is REPLAYING a rollback. The DETERMINISTIC sim mutations
    // below (belt decrement, reload/recoil arming, hull impulse, the tracer counter) MUST replay so
    // rolled-back `TankSim` re-derives exactly — but the cosmetic `FireShell` trigger must NOT, or a
    // replay re-crossing this fire tick spawns a DUPLICATE own shell sharing the round's `ShotId`
    // (the forward tick already spawned it; the shell entity is not rolled back). Absent on the
    // authority (server/SP/sandbox never roll back), so it fires there unconditionally.
    replaying: Option<Res<crate::Replaying>>,
    mut commands: Commands,
) {
    let replaying = replaying.is_some_and(|r| r.0);
    for (muzzle_entity, weapon, slot, root) in &weapons {
        let Ok((command, tank_volumes, position, rotation)) = tanks.get(root.0) else {
            continue;
        };
        // Input routing only — edge vs level is baked into the command fields themselves
        // (`fire_primary` is a one-tick click latch, `fire_secondary` a held level; see
        // `TankCommand`), so a `Single` on Primary consumes the edge and an `Automatic` on
        // Secondary reads the level, per the mode's contract.
        let triggered = match weapon.trigger {
            Trigger::Primary => command.fire_primary,
            Trigger::Secondary => command.fire_secondary,
        };
        // Ready = fire timer at 0, plus rounds on the belt for an `Automatic` (a dry belt is
        // mid-swap: `reload_remaining` then carries the swap timer, which a dead crew can freeze
        // at >0 forever — the belt check keeps "no rounds" firm even at timer 0 edge cases).
        let ready = sims
            .get(root.0)
            .ok()
            .and_then(|sim| sim.weapons.get(slot.0))
            .is_some_and(|w| {
                w.reload_remaining <= 0.0
                    && match weapon.fire_mode {
                        FireMode::Single { .. } => true,
                        FireMode::Automatic { .. } => w.belt_remaining > 0,
                    }
            });
        if !triggered || !ready || !requirement_met(tank_volumes, &weapon.fire, &volumes) {
            continue;
        }

        // Tick-truth muzzle pose (`rig_world_pose`, never `GlobalTransform` — see its doc): the
        // muzzle decides where the shell goes, so it must be the pose the server's tick also
        // computes, not the render picture. The chain composes the servo angles (tick-truth in
        // the fixed loop) and the barrel's recoil offset (stepped in `FixedUpdate`).
        let Some((muzzle_position, muzzle_rotation)) = rig_world_pose(
            muzzle_entity,
            root.0,
            position.0,
            rotation.0,
            &parents,
            &locals,
        ) else {
            continue;
        };
        let Ok(bore) = Dir3::new(muzzle_rotation * Vec3::NEG_Z) else {
            continue; // corrupt pose frame — hold the shot rather than fire NaN
        };

        // Round bookkeeping: is THIS round a tracer, then advance the counter. A `Single`'s round
        // always traces (its visual is the shell scene, not a streak — `tracer_round` is belt
        // arithmetic and only the `Automatic` arm calls it). Decided from root-resident `TankSim`
        // state so the server and the predicted client — which both run this `fire` and count
        // their own belts from 0 — agree on each round's tracer-ness (and a rollback replay
        // restores the counter, re-deriving the same answer). The flag rides FireShell → FireEvent
        // so remote clients match too. A rollback that drops a predicted shot can leave THIS
        // client's own counter one round out of phase with the server's; the resulting one-round
        // tracer skew on the shooter's own view is cosmetic and accepted (see
        // `WeaponState::rounds_fired`).
        let tracer = if let Ok(mut sim) = sims.get_mut(root.0) {
            match sim.weapons.get_mut(slot.0) {
                Some(state) => {
                    let is_tracer = match weapon.fire_mode {
                        FireMode::Single { .. } => true,
                        FireMode::Automatic { tracer_every, .. } => {
                            tracer_round(state.rounds_fired, tracer_every)
                        }
                    };
                    state.rounds_fired = state.rounds_fired.wrapping_add(1);
                    is_tracer
                }
                None => false,
            }
        } else {
            false
        };

        // Hand off to ballistics: fire down the bore at the weapon's muzzle speed. SUPPRESSED on a
        // rollback replay (F1): the cosmetic shell was already spawned on the original forward tick
        // and is not rolled back, so re-triggering here would spawn a duplicate own shell sharing this
        // round's `ShotId`. The sim mutations above/below (tracer counter, and belt/reload/recoil/hull
        // below) still run — they MUST replay for the rolled-back `TankSim` to re-derive exactly.
        if !replaying {
            commands.trigger(FireShell {
                origin: muzzle_position,
                direction: bore,
                speed: weapon.speed,
                caliber: weapon.caliber,
                mass: weapon.mass,
                tracer,
                // This shell belongs to a tank: name its root AND the firing weapon slot so the net
                // server can broadcast the cosmetic tracer and the barrel-recoil cause to every OTHER
                // client (`net::server`'s FireShell observer), which each derive the kick locally.
                shooter: Some(ShotSource {
                    tank: root.0,
                    weapon: slot.0,
                }),
                // Locally fired: the shell spawns at the muzzle THIS tick — no net catch-up.
                catch_up_ticks: 0,
                // The sim cannot read the fire tick (it lives in the netcode timeline), so `fire` never
                // stamps the shot identity: on a net composition the shared `net::protocol::stamp_shot_ids`
                // completes it after spawn from the `ShotSource` above + the timeline — on the server AND
                // on the shooter's own client, so the shooter's own round re-seeds from the server's
                // ricochet keyframes too (the fall-of-shot read). Always `None` here.
                shot: None,
            });
        }
        // Kick the barrel back (root-resident recoil state); apply_recoil springs it home. The
        // shared `kick_recoil` owns the whole decision (barrel + recoil spec present, slot valid), so
        // this path and the opponent-view path (`net::client`) can't diverge on how a shot recoils.
        if let Ok(mut sim) = sims.get_mut(root.0) {
            kick_recoil(&mut sim, slot.0, weapon);
        }
        // Recoil reaction on the hull: the shell's momentum, opposite the bore, applied on the bore
        // axis. The line of action passes above the centre of mass, so the impulse-at-point also
        // pitches the nose up (gun climb), not just shoves the hull back. Each weapon kicks by its
        // own momentum, so the MGs barely register.
        if let Ok(mut forces) = bodies.get_mut(root.0) {
            let impulse = bore * (-weapon.mass * weapon.speed * RECOIL_FEEL);
            forces.apply_linear_impulse_at_point(impulse, muzzle_position);
        }
        // Arm the fire timer per mechanism. `Single`: the crew-gated reload. `Automatic`: consume
        // one belt round; a dry belt automatically starts the (crew-gated) belt swap, otherwise
        // the timer is the plain cyclic interval.
        if let Ok(mut sim) = sims.get_mut(root.0)
            && let Some(state) = sim.weapons.get_mut(slot.0)
        {
            match weapon.fire_mode {
                FireMode::Single { reload_secs } => state.reload_remaining = reload_secs,
                FireMode::Automatic {
                    rpm,
                    belt_swap_secs,
                    ..
                } => {
                    state.belt_remaining = state.belt_remaining.saturating_sub(1);
                    state.reload_remaining = if state.belt_remaining == 0 {
                        belt_swap_secs
                    } else {
                        60.0 / rpm
                    };
                }
            }
        }
    }
}

/// Step each recoiling barrel's damped spring (state root-resident in `TankSim::weapons`, so a
/// rollback replay re-derives the barrel — and therefore the muzzle — from restored state) and
/// write the barrel's node `Transform`.
fn apply_recoil(
    mut barrels: Query<(&mut Transform, &RecoilParams, &WeaponIndex, &TankRoot)>,
    mut sims: Query<&mut TankSim>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut transform, params, slot, root) in &mut barrels {
        let Ok(mut sim) = sims.get_mut(root.0) else {
            continue;
        };
        let Some(state) = sim.weapons.get_mut(slot.0) else {
            continue;
        };
        // Damped spring back to battery: offset'' = -k·offset - c·offset'.
        let accel =
            -params.stiffness * state.recoil_offset - params.damping * state.recoil_velocity;
        state.recoil_velocity += accel * dt;
        state.recoil_offset += state.recoil_velocity * dt;
        // Battery stop — the barrel can't return past its rest position.
        if state.recoil_offset < 0.0 {
            state.recoil_offset = 0.0;
            state.recoil_velocity = 0.0;
        }
        // Recoil rides back along the bore (+local Z), measured from the rest position.
        transform.translation = params.rest + Vec3::Z * state.recoil_offset;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use avian3d::prelude::{Position, Rotation};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;

    use super::{fire, tick_reload, tracer_round};
    use crate::command::TankCommand;
    use crate::damage::{CrewStation, Dead, Group, Part, Requirement, VolumeOf};
    use crate::spec::{FireMode, Trigger};
    use crate::tank::{Muzzle, Tank, TankRoot, TankSim, Weapon, WeaponIndex, WeaponState};

    /// The belt-fed test mode: 600 rpm = a 0.1 s cyclic interval, a 2-round belt, a 1 s swap.
    const MG_MODE: FireMode = FireMode::Automatic {
        rpm: 600.0,
        belt_size: 2,
        belt_swap_secs: 1.0,
        tracer_every: 5,
    };

    /// A minimal `Automatic` rig the real `fire`/`tick_reload` systems run over: a tank root
    /// (command holding the secondary trigger, physics pose, one-slot `TankSim` with a FULL belt),
    /// a live Loader crew volume (so `TankVolumes` exists and a `[Loader]` gate is meetable), and
    /// a muzzle child carrying the `Weapon` with the given `load` gate. Returns (root, loader).
    fn spawn_mg_rig(world: &mut World, load: Requirement) -> (Entity, Entity) {
        let root = world
            .spawn((
                Tank,
                TankCommand {
                    fire_secondary: true,
                    ..default()
                },
                Position::default(),
                Rotation::default(),
                TankSim {
                    weapons: vec![WeaponState::for_mode(&MG_MODE)],
                    ..default()
                },
            ))
            .id();
        let loader = world.spawn((CrewStation::Loader, VolumeOf(root))).id();
        world.spawn((
            Muzzle,
            WeaponIndex(0),
            TankRoot(root),
            Transform::default(),
            ChildOf(root),
            Weapon {
                name: "MG".into(),
                speed: 755.0,
                caliber: 0.0079,
                mass: 0.0118,
                fire_mode: MG_MODE,
                recoil: None,
                barrel: None,
                fire: Vec::new(),
                load,
                trigger: Trigger::Secondary,
            },
        ));
        (root, loader)
    }

    fn weapon_state(world: &mut World, root: Entity) -> WeaponState {
        world.get::<TankSim>(root).expect("sim on root").weapons[0]
    }

    fn advance(world: &mut World, secs: f32) {
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(secs));
        world.run_system_once(tick_reload).unwrap();
    }

    /// The determinism-relevant belt lifecycle, on the real systems: firing walks the belt down,
    /// a dry belt BLOCKS fire and automatically starts the swap, the swap timer runs it out, the
    /// belt refills, and fire resumes — all of it in root-resident `TankSim` state (so a rollback
    /// replay re-derives the identical sequence).
    #[test]
    fn belt_runs_dry_swap_completes_fire_resumes() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        // Ungated swap (`load: []`) — the crew gate has its own test below.
        let (root, _) = spawn_mg_rig(&mut world, Vec::new());

        // Round 1: belt 2→1, the cyclic interval (60/600 = 0.1 s) arms.
        world.run_system_once(fire).unwrap();
        let s = weapon_state(&mut world, root);
        assert_eq!((s.rounds_fired, s.belt_remaining), (1, 1));
        assert!(
            (s.reload_remaining - 0.1).abs() < 1e-6,
            "cyclic interval armed"
        );

        // Held trigger inside the cyclic interval: no shot.
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 1);

        // Interval elapses; round 2 empties the belt — the swap starts AUTOMATICALLY (timer
        // becomes belt_swap_secs, not the cyclic interval).
        advance(&mut world, 0.15);
        world.run_system_once(fire).unwrap();
        let s = weapon_state(&mut world, root);
        assert_eq!((s.rounds_fired, s.belt_remaining), (2, 0));
        assert_eq!(s.reload_remaining, 1.0, "dry belt arms the swap timer");

        // Dry belt blocks fire mid-swap.
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 2);

        // Half the swap: still dry, still blocked.
        advance(&mut world, 0.5);
        let s = weapon_state(&mut world, root);
        assert_eq!(s.belt_remaining, 0);
        assert!((s.reload_remaining - 0.5).abs() < 1e-6);
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 2);

        // Swap completes: the belt refills to belt_size and fire resumes.
        advance(&mut world, 0.6);
        let s = weapon_state(&mut world, root);
        assert_eq!(s.belt_remaining, 2, "completed swap refills the belt");
        assert_eq!(s.reload_remaining, 0.0);
        world.run_system_once(fire).unwrap();
        let s = weapon_state(&mut world, root);
        assert_eq!((s.rounds_fired, s.belt_remaining), (3, 1));
    }

    /// The crew-gate split the redesign exists for: the CYCLIC interval ticks with the gun crew
    /// dead (a dead loader must not freeze an MG's rate of fire — the old single-path trap), but
    /// the BELT SWAP is crew-gated like the 88's reload: dead crew = frozen swap = no fire, and a
    /// revived crew resumes and completes it.
    #[test]
    fn belt_swap_is_crew_gated_but_cyclic_interval_is_not() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        let (root, loader) = spawn_mg_rig(&mut world, vec![Group::Single(Part::Loader)]);

        // Kill the loader BEFORE anything fires (`fire: []` stays met — only `load` cares).
        world.entity_mut(loader).insert(Dead);

        // Round 1 fires, and the cyclic interval ticks out DESPITE the dead loader.
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).belt_remaining, 1);
        advance(&mut world, 0.15);
        assert_eq!(
            weapon_state(&mut world, root).reload_remaining,
            0.0,
            "the cyclic interval is mechanism, not crew work — it must tick with the crew dead"
        );

        // Round 2 empties the belt; the swap arms…
        world.run_system_once(fire).unwrap();
        let s = weapon_state(&mut world, root);
        assert_eq!((s.rounds_fired, s.belt_remaining), (2, 0));
        assert_eq!(s.reload_remaining, 1.0);

        // …and FREEZES: dead gun crew = no swap, however long we wait, and no fire.
        for _ in 0..4 {
            advance(&mut world, 5.0);
        }
        let s = weapon_state(&mut world, root);
        assert_eq!(s.reload_remaining, 1.0, "dead crew freezes the swap timer");
        assert_eq!(s.belt_remaining, 0, "no refill while the swap is frozen");
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 2);

        // Revive the loader (slice-2 backfill shape): the swap resumes, completes, fire returns.
        world.entity_mut(loader).remove::<Dead>();
        advance(&mut world, 0.6);
        advance(&mut world, 0.6);
        let s = weapon_state(&mut world, root);
        assert_eq!(s.belt_remaining, 2, "revived crew completes the swap");
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 3);
    }

    /// The belt cadence is exact: `tracer_every == 1` traces every round; `5` traces exactly rounds
    /// 0, 5, 10, … (one-in-five, counting the belt from the first round); `0` is a tracerless belt
    /// that never traces. This is the arithmetic the server and the predicted client both walk, so
    /// they can only agree if it is this deterministic.
    #[test]
    fn tracer_cadence_is_every_nth() {
        // 1 = always.
        for n in 0..20u32 {
            assert!(
                tracer_round(n, 1),
                "tracer_every=1 traces every round (round {n})"
            );
        }
        // 5 = one-in-five, phased on the belt start.
        for n in 0..20u32 {
            assert_eq!(
                tracer_round(n, 5),
                n % 5 == 0,
                "tracer_every=5 traces rounds 0,5,10,… (round {n})"
            );
        }
        assert!(tracer_round(0, 5), "the first round of a belt traces");
        assert!(!tracer_round(1, 5));
        assert!(!tracer_round(4, 5));
        assert!(tracer_round(5, 5));
        // 0 = never (a future stealth belt).
        for n in 0..20u32 {
            assert!(
                !tracer_round(n, 0),
                "tracer_every=0 never traces (round {n})"
            );
        }
    }

    /// F1: a rollback replay re-runs `fire` for a tick that already fired on the forward pass. The
    /// DETERMINISTIC sim mutation (the belt walk) MUST replay so the rolled-back `TankSim` re-derives
    /// exactly — but the cosmetic `FireShell` trigger must NOT, or the replay spawns a duplicate own
    /// shell sharing the round's `ShotId` (the forward tick's shell is not rolled back).
    #[test]
    fn rollback_replay_walks_the_belt_but_spawns_no_duplicate_own_shell() {
        use crate::ballistics::FireShell;

        #[derive(Resource, Default)]
        struct FireShellCount(usize);
        fn count_fire_shells(_: On<FireShell>, mut c: ResMut<FireShellCount>) {
            c.0 += 1;
        }

        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.init_resource::<FireShellCount>();
        world.add_observer(count_fire_shells);
        let (root, _) = spawn_mg_rig(&mut world, Vec::new());

        // A FORWARD tick (no `Replaying` resource → the flag reads `false`): one cosmetic shell spawns
        // and the belt walks 2 → 1.
        world.run_system_once(fire).unwrap();
        assert_eq!(
            world.resource::<FireShellCount>().0,
            1,
            "the forward tick spawns exactly one own shell",
        );
        assert_eq!(weapon_state(&mut world, root).belt_remaining, 1);

        // Arm past the cyclic interval so the weapon is ready to fire again.
        advance(&mut world, 0.15);

        // Now REPLAYING: `fire` re-runs for this ready tick. The belt still walks 1 → 0 (determinism),
        // but no SECOND cosmetic shell may spawn.
        world.insert_resource(crate::Replaying(true));
        world.run_system_once(fire).unwrap();
        assert_eq!(
            world.resource::<FireShellCount>().0,
            1,
            "a replayed fire tick spawns NO duplicate own shell",
        );
        assert_eq!(
            weapon_state(&mut world, root).belt_remaining,
            0,
            "the belt still decrements on the replay — TankSim must re-derive the rolled-back state",
        );
    }
}
