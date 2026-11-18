//! The game's fire-control and damage-model input proofs concern game state only.
//! The UDP fixture mounts the shared protocol and the shipped command and firing plugins.

use super::super::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};
use super::*;
use crate::tank::{TankRoot, Weapon, WeaponLoads};
use bevy::time::TimeUpdateStrategy;
use core::time::Duration;
use lightyear::link::{LinkReceiveSystems, LinkSystems};
use lightyear::prelude::client::{ClientPlugins, Connect, NetcodeClient, NetcodeConfig};
use lightyear::prelude::server::{
    NetcodeConfig as ServerConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
};
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};

/// Fixture conditioner on either receiving link: loss is opaque datagram loss in one direction.
#[derive(Component)]
struct Loss {
    blackout: bool,
    rate: f32,
    state: u64,
    dropped: usize,
}
impl Default for Loss {
    fn default() -> Self {
        Self {
            blackout: false,
            rate: 0.0,
            state: 0x1452_abcd,
            dropped: 0,
        }
    }
}
fn condition(mut links: Query<(&mut Link, &mut Loss)>) {
    for (mut link, mut loss) in &mut links {
        let packets: Vec<_> = link.recv.drain().collect();
        for packet in packets {
            loss.state = loss
                .state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let draw = (loss.state >> 40) as f32 / (1u32 << 24) as f32;
            if loss.blackout || draw < loss.rate {
                loss.dropped += 1;
            } else {
                link.recv.push_raw(packet);
            }
        }
    }
}
fn install_conditioner(app: &mut App) {
    app.add_systems(
        PreUpdate,
        condition
            .in_set(LinkSystems::Receive)
            .after(LinkReceiveSystems::ApplyConditioner),
    );
}
#[derive(Resource, Default)]
struct Observations {
    shots: usize,
    held: Vec<bool>,
    selects: usize,
}
fn observe_fire(_: On<crate::ballistics::FireShell>, mut observations: ResMut<Observations>) {
    observations.shots += 1;
}
fn observe_command(
    tanks: Query<&TankCommand, With<ControlledBy>>,
    mut observations: ResMut<Observations>,
) {
    for command in &tanks {
        observations.held.push(command.fire_secondary);
        observations.selects += usize::from(command.select_round.is_some());
    }
}
struct Pair {
    server: App,
    client: App,
    up: Entity,
    down: Entity,
    tank: Entity,
}
impl Pair {
    fn new() -> Self {
        let port = free_port();
        let mut server = base_app();
        server.add_plugins(ServerPlugins {
            tick_duration: TICK,
        });
        server.add_plugins((
            super::super::protocol::plugin,
            crate::state::sim_plugin,
            crate::command::core_plugin,
            crate::shooting::plugin,
        ));
        server
            .init_resource::<Observations>()
            .add_observer(observe_fire)
            .add_observer(super::super::server::attach_replication_sender)
            .add_systems(
                FixedUpdate,
                observe_command
                    .after(InputBridge)
                    .before(crate::command::ConsumeCommandEdges),
            );
        server
            .world_mut()
            .spawn(super::super::battle::BattleState::combat_fixture());
        server
            .world_mut()
            .resource_mut::<NextState<crate::state::AppState>>()
            .set(crate::state::AppState::Playing);
        install_conditioner(&mut server);
        let endpoint = server
            .world_mut()
            .spawn((
                NetcodeServer::new(ServerConfig {
                    protocol_id: super::super::protocol::protocol_id(),
                    private_key: [0; 32],
                    ..default()
                }),
                LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)),
                ServerUdpIo,
            ))
            .id();
        server
            .world_mut()
            .commands()
            .trigger(Start { entity: endpoint });
        let mut client = base_app();
        client.add_plugins(ClientPlugins {
            tick_duration: TICK,
        });
        client.add_plugins(super::super::protocol::plugin);
        client.insert_resource(crate::ClientReplica);
        install_conditioner(&mut client);
        let address = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
        let down = client
            .world_mut()
            .spawn((
                Client,
                ReplicationReceiver,
                NetcodeClient::new(
                    Authentication::Manual {
                        server_addr: address,
                        client_id: 1,
                        private_key: [0; 32],
                        protocol_id: super::super::protocol::protocol_id(),
                    },
                    NetcodeConfig::default(),
                )
                .unwrap(),
                LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)),
                PeerAddr(address),
                UdpIo::default(),
                Loss::default(),
            ))
            .id();
        client
            .world_mut()
            .commands()
            .trigger(Connect { entity: down });
        let registry_names = |app: &App| {
            let registry = app.world().resource::<MessageRegistry>();
            (0..)
                .map_while(|id| {
                    registry
                        .kind_map
                        .kind(id)
                        .and_then(|kind| registry.kind_map.name(kind))
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            registry_names(&server),
            registry_names(&client),
            "both endpoints must assign identical message ids"
        );
        finish(&mut server);
        finish(&mut client);
        let up = (0..600)
            .find_map(|_| {
                server.update();
                client.update();
                std::thread::sleep(Duration::from_millis(1));
                server
                    .world_mut()
                    .query_filtered::<Entity, (With<ClientOf>, With<Connected>)>()
                    .iter(server.world())
                    .next()
            })
            .expect("the real UDP client connects");
        server.world_mut().entity_mut(up).insert(Loss::default());
        // Fixture: one-tick reload makes a replayed edge observable as a second round.
        let tank = crate::shooting::spawn_bot_gate_fixture(
            server.world_mut(),
            crate::spec::FireCycle::Single {
                reload: crate::sim_tick::TickSpan::ONE,
            },
        );
        server.world_mut().entity_mut(tank).insert(ControlledBy {
            owner: up,
            lifetime: default(),
        });
        let mut pair = Self {
            server,
            client,
            up,
            down,
            tank,
        };
        pair.steps(12);
        *pair.server.world_mut().resource_mut::<Observations>() = default();
        pair
    }
    fn step(&mut self) {
        self.client.update();
        std::thread::sleep(Duration::from_millis(1));
        self.server.update();
    }
    fn deliver_one_client_frame(&mut self) {
        self.client.update();
        let sent = self.state().seq;
        // This case promises delivery of the fresh first copy. Do not replace it with a
        // later resend before the receiver has polled the real socket.
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(1));
            self.server.update();
            if self.server_input().answer.seq == sent {
                return;
            }
        }
        panic!("the unconditioned recovery frame must arrive over loopback UDP");
    }
    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }
    fn click(&mut self) {
        self.client
            .world_mut()
            .resource_mut::<FrameInput>()
            .command
            .fire_primary = true;
    }
    fn blackout(&mut self, up: bool, enabled: bool) {
        if up {
            self.server
                .world_mut()
                .get_mut::<Loss>(self.up)
                .unwrap()
                .blackout = enabled;
        } else {
            self.client
                .world_mut()
                .get_mut::<Loss>(self.down)
                .unwrap()
                .blackout = enabled;
        }
    }
    fn over_ceiling(&self) -> usize {
        (Timing::shipped().stale_ceil_s / TICK.as_secs_f64()).ceil() as usize + 3
    }
    fn shots(&self) -> usize {
        self.server.world().resource::<Observations>().shots
    }
    fn state(&self) -> &ClientInput {
        self.client.world().resource::<ClientInput>()
    }
    fn server_input(&self) -> &ArrivalInput {
        self.server.world().get::<ArrivalInput>(self.up).unwrap()
    }
    fn automatic_route(&mut self) {
        let mut query = self.server.world_mut().query::<(&TankRoot, &mut Weapon)>();
        for (root, mut weapon) in query.iter_mut(self.server.world_mut()) {
            if root.0 == self.tank {
                weapon.trigger = crate::spec::Trigger::Secondary;
            }
        }
        self.client
            .world_mut()
            .resource_mut::<FrameInput>()
            .command
            .fire_secondary = true;
        self.steps(6);
    }
}

/// A click polled after a client frame freeze survives regardless of how many fixed ticks ran.
#[test]
fn stalled_frame_plus_click_applies_exactly_one_shot() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    let stall_ticks = pair.over_ceiling();
    for _ in 0..stall_ticks {
        pair.server.update();
    }
    assert!(
        pair.server
            .world()
            .get::<InputGap>(pair.up)
            .unwrap()
            .stale()
    );
    pair.click();
    pair.client
        .insert_resource(TimeUpdateStrategy::ManualDuration(
            TICK * stall_ticks as u32,
        ));
    pair.deliver_one_client_frame();
    pair.client
        .insert_resource(TimeUpdateStrategy::ManualDuration(TICK));
    pair.steps(40);
    assert_eq!(pair.shots(), 1);
    assert_eq!(pair.state().answer.resolved, 1);
}

/// Resending unanswered ids survives opaque upstream datagram loss without replaying a shot.
#[test]
fn thirty_percent_uplink_loss_applies_one_click_exactly_once() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.server
        .world_mut()
        .get_mut::<Loss>(pair.up)
        .unwrap()
        .rate = 0.30; // Fixture loss.
    pair.click();
    pair.steps(100);
    assert!(pair.server.world().get::<Loss>(pair.up).unwrap().dropped > 0);
    assert_eq!(pair.shots(), 1);
    assert_eq!(pair.state().answer.resolved, 1);
}

/// A recovery copy cannot surprise the player with a start from before an upstream outage.
#[test]
fn uplink_blackout_refuses_an_old_click_and_answers_its_fate() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.blackout(true, true);
    pair.click();
    let budget = Timing::shipped().recovery_fresh_copies;
    pair.steps(pair.over_ceiling().max(usize::from(budget)));
    assert_eq!(pair.state().pending.front().unwrap().copies, budget);
    assert!(pair.server.world().get::<Loss>(pair.up).unwrap().dropped > 0);
    pair.blackout(true, false);
    pair.steps(20);
    assert_eq!(pair.shots(), 0);
    assert_eq!(pair.state().refused, 1);
    assert!(pair.state().pending.is_empty());
}

/// No clock can distinguish a new recovery-frame press from a press polled after a freeze.
#[test]
fn uplink_recovery_applies_a_fresh_click() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.blackout(true, true);
    pair.steps(pair.over_ceiling());
    pair.blackout(true, false);
    pair.click();
    pair.deliver_one_client_frame();
    pair.steps(20);
    assert_eq!(
        pair.shots(),
        1,
        "answer {:?}, resolved {}, refused {}",
        pair.state().answer,
        pair.state().answer.resolved,
        pair.state().refused
    );
    assert_eq!(pair.state().refused, 0);
}

/// Downstream loss delays knowledge only; the client cannot void a server-applied click.
#[test]
fn downlink_blackout_never_refuses_an_applied_click() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.blackout(false, true);
    pair.click();
    pair.steps((Timing::shipped().link_warning_s / TICK.as_secs_f64()).ceil() as usize + 3);
    assert_eq!(pair.shots(), 1);
    assert_eq!(pair.state().refused, 0);
    assert!(!pair.state().pending.is_empty());
    assert!(pair.state().warning);
    pair.blackout(false, false);
    pair.steps(20);
    assert_eq!(pair.shots(), 1);
    assert_eq!(pair.state().answer.resolved, 1);
    assert_eq!(pair.state().refused, 0);
    assert!(!pair.state().warning);
}

/// The same upstream stale state releases fire; answered bits and pending edges resume it.
#[test]
fn held_trigger_stops_by_stale_threshold_and_resumes_without_a_physical_press() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.automatic_route();
    assert!(
        pair.shots() > 0,
        "fixture: the server was firing before silence"
    );
    let threshold = pair
        .server
        .world()
        .get::<InputGap>(pair.up)
        .unwrap()
        .threshold(TICK.as_secs_f64(), &Timing::shipped());
    pair.blackout(true, true);
    let mut stop_ticks = 0;
    while pair.server_input().answer.fire_secondary {
        pair.step();
        stop_ticks += 1;
        assert!(
            stop_ticks as f64 * TICK.as_secs_f64() <= threshold + TICK.as_secs_f64(),
            "held fire must release by S + T"
        );
    }
    pair.steps(pair.over_ceiling());
    assert!(!pair.server_input().answer.fire_secondary);
    let stopped = pair.shots();
    pair.steps(4);
    assert_eq!(pair.shots(), stopped);
    pair.blackout(true, false);
    pair.steps(20);
    assert!(pair.shots() > stopped);
    assert!(pair.server_input().answer.fire_secondary);
}

/// A lost withdrawal is never reconstructed as another press on recovery.
#[test]
fn a_release_lost_in_blackout_stays_released_after_recovery() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.automatic_route();
    pair.blackout(true, true);
    pair.client
        .world_mut()
        .resource_mut::<FrameInput>()
        .command
        .fire_secondary = false;
    pair.steps(pair.over_ceiling());
    let stopped = pair.shots();
    pair.blackout(true, false);
    pair.steps(20);
    assert_eq!(pair.shots(), stopped);
    assert!(!pair.server_input().answer.fire_secondary);
}

/// A sub-frame tap occupies one trigger command slot, followed by its withdrawal next tick.
#[test]
fn a_tap_inside_one_frame_holds_the_trigger_for_exactly_one_tick() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    for (_, mut weapon) in pair
        .server
        .world_mut()
        .query::<(&TankRoot, &mut Weapon)>()
        .iter_mut(pair.server.world_mut())
    {
        weapon.trigger = crate::spec::Trigger::Secondary;
    }
    pair.client
        .world_mut()
        .resource_mut::<FrameInput>()
        .secondary_cycle = true;
    pair.steps(20);
    assert_eq!(
        pair.server
            .world()
            .resource::<Observations>()
            .held
            .iter()
            .filter(|&&held| held)
            .count(),
        1
    );
    assert_eq!(pair.shots(), 1);
}

/// Socket input queued across server lateness resets silence before the fixed consumers run.
#[test]
fn a_two_hundred_ms_server_stall_does_not_declare_a_live_client_stale() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.automatic_route();
    pair.client.update();
    std::thread::sleep(Duration::from_millis(2));
    pair.server
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            200,
        ))); // Fixture stall.
    pair.server.update();
    assert!(
        !pair
            .server
            .world()
            .get::<InputGap>(pair.up)
            .unwrap()
            .stale()
    );
    assert!(pair.server_input().answer.fire_secondary);
    pair.server
        .insert_resource(TimeUpdateStrategy::ManualDuration(TICK));
}

/// Relative selection steps are requests, so replay and coalescing would both change the result.
#[test]
fn three_round_select_steps_under_loss_apply_exactly_three_steps() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.server
        .world_mut()
        .get_mut::<Loss>(pair.up)
        .unwrap()
        .rate = 0.30;
    let now = pair
        .client
        .world()
        .resource::<Time<Real>>()
        .elapsed_secs_f64();
    for _ in 0..3 {
        pair.client.world_mut().resource_mut::<ClientInput>().issue(
            InputEdge::Select(RoundSelect {
                weapon: 0,
                step: crate::command::RoundStep::Next,
            }),
            now,
        );
    }
    let before = pair
        .server
        .world()
        .get::<WeaponLoads>(pair.tank)
        .unwrap()
        .weapons[0]
        .selected;
    pair.steps(100);
    assert_eq!(pair.server.world().resource::<Observations>().selects, 3);
    let count = pair
        .server
        .world_mut()
        .query::<&Weapon>()
        .iter(pair.server.world())
        .next()
        .unwrap()
        .rounds
        .len();
    assert_eq!(
        usize::from(
            pair.server
                .world()
                .get::<WeaponLoads>(pair.tank)
                .unwrap()
                .weapons[0]
                .selected
        ),
        (usize::from(before) + 3) % count
    );
    assert_eq!(pair.state().answer.resolved, 3);
}

/// Malformed outside input leaves the last validated levels intact and records a typed refusal.
#[test]
fn malformed_message_is_refused_with_levels_unchanged() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    let before = pair.server_input().levels.command();
    let message = TankInput {
        seq: pair.state().seq.wrapping_add(1),
        levels: InputLevels {
            throttle: f32::NAN,
            ..default()
        },
        first_id: 1,
        edges: vec![InputEdge::Fire],
        fresh_from: 0,
    };
    pair.client
        .world_mut()
        .get_mut::<MessageSender<TankInput>>(pair.down)
        .unwrap()
        .send::<TankInputChannel>(message);
    // Sending at the final send set avoids replacing the probe with the client's ordinary frame.
    pair.client
        .world_mut()
        .resource_mut::<ClientInput>()
        .last_send_s = Some(f64::MAX);
    pair.steps(3);
    assert_eq!(
        pair.server_input().last_refusal,
        Some(InputError::Levels(CommandError::Throttle))
    );
    assert_eq!(pair.server_input().levels.command(), before);
    assert_eq!(pair.shots(), 0);
}

/// Decision ordering must not acknowledge a queued accepted edge as if it had already applied.
#[test]
fn refusals_never_acknowledge_past_an_accepted_edge_waiting_for_its_slot() {
    let mut state = ArrivalInput::default();
    state
        .accept(
            TankInput {
                seq: 1,
                levels: default(),
                first_id: 1,
                edges: vec![InputEdge::Fire, InputEdge::Fire],
                fresh_from: 0,
            },
            false,
        )
        .unwrap();
    assert!(state.command(false, false).fire_primary);
    state
        .accept(
            TankInput {
                seq: 2,
                levels: default(),
                first_id: 1,
                edges: vec![
                    InputEdge::Fire,
                    InputEdge::Fire,
                    InputEdge::Repair,
                    InputEdge::TriggerUp,
                ],
                fresh_from: 4,
            },
            true,
        )
        .unwrap();
    assert_eq!(state.answer.resolved, 1);
    assert!(state.command(false, false).fire_primary);
    state.command(false, false);
    assert_eq!(state.answer.resolved, 4);
    assert_eq!(state.answer.refused_mask, 2);
}

/// Wire sizes are observations of postcard, not bandwidth or transport-header obligations.
#[test]
fn measure_input_and_answer_payload_sizes() {
    let mut client = ClientInput::default();
    client.issue(InputEdge::Fire, 0.0);
    let message = client.message(
        InputLevels {
            throttle: 1.0,
            steer: 0.0,
            aim: Some(AimIntent::HullLocal(Vec3::NEG_Z)),
            range: 100.0,
        },
        0.0,
        Timing::shipped().recovery_fresh_copies,
    );
    println!(
        "MEASURED postcard bytes: TankInput click={}, idle={}, InputAnswer={} (payload only)",
        postcard::to_allocvec(&message).unwrap().len(),
        postcard::to_allocvec(&TankInput {
            edges: vec![],
            ..message
        })
        .unwrap()
        .len(),
        postcard::to_allocvec(&InputAnswer::default())
            .unwrap()
            .len()
    );
}

/// The loopback measurement uses wall time outside the client; no fate timer is introduced.
#[test]
fn measure_click_to_answer_over_real_udp() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    let pressed = std::time::Instant::now();
    pair.click();
    for _ in 0..100 {
        pair.step();
        if pair.state().answer.resolved == 1 {
            break;
        }
    }
    assert_eq!(pair.state().answer.resolved, 1);
    println!(
        "MEASURED real-UDP click-to-answer wall_ms={:.3}",
        pressed.elapsed().as_secs_f64() * 1e3
    );
}

/// Keepalives can keep a transport live after tank input stops. Drive holds at stale, then brakes.
#[test]
fn movement_holds_at_stale_and_brakes_at_input_loss_while_keepalives_continue() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.client
        .world_mut()
        .resource_mut::<FrameInput>()
        .command
        .throttle = 1.0;
    pair.steps(4);
    pair.client
        .world_mut()
        .resource_mut::<ClientInput>()
        .last_send_s = Some(f64::MAX);
    pair.steps(pair.over_ceiling());
    assert_eq!(
        pair.server
            .world()
            .get::<TankCommand>(pair.tank)
            .unwrap()
            .throttle,
        1.0
    );
    let frames = (super::super::server::connection_timeout().as_secs_f64() / TICK.as_secs_f64())
        .ceil() as usize
        + 4;
    pair.steps(frames);
    assert!(
        pair.server.world().get::<Connected>(pair.up).is_some(),
        "fixture: authenticated keepalives continue"
    );
    assert!(
        pair.server
            .world()
            .get::<ParsedTankCommand>(pair.tank)
            .unwrap()
            .held()
    );
}

/// A saturated unanswered window cannot prevent releasing a trigger or cancelling a swap.
#[test]
fn the_bounded_pending_window_reserves_withdrawals_and_reports_full_presses() {
    let mut state = ClientInput::default();
    for _ in 0..PENDING - WITHDRAWAL_RESERVE {
        assert!(state.issue(InputEdge::Fire, 0.0));
    }
    assert!(!state.issue(InputEdge::Repair, 0.0));
    assert_eq!(state.refused, 1);
    for edge in [InputEdge::TriggerUp, InputEdge::Swap(CrewSwap::Cancel)] {
        assert!(state.issue(edge, 0.0));
        assert!(
            state.issue(edge, 0.0),
            "a duplicate withdrawal is covered by its pending id"
        );
    }
    assert_eq!(state.pending.len(), PENDING);
}

/// The answer mask and ids are wire forms, so wrapping cannot repeat or mislabel a request.
#[test]
fn wrapped_ids_keep_each_refusal_and_each_application_exact() {
    let mut client = ClientInput {
        issued: u16::MAX - 1,
        answer: InputAnswer {
            resolved: u16::MAX - 1,
            ..default()
        },
        ..default()
    };
    let mut server = ArrivalInput {
        answer: client.answer,
        ..default()
    };
    client.issue(InputEdge::Repair, 0.0);
    client.issue(InputEdge::Fire, 0.0);
    let budget = Timing::shipped().recovery_fresh_copies;
    for copy in 0..budget {
        let _lost = client.message(default(), f64::from(copy), budget);
    }
    let now = f64::from(budget);
    let resent = client.message(default(), now, budget);
    server.accept(resent, true).unwrap();
    server.command(false, false);
    assert_eq!(server.answer.resolved, 0);
    assert_eq!(server.answer.refused_mask, 3);
    client.receive(server.answer, now).unwrap();
    assert_eq!(client.refused, 2);
    assert!(client.pending.is_empty());
    client.issue(InputEdge::Fire, now);
    server
        .accept(client.message(default(), now, budget), false)
        .unwrap();
    assert!(server.command(false, false).fire_primary);
    client.receive(server.answer, now + 1.0).unwrap();
    assert_eq!(client.answer.resolved, client.issued);
}

/// Each malformed shape is rejected before it can replace previously checked levels.
#[test]
fn malformed_shapes_have_typed_refusals_before_any_mutation() {
    let mut server = ArrivalInput::default();
    let base = TankInput {
        seq: 1,
        levels: default(),
        first_id: 1,
        edges: vec![],
        fresh_from: 0,
    };
    for (input, error) in [
        (
            TankInput {
                edges: vec![InputEdge::Fire; PENDING + 1],
                ..base.clone()
            },
            InputError::TooManyEdges(PENDING + 1),
        ),
        (
            TankInput {
                first_id: 2,
                ..base.clone()
            },
            InputError::SkippedId { first: 2, next: 1 },
        ),
        (
            TankInput {
                fresh_from: 1,
                ..base
            },
            InputError::FreshIndex(1),
        ),
    ] {
        assert_eq!(server.accept(input, false), Err(error));
        assert_eq!(server.answer, InputAnswer::default());
        assert!(server.queue.is_empty());
    }
}

#[test]
fn measure_maximum_bounded_payload_sizes() {
    let input = TankInput {
        seq: u16::MAX,
        first_id: u16::MAX,
        levels: InputLevels {
            aim: Some(AimIntent::HullLocal(Vec3::NEG_Z)),
            ..default()
        },
        edges: vec![
            InputEdge::Swap(CrewSwap::Start(
                crate::damage::CrewStation::Driver,
                crate::damage::CrewStation::Gunner
            ));
            PENDING
        ],
        fresh_from: PENDING as u8,
    };
    let answer = InputAnswer {
        seq: u16::MAX,
        resolved: u16::MAX,
        refused_mask: u16::MAX,
        fire_secondary: true,
    };
    println!(
        "MEASURED maximum postcard payload bytes: TankInput={}, InputAnswer={}",
        postcard::to_allocvec(&input).unwrap().len(),
        postcard::to_allocvec(&answer).unwrap().len()
    );
}

/// An outside answer cannot resolve unissued ids or shift the fate mask outside its wire width.
#[test]
fn malformed_answers_leave_pending_fates_undecided() {
    let mut client = ClientInput::default();
    client.issue(InputEdge::Fire, 0.0);
    for resolved in [2, 1 << (u16::BITS - 1)] {
        assert_eq!(
            client.receive(
                InputAnswer {
                    resolved,
                    ..default()
                },
                1.0
            ),
            Err(InputError::AnswerBeyondIssued(resolved))
        );
        assert_eq!(client.pending.len(), 1);
        assert_eq!(client.answer.resolved, 0);
    }
    client
        .receive(
            InputAnswer {
                resolved: 1,
                ..default()
            },
            1.0,
        )
        .unwrap();
    assert_eq!(client.answer.resolved, client.issued);
}

/// A refused trigger tap gets feedback; a held button uses the single re-press rule silently.
#[test]
fn trigger_refusal_cues_only_after_the_physical_button_is_released() {
    for held in [false, true] {
        let mut client = ClientInput {
            button: held,
            ..default()
        };
        client.issue(InputEdge::TriggerDown, 0.0);
        client
            .receive(
                InputAnswer {
                    resolved: 1,
                    refused_mask: 1,
                    ..default()
                },
                1.0,
            )
            .unwrap();
        assert_eq!(client.refused, u64::from(!held));
    }
}

/// Teardown may remove role markers before Connected; the next link must inherit no request ids.
#[test]
fn despawning_a_connection_resets_its_pending_edges_and_frame_state() {
    let mut app = App::new();
    app.init_resource::<ClientInput>()
        .init_resource::<FrameInput>()
        .add_observer(reset);
    app.world_mut()
        .resource_mut::<ClientInput>()
        .issue(InputEdge::Fire, 0.0);
    app.world_mut()
        .resource_mut::<FrameInput>()
        .command
        .fire_primary = true;
    let link = app
        .world_mut()
        .spawn((RemoteId(PeerId::Netcode(1)), Connected))
        .id();
    app.world_mut().despawn(link);
    assert!(app.world().resource::<ClientInput>().pending.is_empty());
    assert_eq!(app.world().resource::<ClientInput>().issued, 0);
    assert_eq!(
        app.world().resource::<FrameInput>().command,
        TankCommand::default()
    );
}

/// Each player relies on the bridge using its owner link, while bots keep their own producer.
#[test]
fn the_bridge_applies_only_the_controlling_links_command_and_holds_disconnected_owners() {
    use bevy::ecs::system::RunSystemOnce;
    let mut world = World::new();
    let mut roots = Vec::new();
    // Fixture absolute levels distinguish the two owners without depending on vehicle data.
    for (peer, throttle) in [(1, 0.25), (2, -0.75)] {
        let parsed = ParsedTankCommand::try_from(TankCommand {
            throttle,
            ..default()
        })
        .unwrap();
        let link = world
            .spawn((
                RemoteId(PeerId::Netcode(peer)),
                Connected,
                ArrivalInput {
                    staged: parsed,
                    ..default()
                },
            ))
            .id();
        let root = world
            .spawn((
                TankCommand::default(),
                ControlledBy {
                    owner: link,
                    lifetime: default(),
                },
            ))
            .id();
        roots.push((root, link, throttle));
    }
    let bot = world
        .spawn(TankCommand {
            throttle: 1.0,
            ..default()
        })
        .id();
    world.run_system_once(apply_inputs).unwrap();
    for &(root, _, throttle) in &roots {
        assert_eq!(world.get::<TankCommand>(root).unwrap().throttle, throttle);
    }
    assert_eq!(world.get::<TankCommand>(bot).unwrap().throttle, 1.0);
    world.entity_mut(roots[0].1).remove::<Connected>();
    world.run_system_once(apply_inputs).unwrap();
    assert!(world.get::<ParsedTankCommand>(roots[0].0).unwrap().held());
    assert_eq!(
        world.get::<TankCommand>(roots[1].0).unwrap().throttle,
        roots[1].2
    );
}

/// Fire-control players rely on the last eligible recovery copy surviving earlier lost or
/// superseded copies. A new edge bypasses the tick send gate when the budget permits a resend.
#[test]
fn a_recovery_press_survives_the_authored_lost_or_superseded_copy_budget() {
    let _guard = lock_real_udp_test();
    let budget = Timing::shipped().recovery_fresh_copies;
    for lost in [false, true] {
        let mut pair = Pair::new();
        for _ in 0..pair.over_ceiling() {
            pair.server.update();
        }
        pair.click();
        for _ in 1..budget {
            pair.client.update();
            if lost {
                pair.blackout(true, true);
                let before = pair.server.world().get::<Loss>(pair.up).unwrap().dropped;
                for _ in 0..100 {
                    std::thread::sleep(Duration::from_millis(1));
                    pair.server.update();
                    if pair.server.world().get::<Loss>(pair.up).unwrap().dropped > before {
                        break;
                    }
                }
                assert!(pair.server.world().get::<Loss>(pair.up).unwrap().dropped > before);
                pair.blackout(true, false);
            }
        }
        assert_eq!(
            pair.state().pending.front().map_or(0, |edge| edge.copies),
            budget - 1
        );
        if budget > 1 {
            pair.client
                .insert_resource(TimeUpdateStrategy::ManualDuration(TICK / 4));
            pair.client
                .world_mut()
                .resource_mut::<FrameInput>()
                .command
                .select_round = Some(RoundSelect {
                weapon: 0,
                step: crate::command::RoundStep::Next,
            });
        }
        let previous = pair.state().seq;
        pair.client.update();
        let eligible = pair.state().seq;
        assert_eq!(
            eligible,
            previous.wrapping_add(1),
            "a fresh edge sends immediately, including between ticks"
        );
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(1));
            pair.server.update();
            if pair.server_input().answer.seq == eligible {
                break;
            }
        }
        let selects = usize::from(budget > 1);
        assert_eq!(pair.server_input().answer.seq, eligible);
        assert_eq!(pair.server_input().answer.resolved, 1 + selects as u16);
        assert_eq!(pair.server_input().answer.refused_mask, 0);
        assert_eq!(pair.shots(), 1);
        assert_eq!(
            pair.server.world().resource::<Observations>().selects,
            selects
        );
    }
}

/// Malformed outside input cannot sustain fire or drive forever: players rely on the same
/// stale release and lost-input brake as silence, while authenticated keepalives still arrive.
#[test]
fn repeated_malformed_input_goes_stale_then_lost_and_valid_input_clears_the_log_cause() {
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.automatic_route();
    pair.client
        .world_mut()
        .resource_mut::<FrameInput>()
        .command
        .throttle = 1.0;
    pair.steps(4);
    // The peer's normal cumulative sender carries malformed levels and a real release edge.
    // Every copy must be refused, including the withdrawal, without sustaining the live link.
    pair.client.world_mut().resource_mut::<FrameInput>().command = TankCommand {
        throttle: f32::NAN,
        ..default()
    };
    let frames = (super::super::server::connection_timeout().as_secs_f64() / TICK.as_secs_f64())
        .ceil() as usize
        + 4;
    for frame in 0..frames {
        pair.step();
        if frame == pair.over_ceiling() {
            assert!(
                pair.server
                    .world()
                    .get::<InputGap>(pair.up)
                    .unwrap()
                    .stale()
            );
            assert!(!pair.server_input().answer.fire_secondary);
            assert_eq!(
                pair.server
                    .world()
                    .get::<TankCommand>(pair.tank)
                    .unwrap()
                    .throttle,
                1.0
            );
        }
    }
    assert!(pair.server.world().get::<Connected>(pair.up).is_some());
    assert!(
        pair.server
            .world()
            .get::<ParsedTankCommand>(pair.tank)
            .unwrap()
            .held()
    );
    assert_eq!(
        pair.server_input().last_refusal,
        Some(InputError::Levels(CommandError::Throttle))
    );
    pair.client
        .world_mut()
        .resource_mut::<FrameInput>()
        .command
        .throttle = 0.0;
    pair.steps(6);
    assert_eq!(pair.server_input().last_refusal, None);
}

/// Measurement clients rely on the combat-release writer putting script tick zero's click on
/// the real input channel. Advancing only its bookkeeping would silently omit the first shot.
#[test]
fn combat_release_sends_the_first_scripted_click_over_udp() {
    use super::super::harness::{Drive, Script, SimulateInput, buffer_input};
    let _guard = lock_real_udp_test();
    let mut pair = Pair::new();
    pair.client
        .insert_resource(SimulateInput::new(&Script {
            // Fixture: two ticks contain exactly the first primary click.
            drive: Drive::Fire {
                primary: true,
                secondary: false,
            },
            fire_tick: 0,
            fire_interval: 2,
            ticks: Some(2),
            ..default()
        }))
        .add_systems(FixedPreUpdate, buffer_input);
    // A one-weapon fixture tank whose primary is ready.
    let bore = pair
        .client
        .world_mut()
        .spawn(crate::tank::WeaponIndex(0))
        .id();
    pair.client.world_mut().spawn((
        lightyear::prelude::Controlled,
        crate::tank::Rig {
            hull: bore,
            turret: bore,
            gunner_yaw: bore,
            gun: bore,
            muzzle: bore,
        },
        crate::tank::WeaponGate {
            weapons: vec![crate::tank::WeaponGateState::default()],
        },
    ));
    let battle = pair
        .client
        .world_mut()
        .spawn(super::super::battle::BattleState::default())
        .id();
    pair.steps(4);
    assert_eq!(pair.shots(), 0);
    assert!(pair.state().pending.is_empty());
    *pair
        .client
        .world_mut()
        .get_mut::<super::super::battle::BattleState>(battle)
        .unwrap() = super::super::battle::BattleState::combat_fixture();
    pair.steps(20);
    assert_eq!(pair.shots(), 1);
    assert_eq!(pair.state().answer.resolved, 1);
    assert_eq!(pair.state().answer.refused_mask, 0);
}

/// Recovery admission must follow the policy budget, including saturation at its type limit.
/// Old pending edges age independently of a newly pressed edge on the same send.
#[test]
fn recovery_freshness_uses_the_authored_copy_budget() {
    // Fixture: distinct policy budgets, including the largest representable choice.
    for budget in [1, 4, u8::MAX] {
        let mut client = ClientInput::default();
        client.issue(InputEdge::Repair, 0.0);
        for copy in 0..budget {
            assert_eq!(
                client
                    .message(default(), f64::from(copy), budget)
                    .fresh_from,
                0
            );
        }
        client.issue(InputEdge::Fire, f64::from(budget));
        assert_eq!(
            client
                .message(default(), f64::from(budget), budget)
                .fresh_from,
            1
        );
        for _ in 1..budget {
            client.message(default(), f64::from(budget), budget);
        }
        assert_eq!(
            client
                .message(default(), f64::from(budget), budget)
                .fresh_from,
            2
        );
        assert!(client.pending.iter().all(|edge| edge.copies == budget));
    }
}
