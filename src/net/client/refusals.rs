//! Terminal handling through the shipped lifecycle and connect-screen mounts.

use super::*;
use crate::net::matchmaking::{ServerGone, printable};
use crate::net::test_harness::{TICK, base_app, finish};

#[derive(Resource, Default)]
struct Connects(usize);

fn count_connects(_: On<Connect>, mut connects: ResMut<Connects>) {
    connects.0 += 1;
}

pub(in crate::net) fn direct_app() -> App {
    let mut app = base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    crate::net::protocol::plugin(&mut app);
    // The explicit direct-screen mount does not depend on Flow already being initialized.
    install_direct_connect_screen(&mut app);
    app.init_asset::<Font>()
        .add_plugins((crate::ui_font::plugin, crate::flow::plugin))
        .insert_state(AppState::Playing)
        .init_resource::<crate::overlay::Overlays>()
        .insert_resource(PendingTankAssets {
            spec: Handle::default(),
            scene: None,
            effects: Vec::new(),
        });
    install_connection_lifecycle(&mut app);
    app.init_resource::<Connects>().add_observer(count_connects);
    app.add_plugins(connect_screen_plugin);
    install_screen_flow(&mut app);
    // Fixture endpoint and client identity; no Connect is sent while assets are pending.
    install_direct_link(
        &mut app,
        &ClientLinkTemplate {
            client_id: 7,
            conditioner: None,
            seeded: None,
        },
        "127.0.0.1:9".parse().unwrap(),
    );
    finish(&mut app);
    app
}

fn matchmade_app() -> App {
    let mut app = super::match_exit::client_app();
    finish(&mut app);
    app.update();
    crate::net::matchmaking::ready_fixture(
        app.world_mut(),
        crate::net::matchmaking::ReadyMatch {
            addr: "127.0.0.1:9".parse().unwrap(),
            key: [0; 32], // Fixture endpoint and development key.
        },
    );
    app.update();
    app
}

pub(in crate::net) fn refusal_kinds() -> Vec<ServerGone> {
    vec![
        ServerGone::Admission(crate::net::admission::JoinRefusal::ServerFull),
        // Fixture Worker diagnostic also exercises sanitization independently of cause wording.
        ServerGone::Matchmaking("fixture server start failed — try again".into()),
        ServerGone::Construction(crate::net::construction::ConstructionRefusal::DuplicateLedgers),
        ServerGone::Debris(crate::net::debris_stress::PoseRefusal::Position),
        ServerGone::TickStamp(crate::net::tick_stamp::StampRefusal::invalid_fixture()),
        // Fixture tick; the stream order check is also terminal.
        ServerGone::Stream(crate::net::view_clock::StreamRefusal::Order { tick: Tick(4) }),
    ]
}

/// Direct players rely on every stream refusal surviving the retry reset and message expiry,
/// closing before a due reconnect, and reaching the direct connection screen.
#[test]
fn direct_refusals_close_without_reconnect_and_retain_the_typed_cause_until_shown() {
    for (cause, connected_once) in refusal_kinds()
        .into_iter()
        .flat_map(|cause| [(cause.clone(), false), (cause, true)])
    {
        let mut app = direct_app();
        app.update();
        let expected = format!(
            "{}: {}",
            if connected_once {
                "Match server lost"
            } else {
                "Couldn't join"
            },
            printable(&cause.to_string())
        );
        // Fixture: an already initiated retry is due now, so reconnecting would reach netcode.
        let mut retry = app.world_mut().resource_mut::<ConnectRetry>();
        retry.initiated = true;
        retry.connected_once = connected_once;
        retry.next_retry_at = Some(0.0);
        app.world_mut().write_message(cause.clone());
        app.update();
        assert!(
            app.world_mut()
                .query_filtered::<(), With<NetcodeClient>>()
                .iter(app.world())
                .next()
                .is_none()
        );
        assert_eq!(app.world().resource::<ConnectRetry>().attempts, 0);
        assert_eq!(
            app.world().resource::<Connects>().0,
            0,
            "no Connect may precede the close"
        );
        assert_retained(&app, &cause);
        // Fixture frames: allow the no-link screen transition and expire buffered messages.
        for _ in 0..8 {
            app.update();
        }
        assert_eq!(
            *app.world().resource::<State<crate::flow::Flow>>().get(),
            crate::flow::Flow::Lobby
        );
        let world = app.world_mut();
        assert!(
            world
                .query_filtered::<&Text, With<JoiningText>>()
                .iter(world)
                .any(|text| text.0 == expected)
        );
        assert_retained(&app, &cause);
        assert_eq!(
            app.world().resource::<Connects>().0,
            0,
            "no reconnect after the close"
        );
        assert!(
            app.world_mut()
                .query_filtered::<(), With<NetcodeClient>>()
                .iter(app.world())
                .next()
                .is_none()
        );
    }
}

/// A refused peer can finish decoding after its link is gone. Players still need its first typed
/// cause when the no-link screen appears; a later failure must not overwrite that explanation.
#[test]
fn a_refusal_without_a_link_survives_until_the_direct_screen_shows_it() {
    let mut app = direct_app();
    let link = app
        .world_mut()
        .query_filtered::<Entity, With<NetcodeClient>>()
        .single(app.world())
        .unwrap();
    app.world_mut().despawn(link);
    for _ in 0..3 {
        app.update();
    } // Fixture: the no-link screen is already present.
    let cause =
        ServerGone::Construction(crate::net::construction::ConstructionRefusal::DuplicateLedgers);
    app.world_mut().write_message(cause.clone());
    app.update();
    app.world_mut().write_message(ServerGone::StoppedAnswering);
    app.update();
    assert_retained(&app, &cause);
    let world = app.world_mut();
    assert!(
        world
            .query_filtered::<&Text, With<JoiningText>>()
            .iter(world)
            .any(|text| text.0 == format!("Couldn't join: {}", printable(&cause.to_string())))
    );
}

/// Direct players can start the server later: silence past matchmaking's give-up threshold
/// must retain the link and issue another Connect, with the existing unreachable hint.
#[test]
fn a_direct_link_keeps_retrying_past_the_matchmade_threshold() {
    for connected_once in [false, true] {
        let mut app = direct_app();
        app.update();
        let mut retry = app.world_mut().resource_mut::<ConnectRetry>();
        retry.initiated = true;
        retry.connected_once = connected_once;
        retry.exhausted = true;
        if connected_once {
            retry.down_since = Some(0.0);
            retry.last_attempt_at = Some(client_policy().reconnect_give_up_s.0);
        }
        retry.attempts = client_policy().unanswered_after_attempts.0;
        retry.next_retry_at = Some(0.0);
        app.update();
        assert_eq!(app.world().resource::<Connects>().0, 1);
        assert!(app.world().resource::<ClosedConnection>().cause.is_none());
        assert!(
            app.world().resource::<ConnectRetry>().attempts
                > client_policy().unanswered_after_attempts.0
        );
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<(), With<NetcodeClient>>()
                .iter(world)
                .count(),
            1
        );
        assert!(
            world
                .query_filtered::<&Text, With<JoiningText>>()
                .iter(world)
                .any(|text| text.0
                    == if connected_once {
                        RECONNECT_LABEL.to_owned()
                    } else {
                        joining_label(client_policy().unanswered_after_attempts.0)
                    })
        );
    }
}

/// Matchmade players return to the lobby at the policy's unanswered-attempt limit;
/// one fewer failed attempt must still permit the server to answer.
#[test]
fn matchmade_links_give_up_at_the_owned_attempt_threshold() {
    let threshold = client_policy().unanswered_after_attempts.0;
    for attempts in [threshold - 1, threshold] {
        let mut app = matchmade_app();
        let mut retry = app.world_mut().resource_mut::<ConnectRetry>();
        retry.initiated = true;
        retry.attempts = attempts;
        retry.next_retry_at = Some(f64::MAX); // Fixture: no further attempt falls due this frame.
        app.update();
        if attempts == threshold {
            assert_retained(&app, &ServerGone::Unreachable);
        } else {
            assert!(app.world().resource::<ClosedConnection>().cause.is_none());
        }
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<(), With<NetcodeClient>>()
                .iter(world)
                .count(),
            usize::from(attempts < threshold)
        );
    }
}

fn assert_retained(app: &App, cause: &ServerGone) {
    let retained = app
        .world()
        .resource::<ClosedConnection>()
        .cause
        .as_ref()
        .expect("terminal cause retained");
    assert_eq!(
        std::mem::discriminant(retained),
        std::mem::discriminant(cause)
    );
    assert_eq!(retained.to_string(), cause.to_string());
}

/// Matchmade players rely on the same terminal handler preserving the lobby's match-end notice.
#[test]
fn matchmade_refusals_reach_the_existing_lobby_status() {
    for cause in refusal_kinds() {
        let mut app = matchmade_app();
        app.world_mut()
            .resource_mut::<ConnectRetry>()
            .connected_once = true;
        app.world_mut().write_message(cause.clone());
        for _ in 0..8 {
            app.update();
        } // Fixture: close and enter the lobby.
        assert_retained(&app, &cause);
        let world = app.world_mut();
        assert!(
            world
                .query::<&Text>()
                .iter(world)
                .any(|text| text.0.contains("Match server lost")
                    && text.0.contains(&printable(&cause.to_string())))
        );
        assert!(
            world
                .query_filtered::<(), With<NetcodeClient>>()
                .iter(world)
                .next()
                .is_none()
        );
    }
}

/// A direct player's due retry cannot outrun admission of this frame's construction stream.
#[test]
fn construction_admission_closes_before_a_due_reconnect_in_the_same_update() {
    let mut app = direct_app();
    app.update();
    app.init_resource::<crate::construction::breakage::Record>();
    crate::net::construction::install_client(&mut app);
    // Fixture outside input: a ledger without its required world identity.
    app.world_mut()
        .spawn(crate::net::construction::Ledger::default());
    let mut retry = app.world_mut().resource_mut::<ConnectRetry>();
    retry.initiated = true;
    retry.next_retry_at = Some(0.0);
    app.update();
    let world = app.world_mut();
    assert!(
        world
            .query_filtered::<(), With<NetcodeClient>>()
            .iter(world)
            .next()
            .is_none()
    );
    assert_eq!(
        world.resource::<Connects>().0,
        0,
        "admission must precede reconnect"
    );
    assert!(matches!(
        world.resource::<ClosedConnection>().cause,
        Some(ServerGone::Construction(
            crate::net::construction::ConstructionRefusal::UntaggedLedger
        ))
    ));
}

/// A matchmade player whose retry just connected must keep that link, even when its previous attempt ledger
/// met the give-up limit. The shipped lifecycle observes success before deciding a terminal close.
#[test]
fn a_successful_retry_clears_the_failed_attempt_ledger_before_terminal_handling() {
    let mut app = matchmade_app();
    let world = app.world_mut();
    let link = world
        .query_filtered::<Entity, With<NetcodeClient>>()
        .single(world)
        .unwrap();
    // Fixture: netcode has delivered the Connected marker for this identity.
    world
        .entity_mut(link)
        .insert((RemoteId(PeerId::Netcode(7)), Connected));
    let mut retry = world.resource_mut::<ConnectRetry>();
    retry.initiated = true;
    retry.attempts = client_policy().unanswered_after_attempts.0;
    retry.exhausted = true;
    world.run_schedule(Update);
    assert!(world.get_entity(link).is_ok());
    assert!(world.resource::<ClosedConnection>().cause.is_none());
    let retry = world.resource::<ConnectRetry>();
    assert!(retry.connected_once && !retry.exhausted);
    assert_eq!(retry.attempts, 0);
}
