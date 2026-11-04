use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use bevy::prelude::*;
use lightyear::prelude::client::{ClientPlugins, NetcodeClient};
use lightyear::prelude::*;

use super::*;
use crate::net::debris_stress::{debris_pose_lerp, install_client};
use crate::net::matchmaking::ServerGone;
use crate::net::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};

/// Replica interpolation may only see finite positions and unit rotations. Fixture coordinates
/// span malformed components as well as the finite extremes; no received value is normalized away.
#[test]
fn deserialization_checks_the_pose_without_changing_valid_wire_values() {
    let valid = [
        DebrisPose {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        },
        DebrisPose {
            position: Vec3::splat(f32::MAX),
            rotation: Quat::from_rotation_y(1.0),
        },
        DebrisPose {
            position: Vec3::splat(-f32::MAX),
            rotation: -Quat::IDENTITY,
        },
    ];
    for pose in valid {
        let wire = WirePose {
            position: pose.position,
            rotation: pose.rotation,
        };
        let bytes = postcard::to_allocvec(&wire).unwrap();
        assert_eq!(
            postcard::to_allocvec(&pose).unwrap(),
            bytes,
            "valid layout is unchanged"
        );
        let checked: DebrisPose = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(checked, pose);
        assert!(
            debris_pose_lerp(checked, valid[0], 0.5)
                .position
                .is_finite()
        );
    }
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for axis in 0..3 {
            let mut position = Vec3::ZERO;
            position[axis] = bad;
            let bytes = postcard::to_allocvec(&WirePose {
                position,
                rotation: Quat::IDENTITY,
            })
            .unwrap();
            assert!(postcard::from_bytes::<DebrisPose>(&bytes).is_err());
        }
    }
    for rotation in [
        Quat::from_array([0.0; 4]),
        Quat::from_array([f32::NAN, 0.0, 0.0, 1.0]),
        Quat::from_array([0.0, f32::INFINITY, 0.0, 1.0]),
        Quat::from_array([0.0, 0.0, 0.0, 2.0]),
    ] {
        let bytes = postcard::to_allocvec(&WirePose {
            position: Vec3::ZERO,
            rotation,
        })
        .unwrap();
        assert!(postcard::from_bytes::<DebrisPose>(&bytes).is_err());
    }
}

fn client(port: u16) -> App {
    let mut app = base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    crate::net::protocol::plugin(&mut app);
    install_client(&mut app);
    let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    let link = app
        .world_mut()
        .spawn((
            Client,
            Link::default(),
            LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)),
            PeerAddr(server_addr),
            ReplicationReceiver,
            NetcodeClient::new(
                Authentication::Manual {
                    server_addr,
                    client_id: 1,
                    private_key: [0; 32],
                    protocol_id: crate::net::protocol::protocol_id(),
                },
                lightyear::prelude::client::NetcodeConfig::default(),
            )
            .unwrap(),
            UdpIo::default(),
        ))
        .id();
    app.world_mut().commands().trigger(Connect { entity: link });
    finish(&mut app);
    app
}

/// Clients rely on the installed codec refusing mutations before interpolation. Bypassing the
/// checked registration loses the visible cause; admitting the record puts invalid game state in
/// history. Both fail this real-UDP witness, which mounts the production protocol and installer.
#[test]
fn malformed_updates_are_visibly_refused_before_interpolation() {
    let _udp = lock_real_udp_test();
    let port = free_port();
    let mut server = crate::net::link_drop::build_server(port);
    // Fixture pose: the first sample establishes a real interpolation history before corruption.
    let valid = DebrisPose {
        position: Vec3::new(3.0, 2.0, -1.0),
        rotation: Quat::IDENTITY,
    };
    let carrier = server
        .world_mut()
        .spawn((
            valid,
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    finish(&mut server);
    let mut client = client(port);
    let mut corrupted = false;
    let mut refused = false;
    for _ in 0..2000 {
        server.update();
        std::thread::sleep(Duration::from_micros(300));
        client.update();
        std::thread::sleep(Duration::from_micros(300));
        let poses: Vec<_> = client
            .world_mut()
            .query::<&DebrisPose>()
            .iter(client.world())
            .copied()
            .collect();
        assert!(
            poses
                .iter()
                .all(|pose| pose.position.is_finite() && pose.rotation.is_normalized())
        );
        if !corrupted && poses.contains(&valid) {
            server
                .world_mut()
                .get_mut::<DebrisPose>(carrier)
                .unwrap()
                .rotation = Quat::from_array([0.0; 4]);
            corrupted = true;
        }
        let causes: Vec<_> = client
            .world_mut()
            .resource_mut::<Messages<ServerGone>>()
            .drain()
            .collect();
        if !causes.is_empty() {
            assert!(corrupted, "a valid stream must not be refused");
            assert!(
                causes
                    .iter()
                    .all(|cause| cause.0 == PoseRefusal::Rotation.to_string())
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
}
