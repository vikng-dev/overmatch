//! Real-UDP probe of the client's input-message cadence: a client rendering several frames per
//! fixed tick sends at most one input message per tick, and still sends one for every tick.
//!
//! The same loopback pair runs twice, with and without `install_input_send_cadence`, so the probe
//! proves it can see the per-frame cadence the gate removes.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use lightyear::input::input_message::InputMessage;
use lightyear::prelude::client::{
    Client, ClientPlugins, Connect, Connected, NetcodeClient, NetcodeConfig,
};
use lightyear::prelude::input::native::NativeStateSequence;
use lightyear::prelude::input::server::InputSystems as ServerInputSystems;
use lightyear::prelude::server::{
    NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
};
use lightyear::prelude::*;

use super::client::{install_input_send_cadence, shipping_input_delay};
use super::protocol::protocol_id;
use super::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};
use crate::command::TankCommand;

type TankInputMessage = InputMessage<NativeStateSequence<TankCommand>>;

/// Client frames per fixed tick: a ~512 fps client against the 64 Hz sim.
const FRAMES_PER_TICK: u32 = 8;
const CONNECT_STEPS: u32 = 2_000;
/// Steps after the first received input message before counting, so sync has settled.
const WARMUP_STEPS: u32 = 64;
const MEASURED_STEPS: u32 = 256;
/// Messages still in flight or already queued when the window opens or closes.
const EDGE_SLACK: usize = 2;

/// End ticks of every input message the server received, in arrival order.
#[derive(Resource, Default)]
struct ReceivedInputTicks(Vec<Tick>);

#[derive(Resource, Default)]
struct ClientTicks(u32);

fn record_received_inputs(
    mut receivers: Query<&mut MessageReceiver<TankInputMessage>>,
    mut received: ResMut<ReceivedInputTicks>,
) {
    for mut receiver in &mut receivers {
        receiver.retain_messages(|message| {
            received.0.push(message.end_tick);
            true
        });
    }
}

fn count_client_tick(mut ticks: ResMut<ClientTicks>) {
    ticks.0 += 1;
}

fn build_server(port: u16) -> App {
    let mut app = base_app();
    app.add_plugins(ServerPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
    app.init_resource::<ReceivedInputTicks>().add_systems(
        PreUpdate,
        record_received_inputs.in_set(ServerInputSystems::ValidateInputs),
    );
    let server = app
        .world_mut()
        .spawn((
            NetcodeServer::new(ServerNetcodeConfig {
                protocol_id: protocol_id(),
                private_key: [0; 32],
                ..default()
            }),
            LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)),
            ServerUdpIo::default(),
        ))
        .id();
    app.world_mut().commands().trigger(Start { entity: server });
    app
}

fn build_client(port: u16, gated: bool) -> App {
    let mut app = base_app();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(TICK / FRAMES_PER_TICK));
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
    if gated {
        install_input_send_cadence(&mut app);
    }
    app.init_resource::<ClientTicks>()
        .add_systems(FixedFirst, count_client_tick);
    app.insert_resource(InputTimelineConfig::new(
        SyncConfig::default(),
        shipping_input_delay(),
    ));
    let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    let client = app
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
                    protocol_id: protocol_id(),
                },
                NetcodeConfig::default(),
            )
            .expect("manual dev token should always build"),
            UdpIo::default(),
        ))
        .id();
    app.world_mut()
        .commands()
        .trigger(Connect { entity: client });
    app
}

/// One server tick and the client's `FRAMES_PER_TICK` frames, which together advance one tick.
fn step(server: &mut App, client: &mut App) {
    server.update();
    std::thread::sleep(Duration::from_micros(300));
    for _ in 0..FRAMES_PER_TICK {
        client.update();
    }
    std::thread::sleep(Duration::from_micros(300));
}

struct Window {
    client_ticks: usize,
    messages: usize,
    distinct_end_ticks: usize,
}

fn measure(gated: bool) -> Window {
    let _udp = lock_real_udp_test();
    let port = free_port();
    let mut server = build_server(port);
    let mut client = build_client(port, gated);
    finish(&mut server);
    finish(&mut client);

    let mut connected = false;
    for _ in 0..CONNECT_STEPS {
        step(&mut server, &mut client);
        let link_up = client
            .world_mut()
            .query_filtered::<(), (With<Client>, With<Connected>)>()
            .iter(client.world())
            .next()
            .is_some();
        if link_up && !server.world().resource::<ReceivedInputTicks>().0.is_empty() {
            connected = true;
            break;
        }
    }
    assert!(
        connected,
        "the client never delivered an input message within {CONNECT_STEPS} steps"
    );
    for _ in 0..WARMUP_STEPS {
        step(&mut server, &mut client);
    }

    server
        .world_mut()
        .resource_mut::<ReceivedInputTicks>()
        .0
        .clear();
    let ticks_before = client.world().resource::<ClientTicks>().0;
    for _ in 0..MEASURED_STEPS {
        step(&mut server, &mut client);
    }
    let client_ticks = (client.world().resource::<ClientTicks>().0 - ticks_before) as usize;
    let received = &server.world().resource::<ReceivedInputTicks>().0;
    let mut distinct = received.clone();
    distinct.sort_unstable_by_key(|tick| tick.0);
    distinct.dedup();
    Window {
        client_ticks,
        messages: received.len(),
        distinct_end_ticks: distinct.len(),
    }
}

/// A client running `FRAMES_PER_TICK` frames per tick: gated, it sends one input message per
/// tick, each ending on a distinct tick, with none skipped; ungated, the same harness sees the
/// per-frame cadence.
#[test]
fn a_fast_client_sends_one_input_message_per_tick() {
    let gated = measure(true);
    println!(
        "gated: {} input messages ({} distinct end ticks) over {} client ticks at {FRAMES_PER_TICK} \
         frames/tick",
        gated.messages, gated.distinct_end_ticks, gated.client_ticks
    );
    assert!(
        gated.messages <= gated.client_ticks + EDGE_SLACK,
        "a client at {FRAMES_PER_TICK} frames/tick sent {} input messages over {} ticks: more \
         than one per tick",
        gated.messages,
        gated.client_ticks,
    );
    assert_eq!(
        gated.distinct_end_ticks, gated.messages,
        "two input messages ended on the same tick"
    );
    assert!(
        gated.messages + EDGE_SLACK >= gated.client_ticks,
        "the gate skipped ticks: {} input messages over {} ticks",
        gated.messages,
        gated.client_ticks,
    );

    let ungated = measure(false);
    println!(
        "ungated: {} input messages ({} distinct end ticks) over {} client ticks",
        ungated.messages, ungated.distinct_end_ticks, ungated.client_ticks
    );
    assert!(
        ungated.messages > 2 * ungated.client_ticks,
        "without the gate the probe must see the per-frame cadence ({} messages over {} ticks); \
         if lightyear now sends once per tick on its own, the gate may be retirable",
        ungated.messages,
        ungated.client_ticks,
    );
}
