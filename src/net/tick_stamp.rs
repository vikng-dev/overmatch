//! Frame start feeds the tick-to-time map; departure feeds the server-work diagnostic.
//! The simulation tick rides each replication message's userdata; the stamp is a component
//! on the globally visible scoreboard.

use std::{collections::VecDeque, sync::Arc};

use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy_replicon::prelude::{
    ClientSystems, Remote, ReplicationStorage, RepliconTick, ServerState, ServerSystems,
};
use serde::Serialize;

mod wire;
pub(super) use wire::replication;

/// The decoder's local failure crosses into client lifecycle handling without exposing its codec.
#[derive(Debug, Clone)]
pub(super) struct StampRefusal(wire::StampRefusal);

impl std::fmt::Display for StampRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
impl StampRefusal {
    pub(super) fn invalid_fixture() -> Self {
        Self(wire::StampRefusal::Invalid(wire::InvalidStamp))
    }
}

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
        previous_frame_s - self.frame_start_s
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
#[derive(Resource, Clone)]
pub(super) struct DepartureClock(Arc<dyn Fn() -> Instant + Send + Sync>);

impl DepartureClock {
    pub(super) fn new(now: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }
}

impl Default for DepartureClock {
    fn default() -> Self {
        Self::new(Instant::now)
    }
}

struct SerializationClock {
    frame_start: Instant,
    clock: DepartureClock,
}

/// Shared protocol installation covers every server composition and every decoding client.
/// Publication runs only on a running server; clients only forward decoder refusals.
pub(super) fn install(app: &mut App) {
    app.init_resource::<DepartureClock>()
        .add_message::<super::matchmaking::ServerGone>()
        .add_systems(
            PostUpdate,
            publish
                .before(ServerSystems::Send)
                .run_if(in_state(ServerState::Running)),
        )
        .add_systems(PreUpdate, refuse_peer_stamp.after(ClientSystems::Receive));
}

pub(super) fn publish(
    real: Res<Time<Real>>,
    clock: Res<DepartureClock>,
    mut storage: ResMut<ReplicationStorage>,
    mut stamps: Query<&mut TickStamp, Without<Remote>>,
) {
    storage.global.insert(SerializationClock {
        frame_start: real
            .last_update()
            .expect("TimePlugin samples Real before PostUpdate"),
        clock: clock.clone(),
    });
    let frame = TickStamp::new(real.elapsed_secs_f64(), 0.0)
        .expect("the monotonic frame clock produces finite nonnegative stamps");
    for mut stamp in &mut stamps {
        *stamp = frame;
    }
}

fn refuse_peer_stamp(
    mut storage: ResMut<ReplicationStorage>,
    mut gone: MessageWriter<super::matchmaking::ServerGone>,
) {
    // A valid stream has no refusal. Replicon's decoder cannot write ECS messages directly.
    if let Some(refusal) = storage.global.remove::<wire::StampRefusal>() {
        warn!("client: refusing server tick stamp: {refusal}");
        gone.write(super::matchmaking::ServerGone::TickStamp(StampRefusal(
            refusal,
        )));
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

    /// The player relies on decoder refusals reaching the ordinary match-close path, including
    /// the lobby cause and no reconnect. The test adapter exposes errors the codec helper panics on.
    #[test]
    fn an_invalid_stamp_closes_the_match_with_its_typed_cause() {
        use super::super::matchmaking::MatchClosed;
        use bevy::ecs::message::MessageCursor;
        use bevy_replicon::shared::replication::registry::{
            ReplicationRegistry, test_fns::TestFnsEntityExt,
        };
        use lightyear::prelude::client::NetcodeClient;
        let _udp = super::super::test_harness::lock_real_udp_test();
        let (mut server, mut client) = super::super::client::match_exit::in_match();
        let (_, fns_id) =
            client
                .world_mut()
                .resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    registry.register_rule_fns(
                        world,
                        replication().with_in_place(|decode, ctx, _stamp, message| {
                            assert!(decode(ctx, message).is_err());
                            Ok(())
                        }),
                    )
                });
        // Fixture: individually finite but unrepresentable server epoch; the old component stays valid.
        let bytes = postcard::to_allocvec(&(1e30_f64, 0.0_f32)).unwrap();
        let carrier = client
            .world_mut()
            .spawn(TickStamp::new(0.0, 0.0).unwrap())
            .id();
        client
            .world_mut()
            .entity_mut(carrier)
            .apply_write(bytes, fns_id, RepliconTick::new(1));
        let refusal = client
            .world()
            .resource::<ReplicationStorage>()
            .global
            .get::<wire::StampRefusal>()
            .unwrap();
        assert!(matches!(refusal, wire::StampRefusal::Invalid(_)));
        let cause = refusal.to_string();
        let mut closed = MessageCursor::<MatchClosed>::default();
        let mut observed = false;
        // Fixture: allow deferred link removal and the lobby transition to settle.
        for _ in 0..8 {
            server.update();
            client.update();
            let causes: Vec<_> = closed
                .read(client.world().resource::<Messages<MatchClosed>>())
                .cloned()
                .collect();
            if !causes.is_empty() {
                assert_eq!(causes, [MatchClosed::Ended(cause.clone())]);
                observed = true;
                break;
            }
        }
        assert!(observed, "a refused stamp must close the match visibly");
        super::super::client::match_exit::advance_past_reconnect_window(&mut client);
        for _ in 0..8 {
            server.update();
            client.update();
        }
        assert_eq!(
            client
                .world_mut()
                .query_filtered::<(), With<NetcodeClient>>()
                .iter(client.world())
                .count(),
            0,
            "a refused link cannot reconnect"
        );
        assert_eq!(
            *client.world().resource::<State<crate::flow::Flow>>().get(),
            crate::flow::Flow::Lobby
        );
        assert!(
            client
                .world_mut()
                .query::<&Text>()
                .iter(client.world())
                .any(|text| text.0.contains(&cause)),
            "the lobby must show the refusal cause"
        );
    }

    /// The client's timing arithmetic must receive only valid stamps; malformed payloads leave
    /// the component and receipt queue untouched and retain the typed refusal for match-close.
    #[test]
    fn wire_admission_refuses_malformed_stamp_payloads() {
        use bevy_replicon::prelude::RepliconPlugins;
        use bevy_replicon::shared::replication::registry::{
            ReplicationRegistry, test_fns::TestFnsEntityExt,
        };
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::state::app::StatesPlugin,
            RepliconPlugins,
        ));
        let (_, fns_id) =
            app.world_mut()
                .resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    registry.register_rule_fns(
                        world,
                        // The adapter exposes decode errors without the test helper's panic.
                        replication().with_in_place(|decode, ctx, _stamp, message| {
                            assert!(decode(ctx, message).is_err());
                            Ok(())
                        }),
                    )
                });
        let initial = TickStamp::new(0.0, 0.0).unwrap();
        let carrier = app.world_mut().spawn(initial).id();
        // Fixture: tuples have the same postcard field layout as the stamp's serialized struct.
        for fields in [
            (f64::NAN, 0.0_f32),
            (f64::INFINITY, 0.0),
            (f64::NEG_INFINITY, 0.0),
            (-1.0, 0.0),
            (1e30, 0.0),
            (0.0, 1e30),
            (0.0, f32::NAN),
            (0.0, f32::INFINITY),
            (0.0, f32::NEG_INFINITY),
            (0.0, -1.0),
        ] {
            let bytes = postcard::to_allocvec(&fields).unwrap();
            app.world_mut()
                .entity_mut(carrier)
                .apply_write(bytes, fns_id, RepliconTick::new(1));
            assert!(matches!(
                app.world_mut()
                    .resource_mut::<ReplicationStorage>()
                    .global
                    .remove::<wire::StampRefusal>(),
                Some(wire::StampRefusal::Invalid(_))
            ));
        }
        // Fixture: empty and truncated payloads cannot supply both fields.
        for bytes in [vec![], vec![0; size_of::<f64>() + size_of::<f32>() - 1]] {
            app.world_mut()
                .entity_mut(carrier)
                .apply_write(bytes, fns_id, RepliconTick::new(1));
            assert!(matches!(
                app.world_mut()
                    .resource_mut::<ReplicationStorage>()
                    .global
                    .remove::<wire::StampRefusal>(),
                Some(wire::StampRefusal::Encoding(_))
            ));
        }
        assert_eq!(
            *app.world().entity(carrier).get::<TickStamp>().unwrap(),
            initial
        );
        assert!(
            app.world()
                .resource::<ReplicationStorage>()
                .global
                .get::<ReceivedStamps>()
                .is_none()
        );
    }
}
