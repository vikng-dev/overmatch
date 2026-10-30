//! Each codec's stated error bound, held over the extremes of its range and a dense sample of
//! the inside, and its byte count through replicon's own rule-function path. A codec whose step,
//! range or layout drifts from its doc fails here before a client misreads a tank.

use bevy::state::app::StatesPlugin;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::prelude::{RepliconPlugins, RepliconTick, RuleFns};
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use bevy_replicon::shared::replication::registry::receive_fns::MutWrite;
use bevy_replicon::shared::replication::registry::test_fns::TestFnsEntityExt;

use super::*;

/// Appended after every encoding: a reader that consumes more or fewer bytes than its writer
/// appended leaves something other than exactly this behind.
const SENTINEL: u8 = 0xA5;

/// Write with `write`, read back with `read`, and prove the reader consumed exactly the writer's
/// bytes. Returns the byte count and the decoded value.
fn round_trip<T, U>(
    value: T,
    write: impl Fn(T, &mut Vec<u8>),
    read: impl Fn(&mut Bytes) -> Result<U>,
) -> (usize, U) {
    let mut message = Vec::new();
    write(value, &mut message);
    let len = message.len();
    message.push(SENTINEL);
    let mut bytes = Bytes::from(message);
    let decoded = read(&mut bytes).expect("a codec reads what it wrote");
    assert_eq!(
        &bytes[..],
        &[SENTINEL],
        "the reader consumed exactly the writer's bytes"
    );
    (len, decoded)
}

/// The component's byte count and its decoded value through replicon's registered rule functions
/// — the path a real mutation message takes.
fn through_rule_fns<C: Component<Mutability: MutWrite<C>> + Clone>(
    fns: RuleFns<C>,
    value: C,
) -> (usize, C) {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin, RepliconPlugins));
    let (_, fns_id) =
        app.world_mut()
            .resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                registry.register_rule_fns(world, fns)
            });
    let tick = RepliconTick::new(1);
    let mut entity = app.world_mut().spawn(value);
    let data = entity.serialize(fns_id, tick);
    entity.remove::<C>();
    entity.apply_write(data.clone(), fns_id, tick);
    let decoded = entity
        .get::<C>()
        .expect("the write inserts the component")
        .clone();
    (data.len(), decoded)
}

/// A reproducible stream in `[−1, 1)` (xorshift64*), so a failure names the same sample every run.
struct Samples(u64);

impl Samples {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let bits = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40;
        bits as f32 / (1u64 << 23) as f32 - 1.0
    }
}

/// The angle (rad) of the rotation between unit quaternions `a` and `b`, in f64 and stable at
/// small angles: `4·asin(|a − b|/2)` with `b` taken into `a`'s hemisphere.
fn angle_between(a: Quat, b: Quat) -> f64 {
    let (a, b) = (a.to_array().map(f64::from), b.to_array().map(f64::from));
    let dot: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let s = if dot < 0.0 { -1.0 } else { 1.0 };
    let chord = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (x - s * y).powi(2))
        .sum::<f64>()
        .sqrt();
    4.0 * (chord / 2.0).asin()
}

/// [`write_rotation`]'s DERIVED bound: `2·√12·δ`, `δ = √2/(2·(2¹⁵−1))`.
fn rotation_bound_rad() -> f64 {
    2.0 * 12f64.sqrt() * std::f64::consts::SQRT_2 / (2.0 * 32_767.0)
}

/// The extremes of a rotation's range — the four unit-axis quaternions and their negatives (the
/// identity and the three half-turns: one component carries everything), every pair of
/// components tied at `±1/√2` (two equal largest, the others' range edge), and every sign of
/// `(½, ½, ½, ½)` (the derivation's worst case: the recovered component is smallest there) — then
/// a dense sample of the inside, and a cloud around the worst case.
fn rotation_samples() -> Vec<Quat> {
    let mut out = Vec::new();
    for axis in 0..4 {
        for sign in [1.0, -1.0] {
            let mut c = [0.0; 4];
            c[axis] = sign;
            out.push(Quat::from_array(c));
        }
    }
    let h = std::f32::consts::FRAC_1_SQRT_2;
    for a in 0..4 {
        for b in (a + 1)..4 {
            for (sa, sb) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                let mut c = [0.0; 4];
                c[a] = sa * h;
                c[b] = sb * h;
                out.push(Quat::from_array(c));
            }
        }
    }
    for signs in 0..16u32 {
        let c = std::array::from_fn(|i| if signs >> i & 1 == 1 { -0.5 } else { 0.5 });
        out.push(Quat::from_array(c));
    }
    let mut samples = Samples(0x9E37_79B9_7F4A_7C15);
    while out.len() < 200_000 {
        let c = [
            samples.next(),
            samples.next(),
            samples.next(),
            samples.next(),
        ];
        let q = Quat::from_array(c);
        if q.length_squared() > 1e-3 && q.length_squared() <= 1.0 {
            out.push(q.normalize());
        }
    }
    for _ in 0..20_000 {
        let c = std::array::from_fn(|_| 0.5 + 1e-3 * samples.next());
        out.push(Quat::from_array(c).normalize());
    }
    out
}

/// Every rotation comes back within the bound its doc derives and in the authority's hemisphere
/// (the carried sign): the interpolator slerps between consecutive decoded samples, and the
/// track view's snap test and every pose reader take them as the hull's orientation.
#[test]
fn a_rotation_round_trips_within_its_bound_in_its_hemisphere() {
    let bound = rotation_bound_rad();
    let mut worst = 0.0f64;
    for q in rotation_samples() {
        let (len, back) = round_trip(q, write_rotation, read_rotation);
        assert_eq!(len, 6, "{q:?}");
        assert!(
            q.dot(back) > 0.0,
            "{q:?} decoded into the other hemisphere: {back:?}"
        );
        let angle = angle_between(q.normalize(), back);
        assert!(
            angle <= bound,
            "{q:?}: {angle:e} rad over the {bound:e} rad bound"
        );
        worst = worst.max(angle);
    }
    // The bound is a worst case, not a typical error: say how close the samples came.
    println!("rotation: worst {worst:e} rad of the {bound:e} rad bound");
}

/// Position: [`write_position`]'s half step per axis, 2⁻¹¹ m.
const POSITION_HALF_STEP_M: f32 = 1.0 / 2048.0;

/// The position's range edges in both directions on every axis (the last packed code, the first
/// value past it, a value that rounds onto the range's end, far outside), the origin and negative
/// zero, a non-finite axis, then a dense sample over ±1100 m — both forms.
fn position_samples() -> Vec<Vec3> {
    let last = 1024.0 - 1.0 / 1024.0;
    let mut out = vec![Vec3::ZERO, Vec3::splat(-0.0)];
    for edge in [last, 1024.0, 1024.0 - 1.0 / 4096.0, 2048.0, 1.0e7] {
        for sign in [1.0, -1.0] {
            for axis in 0..3 {
                let mut v = Vec3::new(123.456, -36.5, 700.25);
                v[axis] = sign * edge;
                out.push(v);
            }
        }
    }
    out.push(Vec3::new(f32::NAN, 0.0, 0.0));
    out.push(Vec3::new(0.0, f32::INFINITY, 0.0));
    let mut samples = Samples(0xD1B5_4A32_D192_ED03);
    for _ in 0..200_000 {
        out.push(Vec3::new(samples.next(), samples.next(), samples.next()) * 1100.0);
    }
    out
}

/// Inside ±1024 m on every axis a position rides 8 bytes within half a step per axis; outside it,
/// or not finite, it rides 13 bytes bit-exact. The interpolator lerps decoded samples and the
/// extrapolator projects from the newest, so this half step is the whole of the codec's share of
/// a presented hull's position error.
#[test]
fn a_position_round_trips_within_half_a_step_or_exactly() {
    let half = (1i32 << 20) as f32;
    let mut packed = 0;
    for v in position_samples() {
        let (len, back) = round_trip(v, write_position, read_position);
        let inside = v.to_array().iter().all(|axis| {
            let steps = (axis * 1024.0).round();
            steps >= -half && steps < half
        });
        if inside {
            packed += 1;
            assert_eq!(len, 8, "{v:?}");
            for axis in 0..3 {
                assert!(
                    (back[axis] - v[axis]).abs() <= POSITION_HALF_STEP_M,
                    "{v:?} -> {back:?}"
                );
            }
        } else {
            assert_eq!(len, 13, "{v:?}");
            assert_eq!(
                back.to_array().map(f32::to_bits),
                v.to_array().map(f32::to_bits),
                "the full form is exact"
            );
        }
    }
    assert!(packed > 150_000, "the sample exercises the packed form");
}

/// Each velocity codec at its steps: inside its i16 range, half a step per axis; past either end,
/// the end itself (clamped, sign kept); NaN, zero. The extrapolator projects at these velocities
/// through a gap, so the half step times its horizon is the codec's share of a projected pose.
#[test]
fn a_velocity_round_trips_within_half_a_step_or_clamps() {
    for steps in [LINEAR_VELOCITY_STEPS, ANGULAR_VELOCITY_STEPS] {
        let top = f32::from(i16::MAX) / steps;
        let bottom = f32::from(i16::MIN) / steps;
        let write = |v: Vec3, message: &mut Vec<u8>| write_i16x3(v, steps, message);
        let read = |message: &mut Bytes| read_i16x3(message, steps);
        for (v, expected) in [
            (Vec3::new(top, bottom, 0.0), Vec3::new(top, bottom, 0.0)),
            (Vec3::new(1.0e6, -1.0e6, 0.0), Vec3::new(top, bottom, 0.0)),
            (
                Vec3::new(-top * 1.5, top * 1.5, 0.0),
                Vec3::new(bottom, top, 0.0),
            ),
            (Vec3::new(f32::NAN, 0.0, -0.0), Vec3::ZERO),
        ] {
            let (len, back) = round_trip(v, write, read);
            assert_eq!(len, 6);
            assert_eq!(back, expected, "{v:?} at {steps} steps");
        }
        let mut samples = Samples(0x94D0_49BB_1331_11EB);
        for _ in 0..100_000 {
            let v = Vec3::new(samples.next(), samples.next(), samples.next()) * top;
            let (len, back) = round_trip(v, write, read);
            assert_eq!(len, 6);
            for axis in 0..3 {
                assert!(
                    (back[axis] - v[axis]).abs() <= 0.5 / steps,
                    "{v:?} -> {back:?} at {steps} steps"
                );
            }
        }
    }
}

/// The belt's half steps: speed 2⁻¹¹ m/s, phase 2⁻¹⁵ m.
const BELT_SPEED_HALF_STEP: f32 = 0.5 / BELT_SPEED_STEPS;
const BELT_PHASE_HALF_STEP_M: f64 = 1.0 / 32_768.0;

fn belt(left: (f32, f64), right: (f32, f64)) -> TrackDrive {
    let side = |(speed, phase)| TrackDriveSide { speed, phase };
    TrackDrive {
        sides: [side(left), side(right)],
    }
}

/// The belt state at its range edges and inside: each speed within half a step inside ±32 m/s and
/// clamped to the bound past it (NaN 0); each phase, unbounded and of either sign, decoded into
/// `[0, BELT_PHASE_WRAP_M)` within half a step of it modulo the wrap (non-finite 0); 10 bytes. The
/// track view draws a phase carried at this speed and eased onto this phase, both modulo the wrap.
#[test]
fn a_belt_round_trips_within_half_a_step_modulo_its_wrap() {
    let top = f32::from(i16::MAX) / BELT_SPEED_STEPS;
    let bottom = f32::from(i16::MIN) / BELT_SPEED_STEPS;
    for (drive, expected_speeds) in [
        (belt((top, 0.0), (bottom, -0.0)), [top, bottom]),
        (
            belt((1.0e4, BELT_PHASE_WRAP_M), (-1.0e4, -BELT_PHASE_WRAP_M)),
            [top, bottom],
        ),
        (belt((f32::NAN, f64::NAN), (0.0, f64::INFINITY)), [0.0, 0.0]),
    ] {
        let (len, back) = round_trip(drive, |d, m| write_track_drive(&d, m), read_track_drive);
        assert_eq!(len, 10);
        assert_eq!(back.sides.map(|side| side.speed), expected_speeds);
        assert_eq!(back.sides.map(|side| side.phase), [0.0, 0.0], "{drive:?}");
    }
    let mut samples = Samples(0xBF58_476D_1CE4_E5B9);
    for _ in 0..100_000 {
        let speed = samples.next() * top;
        // Up to ±50 km of travel: a long match's belt, far past one wrap, either direction.
        let phase = f64::from(samples.next()) * 5.0e4;
        let drive = belt((speed, phase), (-speed, phase + 0.5));
        let (len, back) = round_trip(drive, |d, m| write_track_drive(&d, m), read_track_drive);
        assert_eq!(len, 10);
        for (sent, got) in drive.sides.iter().zip(back.sides) {
            assert!((got.speed - sent.speed).abs() <= BELT_SPEED_HALF_STEP);
            assert!((0.0..BELT_PHASE_WRAP_M).contains(&got.phase), "{got:?}");
            assert!(
                crate::track::sim::wrapped_phase_delta(sent.phase, got.phase).abs()
                    <= BELT_PHASE_HALF_STEP_M,
                "{sent:?} -> {got:?}"
            );
        }
    }
}

/// Every codec rides its quantised byte count through the registered rule functions — the path a
/// mutation message takes — not postcard's 12 (a vector) or 16 (a quaternion).
#[test]
fn the_rule_fns_send_the_quantised_bytes() {
    let rotation = Rotation(Quat::from_euler(EulerRot::YXZ, 1.0, -0.3, 0.2));
    let (len, back) = through_rule_fns(rotation_fns(), rotation);
    assert_eq!(len, 6);
    assert!(angle_between(rotation.0, back.0) <= rotation_bound_rad());

    let position = Position(Vec3::new(-412.3, 36.7, 655.1));
    let (len, back) = through_rule_fns(position_fns(), position);
    assert_eq!(len, 8);
    assert!(back.0.abs_diff_eq(position.0, POSITION_HALF_STEP_M));

    let linear = LinearVelocity(Vec3::new(9.3, -0.4, -12.7));
    let (len, back) = through_rule_fns(linear_velocity_fns(), linear);
    assert_eq!(len, 6);
    assert!(back.0.abs_diff_eq(linear.0, 0.5 / LINEAR_VELOCITY_STEPS));

    let angular = AngularVelocity(Vec3::new(0.02, -1.1, 0.3));
    let (len, back) = through_rule_fns(angular_velocity_fns(), angular);
    assert_eq!(len, 6);
    assert!(back.0.abs_diff_eq(angular.0, 0.5 / ANGULAR_VELOCITY_STEPS));

    // The belt state: 10 bytes, not postcard's 32 for two f32 speeds and two f64 phases.
    let drive = belt((4.25, 1500.5), (-3.0, -20.125));
    let (len, back) = through_rule_fns(track_drive_fns(), drive);
    assert_eq!(len, 10);
    assert_eq!(
        back,
        belt(
            (4.25, 1500.5 - BELT_PHASE_WRAP_M),
            (-3.0, BELT_PHASE_WRAP_M - 20.125)
        )
    );
}

/// A message cut short inside a component is refused, never read past: a reader that ran off the
/// end would take the next component's bytes for its own.
#[test]
fn a_truncated_message_is_refused() {
    let short = |write: &dyn Fn(&mut Vec<u8>)| {
        let mut message = Vec::new();
        write(&mut message);
        message.pop();
        Bytes::from(message)
    };
    assert!(read_rotation(&mut short(&|m| write_rotation(Quat::IDENTITY, m))).is_err());
    assert!(read_position(&mut short(&|m| write_position(Vec3::ONE, m))).is_err());
    assert!(read_position(&mut short(&|m| write_position(Vec3::splat(5000.0), m))).is_err());
    assert!(read_i16x3(&mut short(&|m| write_i16x3(Vec3::ONE, 256.0, m)), 256.0).is_err());
    let drive = belt((1.0, 2.0), (3.0, 4.0));
    assert!(read_track_drive(&mut short(&|m| write_track_drive(&drive, m))).is_err());
}
