//! Canonical, world-independent simulation-state hashing.

use bevy::prelude::{Quat, Vec3};

use crate::tank::TankSim;
use crate::track::sim::{TankTransmission, TrackDrive, TrackGrip, TrackGripElements};
use crate::track::transmission::{TransmissionProjectionValue, transmission_state_projection};

// Per-tick state hashes use a fixed field/slot order and raw float bits. Entity IDs and unordered
// collections must not enter the hash: client and server use different ECS identities.

/// A tiny FNV-1a 64-bit hasher over an explicit byte stream. Chosen over `std::hash::DefaultHasher`
/// deliberately: its algorithm is fixed here (not a std-version-dependent SipHash seed), so a hash is
/// reproducible across builds and trivially re-derivable by an offline tool, and it is fed only the
/// f32 bits we hand it — the world-independence guarantee lives in WHAT we write, and this type keeps
/// the HOW dependency-free and testable.
struct Fnv64(u64);

impl Fnv64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn write_u32(&mut self, x: u32) {
        for b in x.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn write_u8(&mut self, x: u8) {
        self.0 ^= u64::from(x);
        self.0 = self.0.wrapping_mul(Self::PRIME);
    }

    fn write_i8(&mut self, x: i8) {
        self.write_u8(x.to_le_bytes()[0]);
    }

    fn write_bool(&mut self, x: bool) {
        self.write_u8(u8::from(x));
    }

    fn write_u64(&mut self, x: u64) {
        for b in x.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    /// Hash the f32's RAW BITS — bit-exactness is the divergence bar, so `1.0` and the next
    /// representable value must hash apart, and `+0.0`/`−0.0` must not collide.
    fn write_f32(&mut self, x: f32) {
        self.write_u32(x.to_bits());
    }

    fn write_vec3(&mut self, v: Vec3) {
        self.write_f32(v.x);
        self.write_f32(v.y);
        self.write_f32(v.z);
    }

    fn write_quat(&mut self, q: Quat) {
        self.write_f32(q.x);
        self.write_f32(q.y);
        self.write_f32(q.z);
        self.write_f32(q.w);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// One tank's per-tick state hash plus per-component breakdown. `sim` includes authority-relevant
/// carried state that pose and velocity fields do not expose. `WeaponState::rounds_fired` is
/// deliberately absent: it selects a local tracer phase and can legitimately lag the authority by
/// one predicted round, so feeding it into a cross-world divergence rate would make a benign view
/// skew look like simulation drift. The fresh-App test has a stricter, rollback-complete digest.
pub(super) struct TankStateHash {
    pub(super) combined: u64,
    pub(super) pos: u64,
    pub(super) rot: u64,
    pub(super) lv: u64,
    pub(super) av: u64,
    /// The carried-state combination (fixed order: `drv, srv, rld, rec, blt, trn, elm`) — kept so
    /// existing analysis keyed on `hsim` still gets its single "did any carried state differ?"
    /// boolean.
    pub(super) sim: u64,
    /// `TrackDrive` shaped throttle/steer.
    pub(super) drv: u64,
    /// Servo current/previous/velocity, every servo in slot order.
    pub(super) srv: u64,
    /// Weapon reload timers, every weapon in slot order.
    pub(super) rld: u64,
    /// Barrel recoil offset/velocity, every weapon in slot order.
    pub(super) rec: u64,
    /// Per-side belt state: speed + phase.
    pub(super) blt: u64,
    /// Complete atomic `TankTransmission` state in authoritative inventory order.
    pub(super) trn: u64,
    /// Exact per-element strain/dwell in side, link, column order.
    pub(super) elm: u64,
}

/// Hash a tank root's canonical sim state (see the module-level note on world-independence). Pure and
/// ECS-free precisely so it is unit-testable: same inputs → same hash, one flipped velocity bit → a
/// different hash, and — because no entity ever enters it — hash equality is independent of the two
/// worlds' entity ids. Field order is fixed and load-bearing: `position, rotation, linvel, angvel`,
/// then `TrackDrive` (shaped command + per-side belt state), `TrackGrip`, `TrackGripElements`, the
/// complete `TankTransmission` inventory, then each `TankSim` `Vec` in slot order.
pub(super) fn hash_tank_state_with_elements(
    position: Vec3,
    rotation: Quat,
    linvel: Vec3,
    angvel: Vec3,
    drive: &TrackDrive,
    grip: &TrackGrip,
    elements: Option<&TrackGripElements>,
    transmission: &TankTransmission,
    sim: &TankSim,
) -> TankStateHash {
    let mut hp = Fnv64::new();
    hp.write_vec3(position);
    let pos = hp.finish();

    let mut hr = Fnv64::new();
    hr.write_quat(rotation);
    let rot = hr.finish();

    let mut hl = Fnv64::new();
    hl.write_vec3(linvel);
    let lv = hl.finish();

    let mut ha = Fnv64::new();
    ha.write_vec3(angvel);
    let av = ha.finish();

    // The carried state hashes as independent field-family streams so a `hsim` mismatch names its
    // field, then combines into the single `sim` boolean existing analysis keys on.
    let mut hd = Fnv64::new();
    hd.write_f32(drive.throttle);
    hd.write_f32(drive.steer);
    let drv = hd.finish();

    let mut hsv = Fnv64::new();
    for servo in &sim.servos {
        for field in servo.hash_fields() {
            hsv.write_f32(field);
        }
    }
    let srv = hsv.finish();

    let mut hrl = Fnv64::new();
    let mut hrc = Fnv64::new();
    for weapon in &sim.weapons {
        hrl.write_f32(weapon.reload_remaining);
        // `belt_remaining` GATES fire (a dry belt cannot shoot; the swap timer's meaning depends
        // on it), so it enters the hash — in the reload stream, whose fire-timer it modulates.
        // Contrast `rounds_fired`, which is deliberately EXCLUDED: that counter only picks which
        // rounds trace, a cosmetic phase that a dropped predicted shot legitimately skews by one
        // (see `WeaponState::rounds_fired`) — hashing it would flag benign skew as divergence.
        hrl.write_u32(weapon.belt_remaining);
        hrc.write_f32(weapon.recoil_offset);
        hrc.write_f32(weapon.recoil_velocity);
    }
    let rld = hrl.finish();
    let rec = hrc.finish();

    let mut hbl = Fnv64::new();
    for side in &drive.sides {
        hbl.write_f32(side.speed);
        // Phase is f64 sim state; both halves enter so no precision is silently dropped.
        hbl.write_u64(side.phase.to_bits());
    }
    // The static-friction state rides the belt stream (ADR-0026): per side, both grip axes.
    for side in &grip.sides {
        hbl.write_f32(side[0]);
        hbl.write_f32(side[1]);
    }
    let blt = hbl.finish();

    // The transmission module owns the exhaustive REV-14 field order and pinned scheduler tags.
    // This encoder preserves the original byte stream: scheduler from/to bytes exist only for a
    // GradeShift tag.
    let mut htr = Fnv64::new();
    for field in transmission_state_projection(&transmission.0) {
        match field.value {
            TransmissionProjectionValue::U8(value) => htr.write_u8(value),
            TransmissionProjectionValue::I8(value) => htr.write_i8(value),
            TransmissionProjectionValue::Bool(value) => htr.write_bool(value),
            TransmissionProjectionValue::F32(value) => htr.write_f32(value),
            TransmissionProjectionValue::Scheduler { tag, from, to } => {
                htr.write_u8(tag);
                if tag == 1 {
                    htr.write_u8(from);
                    htr.write_u8(to);
                }
            }
        }
    }
    let trn = htr.finish();

    // Explicit side then flat `link * 3 + column` order. Raw float bits and the exact
    // force-affecting dwell byte make this the determinism hash, not the coarse anchor digest.
    let mut hel = Fnv64::new();
    hel.write_bool(elements.is_some());
    if let Some(elements) = elements {
        for (side_index, side) in elements.sides.iter().enumerate() {
            hel.write_u8(side_index as u8);
            hel.write_u32(side.strain.len() as u32);
            for (element, (&strain, &dwell)) in side.strain.iter().zip(&side.dwell).enumerate() {
                hel.write_u32(element as u32);
                hel.write_vec3(strain);
                hel.write_u8(dwell);
            }
        }
    }
    let elm = hel.finish();

    let mut hs = Fnv64::new();
    for sub in [drv, srv, rld, rec, blt, trn, elm] {
        hs.write_u64(sub);
    }
    let sim_hash = hs.finish();

    // Combine the sub-hashes in fixed order so `combined` reflects EVERY field and no sub-component's
    // difference can cancel another's.
    let mut hc = Fnv64::new();
    for sub in [pos, rot, lv, av, sim_hash] {
        hc.write_u64(sub);
    }
    TankStateHash {
        combined: hc.finish(),
        pos,
        rot,
        lv,
        av,
        sim: sim_hash,
        drv,
        srv,
        rld,
        rec,
        blt,
        trn,
        elm,
    }
}

/// Test convenience for hashes whose element field is intentionally absent. Production always
/// calls [`hash_tank_state_with_elements`] explicitly so omission cannot be accidental.
#[cfg(test)]
fn hash_tank_state(
    position: Vec3,
    rotation: Quat,
    linvel: Vec3,
    angvel: Vec3,
    drive: &TrackDrive,
    grip: &TrackGrip,
    transmission: &TankTransmission,
    sim: &TankSim,
) -> TankStateHash {
    hash_tank_state_with_elements(
        position,
        rotation,
        linvel,
        angvel,
        drive,
        grip,
        None,
        transmission,
        sim,
    )
}

/// In-memory fresh-App digest. `simulation` is exactly the production trace's cross-world hash;
/// `rollback` additionally folds every `WeaponState::rounds_fired` value. The latter is rollback
/// state but deliberately excluded from the production trace (see [`TankStateHash`]), whose job is
/// to compare authority-relevant simulation across a predicted client and server.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CanonicalTankStateDigest {
    simulation: u64,
    rollback: u64,
    position: u64,
    rotation: u64,
    linear_velocity: u64,
    angular_velocity: u64,
    drive: u64,
    servo: u64,
    reload: u64,
    recoil: u64,
    belts: u64,
    transmission: u64,
    elements: u64,
    rounds_fired: u64,
}

#[cfg(test)]
pub(crate) fn canonical_tank_state_digest(
    position: Vec3,
    rotation: Quat,
    linvel: Vec3,
    angvel: Vec3,
    drive: &TrackDrive,
    grip: &TrackGrip,
    elements: &TrackGripElements,
    transmission: &TankTransmission,
    sim: &TankSim,
) -> CanonicalTankStateDigest {
    let hash = hash_tank_state_with_elements(
        position,
        rotation,
        linvel,
        angvel,
        drive,
        grip,
        Some(elements),
        transmission,
        sim,
    );
    let mut phase = Fnv64::new();
    for weapon in &sim.weapons {
        phase.write_u32(weapon.rounds_fired);
    }
    let rounds_fired = phase.finish();
    let mut rollback = Fnv64::new();
    rollback.write_u64(hash.combined);
    rollback.write_u64(rounds_fired);
    CanonicalTankStateDigest {
        simulation: hash.combined,
        rollback: rollback.finish(),
        position: hash.pos,
        rotation: hash.rot,
        linear_velocity: hash.lv,
        angular_velocity: hash.av,
        drive: hash.drv,
        servo: hash.srv,
        reload: hash.rld,
        recoil: hash.rec,
        belts: hash.blt,
        transmission: hash.trn,
        elements: hash.elm,
        rounds_fired,
    }
}

/// Test-only readout for the exact `helm` stream. Keeping the digest fields private prevents
/// production callers from depending on its decomposition while the netcode battery can name the
/// element-hash assertion explicitly.
#[cfg(test)]
pub(crate) fn canonical_element_hash(
    position: Vec3,
    rotation: Quat,
    linvel: Vec3,
    angvel: Vec3,
    drive: &TrackDrive,
    grip: &TrackGrip,
    elements: &TrackGripElements,
    transmission: &TankTransmission,
    sim: &TankSim,
) -> u64 {
    canonical_tank_state_digest(
        position,
        rotation,
        linvel,
        angvel,
        drive,
        grip,
        elements,
        transmission,
        sim,
    )
    .elements
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tank::WeaponState;
    use crate::track::transmission::SchedulerState;

    /// A representative, non-trivial sim state (every `Vec` populated, one side anchored and
    /// one released, non-default drive) so the canonicalization is exercised over every
    /// production field path — including every carried-state sub-hash stream.
    fn sample() -> (Vec3, Quat, Vec3, Vec3, TrackDrive, TankSim) {
        // Callers bind a default TrackGrip separately (`let grip = ...`) — zero grip keeps
        // the legacy expectations intact; the grip-ULP test flips it explicitly.

        let position = Vec3::new(1.5, 2.0, -70.25);
        let rotation = Quat::from_rotation_y(0.3);
        let linvel = Vec3::new(0.0, -0.153, 4.2);
        let angvel = Vec3::new(0.01, -0.02, 0.03);
        let drive = TrackDrive {
            throttle: 0.75,
            steer: -0.25,
            sides: [
                crate::track::sim::TrackDriveSide {
                    speed: 4.2,
                    phase: 137.25,
                },
                crate::track::sim::TrackDriveSide {
                    speed: 4.1,
                    phase: 136.9,
                },
            ],
        };
        let sim = TankSim {
            servos: vec![crate::tank::ServoState::test_new(0.4, 0.39, 0.64)],
            weapons: vec![WeaponState {
                reload_remaining: 1.25,
                recoil_offset: -0.4,
                recoil_velocity: 0.0,
                // Belt counter — cosmetic, deliberately NOT folded into the state hash below (see
                // `WeaponState::rounds_fired`); carried here only so the literal is complete.
                rounds_fired: 3,
                // Rounds left on the belt — fire-gating, hashed (in the `rld` stream), unlike the
                // cosmetic counter above.
                belt_remaining: 47,
            }],
        };
        (position, rotation, linvel, angvel, drive, sim)
    }

    fn sample_transmission() -> TankTransmission {
        TankTransmission(crate::track::transmission::TransmissionState {
            gear: 5,
            shift_ticks: 3,
            steer_step: 2,
            reverse: true,
            park: true,
            last_shift_dir: -1,
            dwell_ticks: 7,
            omega_e: 250.0,
            clutch_out: true,
            demand_n: 42_000.0,
            demand_initialized: true,
            grade_confirm_ticks: 9,
            grade_target: 3,
            scheduler: SchedulerState::GradeShift { from: 5, to: 3 },
            hill_hold: true,
            hold_reengage_ticks: 11,
        })
    }

    /// Same state → same combined hash AND same sub-hashes. This is the join's core assumption: two
    /// worlds that reached an identical logical state must produce byte-identical hashes.
    #[test]
    fn identical_state_hashes_identically() {
        let (p, q, lv, av, drive, sim) = sample();
        let grip = TrackGrip::default();
        let transmission = sample_transmission();
        let a = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &sim);
        let b = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &sim);
        // MEASURED before the extraction: every stream remains byte-identical across the refactor.
        assert_eq!(
            [
                a.combined, a.pos, a.rot, a.lv, a.av, a.sim, a.drv, a.srv, a.rld, a.rec, a.blt,
                a.trn, a.elm,
            ],
            [
                13_073_156_975_648_890_420,
                5_276_285_167_157_175_194,
                5_407_327_877_548_523_030,
                8_825_002_124_784_658_797,
                15_886_300_944_198_297_253,
                11_243_118_599_694_738_606,
                3_269_583_271_824_065_410,
                14_071_911_453_643_095_408,
                222_436_822_907_033_607,
                12_037_784_973_900_930_602,
                16_317_528_332_690_472_771,
                4_854_495_176_564_399_426,
                12_638_153_115_695_167_455,
            ]
        );
        assert_eq!(a.combined, b.combined);
        assert_eq!(
            (a.pos, a.rot, a.lv, a.av, a.sim),
            (b.pos, b.rot, b.lv, b.av, b.sim)
        );
    }

    /// A SINGLE flipped bit of angular velocity changes the combined hash and the `av` sub-hash, and
    /// leaves every other sub-hash untouched — the property that lets the analyzer localize a
    /// divergence to one component. The flip is one ULP: `av.z`'s least-significant mantissa bit.
    #[test]
    fn one_flipped_velocity_bit_diverges_only_that_component() {
        let (p, q, lv, av, drive, sim) = sample();
        let grip = TrackGrip::default();
        let transmission = sample_transmission();
        let base = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &sim);

        let mut av2 = av;
        av2.z = f32::from_bits(av.z.to_bits() ^ 1);
        assert_ne!(
            av2.z.to_bits(),
            av.z.to_bits(),
            "the bit flip must change the bits"
        );
        let flipped = hash_tank_state(p, q, lv, av2, &drive, &grip, &transmission, &sim);

        assert_ne!(
            base.combined, flipped.combined,
            "combined hash must catch the flip"
        );
        assert_ne!(base.av, flipped.av, "the av sub-hash must catch the flip");
        // Every OTHER component is unchanged — the localization guarantee.
        assert_eq!(base.pos, flipped.pos);
        assert_eq!(base.rot, flipped.rot);
        assert_eq!(base.lv, flipped.lv);
        assert_eq!(base.sim, flipped.sim);
    }

    /// `+0.0` and `−0.0` are the same number but different bits, and bit-exactness is the bar, so they
    /// must hash apart (a sign-flip through zero is a real last-bit divergence).
    #[test]
    fn signed_zero_hashes_apart() {
        let (p, q, _lv, av, drive, sim) = sample();
        let grip = TrackGrip::default();
        let transmission = sample_transmission();
        let pos_zero = hash_tank_state(
            p,
            q,
            Vec3::new(0.0, 0.0, 0.0),
            av,
            &drive,
            &grip,
            &transmission,
            &sim,
        );
        let neg_zero = hash_tank_state(
            p,
            q,
            Vec3::new(-0.0, 0.0, 0.0),
            av,
            &drive,
            &grip,
            &transmission,
            &sim,
        );
        assert_ne!(pos_zero.lv, neg_zero.lv);
    }

    /// A one-ULP belt-phase difference must hash apart and localize to the `blt` stream —
    /// phase is force-station-advecting sim state, not cosmetic.
    #[test]
    fn belt_phase_ulp_localizes_to_belt_stream() {
        let (p, q, lv, av, drive, sim) = sample();
        let grip = TrackGrip::default();
        let transmission = sample_transmission();
        let mut shifted = drive;
        shifted.sides[1].phase = f64::from_bits(drive.sides[1].phase.to_bits() ^ 1);
        let hn = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &sim);
        let hs = hash_tank_state(p, q, lv, av, &shifted, &grip, &transmission, &sim);
        assert_ne!(hn.sim, hs.sim);
        assert_ne!(hn.blt, hs.blt);
        // The other carried-state streams are untouched by a belt flip.
        assert_eq!(hn.drv, hs.drv);
        assert_eq!(hn.srv, hs.srv);
        assert_eq!(hn.rld, hs.rld);
        assert_eq!(hn.rec, hs.rec);
        assert_eq!(hn.trn, hs.trn);
    }

    /// Each carried-state field family flips ITS sub-hash (plus `sim` and `combined`) and no other —
    /// the per-field decode the window attribution relies on. One-ULP flips, same bar as the
    /// velocity-bit test.
    #[test]
    fn carried_state_flip_localizes_to_its_family() {
        let (p, q, lv, av, drive, sim) = sample();
        let grip = TrackGrip::default();
        let transmission = sample_transmission();
        let base = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &sim);

        // Drive: steer one ULP off.
        let mut drive2 = drive;
        drive2.steer = f32::from_bits(drive.steer.to_bits() ^ 1);
        let d = hash_tank_state(p, q, lv, av, &drive2, &grip, &transmission, &sim);
        assert_ne!(base.drv, d.drv);
        assert_ne!(base.sim, d.sim);
        assert_ne!(base.combined, d.combined);
        assert_eq!(
            (base.srv, base.rld, base.rec, base.blt, base.trn),
            (d.srv, d.rld, d.rec, d.blt, d.trn)
        );
        assert_eq!(
            (base.pos, base.rot, base.lv, base.av),
            (d.pos, d.rot, d.lv, d.av)
        );

        // Servo: velocity one ULP off.
        let [cur, prev, vel] = sim.servos[0].hash_fields();
        let mut sim2 = sim.clone();
        sim2.servos[0] =
            crate::tank::ServoState::test_new(cur, prev, f32::from_bits(vel.to_bits() ^ 1));
        let s = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &sim2);
        assert_ne!(base.srv, s.srv);
        assert_ne!(base.sim, s.sim);
        assert_eq!(
            (base.drv, base.rld, base.rec, base.blt, base.trn),
            (s.drv, s.rld, s.rec, s.blt, s.trn)
        );

        // Reload: timer one ULP off — must NOT touch the recoil stream despite sharing the weapon.
        let mut sim3 = sim.clone();
        sim3.weapons[0].reload_remaining =
            f32::from_bits(sim.weapons[0].reload_remaining.to_bits() ^ 1);
        let r = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &sim3);
        assert_ne!(base.rld, r.rld);
        assert_ne!(base.sim, r.sim);
        assert_eq!(
            (base.drv, base.srv, base.rec, base.blt, base.trn),
            (r.drv, r.srv, r.rec, r.blt, r.trn)
        );

        // Belt count: one round off — a fire-gating difference, so it must land in the reload
        // stream (`rld`, the fire-timer family it modulates) and nowhere else. (`rounds_fired`
        // has no such case: it is cosmetic and deliberately unhashed.)
        let mut simb = sim.clone();
        simb.weapons[0].belt_remaining = sim.weapons[0].belt_remaining.wrapping_add(1);
        let b = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &simb);
        assert_ne!(base.rld, b.rld);
        assert_ne!(base.sim, b.sim);
        assert_ne!(base.combined, b.combined);
        assert_eq!(
            (base.drv, base.srv, base.rec, base.blt, base.trn),
            (b.drv, b.srv, b.rec, b.blt, b.trn)
        );

        // Recoil: offset one ULP off — must NOT touch the reload stream.
        let mut sim4 = sim.clone();
        sim4.weapons[0].recoil_offset = f32::from_bits(sim.weapons[0].recoil_offset.to_bits() ^ 1);
        let c = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &sim4);
        assert_ne!(base.rec, c.rec);
        assert_ne!(base.sim, c.sim);
        assert_eq!(
            (base.drv, base.srv, base.rld, base.blt, base.trn),
            (c.drv, c.srv, c.rld, c.blt, c.trn)
        );

        // Belt state: the left side's speed one ULP off — localizes to the `blt` stream.
        let mut drive5 = drive;
        drive5.sides[0].speed = f32::from_bits(drive.sides[0].speed.to_bits() ^ 1);
        let a = hash_tank_state(p, q, lv, av, &drive5, &grip, &transmission, &sim);
        assert_ne!(base.blt, a.blt);
        assert_ne!(base.sim, a.sim);
        assert_eq!(
            (base.drv, base.srv, base.rld, base.rec, base.trn),
            (a.drv, a.srv, a.rld, a.rec, a.trn)
        );

        // Transmission: one crank bit off — localizes to the atomic `trn` stream.
        let mut transmission2 = transmission;
        transmission2.0.omega_e = f32::from_bits(transmission.0.omega_e.to_bits() ^ 1);
        let t = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission2, &sim);
        assert_ne!(base.trn, t.trn);
        assert_ne!(base.sim, t.sim);
        assert_ne!(base.combined, t.combined);
        assert_eq!(
            (base.drv, base.srv, base.rld, base.rec, base.blt),
            (t.drv, t.srv, t.rld, t.rec, t.blt)
        );
    }

    /// Every field in the authoritative 16-field inventory affects the exact transmission stream.
    #[test]
    fn transmission_inventory_fields_all_affect_hash() {
        let (p, q, lv, av, drive, sim) = sample();
        let grip = TrackGrip::default();
        let base = sample_transmission();
        let base_hash = hash_tank_state(p, q, lv, av, &drive, &grip, &base, &sim).trn;
        let mut variants = [base; 16];
        variants[0].0.gear = variants[0].0.gear.wrapping_add(1);
        variants[1].0.shift_ticks = variants[1].0.shift_ticks.wrapping_add(1);
        variants[2].0.steer_step = variants[2].0.steer_step.wrapping_add(1);
        variants[3].0.reverse = !variants[3].0.reverse;
        variants[4].0.park = !variants[4].0.park;
        variants[5].0.last_shift_dir = 1;
        variants[6].0.dwell_ticks = variants[6].0.dwell_ticks.wrapping_add(1);
        variants[7].0.omega_e = f32::from_bits(variants[7].0.omega_e.to_bits() ^ 1);
        variants[8].0.clutch_out = !variants[8].0.clutch_out;
        variants[9].0.demand_n = f32::from_bits(variants[9].0.demand_n.to_bits() ^ 1);
        variants[10].0.demand_initialized = !variants[10].0.demand_initialized;
        variants[11].0.grade_confirm_ticks = variants[11].0.grade_confirm_ticks.wrapping_add(1);
        variants[12].0.grade_target = variants[12].0.grade_target.wrapping_add(1);
        variants[13].0.scheduler = SchedulerState::HillHold;
        variants[14].0.hill_hold = !variants[14].0.hill_hold;
        variants[15].0.hold_reengage_ticks = variants[15].0.hold_reengage_ticks.wrapping_add(1);

        for (index, variant) in variants.iter().enumerate() {
            let hash = hash_tank_state(p, q, lv, av, &drive, &grip, variant, &sim).trn;
            assert_ne!(
                base_hash, hash,
                "transmission inventory field {index} was not hashed"
            );
        }
    }

    /// Element strain and force-affecting contact lifetime are exact state, ordered by side then
    /// flat material `link * 3 + column`, and localize to their own carried-state stream.
    #[test]
    fn element_field_bits_localize_to_element_stream() {
        let (p, q, lv, av, drive, sim) = sample();
        let grip = TrackGrip::default();
        let transmission = sample_transmission();
        let elements = TrackGripElements::for_links(2);
        let base = hash_tank_state_with_elements(
            p,
            q,
            lv,
            av,
            &drive,
            &grip,
            Some(&elements),
            &transmission,
            &sim,
        );

        let mut strain = elements.clone();
        strain.sides[1].strain[4].z = f32::from_bits(1);
        let strain_hash = hash_tank_state_with_elements(
            p,
            q,
            lv,
            av,
            &drive,
            &grip,
            Some(&strain),
            &transmission,
            &sim,
        );
        assert_ne!(base.elm, strain_hash.elm);
        assert_ne!(base.sim, strain_hash.sim);
        assert_eq!(
            (base.drv, base.srv, base.rld, base.rec, base.blt, base.trn),
            (
                strain_hash.drv,
                strain_hash.srv,
                strain_hash.rld,
                strain_hash.rec,
                strain_hash.blt,
                strain_hash.trn,
            )
        );

        let mut dwell = elements.clone();
        dwell.sides[0].dwell[0] = 1;
        let dwell_hash = hash_tank_state_with_elements(
            p,
            q,
            lv,
            av,
            &drive,
            &grip,
            Some(&dwell),
            &transmission,
            &sim,
        );
        assert_ne!(base.elm, dwell_hash.elm);
    }

    /// `rounds_fired` rolls back because it derives tracer cadence, but a dropped predicted round
    /// may leave that phase one round from authority without changing simulation truth. The
    /// production trace therefore excludes it; the same-platform fresh-App digest must not.
    #[test]
    fn fresh_app_digest_covers_the_rollback_tracer_phase() {
        let (p, q, lv, av, drive, sim) = sample();
        let grip = TrackGrip::default();
        let elements = TrackGripElements::for_links(2);
        let transmission = sample_transmission();
        let base_trace = hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &sim);
        let base = canonical_tank_state_digest(
            p,
            q,
            lv,
            av,
            &drive,
            &grip,
            &elements,
            &transmission,
            &sim,
        );
        // MEASURED before the extraction, including the disclosed element and rollback streams.
        assert_eq!(
            base,
            CanonicalTankStateDigest {
                simulation: 13_429_289_563_660_402_279,
                rollback: 12_321_758_811_062_536_473,
                position: 5_276_285_167_157_175_194,
                rotation: 5_407_327_877_548_523_030,
                linear_velocity: 8_825_002_124_784_658_797,
                angular_velocity: 15_886_300_944_198_297_253,
                drive: 3_269_583_271_824_065_410,
                servo: 14_071_911_453_643_095_408,
                reload: 222_436_822_907_033_607,
                recoil: 12_037_784_973_900_930_602,
                belts: 16_317_528_332_690_472_771,
                transmission: 4_854_495_176_564_399_426,
                elements: 6_815_987_066_798_852_301,
                rounds_fired: 17_086_694_953_553_481_862,
            }
        );

        let mut phase_shifted = sim.clone();
        phase_shifted.weapons[0].rounds_fired =
            phase_shifted.weapons[0].rounds_fired.wrapping_add(1);
        let shifted_trace =
            hash_tank_state(p, q, lv, av, &drive, &grip, &transmission, &phase_shifted);
        let shifted = canonical_tank_state_digest(
            p,
            q,
            lv,
            av,
            &drive,
            &grip,
            &elements,
            &transmission,
            &phase_shifted,
        );

        assert_eq!(base_trace.combined, shifted_trace.combined);
        assert_eq!(base.simulation, shifted.simulation);
        assert_ne!(base.rounds_fired, shifted.rounds_fired);
        assert_ne!(base.rollback, shifted.rollback);
        assert_ne!(base, shifted);
    }
}
