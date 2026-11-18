use super::*;
use crate::net::matchmaking::MatchClosed;
use crate::net::test_harness::lock_real_udp_test;
use bevy::ecs::message::MessageCursor;
use lightyear::prelude::client::NetcodeClient;
use lightyear::prelude::*;
use std::time::Duration;

/// Clients rely on the installed codec refusing mutations before interpolation. Bypassing the
/// checked registration loses the visible cause; admitting the record puts invalid game state in
/// history. The shipped lifecycle must close the link and report the cause in the lobby; leaving
/// the link alive or reconnecting after refusal also fails this real-UDP witness.
#[test]
fn malformed_updates_close_the_match_before_interpolation() {
    let _udp = lock_real_udp_test();
    let (mut server, mut client) = crate::net::client::match_exit::in_match();
    let (_, template) = crate::tank::shipped_construction();
    let count = template.spec().servos.len();
    client.insert_resource(template);
    // Fixture: establish a complete real stream, then remove one required slot.
    let valid = ServoAngles(vec![0.25; count]);
    let carrier = server
        .world_mut()
        .spawn((
            valid.clone(),
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    let mut corrupted = false;
    let mut refused = false;
    let mut closed = MessageCursor::<MatchClosed>::default();
    for _ in 0..2000 {
        server.update();
        std::thread::sleep(Duration::from_micros(300));
        client.update();
        std::thread::sleep(Duration::from_micros(300));
        let poses: Vec<_> = client
            .world_mut()
            .query::<&ServoAngles>()
            .iter(client.world())
            .cloned()
            .collect();
        assert!(
            poses
                .iter()
                .all(|pose| pose.0.len() == count && pose.0.iter().all(|angle| angle.is_finite()))
        );
        if !corrupted && poses.contains(&valid) {
            server
                .world_mut()
                .get_mut::<ServoAngles>(carrier)
                .unwrap()
                .0
                .pop();
            corrupted = true;
        }
        let causes: Vec<_> = closed
            .read(client.world().resource::<Messages<MatchClosed>>())
            .cloned()
            .collect();
        if !causes.is_empty() {
            assert!(corrupted, "a valid stream must not be refused");
            assert_eq!(
                causes,
                vec![MatchClosed::Ended(
                    ServoAnglesError::Count {
                        expected: count,
                        received: count - 1
                    }
                    .to_string()
                )]
            );
            refused = true;
            break;
        }
    }
    assert!(corrupted, "the valid first sample reached interpolation");
    assert!(
        refused,
        "malformed peer input must reach the visible match-close path"
    );
    crate::net::client::match_exit::advance_past_reconnect_window(&mut client);
    // Fixture frames allow the lifecycle's deferred despawn and lobby transition to settle,
    // with each frame advancing beyond the policy's reconnect window.
    for _ in 0..8 {
        server.update();
        client.update();
        assert_eq!(
            client
                .world_mut()
                .query_filtered::<(), With<NetcodeClient>>()
                .iter(client.world())
                .count(),
            0,
            "a refused stream loses its link and never reconnects"
        );
    }
    assert_eq!(
        *client.world().resource::<State<crate::flow::Flow>>().get(),
        crate::flow::Flow::Lobby
    );
    assert!(matches!(
        client
            .world()
            .resource::<crate::net::client::ClosedConnection>()
            .cause,
        Some(ServerGone::Servo(ServoAnglesError::Count { expected, received })) if expected == count && received == count - 1
    ));
    let cause = ServoAnglesError::Count {
        expected: count,
        received: count - 1,
    }
    .to_string();
    assert!(
        client
            .world_mut()
            .query::<&Text>()
            .iter(client.world())
            .any(|text| text.0.contains(&crate::net::matchmaking::printable(&cause))),
        "the production lobby tells the player why the stream was refused"
    );
}
