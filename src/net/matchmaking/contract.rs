//! The Worker's `/v1` client-facing wire shapes (`backend/src/types.ts`), and the parse of each
//! answer into what the lobby acts on. One checked-in example per shape lives in
//! `backend/contract/*.json`; the tests below hold these types to those files.
//!
//! The API is additive (a field may be added, never renamed or removed), so unknown fields are
//! ignored and only the fields the client reads are declared.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// `GET /v1/locations?version=V`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct LocationsResponse {
    pub(super) version: String,
    pub(super) continents: Vec<Continent>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct Continent {
    pub(super) continent: String,
    /// Live matches of the asking version over the group's locations.
    pub(super) matches: u32,
    pub(super) locations: Vec<Location>,
}

/// One deployable placement unit. `id` is what a join request names.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct Location {
    pub(super) id: String,
    pub(super) city: String,
    pub(super) country: String,
    /// A beacon in the same city and country, or `None`: the location then has no estimate.
    pub(super) beacon: Option<Beacon>,
    pub(super) matches: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct Beacon {
    pub(super) host: String,
    /// The port a TCP connect is timed against; `None` leaves the location without an estimate.
    pub(super) tcp_port: Option<u16>,
}

/// `POST /v1/join`. `pings` holds a measured location's median RTT in whole ms, and omits every
/// location without an estimate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct JoinRequest {
    pub(super) version: String,
    pub(super) locations: Vec<String>,
    pub(super) pings: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct JoinResponse {
    pub(super) ticket: String,
}

/// `POST /v1/join/status`. The ticket travels in the body, never in a URL.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct TicketRequest {
    pub(super) ticket: String,
}

/// Every 4xx/5xx body.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct ErrorResponse {
    pub(super) error: String,
}

/// The raw `POST /v1/join/status` body; [`TicketResponse::answer`] is its only reader.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(super) struct TicketResponse {
    state: String,
    address: Option<String>,
    port: Option<u16>,
    key: Option<String>,
    reason: Option<String>,
}

/// What a status poll says, with the `ready` fields made mandatory and the key decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TicketAnswer {
    Starting,
    Ready {
        address: String,
        port: u16,
        key: [u8; 32],
    },
    Failed {
        reason: String,
    },
}

impl TicketResponse {
    /// A `ready` answer missing its address, port or a well-formed key is an error, not a match.
    pub(super) fn answer(self) -> Result<TicketAnswer, String> {
        match self.state.as_str() {
            "starting" => Ok(TicketAnswer::Starting),
            "ready" => {
                let (Some(address), Some(port), Some(key)) = (self.address, self.port, self.key)
                else {
                    return Err("the matchmaker's ready answer is incomplete".into());
                };
                Ok(TicketAnswer::Ready {
                    address,
                    port,
                    key: decode_key(&key)?,
                })
            }
            "failed" => Ok(TicketAnswer::Failed {
                reason: self
                    .reason
                    .unwrap_or_else(|| "the matchmaker gave no reason".into()),
            }),
            other => Err(format!("unknown ticket state {other:?}")),
        }
    }
}

/// 64 hex characters → the 32-byte netcode private key.
pub(super) fn decode_key(hex: &str) -> Result<[u8; 32], String> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return Err(format!(
            "the match key is {} hex characters, not 64",
            bytes.len()
        ));
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let mut key = [0u8; 32];
    let (pairs, _) = bytes.as_chunks::<2>();
    for (byte, [high, low]) in key.iter_mut().zip(pairs) {
        let (Some(high), Some(low)) = (nibble(*high), nibble(*low)) else {
            return Err("the match key is not hex".into());
        };
        *byte = high << 4 | low;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCATIONS: &str = include_str!("../../../backend/contract/locations.response.json");
    const JOIN_REQUEST: &str = include_str!("../../../backend/contract/join.request.json");
    const JOIN_RESPONSE: &str = include_str!("../../../backend/contract/join.response.json");
    const TICKET_REQUEST: &str = include_str!("../../../backend/contract/ticket.request.json");
    const TICKET_STARTING: &str =
        include_str!("../../../backend/contract/ticket.starting.response.json");
    const TICKET_READY: &str = include_str!("../../../backend/contract/ticket.ready.response.json");
    const TICKET_FAILED: &str =
        include_str!("../../../backend/contract/ticket.failed.response.json");
    const ERROR: &str = include_str!("../../../backend/contract/error.response.json");

    #[test]
    fn locations_example_parses_with_beacons_and_counts() {
        let response: LocationsResponse = serde_json::from_str(LOCATIONS).unwrap();
        assert!(!response.version.is_empty());
        assert!(!response.continents.is_empty());
        let locations: Vec<&Location> = response
            .continents
            .iter()
            .flat_map(|continent| &continent.locations)
            .collect();
        assert!(
            locations.iter().any(|location| location.beacon.is_none()),
            "the example carries a location without a beacon"
        );
        assert!(
            locations.iter().any(|location| location
                .beacon
                .as_ref()
                .is_some_and(|beacon| beacon.tcp_port.is_some())),
            "the example carries a TCP-pingable beacon"
        );
        assert!(
            locations.iter().any(|location| location
                .beacon
                .as_ref()
                .is_some_and(|beacon| beacon.tcp_port.is_none())),
            "the example carries a beacon without a TCP port"
        );
        for continent in &response.continents {
            let sum: u32 = continent.locations.iter().map(|l| l.matches).sum();
            assert_eq!(continent.matches, sum, "{}", continent.continent);
        }
    }

    /// The request this client builds serializes to exactly the checked-in example.
    #[test]
    fn join_request_serializes_to_the_example() {
        let example: serde_json::Value = serde_json::from_str(JOIN_REQUEST).unwrap();
        let locations = example["locations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_owned())
            .collect();
        let pings = example["pings"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, ms)| (id.clone(), u32::try_from(ms.as_u64().unwrap()).unwrap()))
            .collect();
        let request = JoinRequest {
            version: example["version"].as_str().unwrap().to_owned(),
            locations,
            pings,
        };
        assert_eq!(serde_json::to_value(&request).unwrap(), example);
    }

    #[test]
    fn join_and_ticket_requests_round_trip() {
        let joined: JoinResponse = serde_json::from_str(JOIN_RESPONSE).unwrap();
        let example: serde_json::Value = serde_json::from_str(TICKET_REQUEST).unwrap();
        assert_eq!(
            serde_json::to_value(TicketRequest {
                ticket: joined.ticket
            })
            .unwrap(),
            example
        );
    }

    #[test]
    fn ticket_examples_answer_each_state() {
        let starting: TicketResponse = serde_json::from_str(TICKET_STARTING).unwrap();
        assert_eq!(starting.answer(), Ok(TicketAnswer::Starting));

        let failed: TicketResponse = serde_json::from_str(TICKET_FAILED).unwrap();
        assert!(matches!(
            failed.answer(),
            Ok(TicketAnswer::Failed { reason }) if !reason.is_empty()
        ));

        let raw: serde_json::Value = serde_json::from_str(TICKET_READY).unwrap();
        let ready: TicketResponse = serde_json::from_str(TICKET_READY).unwrap();
        let Ok(TicketAnswer::Ready { address, port, key }) = ready.answer() else {
            panic!("the ready example answers ready");
        };
        assert_eq!(address, raw["address"].as_str().unwrap());
        assert_eq!(u64::from(port), raw["port"].as_u64().unwrap());
        let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, raw["key"].as_str().unwrap().to_ascii_lowercase());
    }

    #[test]
    fn error_example_parses() {
        let error: ErrorResponse = serde_json::from_str(ERROR).unwrap();
        assert!(!error.error.is_empty());
    }

    #[test]
    fn ready_without_its_fields_or_with_a_bad_key_is_an_error() {
        let incomplete: TicketResponse = serde_json::from_str(r#"{"state":"ready"}"#).unwrap();
        assert!(incomplete.answer().is_err());
        let short: TicketResponse = serde_json::from_str(
            r#"{"state":"ready","address":"198.51.100.7","port":1,"key":"00ff"}"#,
        )
        .unwrap();
        assert!(short.answer().is_err());
        let unknown: TicketResponse = serde_json::from_str(r#"{"state":"queued"}"#).unwrap();
        assert!(unknown.answer().is_err());
    }

    #[test]
    fn key_decodes_both_cases_and_rejects_non_hex() {
        let lower = "00".repeat(31) + "ff";
        assert_eq!(decode_key(&lower).unwrap()[31], 0xff);
        assert_eq!(decode_key(&lower.to_ascii_uppercase()).unwrap()[31], 0xff);
        assert!(decode_key(&("zz".repeat(32))).is_err());
        assert!(decode_key(&"ab".repeat(33)).is_err());
    }
}
