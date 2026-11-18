use super::*;
use crate::net::matchmaking::MatchClosed;
use crate::net::test_harness::lock_real_udp_test;
use bevy::ecs::message::MessageCursor;
use lightyear::prelude::client::NetcodeClient;
use lightyear::prelude::*;
use std::time::Duration;

/// Receive admission must apply each mount's stops to its assembled wire slot. Clients would
/// otherwise accept another mount's travel or refuse valid authority angles.
#[test]
fn wire_limits_match_assembled_servo_indices() {
    use crate::tank::{ServoIndex, ServoSpec};

    let (_, template) = crate::tank::shipped_construction();
    let slots = ServoSlots::from_template(&template);
    let mut world = World::new();
    crate::tank::spawn_headless_tank(&mut world.commands(), &template, Transform::IDENTITY);
    world.flush();
    let mut servos = world.query::<(&ServoIndex, &ServoSpec)>();
    assert_eq!(servos.iter(&world).count(), slots.0.len());
    for (index, spec) in servos.iter(&world) {
        assert_eq!(slots.0[index.0], spec.travel_limits());
    }
}

/// Clients rely on the installed codec refusing mutations before interpolation. Bypassing the
/// checked registration loses the visible cause; admitting the record puts invalid game state in
/// history. The shipped lifecycle must close the link and report the cause in the lobby; leaving
/// the link alive or reconnecting after refusal also fails this real-UDP witness.
#[test]
fn malformed_updates_close_the_match_before_interpolation() {
    let _udp = lock_real_udp_test();
    let (_, template) = crate::tank::shipped_construction();
    let slots = ServoSlots::from_template(&template);
    let count = slots.0.len();
    let mut non_finite = vec![0.0; count];
    non_finite[0] = f32::NAN;
    let (slot, (_, max)) = slots
        .0
        .iter()
        .enumerate()
        .find_map(|(slot, limits)| limits.map(|limits| (slot, limits)))
        .unwrap();
    let mut outside = vec![0.0; count];
    outside[slot] = max + std::f32::consts::TAU;
    for (bad, cause) in [
        (
            vec![0.0; count - 1],
            ServoAnglesError::Count {
                expected: count,
                received: count - 1,
            },
        ),
        (non_finite, ServoAnglesError::NonFinite { slot: 0 }),
        (outside, ServoAnglesError::OutOfTravel { slot }),
    ] {
        refuse_update(bad, cause);
    }
}

fn refuse_update(bad: Vec<f32>, expected: ServoAnglesError) {
    let (mut server, mut client) = crate::net::client::match_exit::in_match();
    let (_, template) = crate::tank::shipped_construction();
    let slots = ServoSlots::from_template(&template);
    client.insert_resource(template);
    // Fixture: establish a complete real stream before sending a malformed update.
    let valid = ServoAngles(vec![0.0; slots.0.len()]);
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
        assert!(poses.iter().all(|pose| {
            pose.0.len() == slots.0.len()
                && pose.0.iter().zip(&slots.0).all(|(angle, limits)| {
                    angle.is_finite() && limits.is_none_or(|(min, max)| (min..=max).contains(angle))
                })
        }));
        if !corrupted && poses.contains(&valid) {
            server
                .world_mut()
                .get_mut::<ServoAngles>(carrier)
                .unwrap()
                .0 = bad.clone();
            corrupted = true;
        }
        let causes: Vec<_> = closed
            .read(client.world().resource::<Messages<MatchClosed>>())
            .cloned()
            .collect();
        if !causes.is_empty() {
            assert!(corrupted, "a valid stream must not be refused");
            assert_eq!(causes, vec![MatchClosed::Ended(expected.to_string())]);
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
        &client.world().resource::<crate::net::client::ClosedConnection>().cause,
        Some(ServerGone::Servo(cause)) if *cause == expected
    ));
    let cause = expected.to_string();
    assert!(
        client
            .world_mut()
            .query::<&Text>()
            .iter(client.world())
            .any(|text| text.0.contains(&crate::net::matchmaking::printable(&cause))),
        "the production lobby tells the player why the stream was refused"
    );
}

/// Replica presentation relies on admission rejecting non-finite, incomplete and out-of-travel
/// samples. Continuous mounts deliberately retain finite unwrapped travel without quantisation.
#[test]
fn admission_preserves_only_complete_poses_inside_the_data_domain() {
    // Fixture stops in radians, with one continuous slot beside limited mounts.
    let slots = ServoSlots(vec![
        Some((-0.5, 0.75)),
        None,
        Some((-1.0, 2.0)),
        Some((0.0, 0.0)),
    ]);
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for slot in 0..slots.0.len() {
            let mut angles = vec![0.0; slots.0.len()];
            angles[slot] = value;
            let bytes = postcard::to_allocvec(&angles).unwrap();
            assert_eq!(
                admit(&mut bytes.as_slice(), Some(&slots)),
                Err(ServoAnglesError::NonFinite { slot })
            );
        }
    }
    for received in [0, 1, 3, 5] {
        let bytes = postcard::to_allocvec(&vec![0.0f32; received]).unwrap();
        assert_eq!(
            admit(&mut bytes.as_slice(), Some(&slots)),
            Err(ServoAnglesError::Count {
                expected: slots.0.len(),
                received
            })
        );
    }
    for (slot, limits) in slots.0.iter().enumerate() {
        if let Some((min, max)) = limits {
            for value in [min.next_down(), max.next_up(), f32::MIN, f32::MAX] {
                let mut angles = vec![0.0; slots.0.len()];
                angles[slot] = value;
                let bytes = postcard::to_allocvec(&angles).unwrap();
                assert_eq!(
                    admit(&mut bytes.as_slice(), Some(&slots)),
                    Err(ServoAnglesError::OutOfTravel { slot })
                );
            }
        }
    }
    for continuous in [f32::MIN, f32::MAX, std::f32::consts::TAU * 2.0] {
        let values = vec![-0.5, continuous, 2.0, 0.0];
        let bytes = postcard::to_allocvec(&values).unwrap();
        let pose = admit(&mut bytes.as_slice(), Some(&slots)).unwrap();
        assert_eq!(pose.0, values);
        assert_eq!(postcard::to_allocvec(&pose).unwrap(), bytes);
        assert_eq!(bytes.len(), 1 + 4 * values.len());
    }
    let bytes = postcard::to_allocvec(&vec![0.0f32; slots.0.len()]).unwrap();
    assert_eq!(
        admit(&mut bytes.as_slice(), None),
        Err(ServoAnglesError::MissingRig)
    );
    assert!(matches!(
        admit(&mut [].as_slice(), Some(&slots)),
        Err(ServoAnglesError::Encoding(_))
    ));
}

/// Admitted continuous travel spans finite f32; an overflowing subtraction must never put an
/// infinite interpolated angle into the client's optic or transform readers.
#[test]
fn admitted_extreme_continuous_travel_has_a_finite_convex_blend() {
    let slots = ServoSlots(vec![None]);
    let admit_angle = |angle: f32| {
        let bytes = postcard::to_allocvec(&vec![angle]).unwrap();
        admit(&mut bytes.as_slice(), Some(&slots)).unwrap()
    };
    for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let angle = crate::net::protocol::servo_angles_lerp(
            admit_angle(f32::MIN),
            admit_angle(f32::MAX),
            t,
        )
        .0[0];
        assert!(angle.is_finite());
        assert!((f32::MIN..=f32::MAX).contains(&angle));
        if t == 0.5 {
            assert_eq!(angle, 0.0);
        }
    }
    assert_eq!(
        crate::net::protocol::servo_angles_lerp(admit_angle(f32::MAX), admit_angle(1.0), 1.0).0,
        vec![1.0]
    );
}
