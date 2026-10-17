//! Real-UDP probe of a dead tank's input path through lightyear's native inputs and the input
//! bridge: from the frame the replicated knockout arrives, every tick files the death flow's
//! neutral command and the server receives nothing else; once the input delay has drained the
//! bridge restores nothing else either; and from the frame a live status arrives the device's
//! command is filed again, on that frame's first tick.

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
use crate::command::{AimIntent, CrewSwap, TankCommand, TankDeviceSet, gather_commands};
use crate::damage::CrewStation;
use crate::tank::Controlled;

type TankInputMessage = InputMessage<NativeStateSequence<TankCommand>>;

/// Fixed ticks per client frame: every frame catches up several ticks (fixture data).
const TICKS_PER_FRAME: u32 = 3;
const CONNECT_STEPS: u32 = 2_000;
/// Frames of each phase once the input path is up: more than twice the input delay in ticks
/// (fixture data).
const PHASE_FRAMES: u32 = 4;

/// What the client filed for each tick, `(for_tick, command)`, in filing order.
#[derive(Resource, Default)]
struct Filed(Vec<(u32, TankCommand)>);

/// What the bridge handed the sim on each tick, `(tick, command)`.
#[derive(Resource, Default)]
struct Restored(Vec<(u32, TankCommand)>);

/// The fresh state of every input message the server received, `(end_tick, command)`; `None` when
/// that state is absent.
#[derive(Resource, Default)]
struct Received(Vec<(u32, Option<TankCommand>)>);

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

/// Record each message's fresh state and drop the message, so no target resolution runs.
fn record_received(
    mut receivers: Query<&mut MessageReceiver<TankInputMessage>>,
    mut received: ResMut<Received>,
) {
    for mut receiver in &mut receivers {
        receiver.retain_messages(|message| {
            for target in &message.inputs {
                let mut fresh = None;
                for state in target.states.clone().get_snapshots_from_message(TICK) {
                    match state {
                        Compressed::Input(ActionState(command)) => fresh = Some(command),
                        Compressed::Absent => fresh = None,
                        Compressed::SameAsPrecedent => {}
                    }
                }
                received.0.push((message.end_tick.0, fresh));
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

/// The windowed client's device path on the wire: the gather, the death flow's claim, the feed and
/// the bridge, for a controlled tank that carries its replicated status. The client and the tank.
fn build_client(port: u16) -> (App, Entity) {
    let mut app = base_app();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(TICK * TICKS_PER_FRAME));
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
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
        .add_systems(FixedUpdate, record_restored.after(InputBridge));
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

/// One client frame of [`TICKS_PER_FRAME`] ticks, and the server's ticks that keep pace with it.
/// The commands the client filed in the frame.
fn step(server: &mut App, client: &mut App) -> Vec<(u32, TankCommand)> {
    for _ in 0..TICKS_PER_FRAME {
        server.update();
    }
    std::thread::sleep(std::time::Duration::from_micros(300));
    let before = client.world().resource::<Filed>().0.len();
    client.update();
    std::thread::sleep(std::time::Duration::from_micros(300));
    client.world().resource::<Filed>().0[before..].to_vec()
}

fn neutral(command: &TankCommand) -> bool {
    TankCommand {
        for_tick: 0,
        ..*command
    } == TankCommand::default()
}

fn live(command: &TankCommand) -> bool {
    command.throttle == 1.0
        && command.steer == 1.0
        && command.fire_secondary
        && command.aim.is_some()
        && command.crew_swap.is_some()
}

/// Drive, fire, aim and the crew bar held through a dead life, on frames of several ticks: from
/// the frame the knockout status arrives, every tick files the neutral command and the server
/// receives only it; once the commands filed before that frame have drained the input delay, the
/// bridge restores only it. From the frame a live status arrives, its first tick files the
/// device's command.
#[test]
fn a_dead_tank_files_only_the_neutral_command() {
    let _udp = lock_real_udp_test();
    let port = free_port();
    let mut server = build_server(port);
    let (mut client, tank) = build_client(port);
    finish(&mut server);
    finish(&mut client);
    let mut keys = client.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    for key in [KeyCode::KeyW, KeyCode::KeyD, KeyCode::Space] {
        keys.press(key);
    }
    let delay = usize::from(SHIPPING_INPUT_DELAY_TICKS);

    let mut up = false;
    for _ in 0..CONNECT_STEPS {
        step(&mut server, &mut client);
        if client.world().resource::<Filed>().0.len() > 2 * delay
            && !server.world().resource::<Received>().0.is_empty()
        {
            up = true;
            break;
        }
    }
    assert!(
        up,
        "the client never filed input within {CONNECT_STEPS} frames"
    );
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

    // A live status: the device's command from the frame's first tick.
    client
        .world_mut()
        .entity_mut(tank)
        .insert(NetTankStatus::Active);
    let respawned = step(&mut server, &mut client);
    let (tick, command) = respawned
        .first()
        .expect("the live status's frame files a command");
    assert!(
        live(command),
        "the new life's first tick {tick} filed {command:?}"
    );

    for _ in 0..PHASE_FRAMES {
        step(&mut server, &mut client);
    }
    let received: Vec<_> = server
        .world()
        .resource::<Received>()
        .0
        .iter()
        .filter(|(end, _)| (first_dead..=last_dead).contains(end))
        .copied()
        .collect();
    assert!(
        !received.is_empty(),
        "the dead life's input reached the server"
    );
    for (end, fresh) in &received {
        let command = fresh.expect("a message carries its own tick's command");
        assert!(
            neutral(&command),
            "the server received {command:?} for tick {end} while dead"
        );
    }
}
