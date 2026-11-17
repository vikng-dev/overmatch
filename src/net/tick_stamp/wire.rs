//! Fixed twelve-byte codec; stale stamp decoding relies on Ledger's `replicate_diff` registration
//! setting Replicon's global `registry.needs_history()`. Without that registration, stale entity
//! mutations skip deserialization entirely.

use super::{ReceivedStamps, SerializationClock, TickStamp};
use bevy_replicon::prelude::RuleFns;
use serde::{Deserialize, Deserializer};

impl TickStamp {
    pub(in crate::net) fn new(
        frame_start_s: f64,
        departure_offset_s: f32,
    ) -> Result<Self, InvalidStamp> {
        if !frame_start_s.is_finite()
            || frame_start_s < 0.0
            || !departure_offset_s.is_finite()
            || departure_offset_s < 0.0
            || !(frame_start_s + f64::from(departure_offset_s)).is_finite()
        {
            return Err(InvalidStamp);
        }
        Ok(Self {
            frame_start_s,
            departure_offset_s,
        })
    }
}

#[derive(Deserialize)]
struct Fields {
    frame_start_s: f64,
    departure_offset_s: f32,
}

impl<'de> Deserialize<'de> for TickStamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = Fields::deserialize(deserializer)?;
        TickStamp::new(fields.frame_start_s, fields.departure_offset_s)
            .map_err(serde::de::Error::custom)
    }
}

pub(in crate::net) fn replication() -> RuleFns<TickStamp> {
    RuleFns::new(
        |ctx, stamp, message| {
            let clock =
                ctx.storage.global.get::<SerializationClock>().expect(
                    "running servers publish the frame clock before replication serialization",
                );
            // Resolution: f32 seconds adds four fixed bytes without capping long frames;
            // rounding is bounded by half an offset ULP.
            let offset = (clock.clock.0)()
                .duration_since(clock.frame_start)
                .as_secs_f64() as f32;
            let departing = TickStamp::new(stamp.frame_start_s, offset)
                .expect("the monotonic departure clock produces finite nonnegative stamps");
            // Replicon 0.45.5 collects/serializes changes before send_messages builds and queues
            // packets. Remaining collection, framing, transport, delivery and completion wait
            // still enter x; work before this callback does not.
            bevy_replicon::postcard_utils::to_extend_mut(&departing, message)?;
            Ok(())
        },
        |ctx, message| {
            let stamp = bevy_replicon::postcard_utils::from_buf::<Fields, _>(message)
                .map_err(StampRefusal::Encoding)
                .and_then(|fields| {
                    TickStamp::new(fields.frame_start_s, fields.departure_offset_s)
                        .map_err(StampRefusal::Invalid)
                });
            let stamp = stamp.map_err(|refusal| {
                // Replicon logs and discards decode errors. Preserve the typed first cause for
                // the client's existing ServerGone match-close path.
                ctx.storage.global.get_or_init(|| refusal.clone());
                bevy::ecs::error::BevyError::from(refusal)
            })?;
            ctx.storage
                .global
                .get_or_default::<ReceivedStamps>()
                .push(ctx.message_tick, stamp);
            Ok(stamp)
        },
    )
}

#[derive(Clone, Debug)]
pub(super) enum StampRefusal {
    Encoding(postcard::Error),
    Invalid(InvalidStamp),
}

impl std::fmt::Display for StampRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Server tick stamp refused: ")?;
        match self {
            Self::Encoding(error) => write!(f, "unreadable stamp: {error}"),
            Self::Invalid(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for StampRefusal {}

#[derive(Clone, Debug)]
pub(in crate::net) struct InvalidStamp;
impl std::fmt::Display for InvalidStamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "server tick frame start and departure offset must be finite nonnegative seconds",
        )
    }
}
impl std::error::Error for InvalidStamp {}
