//! Real-UDP probe of the client's input-message cadence: a client rendering several frames per
//! fixed tick sends exactly one input message per buffered tick, and that message carries the
//! command authored for its tick.
//!
//! The client owns a local input slot whose writer authors a distinct value every tick (a sequence
//! number in `range`), stamped by the production `stamp_input_tick`. The server records, in arrival
//! order, the command each message carries at its own `end_tick` — the one fresh state; the other
//! four are redundancy — and drops the message so no target resolution runs. The same pair runs
//! twice, with and without `install_input_send_cadence`, so the probe proves it can see the
//! per-frame cadence the gate removes.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use lightyear::input::input_message::{ActionStateSequence, Compressed, InputMessage};
use lightyear::prelude::client::{
    Client, ClientPlugins, Connect, Connected, NetcodeClient, NetcodeConfig,
};
use lightyear::prelude::input::client::InputSystems as ClientInputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker, NativeStateSequence};
use lightyear::prelude::input::server::InputSystems as ServerInputSystems;
use lightyear::prelude::server::{
    NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
};
use lightyear::prelude::*;

use super::client::{install_input_send_cadence, shipping_input_delay, stamp_input_tick};
use super::protocol::protocol_id;
use super::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};
use crate::command::TankCommand;

type TankInputMessage = InputMessage<NativeStateSequence<TankCommand>>;

/// Client frames per fixed tick: a ~512 fps client against the 64 Hz sim.
const FRAMES_PER_TICK: u32 = 8;
const CONNECT_STEPS: u32 = 2_000;
/// Steps after the first received input message before the window opens, so sync has settled.
const WARMUP_STEPS: u32 = 64;
const MEASURED_STEPS: u32 = 256;
/// Explicit timeline shifts applied inside the window, each between two frames: (step, delta).
const SHIFTS: [(u32, i32); 2] = [(64, 3), (160, -3)];
/// The ungated control's window. It closes before the first shift: the control asserts only that
/// the per-frame cadence sends more than two messages per tick.
const CONTROL_STEPS: u32 = 32;
/// Steps after the window closes, so every message authored inside it has arrived.
const DRAIN_STEPS: u32 = 16;

/// One authored command: the input tick it was stamped for and its sequence number.
type Authored = (u32, u32);

/// Commands the client authored for a buffered tick, in authoring order.
#[derive(Resource, Default)]
struct AuthoredCommands(Vec<Authored>);

/// The fresh state (`end_tick`'s command) of every input message the server received, in arrival
/// order; `None` when that state is absent or carries another tick's stamp.
#[derive(Resource, Default)]
struct ReceivedFresh(Vec<Option<Authored>>);

#[derive(Resource, Default)]
struct ClientTicks(u32);

/// Author a distinct command per tick: the sequence number rides in `range`.
fn author_command(
    mut slots: Query<&mut ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
    authored: Res<AuthoredCommands>,
) {
    for mut state in &mut slots {
        state.0 = TankCommand {
            range: authored.0.len() as f32,
            ..default()
        };
    }
}

/// Record the stamped command `buffer_action_state` files next; unsynced, it buffers nothing.
fn record_authored(
    sync: Res<LocalTimelineSync>,
    slots: Query<&ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
    mut authored: ResMut<AuthoredCommands>,
) {
    if !sync.is_synced() {
        return;
    }
    for state in &slots {
        authored.0.push((state.0.for_tick, state.0.range as u32));
    }
}

fn count_client_tick(mut ticks: ResMut<ClientTicks>) {
    ticks.0 += 1;
}

fn record_received_inputs(
    mut receivers: Query<&mut MessageReceiver<TankInputMessage>>,
    mut received: ResMut<ReceivedFresh>,
) {
    for mut receiver in &mut receivers {
        receiver.retain_messages(|message| {
            let end_tick = message.end_tick.0;
            for target in &message.inputs {
                let mut fresh = None;
                for state in target.states.clone().get_snapshots_from_message(TICK) {
                    match state {
                        Compressed::Input(ActionState(command)) => fresh = Some(command),
                        Compressed::Absent => fresh = None,
                        Compressed::SameAsPrecedent => {}
                    }
                }
                received.0.push(
                    fresh
                        .filter(|command| command.for_tick == end_tick)
                        .map(|command| (command.for_tick, command.range as u32)),
                );
            }
            false
        });
    }
}

fn build_server(port: u16) -> App {
    let mut app = base_app();
    app.add_plugins(ServerPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
    app.init_resource::<ReceivedFresh>().add_systems(
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
        .init_resource::<AuthoredCommands>()
        .add_systems(FixedFirst, count_client_tick)
        .add_systems(
            FixedPreUpdate,
            (author_command, stamp_input_tick, record_authored)
                .chain()
                .in_set(ClientInputSystems::WriteClientInputs),
        );
    app.insert_resource(InputTimelineConfig::new(
        SyncConfig::default(),
        shipping_input_delay(),
    ));
    app.world_mut().spawn((
        InputMarker::<TankCommand>::default(),
        ActionState::<TankCommand>::default(),
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
/// `shift_after_frame` triggers a `LocalTimelineShift` on the client between two of those frames.
fn step(server: &mut App, client: &mut App, shift_after_frame: Option<(u32, i32)>) {
    server.update();
    std::thread::sleep(Duration::from_micros(300));
    for frame in 0..FRAMES_PER_TICK {
        client.update();
        if let Some((at, delta)) = shift_after_frame
            && at == frame
        {
            client.world_mut().trigger(LocalTimelineShift { delta });
        }
    }
    std::thread::sleep(Duration::from_micros(300));
}

struct Run {
    /// Commands authored inside the window, in authoring order.
    authored: Vec<Authored>,
    /// Fresh states received from the first to the last window command, in arrival order.
    received: Vec<Option<Authored>>,
    client_ticks: usize,
    messages: usize,
}

fn run(gated: bool, measured_steps: u32) -> Run {
    let _udp = lock_real_udp_test();
    let port = free_port();
    let mut server = build_server(port);
    let mut client = build_client(port, gated);
    finish(&mut server);
    finish(&mut client);

    let mut connected = false;
    for _ in 0..CONNECT_STEPS {
        step(&mut server, &mut client, None);
        let link_up = client
            .world_mut()
            .query_filtered::<(), (With<Client>, With<Connected>)>()
            .iter(client.world())
            .next()
            .is_some();
        if link_up && !server.world().resource::<ReceivedFresh>().0.is_empty() {
            connected = true;
            break;
        }
    }
    assert!(
        connected,
        "the client never delivered an input message within {CONNECT_STEPS} steps"
    );
    for _ in 0..WARMUP_STEPS {
        step(&mut server, &mut client, None);
    }

    let authored_before = client.world().resource::<AuthoredCommands>().0.len();
    let received_before = server.world().resource::<ReceivedFresh>().0.len();
    let ticks_before = client.world().resource::<ClientTicks>().0;
    for step_index in 0..measured_steps {
        // Mid-tick: the shift lands between the frame that buffered a tick and the next one.
        let shift = SHIFTS
            .iter()
            .find(|(at, _)| *at == step_index)
            .map(|(_, delta)| (FRAMES_PER_TICK / 2, *delta));
        step(&mut server, &mut client, shift);
    }
    let authored_after = client.world().resource::<AuthoredCommands>().0.len();
    let client_ticks = (client.world().resource::<ClientTicks>().0 - ticks_before) as usize;
    for _ in 0..DRAIN_STEPS {
        step(&mut server, &mut client, None);
    }

    let authored =
        client.world().resource::<AuthoredCommands>().0[authored_before..authored_after].to_vec();
    let all_received = &server.world().resource::<ReceivedFresh>().0[received_before..];
    let first = authored.first().copied();
    let last = authored.last().copied();
    let start = all_received
        .iter()
        .position(|fresh| *fresh == first)
        .expect("the window's first command never arrived as a fresh state");
    let end = all_received
        .iter()
        .rposition(|fresh| *fresh == last)
        .expect("the window's last command never arrived as a fresh state");
    Run {
        authored,
        received: all_received[start..=end].to_vec(),
        client_ticks,
        messages: end + 1 - start,
    }
}

/// A client at `FRAMES_PER_TICK` frames per tick, across an explicit forward and backward timeline
/// shift: gated, the server receives exactly one message per buffered tick, in authoring order,
/// each carrying that tick's command as its fresh state — no gap, no repeat, no stale fresh state.
/// Ungated, the same harness sees the per-frame cadence.
#[test]
fn a_fast_client_sends_one_input_message_per_tick() {
    let gated = run(true, MEASURED_STEPS);
    println!(
        "gated: {} messages carried {} authored commands over {} client ticks at {FRAMES_PER_TICK} \
         frames/tick",
        gated.messages,
        gated.authored.len(),
        gated.client_ticks
    );
    assert_eq!(
        gated.authored.len(),
        gated.client_ticks,
        "every synced client tick authors exactly one command"
    );
    let tick_steps: Vec<i64> = gated
        .authored
        .windows(2)
        .map(|pair| i64::from(pair[1].0) - i64::from(pair[0].0))
        .collect();
    for (_, delta) in SHIFTS {
        assert!(
            tick_steps.contains(&(1 + i64::from(delta))),
            "the {delta:+} shift never moved the authored input ticks"
        );
    }
    let received: Vec<Authored> = gated.received.iter().flatten().copied().collect();
    assert_eq!(
        received.len(),
        gated.received.len(),
        "a message's fresh state was absent or stamped for another tick"
    );
    assert_eq!(
        received, gated.authored,
        "the server must receive each authored command exactly once, in order, as its message's \
         fresh state"
    );

    let ungated = run(false, CONTROL_STEPS);
    println!(
        "ungated: {} messages carried {} authored commands over {} client ticks",
        ungated.messages,
        ungated.authored.len(),
        ungated.client_ticks
    );
    assert!(
        ungated.messages > 2 * ungated.client_ticks,
        "without the gate the probe must see the per-frame cadence ({} messages over {} ticks); \
         if lightyear now sends once per tick on its own, the gate may be retirable",
        ungated.messages,
        ungated.client_ticks,
    );
}
