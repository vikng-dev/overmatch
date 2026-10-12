//! Matchmaking mode: the pre-connect Join menu that asks the matchmaking Worker for a match.
//!
//! Mounted by `net::client` only when the client runs against a Worker (`OVERMATCH_API`, or the
//! build-time `OVERMATCH_DEFAULT_API`); `--local` and `OVERMATCH_SERVER` never mount it.
//!
//! - **Flow.** Startup fetches `GET /v1/locations?version=`, then every listed same-city beacon is
//!   probed ([`ping`]); the picker ([`picker`]) shows continents and locations with ping and match
//!   counts. Join posts the selection and the measured pings, then polls the ticket ([`ticket`])
//!   until `ready` or `failed`. A ready ticket is sent as [`MatchReady`]; `net::client` spawns the
//!   link to that address with that key.
//! - **Exits.** `net::client` owns the link and closes it with a [`MatchClosed`], which returns the
//!   lobby to idle: a link that never connects (shown as a failed join), the player's LEAVE MATCH
//!   ([`leave_ui`], sent as [`LeaveMatch`]), or a match whose server is gone (shown as the match's
//!   end). While the match's link is down the ticket is polled again; its `failed` is sent as
//!   [`ServerGone`].
//! - **Presence.** The lobby is the `Flow::Lobby` screen (no client link): its card is spawned on
//!   entering it and leaves with it, and its view systems run only there. [`Lobby`] persists, so a
//!   return to the lobby keeps the selection and the pings.
//! - **Threads.** Every HTTP call, DNS lookup and probe runs on its own thread ([`worker`]); the
//!   frame loop only drains finished replies.
//! - **Contract.** `backend/README.md` and `backend/contract/*.json`, held by [`contract`]'s tests.

use std::collections::BTreeSet;

use bevy::prelude::*;
use lightyear::prelude::client::{Connected, NetcodeClient};

use crate::flow::Flow;

mod contract;
mod leave_ui;
mod lobby_ui;
mod picker;
mod ping;
mod ticket;
mod worker;

use contract::LocationsResponse;
use picker::PingTable;
pub(super) use ticket::ReadyMatch;
use ticket::{JoinFlow, Phase, StatusEvent};
use worker::{Reply, WorkerLink};

/// A ticket came back ready: connect to this match.
#[derive(Message, Debug, Clone, Copy)]
pub(super) struct MatchReady(pub(super) ReadyMatch);

/// `net::client` closed the match's link; the lobby is idle again.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub(super) enum MatchClosed {
    /// The link never connected; shown as a failed join.
    Unreachable(String),
    /// The server went away after the player was in the match; shown as the match's end.
    Ended(String),
    /// The player left.
    Left,
}

/// The ticket of the match whose link is down answered `failed`: its server is gone.
#[derive(Message, Debug, Clone)]
pub(super) struct ServerGone(pub(super) String);

/// The player asked to leave the match.
#[derive(Message, Debug, Clone, Copy)]
pub(super) struct LeaveMatch;

/// The LEAVE MATCH button, for a windowed root that mounts the match screen.
pub(super) fn leave_ui_plugin(app: &mut App) {
    leave_ui::plugin(app);
}

#[derive(Debug, Clone, PartialEq)]
enum Locations {
    Loading,
    Failed(String),
    Loaded(LocationsResponse),
}

/// The lobby's whole state; [`lobby_ui`] draws it and writes the player's choices back.
#[derive(Resource)]
struct Lobby {
    locations: Locations,
    pings: PingTable,
    selection: BTreeSet<String>,
    /// The player changed the selection, so the default is never applied over it.
    touched: bool,
    /// The default selection has been applied to the current location list.
    defaulted: bool,
    /// Continents whose locations are listed.
    expanded: BTreeSet<String>,
    flow: JoinFlow,
}

impl Default for Lobby {
    fn default() -> Self {
        Self {
            locations: Locations::Loading,
            pings: PingTable::new(),
            selection: BTreeSet::new(),
            touched: false,
            defaulted: false,
            expanded: BTreeSet::new(),
            flow: JoinFlow::default(),
        }
    }
}

impl Lobby {
    fn groups(&self) -> Option<Vec<picker::GroupView>> {
        match &self.locations {
            Locations::Loaded(response) => Some(picker::groups(response, &self.pings)),
            _ => None,
        }
    }

    /// Fetch the location list again; pings and the default are redone for the new list.
    fn reload(&mut self, link: &WorkerLink) {
        self.locations = Locations::Loading;
        self.pings.clear();
        self.defaulted = false;
        link.fetch_locations(super::CLIENT_VERSION);
    }

    /// Start a join with the current selection; no-op unless idle with a selection.
    fn join(&mut self, link: &WorkerLink, now: f64) {
        let Some(groups) = self.groups() else { return };
        if self.selection.is_empty() {
            return;
        }
        let request = picker::join_request(super::CLIENT_VERSION, &groups, &self.selection);
        if let Some(generation) = self.flow.join(now) {
            info!(
                "matchmaking: join {:?} with pings {:?}",
                request.locations, request.pings
            );
            link.join(generation, request);
        }
    }
}

/// Mount the lobby against the Worker at `api`.
pub(super) fn install(app: &mut App, api: &str) {
    info!(
        "matchmaking: Worker {api}, version {}",
        super::CLIENT_VERSION
    );
    app.insert_resource(WorkerLink::new(api))
        .init_resource::<Lobby>()
        .add_message::<MatchReady>()
        .add_message::<MatchClosed>()
        .add_message::<ServerGone>()
        .add_message::<LeaveMatch>()
        .add_observer(lobby_ui::clicked)
        .add_systems(Startup, request_locations)
        .add_systems(OnEnter(Flow::Lobby), lobby_ui::spawn)
        .add_systems(
            Update,
            (
                (receive_replies, take_closed, poll_ticket).chain(),
                (lobby_ui::refresh_list, lobby_ui::refresh_footer)
                    .after(poll_ticket)
                    .run_if(in_state(Flow::Lobby)),
            ),
        );
}

fn request_locations(link: Res<WorkerLink>) {
    link.fetch_locations(super::CLIENT_VERSION);
}

fn receive_replies(
    link: Res<WorkerLink>,
    time: Res<Time<Real>>,
    mut lobby: ResMut<Lobby>,
    mut ready: MessageWriter<MatchReady>,
    mut gone: MessageWriter<ServerGone>,
) {
    let replies = link.drain();
    if replies.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    for reply in replies {
        match reply {
            Reply::Locations(Ok(response)) => {
                for location in response.continents.iter().flat_map(|c| &c.locations) {
                    link.probe(&location.id, location.beacon.as_ref());
                }
                lobby.locations = Locations::Loaded(response);
            }
            Reply::Locations(Err(reason)) => {
                warn!("matchmaking: locations unavailable: {reason}");
                lobby.locations = Locations::Failed(reason);
            }
            Reply::Ping { id, estimate } => {
                lobby.pings.insert(id, estimate);
            }
            Reply::Joined { generation, result } => {
                if let Err(reason) = &result {
                    warn!("matchmaking: join refused: {reason}");
                }
                lobby.flow.joined(generation, result, now);
            }
            Reply::Status {
                generation,
                outcome,
            } => match lobby.flow.status(generation, outcome, now) {
                StatusEvent::Ready(match_) => {
                    info!("matchmaking: ticket ready — server {}", match_.addr);
                    ready.write(MatchReady(match_));
                }
                StatusEvent::Failed => {
                    if let Some(reason) = lobby.flow.failure() {
                        warn!("matchmaking: ticket failed: {reason}");
                    }
                }
                StatusEvent::ServerGone(reason) => {
                    warn!("matchmaking: the match's server is gone: {reason}");
                    gone.write(ServerGone(reason));
                }
                StatusEvent::Nothing => {}
            },
        }
    }
    apply_default_selection(&mut lobby);
}

/// Once every probe has finished, tick the best continent and list its locations — unless the
/// player already chose.
fn apply_default_selection(lobby: &mut Lobby) {
    if lobby.touched || lobby.defaulted {
        return;
    }
    let Some(groups) = lobby.groups() else { return };
    if !picker::settled(&groups) {
        return;
    }
    lobby.selection = picker::default_selection(&groups);
    if let Some(best) = groups.first().filter(|_| !lobby.selection.is_empty()) {
        lobby.expanded.insert(best.continent.clone());
    }
    lobby.defaulted = true;
}

fn take_closed(mut closed: MessageReader<MatchClosed>, mut lobby: ResMut<Lobby>) {
    for closed in closed.read() {
        match closed {
            MatchClosed::Unreachable(reason) => lobby.flow.connection_failed(reason.clone()),
            MatchClosed::Ended(reason) => lobby.flow.match_ended(reason.clone()),
            MatchClosed::Left => lobby.flow.left(),
        }
    }
}

/// Poll the ticket while a join waits on it, and while the match it found has a link that is down.
fn poll_ticket(
    link: Res<WorkerLink>,
    time: Res<Time<Real>>,
    links: Query<Has<Connected>, With<NetcodeClient>>,
    mut lobby: ResMut<Lobby>,
) {
    let now = time.elapsed_secs_f64();
    let due = match lobby.flow.phase() {
        Phase::Waiting { .. } => lobby.flow.due_poll(now),
        Phase::Connecting { .. } if links.iter().any(|connected| !connected) => {
            lobby.flow.due_lost_poll(now)
        }
        _ => None,
    };
    if let Some((generation, ticket)) = due {
        link.poll(generation, ticket);
    }
}

/// Test seam (`net::client::hosted_e2e`): once the location list is loaded and every probe has
/// finished, select exactly `ids` and join as the Join button does. True once a join is under way.
#[cfg(test)]
pub(super) fn try_join(world: &mut World, ids: &[&str]) -> bool {
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    world.resource_scope(|world, mut lobby: Mut<Lobby>| {
        if lobby.flow.busy()
            || !lobby
                .groups()
                .is_some_and(|groups| picker::settled(&groups))
        {
            return false;
        }
        lobby.selection = ids.iter().map(|id| (*id).to_owned()).collect();
        lobby.touched = true;
        lobby.join(world.resource::<WorkerLink>(), now);
        lobby.flow.busy()
    })
}

/// Test seam: why the last join failed, if it did.
#[cfg(test)]
pub(super) fn join_failure(world: &World) -> Option<String> {
    world.resource::<Lobby>().flow.failure().map(str::to_owned)
}
