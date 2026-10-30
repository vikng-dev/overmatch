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

/// The rotation rides 6 bytes through the registered rule functions, not the 16 of postcard's
/// four f32s.
#[test]
fn the_rule_fns_send_the_quantised_bytes() {
    let rotation = Rotation(Quat::from_euler(EulerRot::YXZ, 1.0, -0.3, 0.2));
    let (len, back) = through_rule_fns(rotation_fns(), rotation);
    assert_eq!(len, 6);
    assert!(angle_between(rotation.0, back.0) <= rotation_bound_rad());
}

/// A message cut short inside a component is refused, never read past: a reader that ran off the
/// end would take the next component's bytes for its own.
#[test]
fn a_truncated_message_is_refused() {
    let mut message = Vec::new();
    write_rotation(Quat::IDENTITY, &mut message);
    message.pop();
    assert!(read_rotation(&mut Bytes::from(message)).is_err());
}
