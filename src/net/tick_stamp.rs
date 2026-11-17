//! The server frame-start record. Tick identity already rides Replicon's checkpoint userdata.
//! The scoreboard is globally visible, so this adds no entity or message channel.

use bevy::prelude::*;
use bevy_replicon::prelude::RepliconTick;
use serde::{Deserialize, Serialize};

mod wire;
pub(super) use wire::replication;

/// Seconds on the authority's monotonic `Time<Real>` epoch; never an estimate of its clock.
/// DERIVED payload: postcard f64 is eight bytes, with no duplicate tick id.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct TickStamp(pub f64);

/// Every decoded stamp, including intermediate ticks overwritten during one receive frame.
#[derive(Default)]
pub(super) struct ReceivedStamps(pub Vec<(RepliconTick, TickStamp)>);

pub(super) fn install_server(app: &mut App) {
    app.add_systems(FixedFirst, publish);
}

fn publish(real: Res<Time<Real>>, mut stamps: Query<&mut TickStamp>) {
    // The dedicated runner admits exactly one tick per frame; Real is sampled before PreUpdate.
    for mut stamp in &mut stamps {
        *stamp = TickStamp(real.elapsed_secs_f64());
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
                postcard::to_allocvec(&TickStamp(f64::from(tick))).unwrap(),
                fns_id,
                RepliconTick::new(tick),
            );
        }
        assert_eq!(entity.get::<TickStamp>().unwrap().0, 2.0);
        let received = &app
            .world()
            .resource::<ReplicationStorage>()
            .global
            .get::<ReceivedStamps>()
            .unwrap()
            .0;
        assert_eq!(
            received,
            &[
                (RepliconTick::new(1), TickStamp(1.0)),
                (RepliconTick::new(2), TickStamp(2.0))
            ]
        );
    }

    /// The wire budget depends on the fixed f64 payload rather than epoch-dependent varints.
    #[test]
    fn stamp_payload_is_eight_bytes_at_every_epoch() {
        // Fixture epochs: initial, an hour, and beyond a year.
        for epoch in [0.0, 3600.0, 40_000_000.0] {
            assert_eq!(
                postcard::to_allocvec(&TickStamp(epoch)).unwrap().len(),
                size_of::<f64>()
            );
        }
    }
}
