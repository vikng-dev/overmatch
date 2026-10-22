//! The real-UDP server fixture for the match-exit probes (`net::client::match_exit`): a lightyear
//! server on a local port, replicating through the production sender observer.

use std::net::{Ipv4Addr, SocketAddr};

use bevy::prelude::*;
use lightyear::prelude::server::{
    NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
};
use lightyear::prelude::*;

use super::protocol::protocol_id;
use super::test_harness::{TICK, base_app};

pub(super) fn build_server(port: u16) -> App {
    let mut app = base_app();
    app.add_plugins(ServerPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
    app.add_observer(super::server::attach_replication_sender);
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
