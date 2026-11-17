//! Frame start feeds the tick-to-time map; departure feeds the network absence bound.
//! Tick identity rides Replicon's checkpoint userdata on the globally visible scoreboard.

use std::collections::VecDeque;

use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy_replicon::prelude::{Remote, RepliconTick, ServerSystems};
use serde::Serialize;

mod wire;
pub(super) use wire::replication;

/// Checked nonnegative, finite seconds on the authority's monotonic Real epoch.
/// DERIVED payload: f64 frame start + f32 departure offset = twelve fixed postcard bytes.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize)]
pub(super) struct TickStamp {
    frame_start_s: f64,
    departure_offset_s: f32,
}

impl TickStamp {
    pub(super) fn frame_start_s(self) -> f64 {
        self.frame_start_s
    }
    pub(super) fn absence_bound_s(self, previous_frame_s: f64) -> f64 {
        previous_frame_s - self.departure_s()
    }

    pub(super) fn departure_s(self) -> f64 {
        self.frame_start_s + f64::from(self.departure_offset_s)
    }
}

// Resolution: Replicon's mutate completion tracker retains u64::BITS checkpoints. Bound decoded
// stamps even in protocol-only apps with no view consumer; overflow is reported when consumed.
pub(super) const RECEIPT_CAP: usize = u64::BITS as usize;

#[derive(Default)]
pub(super) struct ReceivedStamps {
    pub queue: VecDeque<(RepliconTick, TickStamp)>,
    pub overflow: u64,
}

impl ReceivedStamps {
    fn push(&mut self, tick: RepliconTick, stamp: TickStamp) {
        if self.queue.len() == RECEIPT_CAP {
            self.queue.pop_front();
            self.overflow += 1;
        }
        self.queue.push_back((tick, stamp));
    }
}

/// The runner's clock seam also serves departure reads, including in injected-clock proofs.
#[derive(Resource)]
pub(super) struct DepartureClock(Box<dyn Fn() -> Instant + Send + Sync>);

impl DepartureClock {
    pub(super) fn new(now: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Self(Box::new(now))
    }
}

impl Default for DepartureClock {
    fn default() -> Self {
        Self::new(Instant::now)
    }
}

pub(super) fn install_server(app: &mut App) {
    app.init_resource::<DepartureClock>()
        .add_systems(PostUpdate, publish.before(ServerSystems::Send));
}

fn publish(
    real: Res<Time<Real>>,
    clock: Res<DepartureClock>,
    mut stamps: Query<&mut TickStamp, Without<Remote>>,
) {
    let frame_start = real
        .last_update()
        .expect("TimePlugin samples Real before PostUpdate");
    // Resolution: f32 seconds adds four fixed bytes without capping long frames. Subsecond work
    // has at most half an f32 ULP of rounding; unlike integer microseconds it cannot overflow Duration.
    let offset = (clock.0)().duration_since(frame_start).as_secs_f64() as f32;
    let departure = TickStamp::new(real.elapsed_secs_f64(), offset)
        .expect("the monotonic frame clock produces finite nonnegative stamps");
    for mut stamp in &mut stamps {
        *stamp = departure;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The future view clock needs every decoded tick, even when the live component is overwritten
    /// twice in a receive. Reading Changed<TickStamp> after receive loses the first record.
    #[test]
    fn the_codec_preserves_every_stamp_in_a_batched_receive() {
        use bevy_replicon::prelude::*;
        use bevy_replicon::shared::replication::registry::{
            ReplicationRegistry, test_fns::TestFnsEntityExt,
        };
        use bevy_replicon::shared::replication::storage::ReplicationStorage;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::state::app::StatesPlugin,
            RepliconPlugins,
        ));
        let (_, fns_id) =
            app.world_mut()
                .resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    registry.register_rule_fns(world, replication())
                });
        let mut entity = app.world_mut().spawn_empty();
        // Fixture: two ticks decoded before a scheduled consumer gets to read.
        for tick in [1, 2] {
            entity.apply_write(
                postcard::to_allocvec(&TickStamp::new(f64::from(tick), 0.0).unwrap()).unwrap(),
                fns_id,
                RepliconTick::new(tick),
            );
        }
        assert_eq!(entity.get::<TickStamp>().unwrap().frame_start_s(), 2.0);
        let received = &app
            .world()
            .resource::<ReplicationStorage>()
            .global
            .get::<ReceivedStamps>()
            .unwrap()
            .queue;
        assert_eq!(
            received,
            &std::collections::VecDeque::from([
                (RepliconTick::new(1), TickStamp::new(1.0, 0.0).unwrap()),
                (RepliconTick::new(2), TickStamp::new(2.0, 0.0).unwrap())
            ])
        );
    }

    /// The wire budget depends on the fixed f64 payload rather than epoch-dependent varints.
    #[test]
    fn stamp_payload_is_twelve_bytes_at_every_epoch() {
        // Fixture epochs: initial, an hour, and beyond a year.
        for epoch in [0.0, 3600.0, 40_000_000.0] {
            assert_eq!(
                postcard::to_allocvec(&TickStamp::new(epoch, 0.25).unwrap())
                    .unwrap()
                    .len(),
                size_of::<f64>() + size_of::<f32>()
            );
        }
    }

    /// A protocol-only client has no drain system; the decoder must still bound its memory.
    #[test]
    fn receipts_are_bounded_without_a_view_clock() {
        let mut received = ReceivedStamps::default();
        // Fixture: one more arrival than the upstream confirmation horizon can retain.
        for tick in 0..=RECEIPT_CAP as u32 {
            received.push(
                RepliconTick::new(tick),
                TickStamp::new(f64::from(tick), 0.0).unwrap(),
            );
        }
        assert_eq!(received.queue.len(), RECEIPT_CAP);
        assert_eq!(received.queue.front().unwrap().0, RepliconTick::new(1));
        assert_eq!(received.overflow, 1);
    }

    /// Invalid peer stamps must be refused during decoding, before reaching timing arithmetic.
    #[test]
    fn wire_admission_refuses_nonfinite_and_negative_stamp_fields() {
        // Fixture: tuples have the same postcard field layout as the stamp's serialized struct.
        for fields in [
            (f64::NAN, 0.0_f32),
            (-1.0, 0.0),
            (0.0, f32::INFINITY),
            (0.0, -1.0),
        ] {
            let bytes = postcard::to_allocvec(&fields).unwrap();
            assert!(postcard::from_bytes::<TickStamp>(&bytes).is_err());
        }
    }
}
