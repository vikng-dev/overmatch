//! The Join button's state machine: request a ticket, poll it until the Worker answers `ready` or
//! `failed`, hand a ready match to the connection, and — while that match's link is down — keep
//! polling the ticket, whose `failed` says the server is gone. Pure — times are seconds of real time passed in
//! by the caller, and every request it asks for carries the flow's generation, so an answer that
//! lands after a cancel (or after a newer join) is recognised as stale and dropped.

use std::net::SocketAddr;

/// Seconds between two status polls of one ticket.
pub(super) const POLL_INTERVAL_S: f64 = 1.0;
/// Consecutive transport failures of status polls after which the ticket is abandoned.
pub(super) const POLL_ERROR_LIMIT: u32 = 5;

/// What the connection needs from a ready ticket: the resolved server address and the match's
/// netcode private key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::net) struct ReadyMatch {
    pub(in crate::net) addr: SocketAddr,
    pub(in crate::net) key: [u8; 32],
}

/// One status poll's outcome as the worker thread reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StatusOutcome {
    Starting,
    Ready(ReadyMatch),
    /// The Worker's `failed` reason.
    Failed(String),
    /// A 4xx answer (an unknown or expired ticket): retrying cannot help.
    Rejected(String),
    /// No answer, a 5xx, or an unreadable body: the next poll may succeed.
    Transient(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Phase {
    Idle,
    /// `POST /v1/join` is in flight.
    Requesting {
        since: f64,
    },
    /// Holding a ticket. `starting` once the Worker has answered `starting`; `in_flight` while a
    /// poll is out, so at most one is.
    Waiting {
        ticket: String,
        since: f64,
        starting: bool,
        next_poll: f64,
        in_flight: bool,
        errors: u32,
    },
    /// Ready: the lobby has handed the match to the connection ([`StatusEvent::Ready`]) and stays
    /// closed while it lives. The ticket is kept: while the match's link is down it is polled
    /// ([`JoinFlow::due_lost_poll`]), `in_flight` while a poll is out.
    Connecting {
        ticket: String,
        next_poll: f64,
        in_flight: bool,
    },
}

/// What a status answer changed, for the caller to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StatusEvent {
    Nothing,
    /// The ticket turned ready: connect to this match.
    Ready(ReadyMatch),
    /// The join failed; [`JoinFlow::failure`] says why.
    Failed,
    /// The connected match's server is gone, for this reason.
    ServerGone(String),
}

/// Why the lobby is idle again, shown until the next join.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Notice {
    /// The join failed.
    Failed(String),
    /// The match the player was in ended.
    Ended(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct JoinFlow {
    generation: u64,
    phase: Phase,
    /// Why the last attempt or match ended, shown in the lobby until the next join.
    notice: Option<Notice>,
}

impl Default for JoinFlow {
    fn default() -> Self {
        Self {
            generation: 0,
            phase: Phase::Idle,
            notice: None,
        }
    }
}

impl JoinFlow {
    pub(super) fn phase(&self) -> &Phase {
        &self.phase
    }

    /// Why the last join failed, if it did.
    pub(super) fn failure(&self) -> Option<&str> {
        match &self.notice {
            Some(Notice::Failed(reason)) => Some(reason),
            _ => None,
        }
    }

    /// Why the last match ended, if the server went away.
    pub(super) fn ended(&self) -> Option<&str> {
        match &self.notice {
            Some(Notice::Ended(reason)) => Some(reason),
            _ => None,
        }
    }

    pub(super) fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Start a join from idle; returns the generation the `POST /v1/join` must carry.
    pub(super) fn join(&mut self, now: f64) -> Option<u64> {
        if self.busy() {
            return None;
        }
        self.generation += 1;
        self.notice = None;
        self.phase = Phase::Requesting { since: now };
        Some(self.generation)
    }

    /// Back to idle from a request or a ticket; every answer still out becomes stale. The Worker's
    /// reservation for an abandoned ticket lapses by its own TTL.
    pub(super) fn cancel(&mut self) {
        if matches!(self.phase, Phase::Requesting { .. } | Phase::Waiting { .. }) {
            self.generation += 1;
            self.phase = Phase::Idle;
            self.notice = None;
        }
    }

    /// Back to idle with `notice`; every answer still out becomes stale.
    fn idle(&mut self, notice: Option<Notice>) {
        self.generation += 1;
        self.phase = Phase::Idle;
        self.notice = notice;
    }

    fn fail(&mut self, reason: String) {
        self.idle(Some(Notice::Failed(reason)));
    }

    /// The `POST /v1/join` answer: a ticket to poll at once, or the reason it was refused.
    pub(super) fn joined(&mut self, generation: u64, result: Result<String, String>, now: f64) {
        if generation != self.generation {
            return;
        }
        let Phase::Requesting { since } = self.phase else {
            return;
        };
        match result {
            Ok(ticket) => {
                self.phase = Phase::Waiting {
                    ticket,
                    since,
                    starting: false,
                    next_poll: now,
                    in_flight: false,
                    errors: 0,
                }
            }
            Err(reason) => self.fail(reason),
        }
    }

    /// The poll due now, if any: `(generation, ticket)`. Marks it in flight.
    pub(super) fn due_poll(&mut self, now: f64) -> Option<(u64, String)> {
        let generation = self.generation;
        match &mut self.phase {
            Phase::Waiting {
                ticket,
                next_poll,
                in_flight,
                ..
            } if !*in_flight && now >= *next_poll => {
                *in_flight = true;
                Some((generation, ticket.clone()))
            }
            _ => None,
        }
    }

    /// The poll due now while the match's link is down, if any: `(generation, ticket)`. Marks it in
    /// flight.
    pub(super) fn due_lost_poll(&mut self, now: f64) -> Option<(u64, String)> {
        let generation = self.generation;
        match &mut self.phase {
            Phase::Connecting {
                ticket,
                next_poll,
                in_flight,
                ..
            } if !*in_flight && now >= *next_poll => {
                *in_flight = true;
                Some((generation, ticket.clone()))
            }
            _ => None,
        }
    }

    pub(super) fn status(
        &mut self,
        generation: u64,
        outcome: StatusOutcome,
        now: f64,
    ) -> StatusEvent {
        if generation != self.generation {
            return StatusEvent::Nothing;
        }
        match &mut self.phase {
            Phase::Waiting {
                ticket,
                starting,
                next_poll,
                in_flight,
                errors,
                ..
            } => {
                *in_flight = false;
                match outcome {
                    StatusOutcome::Starting => {
                        *starting = true;
                        *errors = 0;
                        *next_poll = now + POLL_INTERVAL_S;
                        StatusEvent::Nothing
                    }
                    StatusOutcome::Ready(ready) => {
                        let ticket = std::mem::take(ticket);
                        self.phase = Phase::Connecting {
                            ticket,
                            next_poll: now + POLL_INTERVAL_S,
                            in_flight: false,
                        };
                        StatusEvent::Ready(ready)
                    }
                    StatusOutcome::Failed(reason) | StatusOutcome::Rejected(reason) => {
                        self.fail(reason);
                        StatusEvent::Failed
                    }
                    StatusOutcome::Transient(reason) => {
                        *errors += 1;
                        if *errors >= POLL_ERROR_LIMIT {
                            self.fail(format!("lost contact with the matchmaker ({reason})"));
                            StatusEvent::Failed
                        } else {
                            *next_poll = now + POLL_INTERVAL_S;
                            StatusEvent::Nothing
                        }
                    }
                }
            }
            // Only `failed` says the server is gone. `ready` and `starting` (the Worker answers
            // `starting` for a stopping server whose fate it does not yet know) keep the
            // reconnects going; so do an expired ticket (`Rejected`) and a lost answer, which say
            // nothing about the server — the reconnect give-up covers those.
            Phase::Connecting {
                next_poll,
                in_flight,
                ..
            } => {
                *in_flight = false;
                *next_poll = now + POLL_INTERVAL_S;
                match outcome {
                    StatusOutcome::Failed(reason) => StatusEvent::ServerGone(reason),
                    _ => StatusEvent::Nothing,
                }
            }
            _ => StatusEvent::Nothing,
        }
    }

    /// The connection to a ready match gave up before it ever connected.
    pub(super) fn connection_failed(&mut self, reason: String) {
        if matches!(self.phase, Phase::Connecting { .. }) {
            self.fail(reason);
        }
    }

    /// The match's server went away after the player was in it.
    pub(super) fn match_ended(&mut self, reason: String) {
        if matches!(self.phase, Phase::Connecting { .. }) {
            self.idle(Some(Notice::Ended(reason)));
        }
    }

    /// The player left the match.
    pub(super) fn left(&mut self) {
        if matches!(self.phase, Phase::Connecting { .. }) {
            self.idle(None);
        }
    }

    /// Seconds since Join was pressed, while a join is in progress.
    pub(super) fn elapsed(&self, now: f64) -> Option<f64> {
        match self.phase {
            Phase::Requesting { since } | Phase::Waiting { since, .. } => Some(now - since),
            _ => None,
        }
    }

    /// The in-progress line, without the elapsed time.
    pub(super) fn progress_label(&self) -> Option<&'static str> {
        match self.phase {
            Phase::Requesting { .. }
            | Phase::Waiting {
                starting: false, ..
            } => Some("Finding a match…"),
            Phase::Waiting { starting: true, .. } => Some("Starting a server…"),
            _ => None,
        }
    }
}

/// `m:ss` of whole elapsed seconds.
pub(super) fn clock_label(seconds: f64) -> String {
    let whole = seconds.max(0.0) as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> ReadyMatch {
        ReadyMatch {
            addr: SocketAddr::from(([198, 51, 100, 7], 31085)),
            key: [7; 32],
        }
    }

    /// Join, receive a ticket, and take the first poll; returns the generation.
    fn waiting(flow: &mut JoinFlow) -> u64 {
        let generation = flow.join(0.0).unwrap();
        flow.joined(generation, Ok("T".into()), 0.2);
        let (polled, ticket) = flow.due_poll(0.2).expect("the first poll is due at once");
        assert_eq!((polled, ticket.as_str()), (generation, "T"));
        generation
    }

    #[test]
    fn starting_then_ready_hands_over_the_match() {
        let mut flow = JoinFlow::default();
        let generation = waiting(&mut flow);
        assert_eq!(flow.progress_label(), Some("Finding a match…"));
        assert_eq!(flow.due_poll(0.3), None, "one poll in flight at a time");

        flow.status(generation, StatusOutcome::Starting, 0.4);
        assert_eq!(flow.progress_label(), Some("Starting a server…"));
        assert_eq!(flow.due_poll(0.5), None, "the next poll waits its interval");
        let (generation, _) = flow.due_poll(0.4 + POLL_INTERVAL_S).unwrap();
        assert_eq!(flow.elapsed(1.4), Some(1.4));

        assert_eq!(
            flow.status(generation, StatusOutcome::Ready(ready()), 1.6),
            StatusEvent::Ready(ready())
        );
        assert!(matches!(flow.phase(), Phase::Connecting { .. }));
        assert!(flow.busy());
        assert_eq!(flow.join(2.0), None, "no second join while connecting");
    }

    #[test]
    fn failed_returns_to_idle_with_the_reason() {
        let mut flow = JoinFlow::default();
        let generation = waiting(&mut flow);
        flow.status(
            generation,
            StatusOutcome::Failed("the server did not become ready in time".into()),
            3.0,
        );
        assert_eq!(flow.phase(), &Phase::Idle);
        assert_eq!(
            flow.failure(),
            Some("the server did not become ready in time")
        );
        assert!(flow.join(4.0).is_some(), "a new join is possible");
        assert_eq!(flow.failure(), None, "and clears the old reason");
    }

    #[test]
    fn refused_join_and_rejected_ticket_fail() {
        let mut flow = JoinFlow::default();
        let generation = flow.join(0.0).unwrap();
        flow.joined(generation, Err("unknown locations: Atlantis".into()), 0.1);
        assert_eq!(flow.failure(), Some("unknown locations: Atlantis"));

        let mut flow = JoinFlow::default();
        let generation = waiting(&mut flow);
        flow.status(
            generation,
            StatusOutcome::Rejected("unknown ticket".into()),
            1.0,
        );
        assert_eq!(flow.phase(), &Phase::Idle);
        assert_eq!(flow.failure(), Some("unknown ticket"));
    }

    #[test]
    fn cancel_drops_every_late_answer() {
        let mut flow = JoinFlow::default();
        let generation = flow.join(0.0).unwrap();
        flow.cancel();
        assert_eq!(flow.phase(), &Phase::Idle);
        flow.joined(generation, Ok("late".into()), 0.5);
        assert_eq!(flow.phase(), &Phase::Idle, "a late ticket is not polled");

        let generation = waiting(&mut flow);
        flow.cancel();
        flow.status(generation, StatusOutcome::Ready(ready()), 1.0);
        assert_eq!(flow.phase(), &Phase::Idle, "a late ready never connects");
        assert_eq!(flow.failure(), None, "a cancel is not a failure");
    }

    #[test]
    fn answers_of_an_older_join_cannot_reach_a_newer_one() {
        let mut flow = JoinFlow::default();
        let old = flow.join(0.0).unwrap();
        flow.cancel();
        let new = flow.join(1.0).unwrap();
        assert_ne!(old, new);
        flow.joined(old, Ok("old".into()), 1.1);
        assert!(matches!(flow.phase(), Phase::Requesting { .. }));
        flow.joined(new, Ok("new".into()), 1.2);
        assert_eq!(flow.due_poll(1.2).map(|(_, t)| t), Some("new".into()));
    }

    #[test]
    fn transient_errors_retry_until_the_limit() {
        let mut flow = JoinFlow::default();
        let mut generation = waiting(&mut flow);
        let mut now = 0.2;
        for _ in 1..POLL_ERROR_LIMIT {
            flow.status(generation, StatusOutcome::Transient("timeout".into()), now);
            now += POLL_INTERVAL_S;
            generation = flow.due_poll(now).expect("a transient error polls again").0;
        }
        flow.status(generation, StatusOutcome::Transient("timeout".into()), now);
        assert_eq!(flow.phase(), &Phase::Idle);
        assert!(flow.failure().unwrap().contains("lost contact"));
    }

    #[test]
    fn a_starting_answer_resets_the_error_streak() {
        let mut flow = JoinFlow::default();
        let generation = waiting(&mut flow);
        flow.status(generation, StatusOutcome::Transient("x".into()), 0.3);
        let (generation, _) = flow.due_poll(2.0).unwrap();
        flow.status(generation, StatusOutcome::Starting, 2.0);
        let Phase::Waiting { errors, .. } = flow.phase() else {
            panic!("still waiting");
        };
        assert_eq!(*errors, 0);
    }

    #[test]
    fn connection_failure_returns_to_the_menu_with_the_reason() {
        let mut flow = JoinFlow::default();
        let generation = waiting(&mut flow);
        flow.status(generation, StatusOutcome::Ready(ready()), 1.0);
        flow.connection_failed("could not reach the match server".into());
        assert_eq!(flow.phase(), &Phase::Idle);
        assert_eq!(flow.failure(), Some("could not reach the match server"));
        flow.connection_failed("ignored when idle".into());
        assert_eq!(flow.failure(), Some("could not reach the match server"));
    }

    /// While the match's link is down the kept ticket is polled once per interval; only `failed`
    /// reports the server gone, and an ended match or a leave returns to idle.
    #[test]
    fn a_connected_match_polls_its_ticket_and_hears_the_server_go() {
        let mut flow = JoinFlow::default();
        let generation = waiting(&mut flow);
        flow.status(generation, StatusOutcome::Ready(ready()), 1.0);
        assert_eq!(flow.due_poll(5.0), None, "the join poll is over");
        assert_eq!(
            flow.due_lost_poll(1.5),
            None,
            "the first lost poll waits its interval"
        );
        let (polled, ticket) = flow.due_lost_poll(1.0 + POLL_INTERVAL_S).unwrap();
        assert_eq!((polled, ticket.as_str()), (generation, "T"));
        assert_eq!(
            flow.due_lost_poll(9.0),
            None,
            "one poll in flight at a time"
        );
        for keep in [
            StatusOutcome::Ready(ready()),
            StatusOutcome::Starting,
            StatusOutcome::Rejected("unknown ticket".into()),
            StatusOutcome::Transient("timeout".into()),
        ] {
            assert_eq!(flow.status(generation, keep, 2.0), StatusEvent::Nothing);
            assert!(flow.due_lost_poll(2.0 + POLL_INTERVAL_S).is_some());
        }
        assert_eq!(
            flow.status(
                generation,
                StatusOutcome::Failed("server stopped".into()),
                3.0
            ),
            StatusEvent::ServerGone("server stopped".into())
        );
        flow.match_ended("server stopped".into());
        assert_eq!(flow.phase(), &Phase::Idle);
        assert_eq!(flow.ended(), Some("server stopped"));
        assert_eq!(flow.failure(), None, "an ended match is not a failed join");
        flow.join(4.0).unwrap();
        assert_eq!(flow.ended(), None, "a new join clears the notice");
    }

    #[test]
    fn leaving_returns_to_idle_with_no_notice() {
        let mut flow = JoinFlow::default();
        let generation = waiting(&mut flow);
        flow.status(generation, StatusOutcome::Ready(ready()), 1.0);
        flow.left();
        assert_eq!(flow.phase(), &Phase::Idle);
        assert_eq!((flow.failure(), flow.ended()), (None, None));
    }

    #[test]
    fn clock_reads_minutes_and_seconds() {
        assert_eq!(clock_label(7.9), "0:07");
        assert_eq!(clock_label(75.0), "1:15");
        assert_eq!(clock_label(-1.0), "0:00");
    }
}
