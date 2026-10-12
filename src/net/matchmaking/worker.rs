//! The lobby's blocking work — Worker HTTP calls, DNS and beacon probes — each on its own
//! short-lived thread, never on the frame loop. Results come back through one channel that
//! [`WorkerLink::drain`] empties once a frame.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use bevy::prelude::*;
use serde::de::DeserializeOwned;

use super::contract::{
    Beacon, ErrorResponse, JoinRequest, JoinResponse, LocationsResponse, TicketAnswer,
    TicketRequest, TicketResponse,
};
use super::picker::Estimate;
use super::ping;
use super::ticket::{ReadyMatch, StatusOutcome};

/// The bound on one HTTP exchange, connect through body.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// One finished piece of background work.
#[derive(Debug)]
pub(super) enum Reply {
    Locations(Result<LocationsResponse, String>),
    Ping {
        id: String,
        estimate: Estimate,
    },
    Joined {
        generation: u64,
        result: Result<String, String>,
    },
    Status {
        generation: u64,
        outcome: StatusOutcome,
    },
}

/// The Worker's base URL, the HTTP agent, and the reply channel.
#[derive(Resource)]
pub(super) struct WorkerLink {
    api: String,
    agent: ureq::Agent,
    sender: Sender<Reply>,
    replies: Mutex<Receiver<Reply>>,
}

/// How an exchange failed: the Worker answered with a status, or it did not answer usably.
#[derive(Debug)]
enum HttpError {
    /// The status and the body's `error`, or the status line when the body has none.
    Status(u16, String),
    Transport(String),
}

impl HttpError {
    fn reason(self) -> String {
        match self {
            HttpError::Status(_, error) => error,
            HttpError::Transport(error) => format!("can't reach the matchmaker ({error})"),
        }
    }
}

impl WorkerLink {
    /// `api` is the Worker's base URL; a trailing `/` is dropped.
    pub(super) fn new(api: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(HTTP_TIMEOUT))
            // A 4xx carries the Worker's `{error}` body, which is the reason the lobby shows.
            .http_status_as_error(false)
            .build()
            .into();
        let (sender, replies) = channel();
        Self {
            api: api.trim_end_matches('/').to_owned(),
            agent,
            sender,
            replies: Mutex::new(replies),
        }
    }

    pub(super) fn drain(&self) -> Vec<Reply> {
        let replies = self.replies.lock().expect("reply receiver lock poisoned");
        replies.try_iter().collect()
    }

    fn run(&self, name: &str, work: impl FnOnce(&ureq::Agent, &str) -> Reply + Send + 'static) {
        let agent = self.agent.clone();
        let api = self.api.clone();
        let sender = self.sender.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("matchmaking-{name}"))
            .spawn(move || {
                // The lobby may be gone (app exit); a closed channel needs no answer.
                let _ = sender.send(work(&agent, &api));
            });
        if let Err(error) = spawned {
            error!("matchmaking: cannot spawn the {name} thread: {error}");
        }
    }

    /// `GET /v1/locations?version=`.
    pub(super) fn fetch_locations(&self, version: &str) {
        let version = version.to_owned();
        self.run("locations", move |agent, api| {
            let response = agent
                .get(format!("{api}/v1/locations"))
                .query("version", &version)
                .call();
            Reply::Locations(read_json(response).map_err(HttpError::reason))
        });
    }

    /// Probe one location's beacon; a location without a TCP beacon is answered at once.
    pub(super) fn probe(&self, id: &str, beacon: Option<&Beacon>) {
        let id = id.to_owned();
        let Some((host, port)) = beacon.and_then(|b| Some((b.host.clone(), b.tcp_port?))) else {
            let _ = self.sender.send(Reply::Ping {
                id,
                estimate: Estimate::Unavailable,
            });
            return;
        };
        self.run("ping", move |_, _| {
            let samples = ping::beacon_addr(&host, port)
                .map(|addr| {
                    ping::probe_tcp(addr, ping::SAMPLES, ping::SAMPLE_GAP, ping::SAMPLE_TIMEOUT)
                })
                .unwrap_or_default();
            let estimate =
                ping::summarize(&samples).map_or(Estimate::Unavailable, Estimate::Measured);
            Reply::Ping { id, estimate }
        });
    }

    /// `POST /v1/join`.
    pub(super) fn join(&self, generation: u64, request: JoinRequest) {
        self.run("join", move |agent, api| {
            let result = post_json::<_, JoinResponse>(agent, &format!("{api}/v1/join"), &request)
                .map(|joined| joined.ticket)
                .map_err(HttpError::reason);
            Reply::Joined { generation, result }
        });
    }

    /// `POST /v1/join/status`; a ready answer's address is resolved here, off the frame loop.
    pub(super) fn poll(&self, generation: u64, ticket: String) {
        self.run("status", move |agent, api| {
            let response = post_json::<_, TicketResponse>(
                agent,
                &format!("{api}/v1/join/status"),
                &TicketRequest { ticket },
            );
            let outcome = match response {
                Ok(response) => match response.answer() {
                    Ok(TicketAnswer::Starting) => StatusOutcome::Starting,
                    Ok(TicketAnswer::Failed { reason }) => StatusOutcome::Failed(reason),
                    Ok(TicketAnswer::Ready { address, port, key }) => {
                        match match_addr(&address, port) {
                            Ok(addr) => StatusOutcome::Ready(ReadyMatch { addr, key }),
                            Err(error) => StatusOutcome::Rejected(error),
                        }
                    }
                    Err(error) => StatusOutcome::Rejected(error),
                },
                Err(HttpError::Status(status, error)) if status < 500 => {
                    StatusOutcome::Rejected(error)
                }
                Err(error) => StatusOutcome::Transient(match error {
                    HttpError::Status(status, _) => format!("HTTP {status}"),
                    HttpError::Transport(error) => error,
                }),
            };
            Reply::Status {
                generation,
                outcome,
            }
        });
    }
}

fn post_json<B: serde::Serialize, T: DeserializeOwned>(
    agent: &ureq::Agent,
    url: &str,
    body: &B,
) -> Result<T, HttpError> {
    let body =
        serde_json::to_string(body).map_err(|error| HttpError::Transport(error.to_string()))?;
    read_json(
        agent
            .post(url)
            .header("content-type", "application/json")
            .send(body),
    )
}

/// A 2xx body parsed as `T`; any other status with the Worker's `{error}` when it sent one.
fn read_json<T: DeserializeOwned>(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<T, HttpError> {
    let mut response = response.map_err(|error| HttpError::Transport(error.to_string()))?;
    let status = response.status();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| HttpError::Transport(error.to_string()))?;
    if !status.is_success() {
        let error = serde_json::from_str::<ErrorResponse>(&text)
            .map(|body| body.error)
            .unwrap_or_else(|_| format!("the matchmaker answered HTTP {}", status.as_u16()));
        return Err(HttpError::Status(status.as_u16(), error));
    }
    serde_json::from_str(&text)
        .map_err(|error| HttpError::Transport(format!("unreadable answer: {error}")))
}

/// The match server's IPv4 socket address: the UDP transport binds an IPv4 wildcard, so an IPv6
/// address cannot be reached and a hostname takes its first IPv4 result.
fn match_addr(address: &str, port: u16) -> Result<SocketAddr, String> {
    if let Ok(ip) = address.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(_) => Ok(SocketAddr::new(ip, port)),
            IpAddr::V6(_) => Err("the match server has an IPv6 address".into()),
        };
    }
    (address, port)
        .to_socket_addrs()
        .map_err(|error| format!("can't resolve the match server ({error})"))?
        .find(SocketAddr::is_ipv4)
        .ok_or_else(|| "the match server has no IPv4 address".into())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener};
    use std::time::Instant;

    use super::*;

    #[test]
    fn match_addr_takes_ipv4_and_refuses_ipv6() {
        assert_eq!(
            match_addr("198.51.100.7", 31085),
            Ok(SocketAddr::from(([198, 51, 100, 7], 31085)))
        );
        assert!(match_addr("2001:db8::1", 1).is_err());
    }

    /// Serve one HTTP response per connection from `answers`, in order; returns the base URL.
    fn serve(answers: Vec<(u16, &'static str)>) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for (status, body) in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                // Read the head and any body the content-length announces.
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(head_end) = text.find("\r\n\r\n") {
                        let length = text[..head_end]
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if request.len() >= head_end + 4 + length || n == 0 {
                            break;
                        }
                    }
                }
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        base
    }

    fn wait(link: &WorkerLink) -> Reply {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(reply) = link.drain().into_iter().next() {
                return reply;
            }
            assert!(Instant::now() < deadline, "no reply within 10 s");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn join_reads_a_ticket_and_a_refusal_reason() {
        let base = serve(vec![
            (200, r#"{"ticket":"T1"}"#),
            (400, r#"{"error":"unknown locations: Atlantis"}"#),
        ]);
        let link = WorkerLink::new(&format!("{base}/"));
        let request = JoinRequest {
            version: "0.0.0".into(),
            locations: vec!["Paris".into()],
            pings: Default::default(),
        };
        link.join(1, request.clone());
        assert!(matches!(
            wait(&link),
            Reply::Joined { generation: 1, result: Ok(ticket) } if ticket == "T1"
        ));
        link.join(2, request);
        assert!(matches!(
            wait(&link),
            Reply::Joined { generation: 2, result: Err(reason) } if reason == "unknown locations: Atlantis"
        ));
    }

    #[test]
    fn status_maps_starting_ready_rejected_and_transient() {
        let ready = r#"{"state":"ready","address":"198.51.100.7","port":31085,"key":"0000000000000000000000000000000000000000000000000000000000000001"}"#;
        let base = serve(vec![
            (200, r#"{"state":"starting"}"#),
            (200, ready),
            (404, r#"{"error":"unknown ticket"}"#),
            (503, "busy"),
        ]);
        let link = WorkerLink::new(&base);
        link.poll(1, "T".into());
        assert!(matches!(
            wait(&link),
            Reply::Status {
                outcome: StatusOutcome::Starting,
                ..
            }
        ));
        link.poll(1, "T".into());
        let Reply::Status {
            outcome: StatusOutcome::Ready(ready),
            ..
        } = wait(&link)
        else {
            panic!("ready");
        };
        assert_eq!(ready.addr, SocketAddr::from(([198, 51, 100, 7], 31085)));
        assert_eq!(ready.key[31], 1);
        link.poll(1, "T".into());
        assert!(matches!(
            wait(&link),
            Reply::Status { outcome: StatusOutcome::Rejected(reason), .. } if reason == "unknown ticket"
        ));
        link.poll(1, "T".into());
        assert!(matches!(
            wait(&link),
            Reply::Status {
                outcome: StatusOutcome::Transient(_),
                ..
            }
        ));
    }

    #[test]
    fn unreachable_worker_fails_locations_with_a_reason() {
        let closed = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let base = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let link = WorkerLink::new(&base);
        link.fetch_locations("0.0.0");
        assert!(matches!(
            wait(&link),
            Reply::Locations(Err(reason)) if reason.starts_with("can't reach the matchmaker")
        ));
    }

    #[test]
    fn a_location_without_a_tcp_beacon_has_no_estimate_at_once() {
        let link = WorkerLink::new("http://127.0.0.1:9");
        link.probe("Mumbai", None);
        link.probe(
            "Paris",
            Some(&Beacon {
                host: "203.0.113.13".into(),
                tcp_port: None,
            }),
        );
        let replies = link.drain();
        assert_eq!(replies.len(), 2, "answered without a thread");
        assert!(replies.iter().all(|reply| matches!(
            reply,
            Reply::Ping {
                estimate: Estimate::Unavailable,
                ..
            }
        )));
    }
}
