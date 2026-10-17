//! Real-UDP probes of the controlled tank's input path through lightyear's native inputs and the
//! input bridge, on frames of several ticks. Every tick of a frame files the device's command for
//! that frame, never the bridge's restored input: its levels on every tick, each edge on one. From
//! the frame the replicated knockout arrives that command is the death flow's neutral one, and the
//! server receives nothing else; once the input delay has drained the bridge restores nothing else
//! either; from the frame a live status arrives the device's command is filed, restored and
//! received again on every tick.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy::window::PrimaryWindow;
use lightyear::input::input_message::{ActionStateSequence, Compressed, InputMessage};
use lightyear::prelude::client::{
    Client, ClientPlugins, Connect, NetcodeClient, NetcodeConfig, Remote,
};
use lightyear::prelude::input::client::InputSystems as ClientInputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker, NativeStateSequence};
use lightyear::prelude::input::server::InputSystems as ServerInputSystems;
use lightyear::prelude::server::{
    NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
};
use lightyear::prelude::*;

use super::client::{
    SHIPPING_INPUT_DELAY_TICKS, install_device_feed, shipping_input_delay, stamp_input_tick,
};
use super::disclosure::NetTankStatus;
use super::protocol::{InputBridge, protocol_id};
use super::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};
use crate::command::{
    AimIntent, ConsumeCommandEdges, CrewSwap, TankCommand, TankDeviceSet, gather_commands,
};
use crate::damage::CrewStation;
use crate::tank::Controlled;

type TankInputMessage = InputMessage<NativeStateSequence<TankCommand>>;

/// Fixed ticks per client frame: every frame catches up several ticks (fixture data).
const TICKS_PER_FRAME: u32 = 3;
const CONNECT_STEPS: u32 = 2_000;

/// The client's fixed ticks per frame.
#[derive(Resource, Clone, Copy)]
struct FrameTicks(u32);
/// Frames of each phase once the input path is up: more than twice the input delay in ticks
/// (fixture data).
const PHASE_FRAMES: u32 = 4;

/// What the client filed for each tick, `(for_tick, command)`, in filing order.
#[derive(Resource, Default)]
struct Filed(Vec<(u32, TankCommand)>);

/// What the bridge handed the sim on each tick, `(tick, command)`.
#[derive(Resource, Default)]
struct Restored(Vec<(u32, TankCommand)>);

/// Every tick's command the server received, by tick, as the latest message carrying the tick
/// states it; `None` when that state is absent.
#[derive(Resource, Default)]
struct Received(BTreeMap<u32, Option<TankCommand>>);

/// Stand-in for the aim commit and the crew bar, read from the device with the gather.
fn author_aim_and_swap(mut tanks: Query<&mut TankCommand, With<Controlled>>) {
    for mut command in &mut tanks {
        command.aim = Some(AimIntent::World(Vec3::new(0.0, 0.0, -100.0)));
        command.crew_swap = Some(CrewSwap::Start(CrewStation::Commander, CrewStation::Gunner));
    }
}

/// The stamped command `buffer_action_state` files next; unsynced, it files nothing.
fn record_filed(
    sync: Res<LocalTimelineSync>,
    slots: Query<&ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
    mut filed: ResMut<Filed>,
) {
    if !sync.is_synced() {
        return;
    }
    for state in &slots {
        filed.0.push((state.0.for_tick, state.0));
    }
}

fn record_restored(
    sync: Res<LocalTimelineSync>,
    timeline: Res<LocalTimeline>,
    slots: Query<&TankCommand, With<InputMarker<TankCommand>>>,
    mut restored: ResMut<Restored>,
) {
    if !sync.is_synced() {
        return;
    }
    for command in &slots {
        restored.0.push((timeline.tick().0, *command));
    }
}

/// Record each message's states by tick and drop the message, so no target resolution runs.
fn record_received(
    mut receivers: Query<&mut MessageReceiver<TankInputMessage>>,
    mut received: ResMut<Received>,
) {
    for mut receiver in &mut receivers {
        receiver.retain_messages(|message| {
            for target in &message.inputs {
                let states: Vec<_> = target
                    .states
                    .clone()
                    .get_snapshots_from_message(TICK)
                    .collect();
                let start = message.end_tick.0 + 1 - states.len() as u32;
                let mut state = None;
                for (offset, snapshot) in states.into_iter().enumerate() {
                    match snapshot {
                        Compressed::Input(ActionState(command)) => state = Some(command),
                        Compressed::Absent => state = None,
                        Compressed::SameAsPrecedent => {}
                    }
                    received.0.insert(start + offset as u32, state);
                }
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
    app.init_resource::<Received>().add_systems(
        PreUpdate,
        record_received.in_set(ServerInputSystems::ValidateInputs),
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

/// The windowed client's device path on the wire: the gather, the death flow's claim, the feed, the
/// bridge and the edge consumption, for a controlled tank that carries its replicated status, on
/// frames of `frame_ticks` fixed ticks. The client and the tank.
fn build_client(port: u16, frame_ticks: u32) -> (App, Entity) {
    let mut app = base_app();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(TICK * frame_ticks))
        .insert_resource(FrameTicks(frame_ticks));
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
    crate::command::core_plugin(&mut app);
    crate::command::client_plugin(&mut app);
    install_device_feed(&mut app);
    let mut overlays = crate::overlay::Overlays::default();
    overlays.declare_in_match(true);
    app.insert_resource(overlays)
        .init_resource::<super::fire_presentation::AuthoredIntent>()
        .init_resource::<crate::firecontrol::Ranging>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<Filed>()
        .init_resource::<Restored>()
        .add_systems(
            RunFixedMainLoop,
            author_aim_and_swap
                .after(gather_commands)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .in_set(TankDeviceSet),
        )
        .add_systems(
            FixedPreUpdate,
            record_filed
                .after(stamp_input_tick)
                .in_set(ClientInputSystems::WriteClientInputs),
        )
        .add_systems(
            FixedUpdate,
            record_restored
                .after(InputBridge)
                .before(ConsumeCommandEdges),
        );
    app.insert_resource(InputTimelineConfig::new(
        SyncConfig::default(),
        shipping_input_delay(),
    ));
    app.world_mut().spawn((
        Window {
            focused: true,
            ..default()
        },
        PrimaryWindow,
    ));
    let tank = app
        .world_mut()
        .spawn((
            InputMarker::<TankCommand>::default(),
            ActionState::<TankCommand>::default(),
            TankCommand::default(),
            Controlled,
            Remote,
            NetTankStatus::Active,
        ))
        .id();
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
    (app, tank)
}

/// One client frame of [`FrameTicks`] ticks, and the server's ticks that keep pace with it. The
/// commands the client filed in the frame.
fn step(server: &mut App, client: &mut App) -> Vec<(u32, TankCommand)> {
    for _ in 0..client.world().resource::<FrameTicks>().0 {
        server.update();
    }
    std::thread::sleep(std::time::Duration::from_micros(300));
    let before = client.world().resource::<Filed>().0.len();
    client.update();
    std::thread::sleep(std::time::Duration::from_micros(300));
    client.world().resource::<Filed>().0[before..].to_vec()
}

/// A connected server and client with W, D and Space held, once the client has filed more than
/// twice the input delay and the server has received input, on frames of `frame_ticks` fixed
/// ticks. The server, the client and the tank.
fn connected(frame_ticks: u32) -> (App, App, Entity) {
    let port = free_port();
    let mut server = build_server(port);
    let (mut client, tank) = build_client(port, frame_ticks);
    finish(&mut server);
    finish(&mut client);
    let mut keys = client.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    for key in [KeyCode::KeyW, KeyCode::KeyD, KeyCode::Space] {
        keys.press(key);
    }
    let delay = usize::from(SHIPPING_INPUT_DELAY_TICKS);
    for _ in 0..CONNECT_STEPS {
        step(&mut server, &mut client);
        if client.world().resource::<Filed>().0.len() > 2 * delay
            && !server.world().resource::<Received>().0.is_empty()
        {
            return (server, client, tank);
        }
    }
    panic!("the client never filed input within {CONNECT_STEPS} frames");
}

fn ticks(filed: &[(u32, TankCommand)]) -> Vec<u32> {
    filed.iter().map(|(tick, _)| *tick).collect()
}

/// What the bridge restored on each of `ticks`, each of which it must have restored.
fn restored_on(client: &App, ticks: &[u32]) -> Vec<(u32, TankCommand)> {
    let restored = &client.world().resource::<Restored>().0;
    ticks
        .iter()
        .map(|tick| {
            let (_, command) = restored
                .iter()
                .rev()
                .find(|(at, _)| at == tick)
                .unwrap_or_else(|| panic!("tick {tick} was never restored"));
            (*tick, *command)
        })
        .collect()
}

/// What the server received for each of `ticks`, each of which must have arrived with its state.
fn received_on(server: &App, ticks: &[u32]) -> Vec<(u32, TankCommand)> {
    let received = &server.world().resource::<Received>().0;
    ticks
        .iter()
        .map(|tick| {
            let command = received
                .get(tick)
                .copied()
                .flatten()
                .unwrap_or_else(|| panic!("the server received no command for tick {tick}"));
            (*tick, command)
        })
        .collect()
}

/// The ticks the server received a click attested for.
fn received_clicks(server: &App) -> usize {
    server
        .world()
        .resource::<Received>()
        .0
        .iter()
        .filter(|(tick, command)| {
            command.is_some_and(|command| command.fire_primary && command.for_tick == **tick)
        })
        .count()
}

fn neutral(command: &TankCommand) -> bool {
    TankCommand {
        for_tick: 0,
        ..*command
    } == TankCommand::default()
}

/// The held levels: drive, sustained fire and the aim.
fn live(command: &TankCommand) -> bool {
    command.throttle == 1.0
        && command.steer == 1.0
        && command.fire_secondary
        && command.aim.is_some()
}

/// Drive, fire, aim and the crew bar held through a dead life, on frames of several ticks: from
/// the frame the knockout status arrives, every tick files the neutral command and the server
/// receives only it; once the commands filed before that frame have drained the input delay, the
/// bridge restores only it. From the frame a live status arrives, every tick of that frame files
/// the device's command, the first with the frame's crew swap, and the bridge restores and the
/// server receives it on each of them.
#[test]
fn a_dead_tank_files_only_the_neutral_command() {
    let _udp = lock_real_udp_test();
    let (mut server, mut client, tank) = connected(TICKS_PER_FRAME);
    let delay = usize::from(SHIPPING_INPUT_DELAY_TICKS);
    let mut alive = Vec::new();
    for _ in 0..PHASE_FRAMES {
        alive.extend(step(&mut server, &mut client));
    }
    assert!(
        alive.iter().all(|(_, command)| live(command)),
        "alive, the device's command is filed: {alive:?}"
    );

    // As replication writes it, in `PreUpdate` before the frame's ticks.
    client
        .world_mut()
        .entity_mut(tank)
        .insert(NetTankStatus::KnockedOut);
    let restored_from = client.world().resource::<Restored>().0.len();
    let first = step(&mut server, &mut client);
    assert!(
        first.len() > 1,
        "the knockout's frame runs several ticks: {first:?}"
    );
    let mut dead = first;
    for _ in 0..PHASE_FRAMES {
        dead.extend(step(&mut server, &mut client));
    }
    assert!(dead.len() > 2 * delay, "{} ticks", dead.len());
    for (tick, command) in &dead {
        assert!(neutral(command), "tick {tick} filed {command:?} while dead");
    }
    let (first_dead, last_dead) = (dead[0].0, dead[dead.len() - 1].0);
    let restored: Vec<_> = client.world().resource::<Restored>().0[restored_from..]
        .iter()
        .filter(|(tick, _)| *tick >= first_dead)
        .copied()
        .collect();
    assert!(restored.len() > delay, "{restored:?}");
    for (tick, command) in &restored {
        assert!(
            neutral(command),
            "tick {tick} restored {command:?} while dead"
        );
    }

    // A live status: the device's command on every tick of the frame.
    client
        .world_mut()
        .entity_mut(tank)
        .insert(NetTankStatus::Active);
    let respawned = step(&mut server, &mut client);
    assert!(
        respawned.len() > 1,
        "the live status's frame runs several ticks: {respawned:?}"
    );
    for (tick, command) in &respawned {
        assert!(
            live(command),
            "the new life's tick {tick} filed {command:?}"
        );
    }
    assert!(
        respawned[0].1.crew_swap.is_some(),
        "the new life's first tick files the frame's crew swap: {respawned:?}"
    );

    for _ in 0..PHASE_FRAMES {
        step(&mut server, &mut client);
    }
    for (tick, command) in restored_on(&client, &ticks(&respawned)) {
        assert!(
            live(&command),
            "the new life's tick {tick} restored {command:?}"
        );
    }
    for (tick, command) in received_on(&server, &ticks(&respawned)) {
        assert!(
            live(&command),
            "the server received {command:?} for the new life's tick {tick}"
        );
    }
    let dead_ticks: Vec<_> = (first_dead..=last_dead).collect();
    for (tick, command) in received_on(&server, &dead_ticks) {
        assert!(
            neutral(&command),
            "the server received {command:?} for tick {tick} while dead"
        );
    }
}

/// Alive, on frames of several ticks: a throttle changed between frames is filed, restored and
/// received at its new value on every tick of the frame it changes in, and a click gathered in a
/// frame is filed on exactly one of its ticks and reaches the server attested once.
#[test]
fn every_tick_of_a_frame_files_the_devices_command() {
    let _udp = lock_real_udp_test();
    let (mut server, mut client, _) = connected(TICKS_PER_FRAME);
    for _ in 0..PHASE_FRAMES {
        step(&mut server, &mut client);
    }

    let mut keys = client.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    keys.release(KeyCode::KeyW);
    keys.press(KeyCode::KeyS);
    let reversed = step(&mut server, &mut client);
    assert!(
        reversed.len() > 1,
        "the reversing frame runs several ticks: {reversed:?}"
    );
    for (tick, command) in &reversed {
        assert_eq!(command.throttle, -1.0, "tick {tick} filed {command:?}");
    }

    client
        .world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    let clicked = step(&mut server, &mut client);
    client
        .world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .clear_just_pressed(MouseButton::Left);
    let filed_clicks = clicked
        .iter()
        .filter(|(_, command)| command.fire_primary)
        .count();
    assert_eq!(
        filed_clicks, 1,
        "the click is filed on one tick of its frame: {clicked:?}"
    );

    for _ in 0..PHASE_FRAMES {
        step(&mut server, &mut client);
    }
    for (tick, command) in restored_on(&client, &ticks(&reversed)) {
        assert_eq!(command.throttle, -1.0, "tick {tick} restored {command:?}");
    }
    for (tick, command) in received_on(&server, &ticks(&reversed)) {
        assert_eq!(
            command.throttle, -1.0,
            "the server received {command:?} for tick {tick}"
        );
    }
    assert_eq!(
        received_clicks(&server),
        1,
        "the server receives the click attested on one tick"
    );
}

/// On frames of one tick, where every tick is a frame's last: the click the bridge restores, on
/// the last tick of its frame, is consumed with that tick, so no later frame's device command
/// carries it, and the server receives it attested once across the run.
#[test]
fn a_restored_click_is_never_filed_again() {
    let _udp = lock_real_udp_test();
    let (mut server, mut client, _) = connected(1);
    for _ in 0..PHASE_FRAMES {
        step(&mut server, &mut client);
    }

    client
        .world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    let clicked = step(&mut server, &mut client);
    client
        .world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .clear_just_pressed(MouseButton::Left);
    let [(click_tick, click)] = clicked[..] else {
        panic!("a frame of one tick files one command: {clicked:?}");
    };
    assert!(click.fire_primary, "the click's frame files it: {click:?}");

    let delay = u32::from(SHIPPING_INPUT_DELAY_TICKS);
    let mut later = Vec::new();
    for _ in 0..2 * delay + PHASE_FRAMES {
        later.extend(step(&mut server, &mut client));
    }
    let [(_, restored)] = restored_on(&client, &[click_tick])[..] else {
        unreachable!("one tick asked, one restored");
    };
    assert!(
        restored.fire_primary,
        "the bridge restores the click on its tick, {click_tick}: {restored:?}"
    );
    assert!(
        later.iter().all(|(_, command)| !command.fire_primary),
        "no frame after the click's files it again: {later:?}"
    );
    assert_eq!(
        received_clicks(&server),
        1,
        "the server receives the click attested on one tick"
    );
}
