//! The game's damage model: the player's gun control: fire per each weapon's [`FireCycle`] (a `Single`'s click → one shell
//! plus a crew-gated reload; an `Automatic`'s held trigger → cyclic fire off a finite belt, with a
//! crew-gated belt swap when it runs dry), raising a `ballistics::FireShell` per round, and recoil
//! the barrel. The trajectory itself lives in `ballistics` — this module owns only what makes it
//! the *player's* gun. The armor sandbox drives the same `FireShell` from its free-fly camera
//! instead.

use avian3d::prelude::{Collider, CollisionLayers, Forces, Position, Rotation};
use bevy::prelude::*;

mod birth;
pub(crate) use birth::outside_source;

use crate::ballistics::{FireShell, ShotSource};
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::damage::{TankVolumes, VolumeFacets, requirement_met, requirement_rate};
use crate::sim_tick::TickSpan;
use crate::spec::{FireCycle, Trigger};
use crate::state::{AppState, GameplaySet, SimPhase};
use crate::tank::{
    Muzzle, Tank, TankRoot, TankSim, Weapon, WeaponGate, WeaponIndex, WeaponLoads, rig_world_pose,
};
use crate::track::sim::apply_explicit_impulse;

/// Feel multiplier on the hull recoil impulse (1.0 = physical momentum). On a 57 t hull true momentum
/// is a gentle rock; this is the one deliberate exaggeration over physical, so the firing kick reads.
const RECOIL_FEEL: f32 = 2.5;

/// THE single expression of the recoil reaction a fired round puts on its hull: the shell's
/// momentum, opposite the bore. Applied at the muzzle point by [`fire`] (the derive-the-consequence
/// doctrine, ADR-0016; the same rule [`kick_recoil`] states for the barrel).
pub(crate) fn recoil_impulse(bore: Dir3, mass: f32, speed: f32) -> Vec3 {
    bore * (-mass * speed * RECOIL_FEEL)
}

/// Procedural barrel recoil CONFIG: the damped-spring tuning, the recoil stop + the barrel's rest
/// (battery) position, built during complete tank construction from the weapon's `recoil` spec and
/// barrel node's authored translation — spawn-time data, not a bind-time transform capture. The
/// recoil STATE (offset/velocity) is sim truth — the muzzle rides the barrel — and lives
/// root-resident in `TankSim::weapons` (see `TankSim`), keyed by the barrel's `WeaponIndex`. The
/// translational cousin of `Servo`.
#[derive(Component)]
pub(crate) struct RecoilParams {
    pub(crate) rest: Vec3,
    pub(crate) stiffness: f32,
    pub(crate) damping: f32,
    /// The farthest the barrel travels back from `rest`, m (`RecoilSpec::max_length`).
    pub(crate) max_length: f32,
}

/// THE single expression of how a fired weapon kicks its barrel: add `weapon`'s `recoil.kick` to
/// slot `slot`'s recoil velocity in `sim`. Owns the WHOLE decision — a weapon with no `recoil` spec
/// (a coax) or no `barrel` node kicks nothing. `slot` is the firing weapon's own [`WeaponIndex`]
/// on this machine's rig (the remote applier finds that muzzle first), so it indexes
/// `sim.weapons`. Both the local shooter ([`fire`], this module) and the remote-recoil
/// applier (`net::client::apply_pending_recoil_kicks`) pass `(sim, slot, weapon)` and are IDENTICAL
/// by construction: "how a shot recoils the gun" is one implementation, not two that agree today
/// (the derive-the-consequence doctrine, ADR-0016 — a derive that branches differently per end is
/// two implementations).
///
/// The `barrel` gate lives HERE, not at the call sites, and is load-bearing: `apply_recoil` only
/// steps slots that have `RecoilParams`, which tank construction installs on the barrel node — so a
/// kick on a barrel-less slot would land in `recoil_velocity` and NEVER decay, accumulating without
/// bound in `TankSim` state, shot after shot. Gating in one place makes that unreachable on both
/// ends.
pub(crate) fn kick_recoil(sim: &mut TankSim, slot: usize, weapon: &Weapon) {
    // No barrel node to recoil (no `RecoilParams`, so `apply_recoil` never springs it back), or a
    // recoil-less weapon — either way, no kick.
    let (Some(_), Some(recoil)) = (weapon.barrel, weapon.recoil.as_ref()) else {
        return;
    };
    sim.weapons
        .get_mut(slot)
        .expect(SLOT_IN_SIM)
        .recoil_velocity += recoil.kick;
}

/// Whether the `rounds_fired`-th round down a belt with cadence `tracer_every` is a tracer. Pure
/// belt arithmetic (see [`crate::spec::FireCycle::Automatic`]'s `tracer_every` — only the
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
    // The gun is sim: readiness and firing run on the fixed clock, driven by each tank's
    // `TankCommand` — `fire` consumes the click edge, so it must precede edge clearing, and so does
    // the authority's round selection, which follows `fire` so a shot in its tick fires the round
    // chambered before it.
    //
    // `SimPhase` owns the cross-feature order: driving samples tick-start velocity before this
    // impulse changes it, then recoil integrates the new kick in the same tick.
    app.init_resource::<crate::WeaponClock>()
        .add_systems(Startup, check_weapon_timestep)
        .add_systems(
            FixedLast,
            advance_weapon_clock
                .run_if(in_state(AppState::Playing))
                .run_if(not(resource_exists::<crate::PublishedWeaponClock>)),
        )
        .add_systems(
            FixedUpdate,
            (
                (
                    tick_weapon_gate,
                    fire,
                    apply_round_selection.run_if(not(resource_exists::<crate::ClientReplica>)),
                )
                    .chain()
                    .in_set(SimPhase::WeaponFire)
                    .before(ConsumeCommandEdges),
                apply_recoil.in_set(SimPhase::Recoil),
            )
                .in_set(GameplaySet),
        );
}

/// The composition must run at the timestep used by the asset duration parser, before play.
fn check_weapon_timestep(time: Res<Time<Fixed>>) {
    crate::sim_tick::assert_sim_timestep(time.timestep());
}

/// Advance the local-only fallback clock after a completed fixed tick. Network compositions
/// overwrite it from `LocalTimeline` before the next gameplay pass.
pub(crate) fn advance_weapon_clock(mut clock: ResMut<crate::WeaponClock>) {
    clock.0 = clock.0.after(TickSpan::ONE);
}

/// Why a weapon's [`TankRoot`] resolves: [`crate::tank`]'s assembly spawns every weapon under its
/// root in the root's own construction batch, and the root carries `Tank` (which requires its
/// `TankCommand`). Authority poses are spawn-complete; the client presentation boundary retains
/// the last stream pose through an interpolation resync.
const WEAPON_ROOT_IS_A_TANK: &str =
    "a weapon's TankRoot names its live, posed tank, assembled with it in one construction batch";
/// Why a weapon's slot indexes its root's per-weapon state: `WeaponGate`, `WeaponLoads` and
/// `TankSim::weapons` are each built from the spec's name-sorted weapons, the order that assigns
/// every `WeaponIndex`.
const SLOT_IN_GATE: &str = "a weapon's WeaponIndex indexes its root's WeaponGate slots";
const SLOT_IN_LOADS: &str = "a weapon's WeaponIndex indexes its root's WeaponLoads slots";
const SLOT_IN_SIM: &str = "a weapon's WeaponIndex indexes its root's TankSim weapon slots";
const ROOT_HAS_SIM: &str = "a weapon's root carries the TankSim built in its construction batch";

/// Mature every weapon's absolute readiness deadline, with the crew gate applied per [`FireCycle`]:
///
/// * `Single`: load effectiveness sets the reload rate; zero freezes the remaining work.
/// * `Automatic`, belt has rounds: the cyclic deadline matures without a crew gate.
/// * `Automatic`, belt dry: load effectiveness sets the swap rate with the same pause and
///   rate-change rule; the belt refills only when the work completes.
///
/// Per-tank, not controlled-only: a tank keeps loading whether you're in it or it's a network
/// peer's.
fn tick_weapon_gate(
    clock: Res<crate::WeaponClock>,
    tanks: Query<Option<&TankVolumes>, With<Tank>>,
    volumes: Query<VolumeFacets>,
    weapons: Query<(&Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
    mut gates: Query<&mut WeaponGate>,
    replica: Option<Res<crate::ClientReplica>>,
    controlled: Query<(), With<crate::tank::Controlled>>,
) {
    let now = clock.0;
    for (weapon, slot, root) in &weapons {
        let tank_volumes = tanks.get(root.0).expect(WEAPON_ROOT_IS_A_TANK);
        // A client holds no gate for another player's tank: `WeaponGate` is owner-private
        // (`net::disclosure::CombatDisclosure`), so that tank's readiness is the server's alone.
        if replica.is_some() && !controlled.contains(root.0) && !gates.contains(root.0) {
            continue;
        }
        let mut gate = gates.get_mut(root.0).expect(
            "an authority or locally controlled weapon root has its spawn-complete private gate",
        );
        let state = gate.weapons.get_mut(slot.0).expect(SLOT_IN_GATE);
        if state.is_ready() {
            continue;
        }
        match weapon.fire_cycle {
            FireCycle::Single { reload } => {
                let rate = requirement_rate(tank_volumes, &weapon.load, &volumes);
                state.load_at_rate(now, reload, rate);
                if state.deadline_reached(now) {
                    state.finish();
                }
            }
            FireCycle::Automatic {
                belt_size, swap, ..
            } => {
                if state.belt_remaining > 0 {
                    if state.deadline_reached(now) {
                        state.finish();
                    }
                } else {
                    let rate = requirement_rate(tank_volumes, &weapon.load, &volumes);
                    state.load_at_rate(now, swap, rate);
                    if state.deadline_reached(now) {
                        state.belt_remaining = belt_size;
                        state.finish();
                    }
                }
            }
        }
    }
}

/// Fire each tank's weapons whose trigger its command holds this tick — THE one fire system, a
/// `match` on [`FireCycle`] per weapon, never per-mechanism systems (the schedule edges above are
/// determinism-load-bearing and must stay singular). `Trigger` is pure input ROUTING (which
/// command field the weapon reads); the input *semantics* come from the mode: a `Single` weapon
/// consumes a latched click edge (`fire_primary`-style, one tick per click), an `Automatic` reads
/// a held level and cycles at the gun's 60/rpm from a finite belt. Each weapon fires from its *own* muzzle
/// and ballistics, gated by its `fire` requirement + fire timer (+ belt for an `Automatic`) — the
/// gate lives here in the sim, where the server will enforce it, not in the input path.
#[expect(
    clippy::too_many_arguments,
    reason = "the one fire system: tanks, volumes, weapons, sims, gates, bodies, hierarchy, net role and clocks"
)]
fn fire(
    tanks: Query<
        (
            &TankCommand,
            Option<&TankVolumes>,
            &Position,
            &Rotation,
            Option<&crate::CombatantId>,
        ),
        With<Tank>,
    >,
    volumes: Query<VolumeFacets>,
    weapons: Query<(Entity, &Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
    mut sims: Query<&mut TankSim>,
    (mut gates, loads): (Query<&mut WeaponGate>, Query<&WeaponLoads>),
    mut bodies: Query<Forces, With<Tank>>,
    proxies: Query<(Entity, &Collider, &CollisionLayers)>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    // Present only on a net client, where own fire is FUSED: a keyed round's one presentation is
    // the server's echoed fact, released at the interpolation cursor by `net::client`, so the local
    // tick must draw no shell and no barrel kick. Absent everywhere else (server / SP / sandbox),
    // where this system presents locally.
    replica: Option<Res<crate::ClientReplica>>,
    controlled: Query<(), With<crate::tank::Controlled>>,
    // Present in a composed network app, where `net::protocol` republishes the `LocalTimeline`
    // before `GameplaySet`, so an attributed local FireShell is born with the identity that changes
    // its ballistic hold/keyframe behaviour; and in single-player, whose clock
    // (`damage_cam::delivery::offline_plugin`) keys the rounds its pen cam records explain.
    shot_clock: Option<Res<crate::ShotClock>>,
    weapon_clock: Res<crate::WeaponClock>,
    mut commands: Commands,
) {
    for (muzzle_entity, weapon, slot, root) in &weapons {
        let (command, tank_volumes, position, rotation, combatant) =
            tanks.get(root.0).expect(WEAPON_ROOT_IS_A_TANK);
        // Input routing only — edge vs level is baked into the command fields themselves
        // (`fire_primary` is a one-tick click latch, `fire_secondary` a held level; see
        // `TankCommand`), so a `Single` on Primary consumes the edge and an `Automatic` on
        // Secondary reads the level, per the mode's contract.
        let triggered = match weapon.trigger {
            Trigger::Primary => command.fire_primary,
            Trigger::Secondary => command.fire_secondary,
        };
        // A client fires no other player's tank: its `WeaponGate` and `WeaponLoads` are
        // owner-private (`net::disclosure::CombatDisclosure`) and never reach this client.
        if replica.is_some()
            && !controlled.contains(root.0)
            && !gates.contains(root.0)
            && !loads.contains(root.0)
        {
            continue;
        }
        let tank_gate = gates.get(root.0).expect(
            "an authority or locally controlled weapon root has its spawn-complete private gate",
        );
        let tank_loads = loads
            .get(root.0)
            .expect("a weapon root with a private gate also has its spawn-complete private loads");
        // Ready = no active absolute deadline, plus rounds for an `Automatic`. A dry belt remains
        // firm even if malformed state ever carries no deadline.
        let state = tank_gate.weapons.get(slot.0).expect(SLOT_IN_GATE);
        let ready = state.is_ready()
            && match weapon.fire_cycle {
                FireCycle::Single { .. } => true,
                FireCycle::Automatic { .. } => state.belt_remaining > 0,
            };
        if !triggered || !ready || !requirement_met(tank_volumes, &weapon.fire, &volumes) {
            continue;
        }
        // The chambered round ([`crate::tank::WeaponLoad::loaded`]). The fuze mode is read now, at
        // fire time, so a toggle after loading still applies to the chambered round; whether it
        // may is a gameplay question still open.
        let load = tank_loads.weapons.get(slot.0).expect(SLOT_IN_LOADS);
        let fired = weapon.rounds.get(usize::from(load.loaded)).expect(
            "a loaded index names one of the weapon's rounds: apply_round_selection steps it \
             within the list",
        );
        let mode = load.fuze_mode(&weapon.rounds, load.loaded).expect(
            "a loaded round's fuze mode is one its fuze lists: WeaponLoad::toggle_fuze steps \
             within that list",
        );
        let (loaded, fired) = (
            load.loaded,
            fired
                .in_mode(mode)
                .expect("a round fires in a mode its own fuze lists"),
        );

        // Tick-truth muzzle pose (`rig_world_pose`, never `GlobalTransform` — see its doc): the
        // muzzle decides where the shell goes, so it must be the pose the server's tick also
        // computes, not the render picture. The chain composes the servo angles (tick-truth in
        // the fixed loop) and the barrel's recoil offset (stepped in `FixedUpdate`).
        let (muzzle_position, muzzle_rotation) = rig_world_pose(
            muzzle_entity,
            root.0,
            position.0,
            rotation.0,
            &parents,
            &locals,
        )
        .expect("a muzzle's hierarchy reaches its root through nodes that each carry a Transform");
        let bore = Dir3::new(muzzle_rotation * Vec3::NEG_Z).expect(
            "a muzzle's tick pose composes finite unit rotations: the root's from the solver, the \
             rig's from validated servo and recoil state",
        );

        // Round bookkeeping: is THIS round a tracer, then advance the counter. A `Single`'s round
        // always traces (its visual is the shell scene, not a streak — `tracer_round` is belt
        // arithmetic and only the `Automatic` arm calls it). Decided from root-resident `TankSim`
        // state so the server and the client — which both run this `fire` and count their own
        // belts from 0 — agree on each round's tracer-ness. The flag rides FireShell → FireEvent
        // so remote clients match too (see `WeaponState::rounds_fired`).
        let tracer = {
            let mut sim = sims.get_mut(root.0).expect(ROOT_HAS_SIM);
            let state = sim.weapons.get_mut(slot.0).expect(SLOT_IN_SIM);
            let is_tracer = match weapon.fire_cycle {
                FireCycle::Single { .. } => true,
                FireCycle::Automatic { tracer_every, .. } => {
                    tracer_round(state.rounds_fired, tracer_every)
                }
            };
            state.rounds_fired = state.rounds_fired.wrapping_add(1);
            is_tracer
        };

        // The identity belongs to the event, not a later post-spawn repair: `on_fire_shell`
        // includes it in the shell's initial bundle, so lifecycle observers and the first march
        // see the same keyed state. The load gate bounds every weapon slot to the wire's u8.
        let shot = shot_clock.as_ref().map(|clock| {
            let combatant = combatant.expect(
                "network tank fired without spawn-complete CombatantId; immutable sim state \
                 must arrive with the predicted root",
            );
            crate::ShotId {
                combatant: *combatant,
                weapon: u8::try_from(slot.0)
                    .expect("the tank load gate limits weapon slots to the wire's u8"),
                fire_tick: clock.0.raw(),
            }
        });
        // Fused own fire suppresses exactly the rounds the server will echo: a keyed shot's shell,
        // flash, and barrel kick present from that echo at the cursor; an unkeyed shot has no echo
        // and presents locally.
        let fused_echo = replica.is_some() && shot.is_some();
        // Hand off to ballistics: fire the chambered round down the bore at its muzzle speed.
        if !fused_echo {
            let shapes: Vec<_> = proxies
                .iter()
                .filter(|(_, _, layers)| layers.memberships.has_all(crate::Layer::Vehicle))
                .filter_map(|(entity, shape, _)| {
                    rig_world_pose(entity, root.0, position.0, rotation.0, &parents, &locals)
                        .map(|(at, turn)| (at, turn, shape))
                })
                .collect();
            let birth = outside_source(muzzle_position, bore, -weapon.bore_length, &shapes);
            let event = FireShell::of_round(
                &fired,
                birth,
                muzzle_position,
                bore,
                weapon.fire_cycle.mechanism(),
                tracer,
                // This shell belongs to a tank: name its root, the firing weapon slot and the round
                // so the net server can broadcast the cosmetic tracer and the barrel-recoil cause to
                // every OTHER client (`net::server`'s FireShell observer), which each derive the
                // kick locally and bind the round from their own copy of the weapon's round list.
                Some(ShotSource {
                    tank: root.0,
                    weapon: slot.0,
                    round: loaded,
                }),
                shot,
            );
            commands.trigger(event);
        }
        // Kick the barrel back (root-resident recoil state); apply_recoil springs it home. The
        // shared `kick_recoil` owns the whole decision (barrel + recoil spec present, slot valid), so
        // this path and the opponent-view path (`net::client`) can't diverge on how a shot recoils.
        // Under fused own fire the kick arrives with the echo, through that same opponent-view path.
        if !fused_echo {
            kick_recoil(
                &mut sims.get_mut(root.0).expect(ROOT_HAS_SIM),
                slot.0,
                weapon,
            );
        }
        // Recoil reaction on the hull: the shell's momentum, opposite the bore, applied on the bore
        // axis. The line of action passes above the centre of mass, so the impulse-at-point also
        // pitches the nose up (gun climb), not just shoves the hull back. Each weapon kicks by its
        // own momentum, so the MGs barely register. A client's replica body is `Static`
        // (`net::rig`), with no solver state to take an impulse: the server's hull takes it.
        if let Ok(forces) = bodies.get_mut(root.0) {
            let impulse = recoil_impulse(bore, fired.body.mass, fired.muzzle_speed);
            apply_explicit_impulse(forces, impulse, muzzle_position);
        }
        // Arm an absolute deadline per mechanism. `Single`: crew-gated reload. `Automatic`: consume
        // one belt round, then arm either the crew-gated swap or the ungated cyclic interval.
        {
            let mut gate = gates
                .get_mut(root.0)
                .expect("the gate read above for this weapon is still present");
            let state = gate.weapons.get_mut(slot.0).expect(SLOT_IN_GATE);
            let delay = match weapon.fire_cycle {
                FireCycle::Single { reload } => reload,
                FireCycle::Automatic { cyclic, swap, .. } => {
                    state.belt_remaining = state.belt_remaining.saturating_sub(1);
                    if state.belt_remaining == 0 {
                        swap
                    } else {
                        cyclic
                    }
                }
            };
            if matches!(weapon.fire_cycle, FireCycle::Single { .. }) || state.belt_remaining == 0 {
                state.arm_load(
                    weapon_clock.0,
                    delay,
                    requirement_rate(tank_volumes, &weapon.load, &volumes),
                );
            } else {
                state.arm(weapon_clock.0, delay);
            }
        }
    }
}

/// Apply each tank's round-selection edge, then the next-load rule ([`crate::tank::WeaponLoad`]) —
/// authority only: a client reads the replicated [`WeaponLoads`] and never writes it.
///
/// The edge steps the addressed slot's `selected` through the weapon's round list, wrapping; an edge
/// naming no weapon slot of the tank changes nothing. The fuze toggle then steps the selected
/// round's fuze mode ([`crate::tank::WeaponLoad::toggle_fuze`]). Then every slot whose weapon is
/// loading takes `loaded = selected`. Chained after [`fire`]: a shot in this tick fired the round chambered before
/// the edge, and the reload it armed loads the selection.
fn apply_round_selection(
    mut tanks: Query<(&TankCommand, &WeaponGate, &mut WeaponLoads), With<Tank>>,
    weapons: Query<(&Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
) {
    for (weapon, slot, root) in &weapons {
        let (command, gate, mut loads) = tanks.get_mut(root.0).expect(
            "an authority tank carries the WeaponGate and WeaponLoads its construction batch built",
        );
        let state = *gate.weapons.get(slot.0).expect(SLOT_IN_GATE);
        let load = loads.weapons.get(slot.0).expect(SLOT_IN_LOADS);
        let mut next = load.clone();
        if let Some(select) = command.select_round
            && usize::from(select.weapon) == slot.0
            && let Some(selected) = select.step.apply(load.selected, weapon.rounds.len())
        {
            next.selected = selected;
        }
        if command.toggle_fuze == u8::try_from(slot.0).ok() {
            next.toggle_fuze(&weapon.rounds, next.selected);
        }
        if weapon.is_loading(state) {
            next.loaded = next.selected;
        }
        // Written only on a change: an unchanged choice must not re-replicate.
        if next != *load {
            loads.weapons[slot.0] = next;
        }
    }
}

/// Step each recoiling barrel's damped spring (state root-resident in `TankSim::weapons`) and
/// write the barrel's node `Transform`.
fn apply_recoil(
    mut barrels: Query<(&mut Transform, &RecoilParams, &WeaponIndex, &TankRoot)>,
    mut sims: Query<&mut TankSim>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut transform, params, slot, root) in &mut barrels {
        let mut sim = sims.get_mut(root.0).expect(ROOT_HAS_SIM);
        let state = sim.weapons.get_mut(slot.0).expect(SLOT_IN_SIM);
        // Damped spring back to battery: offset'' = -k·offset - c·offset'.
        let accel =
            -params.stiffness * state.recoil_offset - params.damping * state.recoil_velocity;
        state.recoil_velocity += accel * dt;
        state.recoil_offset += state.recoil_velocity * dt;
        // The two stops: battery ahead (the barrel can't return past its rest position) and the
        // recoil stop behind (the buffer's limit). Each holds the barrel and zeroes its velocity,
        // so no run of kicks carries it past either mechanical stop.
        if state.recoil_offset < 0.0 {
            state.recoil_offset = 0.0;
            state.recoil_velocity = 0.0;
        } else if state.recoil_offset > params.max_length {
            state.recoil_offset = params.max_length;
            state.recoil_velocity = 0.0;
        }
        // Recoil rides back along the bore (+local Z), measured from the rest position.
        let translation = params.rest + Vec3::Z * state.recoil_offset;
        if transform.translation != translation {
            transform.translation = translation;
        }
    }
}

/// The game's damage-model cadence fixture mounts the real deadline and fire consumers.
#[cfg(test)]
pub(crate) fn mount_bot_gate_fixture(app: &mut App) {
    app.add_systems(
        Update,
        (tick_weapon_gate, fire)
            .chain()
            .in_set(SimPhase::WeaponFire),
    );
}

#[cfg(test)]
pub(crate) fn spawn_bot_gate_fixture(world: &mut World, cycle: FireCycle) -> Entity {
    let root = tests::spawn_main_gun_rig(world);
    for (mut weapon, tank) in world.query::<(&mut Weapon, &TankRoot)>().iter_mut(world) {
        if tank.0 == root {
            weapon.fire_cycle = cycle;
        }
    }
    world.get_mut::<WeaponGate>(root).unwrap().weapons[0] =
        crate::tank::WeaponGateState::for_cycle(&cycle);
    root
}

#[cfg(test)]
mod tests {
    use avian3d::prelude::{Collider, CollisionLayers, Position, Rotation};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;

    use super::{
        FireShell, RecoilParams, advance_weapon_clock, apply_recoil, fire, plugin,
        tick_weapon_gate, tracer_round,
    };
    use crate::command::{RoundSelect, RoundStep, TankCommand};
    use crate::damage::{CrewStation, Crewman, Dead, Group, Part, Requirement, VolumeOf};
    use crate::sim_tick::{SimTick, TickSpan};
    use crate::spec::{FireCycle, Trigger};
    use crate::tank::{
        Muzzle, Tank, TankRoot, TankSim, Weapon, WeaponGate, WeaponGateState, WeaponIndex,
        WeaponLoad, WeaponLoads, WeaponRound, WeaponState,
    };

    /// The game's damage model: hull recoil is opposite the bore and linear in shot momentum.
    /// The owner requested both the shot-momentum reference and the current response as MEASURED
    /// software readouts; this gate deliberately does not choose their ratio. FIXTURE DATA: the
    /// synthetic mass and speed below exercise the invariant independently of the registry.
    #[test]
    fn hull_recoil_is_an_opposed_linear_response_to_shot_momentum() {
        let (mass, speed) = (3.5, 220.0);
        let impulse = super::recoil_impulse(Dir3::Z, mass, speed);
        assert!(impulse.is_finite() && impulse.z < 0.0);
        assert_eq!(impulse.x, 0.0);
        assert_eq!(impulse.y, 0.0);
        assert_eq!(
            super::recoil_impulse(Dir3::Z, mass * 2.0, speed),
            impulse * 2.0
        );
        assert_eq!(
            super::recoil_impulse(Dir3::Z, mass, speed * 2.0),
            impulse * 2.0
        );
        for (gun, key, round) in crate::guns::GunRegistry::shipped().fired_rounds() {
            let momentum = round.body.mass * round.muzzle_speed;
            let current = -super::recoil_impulse(Dir3::Z, round.body.mass, round.muzzle_speed).z;
            println!(
                "MEASURED software recoil {gun}/{key}: shot_momentum_n_s={momentum:.7} current_n_s={current:.7}"
            );
        }
    }

    /// A loader's seat with its own occupant, as a tank spawns it.
    fn native_loader(root: Entity) -> (CrewStation, Crewman, VolumeOf) {
        (
            CrewStation::Loader,
            Crewman {
                home: CrewStation::Loader,
            },
            VolumeOf(root),
        )
    }

    /// The belt-fed test cycle at 64 Hz (DERIVED): 600 rpm's 0.1 s cyclic interval rounded up to
    /// 7 ticks, a 2-round belt, a 1 s swap.
    const MG_CYCLE: FireCycle = FireCycle::Automatic {
        cyclic: TickSpan::of(7),
        belt_size: 2,
        swap: TickSpan::of(64),
        tracer_every: 5,
    };
    /// A 0.5 s reload at 64 Hz (DERIVED).
    const MAIN_GUN_CYCLE: FireCycle = FireCycle::Single {
        reload: TickSpan::of(32),
    };

    fn at(raw: u32) -> SimTick {
        SimTick::new(raw)
    }

    /// The game's damage model: no authored load work must remain no work at every rate.
    /// The fire consumer still needs one tick between emissions, independently of that rate.
    #[test]
    fn zero_load_work_does_not_scale_the_duplicate_emission_guard() {
        for rate in [0.0, 1.0, 0.5, 0.01] {
            let mut gate = WeaponGateState::default();
            gate.arm_load(at(0), TickSpan::of(0), rate);
            assert!(!gate.deadline_reached(at(0)));
            assert!(gate.deadline_reached(at(1)), "rate {rate}");
        }
    }

    /// The game's damage model: the former peer span is refused at decoding, and a new
    /// work share can never exceed the local full load. Rescaling must not trust a cached span.
    #[test]
    fn peer_work_cannot_panic_when_resumed_at_a_reduced_rate() {
        let legacy = r#"{"timing":{"Paused":{"work":2147483647,"since":0}},"belt_remaining":0,"load_rate":1.0}"#;
        assert!(serde_json::from_str::<WeaponGateState>(legacy).is_err());
        let json = r#"{"timing":{"Paused":{"remaining":2147483647,"since":0,"work":{"share":9223372036854775808,"rate":1.0,"base":2147483647}}},"belt_remaining":0}"#;
        let mut peer: WeaponGateState = serde_json::from_str(json).unwrap();
        peer.load_at_rate(at(0), TickSpan::of(32), 0.5); // Fixture local base.
        assert_eq!(peer.remaining_ticks(at(0)), 64);
        let bytes = postcard::to_allocvec(&peer).unwrap();
        assert_eq!(
            postcard::from_bytes::<WeaponGateState>(&bytes).unwrap(),
            peer
        );
        // DERIVED codec edge cases: positive normal and subnormal retained rates are legal.
        // Their elapsed-work conversion must stay bounded before a local-rate resume.
        for rate in [f32::from_bits(1), f32::MIN_POSITIVE, 1.0] {
            let wire = serde_json::json!({"timing": {"Loading": {
                "ready_tick": TickSpan::MAX, "since": 0,
                "work": {"share": 1_u64 << 63, "rate": rate, "base": TickSpan::MAX}
            }}, "belt_remaining": 0});
            let mut peer: WeaponGateState = serde_json::from_value(wire).unwrap();
            peer.pause(at(1));
            peer.load_at_rate(at(2), TickSpan::of(32), 0.5);
            assert_eq!(peer.remaining_ticks(at(2)), 64);
        }
    }

    /// The game's damage model: the player's frozen readout must equal resumed work at the
    /// retained rate. Fixture late pauses catch a ceiling tick invented by fractional rounding.
    #[test]
    fn pausing_near_completion_never_invents_a_tick_of_work() {
        for base in [7, 49, 192, 1000, TickSpan::MAX] {
            let mut gate = WeaponGateState::default();
            gate.arm_load(at(0), TickSpan::of(base), 1.0);
            gate.pause(at(base - 1));
            let displayed = gate.remaining_ticks(at(100));
            assert_eq!(displayed, 1);
            gate.load_at_rate(at(u32::MAX), TickSpan::of(base), 1.0);
            assert_eq!(gate.remaining_ticks(at(u32::MAX)), displayed, "base {base}");
        }
    }

    /// The game's damage model: an admitted reload must cross clock wrap without panicking.
    /// Fire eligibility relies on the deadline maturing on the far side of the counter's end.
    #[test]
    fn weapon_deadlines_cross_wrap() {
        let mut gate = WeaponGateState::default();
        gate.arm(at(u32::MAX - 1), TickSpan::of(3));
        assert!(!gate.deadline_reached(at(u32::MAX)));
        assert_eq!(gate.remaining_ticks(at(u32::MAX)), 2);
        assert!(gate.deadline_reached(at(1)));
    }

    /// The game's damage model: malformed shares, references and phases are refused once
    /// at the JSON/postcard edge, before the owning client's fire ledger can seed them.
    #[test]
    fn a_peer_cannot_seed_an_invalid_weapon_phase() {
        // DERIVED Q63 codec fixtures: unity is 2^63 work units; larger values are refused.
        for share in [(1_u64 << 63) + 1, u64::MAX] {
            let bytes =
                postcard::to_allocvec(&(3_u32, 3_u32, 0_u32, share, 1.0_f32, 32_u32, 0_u32))
                    .unwrap();
            assert!(postcard::from_bytes::<WeaponGateState>(&bytes).is_err());
            let json = format!(
                r#"{{"timing":{{"Paused":{{"remaining":3,"since":0,"work":{{"share":{share},"rate":1.0,"base":32}}}}}},"belt_remaining":0}}"#
            );
            assert!(
                serde_json::from_str::<WeaponGateState>(&json)
                    .unwrap_err()
                    .to_string()
                    .contains("work units")
            );
        }
        for share in ["-1", "1.1"] {
            let json = format!(
                r#"{{"timing":{{"Paused":{{"remaining":3,"since":0,"work":{{"share":{share},"rate":1.0,"base":32}}}}}},"belt_remaining":0}}"#
            );
            assert!(serde_json::from_str::<WeaponGateState>(&json).is_err());
        }
        for (base, belt, message) in [(0, 0, "positive reference base"), (32, 5, "empty belt")] {
            let json = format!(
                r#"{{"timing":{{"Paused":{{"remaining":3,"since":0,"work":{{"share":9223372036854775808,"rate":1.0,"base":{base}}}}}}},"belt_remaining":{belt}}}"#
            );
            assert!(
                serde_json::from_str::<WeaponGateState>(&json)
                    .unwrap_err()
                    .to_string()
                    .contains(message)
            );
        }
        let mut loading = WeaponGateState::default();
        loading.arm_load(at(0), TickSpan::of(32), 1.0);
        loading.belt_remaining = 1; // Fixture impossible belt beside load work.
        let json = serde_json::to_string(&loading).unwrap();
        assert!(
            serde_json::from_str::<WeaponGateState>(&json)
                .unwrap_err()
                .to_string()
                .contains("empty belt")
        );
        let mut valid = WeaponGateState::default();
        valid.arm_load(at(u32::MAX), TickSpan::of(TickSpan::MAX), 1.0);
        valid.pause(at(u32::MAX));
        let bytes = postcard::to_allocvec(&valid).unwrap();
        assert_eq!(
            postcard::from_bytes::<WeaponGateState>(&bytes).unwrap(),
            valid
        );
    }

    /// The game's damage model: rates exist only in work-bearing phases, and malformed peer
    /// rates must be refused before they can affect elapsed work.
    #[test]
    fn a_peer_cannot_seed_an_invalid_load_rate() {
        for rate in [0.0_f32, -1.0, 1.1, f32::INFINITY, f32::NAN] {
            let bytes =
                postcard::to_allocvec(&(3_u32, 3_u32, 0_u32, 1_u64 << 63, rate, 32_u32, 0_u32))
                    .unwrap();
            assert!(postcard::from_bytes::<WeaponGateState>(&bytes).is_err());
            if rate.is_finite() {
                let json = format!(
                    r#"{{"timing":{{"Paused":{{"remaining":3,"since":0,"work":{{"share":9223372036854775808,"rate":{rate},"base":32}}}}}},"belt_remaining":0}}"#
                );
                assert!(
                    serde_json::from_str::<WeaponGateState>(&json)
                        .unwrap_err()
                        .to_string()
                        .contains("load rate")
                );
            }
        }
    }

    /// The game's damage model: every absolute counter value in a valid peer gate is safe at
    /// the owner boundary. A deadline at the last counter value cannot panic the pause path.
    #[test]
    fn a_peer_gate_at_the_last_tick_can_pause() {
        let mut authority = WeaponGateState::default();
        authority.arm_load(at(u32::MAX - 1), TickSpan::ONE, 1.0);
        let wire = serde_json::to_vec(&authority).unwrap();
        let mut peer: WeaponGateState = serde_json::from_slice(&wire).unwrap();
        peer.pause(at(u32::MAX - 1));
        assert_eq!(peer.remaining_ticks(at(u32::MAX - 1)), 1);
    }

    /// The game's damage model: the published clock is also the UI's legality clock. The
    /// local fallback must not put it one tick ahead between network publications.
    /// FIXTURE DATA: the publisher holds both clocks at tick 42 between fixed ticks.
    #[test]
    fn a_published_weapon_clock_never_advances_between_network_ticks() {
        let mut app = pause_test_app();
        app.insert_resource(crate::PublishedWeaponClock)
            .insert_resource(crate::WeaponClock(SimTick::new(42)))
            .insert_resource(crate::ShotClock(SimTick::new(42)));
        app.world_mut().run_schedule(FixedLast);
        assert_eq!(
            app.world().resource::<crate::WeaponClock>().0,
            app.world().resource::<crate::ShotClock>().0
        );
    }

    /// The game's damage model: weapon eligibility assumes the asset parser's timestep.
    /// Even an unarmed composition must refuse a mismatch at startup, before play.
    #[test]
    #[should_panic(expected = "every composition runs the fixed loop at the default timestep")]
    fn a_nonstandard_weapon_timestep_is_refused_before_play() {
        let mut app = App::new();
        // FIXTURE DATA: a nondefault fixed rate, with a startup schedule even without the gate.
        app.insert_resource(Time::<Fixed>::from_hz(60.0))
            .init_schedule(Startup);
        plugin(&mut app);
        app.world_mut().run_schedule(Startup);
    }

    /// The game's damage model: deadline arming, crew-work pause/resume and the offline clock
    /// all wrap. An absolute tick's numeric position cannot stop a weapon or panic a client.
    #[test]
    fn arming_pausing_resuming_and_the_local_clock_wrap() {
        let mut gate = WeaponGateState::default();
        gate.arm_load(at(u32::MAX - 1), TickSpan::of(3), 1.0);
        gate.pause(at(u32::MAX));
        assert_eq!(gate.remaining_ticks(at(100)), 2);
        gate.load_at_rate(at(1), TickSpan::of(3), 1.0);
        assert_eq!(gate.remaining_ticks(at(1)), 2);
        assert!(!gate.deadline_reached(at(2)));
        assert!(gate.deadline_reached(at(3)));
        let mut world = World::new();
        world.insert_resource(crate::WeaponClock(crate::sim_tick::SimTick::new(u32::MAX)));
        world.run_system_once(advance_weapon_clock).unwrap();
        assert_eq!(world.resource::<crate::WeaponClock>().0.raw(), 0);
        // A peer's complete typed gate, including the last counter value, is safe to pause.
        let wire = serde_json::to_vec(&gate).unwrap();
        let mut peer: WeaponGateState = serde_json::from_slice(&wire).unwrap();
        peer.arm_load(at(u32::MAX - 1), TickSpan::ONE, 1.0);
        peer.pause(at(u32::MAX - 1));
        peer.load_at_rate(at(0), TickSpan::ONE, 1.0);
        assert_eq!(peer.remaining_ticks(at(0)), 1);
        assert!(peer.deadline_reached(at(1)));
    }

    /// The game's damage model: frozen work is a bounded interval, even when a peer's loading
    /// deadline is already due or a maximum admitted reload pauses at its first tick. The HUD
    /// must not show almost a full counter of work for a past deadline; resumption uses the same
    /// remaining work after arbitrarily many clock wraps.
    #[test]
    fn paused_work_keeps_the_signed_horizon_and_clamps_an_already_due_deadline() {
        let mut gate = WeaponGateState::default();
        gate.arm_load(at(0), TickSpan::of(TickSpan::MAX), 1.0);
        gate.pause(at(0));
        assert_eq!(gate.remaining_ticks(at(u32::MAX)), TickSpan::MAX);
        gate.load_at_rate(at(1), TickSpan::of(TickSpan::MAX), 1.0);
        assert_eq!(gate.remaining_ticks(at(1)), i32::MAX as u32);
        gate.arm_load(at(10), TickSpan::ONE, 1.0);
        gate.pause(at(20));
        assert_eq!(gate.remaining_ticks(at(20)), 0);
        gate.load_at_rate(at(25), TickSpan::ONE, 1.0);
        assert!(gate.deadline_reached(at(25)));
        gate.arm_load(at(u32::MAX - 1), TickSpan::of(3), 1.0);
        gate.pause(at(u32::MAX));
        gate.load_at_rate(at(u32::MAX), TickSpan::of(3), 1.0);
        assert_eq!(gate.remaining_ticks(at(u32::MAX)), 2);
    }

    /// The game's damage model: firing carries the collider-exit birth beside the unchanged
    /// visual muzzle. The physics shape and muzzle offset are fixture data.
    #[test]
    fn mounted_fire_keeps_the_muzzle_and_exports_the_source_exit() {
        let mut world = test_world();
        spawn_main_gun_rig(&mut world);
        let muzzle = Vec3::NEG_Z * 5.0;
        world
            .query::<&mut Weapon>()
            .single_mut(&mut world)
            .unwrap()
            .bore_length = 5.0;
        world
            .query_filtered::<&mut Transform, With<Muzzle>>()
            .single_mut(&mut world)
            .unwrap()
            .translation = muzzle;
        let points = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = points.clone();
        world.add_observer(move |fire: On<FireShell>| {
            captured.lock().unwrap().push((fire.origin, fire.birth))
        });
        world.run_system_once(fire).unwrap();
        let points = points.lock().unwrap();
        let &[(visual, birth)] = points.as_slice() else {
            panic!("one mounted round fired")
        };
        assert_eq!(visual, muzzle, "flash and report stay at the muzzle");
        assert!(
            birth.z < -1.0 && birth.z > -1.01,
            "physical birth is the source's forward exit: {birth:?}"
        );
    }

    /// The game's damage model: a fixed bore above its source envelope fires independently of
    /// the body's arbitrary origin. Fixture data puts that origin ahead of the muzzle projection.
    #[test]
    fn a_fixed_weapon_above_the_envelope_fires_with_its_root_ahead_of_its_muzzle() {
        let mut world = test_world();
        spawn_main_gun_rig(&mut world);
        let muzzle = Vec3::new(0.0, 2.0, 1.0);
        world
            .query_filtered::<&mut Transform, With<Muzzle>>()
            .single_mut(&mut world)
            .unwrap()
            .translation = muzzle;
        let points = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = points.clone();
        world.add_observer(move |fire: On<FireShell>| {
            captured.lock().unwrap().push((fire.origin, fire.birth))
        });
        world.run_system_once(fire).unwrap();
        let points = points.lock().unwrap();
        assert_eq!(points.len(), 1, "a clear fixed bore must fire");
        assert_eq!(points[0].0, muzzle);
        assert!(
            points[0].1.z >= muzzle.z,
            "the bore starts behind its muzzle"
        );
    }

    /// A fixture automatic rig: commanded root, posed physics proxy, ready gate, live Loader
    /// and a muzzle child with the given load gate. Returns the root and Loader volume.
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
                    weapons: vec![WeaponState::default()],
                },
                WeaponGate {
                    weapons: vec![WeaponGateState::for_cycle(&MG_CYCLE)],
                },
                WeaponLoads {
                    weapons: vec![WeaponLoad::default()],
                },
                crate::CombatantId(1),
            ))
            .id();
        world.entity_mut(root).insert((
            Collider::cuboid(2.0, 2.0, 2.0),
            CollisionLayers::new([crate::Layer::Vehicle], avian3d::prelude::LayerMask::ALL),
        ));
        let loader = world.spawn(native_loader(root)).id();
        world.spawn((
            Muzzle,
            WeaponIndex(0),
            TankRoot(root),
            Transform::default(),
            ChildOf(root),
            Weapon {
                bore_length: 0.4,
                name: "MG".into(),
                rounds: vec![WeaponRound::resolve("7.9mm_AP").expect("a shipped round")],
                fire_cycle: MG_CYCLE,
                recoil: None,
                barrel: None,
                fire: Vec::new(),
                load,
                trigger: Trigger::Secondary,
                report_clips: Vec::new(),
            },
        ));
        (root, loader)
    }

    pub(super) fn spawn_main_gun_rig(world: &mut World) -> Entity {
        let root = world
            .spawn((
                Tank,
                TankCommand {
                    fire_primary: true,
                    ..default()
                },
                Position::default(),
                Rotation::default(),
                TankSim {
                    weapons: vec![WeaponState::default()],
                },
                WeaponGate {
                    weapons: vec![WeaponGateState::for_cycle(&MAIN_GUN_CYCLE)],
                },
                WeaponLoads {
                    weapons: vec![WeaponLoad::default()],
                },
                crate::CombatantId(1),
            ))
            .id();
        world.entity_mut(root).insert((
            Collider::cuboid(2.0, 2.0, 2.0),
            CollisionLayers::new([crate::Layer::Vehicle], avian3d::prelude::LayerMask::ALL),
        ));
        world.spawn(native_loader(root));
        world.spawn((
            Muzzle,
            WeaponIndex(0),
            TankRoot(root),
            Transform::default(),
            ChildOf(root),
            Weapon {
                bore_length: 0.4,
                name: "88 mm".into(),
                rounds: ["88mm_AP", "88mm_APCR"]
                    .map(|key| WeaponRound::resolve(key).expect("a shipped round"))
                    .into(),
                fire_cycle: MAIN_GUN_CYCLE,
                recoil: None,
                barrel: None,
                fire: Vec::new(),
                load: Vec::new(),
                trigger: Trigger::Primary,
                report_clips: Vec::new(),
            },
        ));
        root
    }

    fn weapon_state(world: &mut World, root: Entity) -> WeaponState {
        world.get::<TankSim>(root).expect("sim on root").weapons[0]
    }

    fn gate_state(world: &World, root: Entity) -> WeaponGateState {
        world.get::<WeaponGate>(root).expect("gate on root").weapons[0]
    }

    fn test_world() -> World {
        let mut world = World::new();
        let mut fixed = Time::<Fixed>::default();
        fixed.set_timestep_hz(64.0);
        world.insert_resource(fixed);
        world.insert_resource(crate::WeaponClock(crate::sim_tick::SimTick::new(0)));
        world
    }

    /// The game's damage model: every firing root has a pose supplied by its composition. The
    /// authority constructs it at spawn; the client retains its presented stream pose at the
    /// interpolation boundary. A missing pose here is a bug on either role, never a silent skip.
    #[test]
    fn fire_requires_its_compositions_pose() {
        for replica in [false, true] {
            for rotation_gap in [false, true] {
                let mut world = test_world();
                if replica {
                    world.insert_resource(crate::ClientReplica);
                }
                let root = spawn_main_gun_rig(&mut world);
                if rotation_gap {
                    world.entity_mut(root).remove::<Rotation>();
                } else {
                    world.entity_mut(root).remove::<Position>();
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    world.run_system_once(fire).unwrap();
                }));
                assert!(
                    result.is_err(),
                    "a missing firing pose is a composition bug"
                );
            }
        }
    }

    /// The game's damage model: only another player's client replica legitimately lacks private
    /// weapon state. An authority/controlled root missing it must fail before silently holding fire.
    #[test]
    fn missing_private_weapon_state_is_only_a_remote_client_absence() {
        for (replica, controlled) in [(false, false), (true, false), (true, true)] {
            for ticking in [false, true] {
                let mut world = test_world();
                let root = spawn_main_gun_rig(&mut world);
                if replica {
                    world.insert_resource(crate::ClientReplica);
                }
                if controlled {
                    world.entity_mut(root).insert(crate::tank::Controlled);
                }
                world.entity_mut(root).remove::<(WeaponGate, WeaponLoads)>();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if ticking {
                        world.run_system_once(tick_weapon_gate).unwrap();
                    } else {
                        world.run_system_once(fire).unwrap();
                    }
                }));
                assert_eq!(result.is_ok(), replica && !controlled);
                assert_eq!(weapon_state(&mut world, root).rounds_fired, 0);
            }
        }
    }

    fn pause_test_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin));
        crate::state::sim_plugin(&mut app);
        app.insert_state(crate::state::AppState::Playing);
        plugin(&mut app);
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .set_timestep_hz(64.0);
        app
    }

    fn set_app_state(app: &mut App, state: crate::state::AppState) {
        app.insert_state(state);
        app.world_mut().run_schedule(StateTransition);
    }

    /// Each fired shell's round, as the shooter source names it, and the muzzle speed it flies at.
    #[derive(Resource, Default)]
    struct FiredRounds(Vec<(u8, f32)>);

    fn record_fired_round(fire: On<crate::ballistics::FireShell>, mut fired: ResMut<FiredRounds>) {
        let source = fire.shooter.expect("a gun's shell names its shooter");
        fired.0.push((source.round, fire.speed));
    }

    /// One fixed tick of the production gun schedule, with this tick's trigger click and
    /// round-select edge on the rig's command.
    fn gun_tick(app: &mut App, root: Entity, trigger: bool, select: Option<RoundStep>) {
        let mut command = app
            .world_mut()
            .get_mut::<TankCommand>(root)
            .expect("command on root");
        command.fire_primary = trigger;
        command.select_round = select.map(|step| RoundSelect { weapon: 0, step });
        app.world_mut().run_schedule(FixedUpdate);
        app.world_mut().run_schedule(FixedLast);
    }

    /// Idle gun ticks until the armed reload completes on the next tick.
    fn idle_until_the_reload_completes_next(app: &mut App, root: Entity) {
        let ready_tick = gate_state(app.world(), root)
            .ready_tick()
            .expect("a reload is armed");
        while !app
            .world()
            .resource::<crate::WeaponClock>()
            .0
            .has_reached(ready_tick)
        {
            gun_tick(app, root, false, None);
        }
    }

    fn load_state(world: &World, root: Entity) -> WeaponLoad {
        world
            .get::<WeaponLoads>(root)
            .expect("loads on root")
            .weapons[0]
            .clone()
    }

    fn advance(world: &mut World, ticks: u32) {
        for _ in 0..ticks {
            let mut clock = world.resource_mut::<crate::WeaponClock>();
            clock.0 = clock.0.after(TickSpan::ONE);
            world.run_system_once(tick_weapon_gate).unwrap();
        }
    }

    /// Each fired shell's fuze mode and its burst's delay (s).
    #[derive(Resource, Default)]
    struct FiredFuzes(Vec<(u8, Option<f32>)>);

    fn record_fired_fuze(fire: On<crate::ballistics::FireShell>, mut fired: ResMut<FiredFuzes>) {
        fired
            .0
            .push((fire.fuze_mode, fire.burst.map(|burst| burst.delay)));
    }

    /// The game's damage model, on the authority's systems: the fuze toggle steps the SELECTED
    /// round type's mode and no other, a one-mode type has nothing to toggle, and the shot fires
    /// the chambered type in the mode picked for it, read at fire time, with that mode's delay in
    /// its burst. A mode the round's fuze does not list is a broken load, never a held shot
    /// ([`a_load_naming_an_unlisted_fuze_mode_is_a_bug`]): the fire event's mode index relies on it.
    #[test]
    fn the_fuze_toggle_picks_the_mode_a_shot_fires_with() {
        let mut world = test_world();
        world.init_resource::<FiredFuzes>();
        world.add_observer(record_fired_fuze);
        let root = spawn_main_gun_rig(&mut world);
        let muzzle = world
            .query_filtered::<Entity, With<Muzzle>>()
            .single(&world)
            .unwrap();
        let rounds: Vec<_> = ["88mm_APHE_ring", "88mm_HE"]
            .map(|key| WeaponRound::resolve(key).unwrap())
            .into();
        let (one_mode, two_modes) = (&rounds[0], &rounds[1]);
        assert_eq!(one_mode.fuze_modes().len(), 1, "fixture: a one-mode fuze");
        let picked = crate::command::RoundStep::Next
            .apply(two_modes.round.fuze_mode, two_modes.fuze_modes().len())
            .unwrap();
        let picked_delay = two_modes.fuze_modes()[usize::from(picked)].delay_s;
        let defaults = [one_mode.round.fuze_mode, two_modes.round.fuze_mode];
        world.get_mut::<Weapon>(muzzle).unwrap().rounds = rounds;

        world.get_mut::<WeaponLoads>(root).unwrap().weapons[0].selected = 1;
        world.get_mut::<TankCommand>(root).unwrap().toggle_fuze = Some(0);
        world.run_system_once(super::apply_round_selection).unwrap();
        assert_eq!(load_state(&world, root).fuze_modes, [defaults[0], picked]);
        assert_eq!(
            load_state(&world, root).loaded,
            0,
            "a ready chamber keeps its type"
        );
        world.get_mut::<WeaponLoads>(root).unwrap().weapons[0].selected = 0;
        world.run_system_once(super::apply_round_selection).unwrap();
        assert_eq!(
            load_state(&world, root).fuze_modes,
            [defaults[0], picked],
            "a one-mode type has nothing to toggle"
        );
        world.get_mut::<TankCommand>(root).unwrap().toggle_fuze = None;

        world.get_mut::<WeaponLoads>(root).unwrap().weapons[0].loaded = 1;
        world.run_system_once(fire).unwrap();
        assert_eq!(
            world.resource::<FiredFuzes>().0,
            [(picked, Some(picked_delay))],
            "the shot carries the picked mode and its delay"
        );
    }

    /// The game's damage model: a chambered round whose stored mode its fuze does not list cannot
    /// fire as a held shot that silently spends nothing; `WeaponLoad::toggle_fuze` steps only
    /// within the list, so such a load is a bug and panics. FIXTURE DATA: the shipped HE round, its
    /// stored mode set past every list.
    #[test]
    #[should_panic(expected = "a round fires in a mode its own fuze lists")]
    fn a_load_naming_an_unlisted_fuze_mode_is_a_bug() {
        let mut world = test_world();
        let root = spawn_main_gun_rig(&mut world);
        let muzzle = world
            .query_filtered::<Entity, With<Muzzle>>()
            .single(&world)
            .unwrap();
        world.get_mut::<Weapon>(muzzle).unwrap().rounds =
            vec![WeaponRound::resolve("88mm_HE").unwrap()];
        world.get_mut::<WeaponLoads>(root).unwrap().weapons[0].fuze_modes = vec![u8::MAX];
        world.run_system_once(fire).unwrap();
    }

    /// The determinism-relevant belt lifecycle, on the real systems: firing walks the belt down,
    /// a dry belt BLOCKS fire and automatically starts the swap, the swap timer runs it out, the
    /// belt refills, and fire resumes — the authoritative gate in root-resident `WeaponGate`, with
    /// cosmetic recoil/tracer phase in `TankSim`.
    #[test]
    fn belt_runs_dry_swap_completes_fire_resumes() {
        let mut world = test_world();
        // Ungated swap (`load: []`) — the crew gate has its own test below.
        let (root, _) = spawn_mg_rig(&mut world, Vec::new());

        // Round 1: belt 2→1, the cyclic interval (60/600 = 0.1 s) arms.
        world.run_system_once(fire).unwrap();
        let s = weapon_state(&mut world, root);
        let gate = gate_state(&world, root);
        assert_eq!((s.rounds_fired, gate.belt_remaining), (1, 1));
        assert_eq!(
            gate.ready_tick(),
            Some(crate::sim_tick::SimTick::new(7)),
            "0.1 s quantizes up to 7 ticks"
        );

        // Held trigger inside the cyclic interval: no shot.
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 1);

        // Interval elapses; round 2 empties the belt — the swap starts AUTOMATICALLY (timer
        // becomes belt_swap_secs, not the cyclic interval).
        advance(&mut world, 7);
        world.run_system_once(fire).unwrap();
        let s = weapon_state(&mut world, root);
        let gate = gate_state(&world, root);
        assert_eq!((s.rounds_fired, gate.belt_remaining), (2, 0));
        assert_eq!(
            gate.ready_tick(),
            Some(crate::sim_tick::SimTick::new(71)),
            "dry belt arms a 64-tick swap"
        );

        // Dry belt blocks fire mid-swap.
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 2);

        // Half the swap: still dry, still blocked.
        advance(&mut world, 32);
        let gate = gate_state(&world, root);
        assert_eq!(gate.belt_remaining, 0);
        assert_eq!(gate.remaining_ticks(at(39)), 32);
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 2);

        // Swap completes: the belt refills to belt_size and fire resumes.
        advance(&mut world, 32);
        let gate = gate_state(&world, root);
        assert_eq!(gate.belt_remaining, 2, "completed swap refills the belt");
        assert_eq!(gate.ready_tick(), None);
        world.run_system_once(fire).unwrap();
        let s = weapon_state(&mut world, root);
        assert_eq!(
            (s.rounds_fired, gate_state(&world, root).belt_remaining),
            (3, 1)
        );
    }

    /// The game's damage model: only belt-swap work uses loader competence; the mechanical
    /// cyclic interval stays fixed. A switch-only swap or scaled cyclic timer fails here.
    /// Fixture: the two-round automatic rig, seven cyclic ticks, 64 base swap ticks.
    #[test]
    fn a_backfilled_loader_swaps_belts_at_its_competence() {
        let mut world = test_world();
        let (root, loader) = spawn_mg_rig(&mut world, vec![Group::Single(Part::Loader)]);
        world.entity_mut(loader).insert(Crewman {
            home: CrewStation::Driver,
        });
        world.run_system_once(fire).unwrap();
        assert_eq!(gate_state(&world, root).ready_tick(), Some(at(7)));
        advance(&mut world, 7);
        world.run_system_once(fire).unwrap();
        let rate = crate::damage::competence(CrewStation::Driver, CrewStation::Loader);
        let swap_ticks = (64.0_f64 / f64::from(rate)).ceil() as u32;
        assert_eq!(
            gate_state(&world, root).ready_tick(),
            Some(at(7 + swap_ticks))
        );
        assert_eq!(gate_state(&world, root).belt_remaining, 0);
        advance(&mut world, swap_ticks - 1);
        assert_eq!(gate_state(&world, root).belt_remaining, 0);
        advance(&mut world, 1);
        assert_eq!(gate_state(&world, root).belt_remaining, 2);
        assert!(gate_state(&world, root).is_ready());
    }

    /// The existing single-shot path uses the same absolute gate without acquiring belt semantics.
    #[test]
    fn single_weapon_reloads_on_absolute_deadline() {
        let mut world = test_world();
        let root = spawn_main_gun_rig(&mut world);

        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 1);
        let mut expected = WeaponGateState::default();
        expected.arm_load(at(0), TickSpan::of(32), 1.0);
        assert_eq!(gate_state(&world, root), expected);

        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 1);
        advance(&mut world, 32);
        assert_eq!(gate_state(&world, root).ready_tick(), None);
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 2);
    }

    /// The game's damage model: the loader's competence is a reload rate, not an on/off gate.
    /// A fixed-span mutant fails the deadline. Fixture: the half-second main-gun test rig.
    #[test]
    fn a_backfilled_loader_reloads_at_its_competence() {
        let mut world = test_world();
        let root = spawn_main_gun_rig(&mut world);
        let loader = world
            .query_filtered::<Entity, With<CrewStation>>()
            .single(&world)
            .unwrap();
        world.entity_mut(loader).insert(Crewman {
            home: CrewStation::Driver,
        });
        world
            .query_filtered::<&mut Weapon, With<Muzzle>>()
            .single_mut(&mut world)
            .unwrap()
            .load = vec![crate::damage::Group::Single(crate::damage::Part::Loader)];
        world.run_system_once(fire).unwrap();
        let rate = crate::damage::competence(CrewStation::Driver, CrewStation::Loader);
        let expected = (32.0_f64 / f64::from(rate)).ceil() as u32;
        assert_eq!(gate_state(&world, root).ready_tick(), Some(at(expected)));
        advance(&mut world, expected - 1);
        assert!(!gate_state(&world, root).is_ready());
        advance(&mut world, 1);
        assert!(gate_state(&world, root).is_ready());
    }

    /// The game's damage model: reload and belt swap consume the same complete-path task
    /// rate as the evaluator. Fixture: foreign operator, cooperative/alternative coefficients,
    /// two reduced module pools, and a separate required group. Cadence remains mechanical.
    #[test]
    fn loading_consumes_one_resolved_rate_for_complete_paths_and_required_groups() {
        use crate::damage::{
            FunctionRole, GradedGroup, Group, Part, TankVolumes, VolumeFacets, VolumeOf,
            requirement_rate,
        };
        for cooperative in [false, true] {
            for automatic in [false, true] {
                let mut world = test_world();
                let members = vec![
                    (0.8, vec![Part::Loader, Part::Breech]),
                    (0.2, vec![Part::Commander]),
                ];
                let req = vec![
                    Group::Graded(if cooperative {
                        GradedGroup::Pool(members)
                    } else {
                        GradedGroup::Backup(members)
                    }),
                    Group::Single(Part::GunBarrel),
                ];
                let (root, loader) = if automatic {
                    spawn_mg_rig(&mut world, req.clone())
                } else {
                    let root = spawn_main_gun_rig(&mut world);
                    world
                        .query_filtered::<&mut Weapon, With<Muzzle>>()
                        .single_mut(&mut world)
                        .unwrap()
                        .load = req.clone();
                    let loader = world
                        .query_filtered::<Entity, With<CrewStation>>()
                        .single(&world)
                        .unwrap();
                    (root, loader)
                };
                world.entity_mut(loader).insert(Crewman {
                    home: CrewStation::Driver,
                });
                world.spawn((
                    CrewStation::Commander,
                    Crewman {
                        home: CrewStation::Commander,
                    },
                    VolumeOf(root),
                ));
                for (function, held) in [(FunctionRole::Breech, 5), (FunctionRole::GunBarrel, 4)] {
                    let mut health = crate::ballistics::ComponentHealth::full(10);
                    health.set_held(held * crate::ballistics::POOL_UNITS_PER_HP);
                    world.spawn((function, health, VolumeOf(root)));
                }
                let rate = world
                    .run_system_once(
                        move |tanks: Query<&TankVolumes>, facets: Query<VolumeFacets>| {
                            requirement_rate(Some(tanks.get(root).unwrap()), &req, &facets)
                        },
                    )
                    .unwrap();
                let competence =
                    crate::damage::competence(CrewStation::Driver, CrewStation::Loader);
                let contribution = 0.8 * competence * 0.5;
                let expected = if cooperative {
                    (contribution + 0.2).min(0.4)
                } else {
                    contribution.clamp(0.2, 0.4)
                };
                assert_eq!(rate, expected);
                if automatic {
                    world.get_mut::<WeaponGate>(root).unwrap().weapons[0].belt_remaining = 1;
                }
                world.run_system_once(fire).unwrap();
                let state = gate_state(&world, root);
                let crate::tank::WeaponTiming::Loading {
                    work, ready_tick, ..
                } = state.timing
                else {
                    panic!("a nonzero fixture load starts work: {state:?}")
                };
                assert_eq!(work.rate(), rate);
                let cycle = if automatic { MG_CYCLE } else { MAIN_GUN_CYCLE };
                let base = match cycle {
                    FireCycle::Single { reload } => reload,
                    FireCycle::Automatic { swap, .. } => swap,
                };
                assert_eq!(
                    ready_tick,
                    at((f64::from(base.raw()) / f64::from(rate)).ceil() as u32)
                );
                advance(&mut world, 1);
                assert_eq!(gate_state(&world, root).ready_tick(), Some(ready_tick));
            }
        }
    }

    /// The game's damage model: the loader preserves completed work across a pause and each
    /// rate change. The UI and fire legality rely on one stable deadline between changes.
    /// Fixture: 32 base ticks; changes after 8 and 4 working ticks, 10 paused ticks.
    #[test]
    fn reload_preserves_work_through_pause_and_rate_changes() {
        let mut world = test_world();
        let root = spawn_main_gun_rig(&mut world);
        let loader = world
            .query_filtered::<Entity, With<CrewStation>>()
            .single(&world)
            .unwrap();
        world
            .query_filtered::<&mut Weapon, With<Muzzle>>()
            .single_mut(&mut world)
            .unwrap()
            .load = vec![crate::damage::Group::Single(crate::damage::Part::Loader)];
        world.run_system_once(fire).unwrap();
        advance(&mut world, 8);
        world.entity_mut(loader).insert(Crewman {
            home: CrewStation::Driver,
        });
        world.run_system_once(tick_weapon_gate).unwrap();
        let deadline = gate_state(&world, root).ready_tick().unwrap();
        assert_eq!(deadline, at(48));
        advance(&mut world, 4);
        assert_eq!(gate_state(&world, root).ready_tick(), Some(deadline));
        world.entity_mut(loader).insert(Dead);
        world.run_system_once(tick_weapon_gate).unwrap();
        let frozen = gate_state(&world, root);
        advance(&mut world, 10);
        assert_eq!(gate_state(&world, root), frozen);
        world.entity_mut(loader).remove::<Dead>().insert(Crewman {
            home: CrewStation::Loader,
        });
        world.run_system_once(tick_weapon_gate).unwrap();
        // DERIVED: ceil(36 remaining ticks × foreign competence) = 22 native ticks.
        assert_eq!(gate_state(&world, root).ready_tick(), Some(at(44)));
        advance(&mut world, 21);
        assert!(!gate_state(&world, root).is_ready());
        advance(&mut world, 1);
        assert!(gate_state(&world, root).is_ready());
    }

    /// An offline pause freezes every weapon's absolute deadline, the main gun's reload and the
    /// MG's belt swap alike: the gate and the fallback clock stand still through the pause, and
    /// nothing fires on the first tick after it.
    #[test]
    fn offline_pause_freezes_mg_belt_swap_deadline() {
        /// Pause for `paused_ticks` with the gate armed at `clock`, resume, and require nothing
        /// moved: the gate, the clock, and the `rounds_fired` count.
        fn assert_frozen(
            mut app: App,
            root: Entity,
            clock: u32,
            paused_ticks: u32,
            rounds_fired: u32,
            what: &str,
        ) {
            let paused_gate = gate_state(app.world(), root);
            assert_eq!(app.world().resource::<crate::WeaponClock>().0.raw(), clock);

            set_app_state(&mut app, crate::state::AppState::Paused);
            for _ in 0..paused_ticks {
                app.world_mut().run_schedule(FixedUpdate);
                app.world_mut().run_schedule(FixedLast);
            }
            assert_eq!(gate_state(app.world(), root), paused_gate, "{what}");
            assert_eq!(
                app.world().resource::<crate::WeaponClock>().0.raw(),
                clock,
                "the fallback clock must not consume {what} time while paused"
            );

            set_app_state(&mut app, crate::state::AppState::Playing);
            app.world_mut().run_schedule(FixedUpdate);
            assert_eq!(gate_state(app.world(), root), paused_gate, "{what}");
            assert_eq!(
                weapon_state(app.world_mut(), root).rounds_fired,
                rounds_fired,
                "{what}"
            );
        }

        let mut app = pause_test_app();
        let root = spawn_main_gun_rig(app.world_mut());
        app.world_mut().run_schedule(FixedUpdate);
        app.world_mut().run_schedule(FixedLast);
        assert_eq!(
            gate_state(app.world(), root).ready_tick(),
            Some(crate::sim_tick::SimTick::new(32))
        );
        assert_frozen(app, root, 1, 64, 1, "reload");

        let mut app = pause_test_app();
        let (root, _) = spawn_mg_rig(app.world_mut(), Vec::new());
        app.world_mut().run_schedule(FixedUpdate);
        for _ in 0..7 {
            app.world_mut().run_schedule(FixedLast);
            app.world_mut().run_schedule(FixedUpdate);
        }
        let gate = gate_state(app.world(), root);
        assert_eq!(gate.belt_remaining, 0);
        assert_eq!(gate.ready_tick(), Some(crate::sim_tick::SimTick::new(71)));
        assert_frozen(app, root, 7, 128, 2, "belt-swap");
    }

    /// The crew-gate split the redesign exists for: the CYCLIC interval ticks with the gun crew
    /// dead (a dead loader must not freeze an MG's rate of fire — the old single-path trap), but
    /// the BELT SWAP is crew-gated like the 88's reload: dead crew = frozen swap = no fire, and a
    /// revived crew resumes and completes it.
    #[test]
    fn belt_swap_is_crew_gated_but_cyclic_interval_is_not() {
        let mut world = test_world();
        let (root, loader) = spawn_mg_rig(&mut world, vec![Group::Single(Part::Loader)]);

        // Kill the loader BEFORE anything fires (`fire: []` stays met — only `load` cares).
        world.entity_mut(loader).insert(Dead);

        // Round 1 fires, and the cyclic interval ticks out DESPITE the dead loader.
        world.run_system_once(fire).unwrap();
        assert_eq!(gate_state(&world, root).belt_remaining, 1);
        advance(&mut world, 7);
        assert_eq!(
            gate_state(&world, root).ready_tick(),
            None,
            "the cyclic interval is mechanism, not crew work — it must tick with the crew dead"
        );

        // Round 2 empties the belt; the swap arms…
        world.run_system_once(fire).unwrap();
        let s = weapon_state(&mut world, root);
        let gate = gate_state(&world, root);
        assert_eq!((s.rounds_fired, gate.belt_remaining), (2, 0));
        assert_eq!(gate.ready_tick(), Some(crate::sim_tick::SimTick::new(71)));

        // …and FREEZES: dead gun crew = no swap, however long we wait, and no fire.
        advance(&mut world, 1);
        let paused_gate = gate_state(&world, root);
        assert_eq!(
            paused_gate.paused_at_tick(),
            Some(crate::sim_tick::SimTick::new(7))
        );
        advance(&mut world, 255);
        let gate = gate_state(&world, root);
        assert_eq!(
            gate, paused_gate,
            "the replicated gate stays byte-identical throughout a crew-work pause"
        );
        assert_eq!(
            gate.remaining_ticks(at(263)),
            64,
            "dead crew freezes the swap deadline"
        );
        assert_eq!(gate.belt_remaining, 0, "no refill while the swap is frozen");
        world.run_system_once(fire).unwrap();
        assert_eq!(weapon_state(&mut world, root).rounds_fired, 2);

        // Revive the loader (slice-2 backfill shape): the swap resumes, completes, fire returns.
        world.entity_mut(loader).remove::<Dead>();
        // The first eligible tick resumes the frozen interval, then all 64 work ticks run.
        advance(&mut world, 64);
        assert_eq!(gate_state(&world, root).belt_remaining, 0);
        advance(&mut world, 1);
        assert_eq!(
            gate_state(&world, root).belt_remaining,
            2,
            "a backfilled crew member completes the full frozen swap work"
        );
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

    /// A network composition publishes the current fixed tick before `GameplaySet`, so a local
    /// firing event is born with the identity the server puts on its matching `FireEvent`. This is
    /// the event seam: downstream observers must never see an attributed local round as unkeyed.
    #[test]
    fn locally_fired_shell_event_already_carries_its_shot_id() {
        use crate::ballistics::FireShell;

        #[derive(Resource, Default)]
        struct FiredShot(Option<crate::ShotId>);

        fn capture(fire: On<FireShell>, mut fired: ResMut<FiredShot>) {
            fired.0 = fire.shot;
        }

        let mut world = test_world();
        world.insert_resource(crate::ShotClock(crate::sim_tick::SimTick::new(81)));
        world.init_resource::<FiredShot>();
        world.add_observer(capture);
        let (_root, _) = spawn_mg_rig(&mut world, Vec::new());

        world.run_system_once(fire).unwrap();

        assert_eq!(
            world.resource::<FiredShot>().0,
            Some(crate::ShotId {
                combatant: crate::CombatantId(1),
                weapon: 0,
                fire_tick: 81,
            }),
            "a locally fired network shell is keyed at the FireShell seam",
        );
    }

    #[test]
    #[should_panic(expected = "network tank fired without spawn-complete CombatantId")]
    fn network_fire_rejects_a_root_missing_its_spawn_identity() {
        let mut world = test_world();
        world.insert_resource(crate::ShotClock(crate::sim_tick::SimTick::new(81)));
        let (root, _) = spawn_mg_rig(&mut world, Vec::new());
        world.entity_mut(root).remove::<crate::CombatantId>();

        world.run_system_once(fire).unwrap();
    }

    /// Give the rig's weapon a barrel and a recoil spec, so `kick_recoil` has a kick to apply and
    /// its suppression is observable in `recoil_velocity`.
    fn arm_barrel_recoil(world: &mut World, barrel: Entity) {
        let mut weapons = world.query_filtered::<&mut Weapon, With<Muzzle>>();
        let mut weapon = weapons.single_mut(world).expect("one rig weapon");
        weapon.recoil = Some(crate::spec::RecoilSpec {
            kick: 1.5,
            stiffness: 10.0,
            damping: 1.0,
            max_length: crate::provenance::Sourced(0.5),
        });
        weapon.barrel = Some(barrel);
    }

    /// FUSED OWN FIRE (a net client's keyed round): NO local shell and NO local barrel kick — its
    /// one presentation is the server echo, released at the cursor by `net::client` — while every
    /// sim mutation (the belt walk here) runs unchanged. On the authority (no `ClientReplica`) the
    /// same tick presents locally, kick included.
    #[test]
    fn fused_own_fire_draws_no_local_shell_and_no_local_kick() {
        use crate::ballistics::FireShell;

        #[derive(Resource, Default)]
        struct FireShellCount(usize);
        fn count_fire_shells(_: On<FireShell>, mut c: ResMut<FireShellCount>) {
            c.0 += 1;
        }

        let mut world = test_world();
        world.insert_resource(crate::ShotClock(crate::sim_tick::SimTick::new(81)));
        world.init_resource::<FireShellCount>();
        world.add_observer(count_fire_shells);
        let (root, _) = spawn_mg_rig(&mut world, Vec::new());
        arm_barrel_recoil(&mut world, root);

        world.insert_resource(crate::ClientReplica);
        world.run_system_once(fire).unwrap();
        assert_eq!(
            world.resource::<FireShellCount>().0,
            0,
            "fused: a keyed round triggers no local FireShell",
        );
        assert_eq!(
            weapon_state(&mut world, root).recoil_velocity,
            0.0,
            "fused: no local barrel kick — it arrives with the echo",
        );
        assert_eq!(
            gate_state(&world, root).belt_remaining,
            1,
            "fused: the belt walk is sim truth and still runs",
        );

        world.remove_resource::<crate::ClientReplica>();
        advance(&mut world, 7);
        world.run_system_once(fire).unwrap();
        assert_eq!(
            world.resource::<FireShellCount>().0,
            1,
            "the authority presents its own shell locally",
        );
        assert!(
            weapon_state(&mut world, root).recoil_velocity > 0.0,
            "the authority applies the local barrel kick",
        );
    }

    /// THE NEXT-LOAD RULE (`WeaponLoad`), on the production gun schedule as the authority: a shot
    /// fires the loaded round, and `loaded` follows `selected` only while the gun is loading. A
    /// selection never swaps the chambered round; one made during a reload applies to that reload;
    /// one made in the tick the reload completes waits for the next load. The HUD's `next:` readout
    /// relies on it, and so does every replica, which binds the fired round by the index this
    /// yields. A mutant that loads on every tick fires the selection from the chamber; one that
    /// never loads fires the default round forever.
    #[test]
    fn a_selection_loads_at_the_next_load_and_the_chambered_round_fires_as_loaded() {
        let mut app = pause_test_app();
        app.init_resource::<FiredRounds>();
        app.add_observer(record_fired_round);
        let root = spawn_main_gun_rig(app.world_mut());
        // Fixture data: the rig's two rounds, told apart by muzzle speed.
        let speeds = ["88mm_AP", "88mm_APCR"].map(|key| {
            WeaponRound::resolve(key)
                .expect("a shipped round")
                .round
                .muzzle_speed
        });
        assert_ne!(speeds[0], speeds[1], "the fixture's rounds must differ");
        let load = |loaded, selected| WeaponLoad {
            loaded,
            selected,
            ..default()
        };

        gun_tick(&mut app, root, false, Some(RoundStep::Next));
        assert_eq!(
            load_state(app.world(), root),
            load(0, 1),
            "a ready gun keeps its chambered round",
        );

        gun_tick(&mut app, root, true, None);
        assert_eq!(app.world().resource::<FiredRounds>().0, [(0, speeds[0])]);
        assert_eq!(
            load_state(app.world(), root),
            load(1, 1),
            "the reload the shot armed loads the selection",
        );

        // Mid-reload, each selection applies to this reload; stepping past either end wraps.
        gun_tick(&mut app, root, false, Some(RoundStep::Next));
        assert_eq!(load_state(app.world(), root), load(0, 0));
        gun_tick(&mut app, root, false, Some(RoundStep::Previous));
        assert_eq!(load_state(app.world(), root), load(1, 1));

        idle_until_the_reload_completes_next(&mut app, root);
        gun_tick(&mut app, root, false, Some(RoundStep::Next));
        assert_eq!(gate_state(app.world(), root).ready_tick(), None);
        assert_eq!(
            load_state(app.world(), root),
            load(1, 0),
            "a selection in the tick the reload completes waits for the next load",
        );

        gun_tick(&mut app, root, true, None);
        assert_eq!(
            app.world().resource::<FiredRounds>().0,
            [(0, speeds[0]), (1, speeds[1])],
            "the shot fires the loaded round, bound from the weapon's list",
        );
        assert_eq!(load_state(app.world(), root), load(0, 0));
    }

    /// THE SELECTION IS SERVER-CONFIRMED: on a client (`ClientReplica`) the gun schedule never
    /// applies a select edge, so an unconfirmed selection fires the round the authority loaded, and
    /// only the replicated `WeaponLoads` changes it. Observed through unkeyed rounds, which present
    /// locally. A client that latched its own selection would fire a round the authority never
    /// loaded, a shell the authority's echo contradicts.
    #[test]
    fn a_client_fires_the_authority_s_loaded_round_until_its_selection_is_confirmed() {
        let mut app = pause_test_app();
        app.insert_resource(crate::ClientReplica);
        app.init_resource::<FiredRounds>();
        app.add_observer(record_fired_round);
        let root = spawn_main_gun_rig(app.world_mut());
        let fired = |app: &App| {
            app.world()
                .resource::<FiredRounds>()
                .0
                .iter()
                .map(|&(round, _)| round)
                .collect::<Vec<_>>()
        };

        // A selection inside a reload, the window in which the authority would load it.
        gun_tick(&mut app, root, true, None);
        gun_tick(&mut app, root, false, Some(RoundStep::Next));
        assert_eq!(
            load_state(app.world(), root),
            WeaponLoad::default(),
            "a client never writes the round choice",
        );
        idle_until_the_reload_completes_next(&mut app, root);
        gun_tick(&mut app, root, true, None);
        assert_eq!(
            fired(&app),
            [0, 0],
            "the unconfirmed selection is not fired"
        );

        // The authority's confirmation arrives as replicated state.
        app.world_mut()
            .get_mut::<WeaponLoads>(root)
            .expect("loads on root")
            .weapons[0] = WeaponLoad {
            loaded: 1,
            selected: 1,
            fuze_modes: Vec::new(),
        };
        idle_until_the_reload_completes_next(&mut app, root);
        gun_tick(&mut app, root, true, None);
        assert_eq!(fired(&app), [0, 0, 1], "the confirmed round fires");
    }

    /// An UNKEYED round (no `ShotClock` → no `ShotId` → no echo will ever arrive) presents locally
    /// even on a net client — suppression covers exactly the rounds the server will echo.
    #[test]
    fn a_client_still_presents_an_unkeyed_round_locally() {
        use crate::ballistics::FireShell;

        #[derive(Resource, Default)]
        struct FireShellCount(usize);
        fn count_fire_shells(_: On<FireShell>, mut c: ResMut<FireShellCount>) {
            c.0 += 1;
        }

        let mut world = test_world();
        world.init_resource::<FireShellCount>();
        world.add_observer(count_fire_shells);
        spawn_mg_rig(&mut world, Vec::new());

        world.insert_resource(crate::ClientReplica);
        world.run_system_once(fire).unwrap();
        assert_eq!(
            world.resource::<FireShellCount>().0,
            1,
            "an echo-less round must not lose its only presentation",
        );
    }

    /// Invariant: a recoiling barrel never travels past its recoil stop, however fast it is kicked,
    /// and stops dead there: `apply_recoil` holds the offset at `max_length` and zeroes the velocity.
    /// A kick every tick, the fastest any weapon fires (`arm` spaces shots at least
    /// a tick apart), drives the spring past the stop; a mutant that drops the clamp carries the
    /// barrel through it, and one that keeps the velocity at the stop leaves it moving there.
    #[test]
    fn rapid_kicks_never_carry_the_barrel_past_its_recoil_stop() {
        // Fixture data: a stiff kick against a short stop, at the game's 64 Hz tick.
        const KICK: f32 = 2.0;
        const STOP: f32 = 0.3;
        let rest = Vec3::new(0.0, 1.0, 2.0);
        let mut world = World::new();
        let mut time = Time::<()>::default();
        time.advance_by(core::time::Duration::from_secs_f64(1.0 / 64.0));
        world.insert_resource(time);
        let root = world
            .spawn(TankSim {
                weapons: vec![WeaponState::default()],
            })
            .id();
        let barrel = world
            .spawn((
                Transform::from_translation(rest),
                RecoilParams {
                    rest,
                    stiffness: 90.0,
                    damping: 14.0,
                    max_length: STOP,
                },
                WeaponIndex(0),
                TankRoot(root),
            ))
            .id();

        let mut at_stop = 0;
        for tick in 0..256 {
            world.get_mut::<TankSim>(root).expect("sim on root").weapons[0].recoil_velocity += KICK;
            world.run_system_once(apply_recoil).unwrap();
            let state = weapon_state(&mut world, root);
            assert!(
                (0.0..=STOP).contains(&state.recoil_offset),
                "tick {tick}: offset {} outside [battery, stop {STOP}]",
                state.recoil_offset
            );
            assert_eq!(
                world.get::<Transform>(barrel).expect("barrel").translation,
                rest + Vec3::Z * state.recoil_offset,
                "tick {tick}: the barrel node sits where the offset says",
            );
            if state.recoil_offset == STOP {
                at_stop += 1;
                assert_eq!(
                    state.recoil_velocity, 0.0,
                    "tick {tick}: the barrel stops dead at the stop"
                );
            }
        }
        assert!(at_stop > 0, "a kick every tick reaches the stop");
    }
}
