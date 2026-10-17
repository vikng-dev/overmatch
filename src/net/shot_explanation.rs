//! The pen cam's explanations on the wire. The authority builds each [`RecordedShot`] once and sends
//! each projection over [`ShotExplanationChannel`] to its one recipient's link: the shooter's and
//! the struck tank's player's, nobody else's. A client stores what arrives in [`ShotExplanations`]
//! and marks an explanation's impact drawn as its own shells draw the shot's authority outcomes
//! ([`AuthorityOutcomeDrawn`]).
//!
//! A recipient is resolved by combatant, not by tank life: the link that controls the combatant's
//! tank now, so a record of a killed tank still reaches its player after the respawn.
//!
//! Under `SPIKE_SHOT_TRACE` both ends log the delivery to the shot-lifecycle trace: the server each
//! projection it sends (`xsend`), a client each explanation it stores (`xrx`, with its presence on
//! arrival), each later presence change (`xpres`) and each authority outcome it draws (`xdraw`).
//! `scripts/shot/pencam_latency.py` joins them.

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::*;
use lightyear::prelude::server::Server;
use lightyear::prelude::*;
use serde_json::json;

use super::protocol::{NetTank, ShotExplanationChannel};
use crate::CombatantId;
use crate::ballistics::AuthorityOutcomeDrawn;
use crate::damage_cam::delivery::{
    ExplanationKey, ImpactPresence, ShotExplanations, expire_explanations,
};
use crate::damage_cam::pen_cam::game::CloseCorner;
use crate::damage_cam::recorder::RecordedShot;
use crate::damage_cam::wire::{Audience, BuiltExplanation, ShotExplanation, ShotVocabulary};
use crate::shot_trace::ShotTrace;
use crate::state::GameplaySet;

pub(super) fn install_server(app: &mut App) {
    app.add_message::<RecordedShot>()
        .add_systems(FixedUpdate, send_shot_explanations.after(GameplaySet));
}

pub(super) fn install_client(app: &mut App) {
    app.init_resource::<ShotExplanations>()
        .add_observer(reset_shot_explanations)
        .add_observer(close_corner)
        .add_observer(note_outcome_drawn)
        .add_systems(
            Update,
            (
                receive_shot_explanations,
                expire_explanations,
                trace_presence.run_if(resource_exists::<ShotTrace>),
            )
                .chain(),
        );
}

/// Build each finished record once and send each projection to the link controlling its
/// combatant's tank, where that link is connected. A bot's combatant has no link and is sent
/// nothing. No life is checked: a record finished after its tank respawned is sent to the same
/// player as history, and its struck life keeps it out of the new life's live view
/// ([`ShotExplanations::live`]).
fn send_shot_explanations(
    mut records: MessageReader<RecordedShot>,
    vocabulary: Option<Res<ShotVocabulary>>,
    servers: Query<&Server>,
    tanks: Query<(&CombatantId, &ControlledBy), With<NetTank>>,
    mut sender: ServerMultiMessageSender,
    mut trace: Option<ResMut<ShotTrace>>,
) {
    if records.is_empty() {
        return;
    }
    let (Some(vocabulary), Ok(server)) = (vocabulary, servers.single()) else {
        warn_once!("server: a shot was recorded with no shot vocabulary or no server to send it");
        records.clear();
        return;
    };
    let links: HashMap<CombatantId, Entity> = tanks
        .iter()
        .map(|(&combatant, controlled)| (combatant, controlled.owner))
        .collect();
    for recorded in records.read() {
        let built = match BuiltExplanation::build(&vocabulary, recorded) {
            Ok(built) => built,
            Err(error) => {
                warn!("server: a recorded shot was not explained: {error}");
                continue;
            }
        };
        for (combatant, explanation) in built.projections() {
            let Some(&link) = links.get(&combatant) else {
                continue;
            };
            if !server.collection().contains(&link) {
                continue;
            }
            if let Err(err) = sender
                .send_to_entities::<ShotExplanation, ShotExplanationChannel>(&explanation, [link])
            {
                error!("server: a shot explanation could not enter transport: {err}");
                continue;
            }
            crate::shot_trace::record(
                &mut trace,
                "xsend",
                explanation.completion_tick,
                explanation.shot,
                || {
                    let mut row = explanation_fields(&explanation);
                    row["to"] = combatant.0.into();
                    row
                },
            );
        }
    }
}

/// Drain every arrived explanation into the store. `Update`: lightyear clears an undrained
/// receiver in `Last` every frame.
fn receive_shot_explanations(
    mut receivers: Query<&mut MessageReceiver<ShotExplanation>>,
    vocabulary: Option<Res<ShotVocabulary>>,
    time: Res<Time<Virtual>>,
    mut store: ResMut<ShotExplanations>,
) {
    let now = time.elapsed_secs_f64();
    for mut receiver in &mut receivers {
        for explanation in receiver.receive() {
            match vocabulary.as_deref() {
                Some(vocabulary) => {
                    store.receive(explanation, vocabulary, now);
                }
                None => warn_once!("client: a shot explanation arrived before the vocabulary"),
            }
        }
    }
}

fn note_outcome_drawn(
    drawn: On<AuthorityOutcomeDrawn>,
    mut store: ResMut<ShotExplanations>,
    timeline: Res<LocalTimeline>,
    mut trace: Option<ResMut<ShotTrace>>,
) {
    store.drew(drawn.shot, drawn.ordinal);
    crate::shot_trace::record(
        &mut trace,
        "xdraw",
        timeline.tick().0,
        drawn.shot,
        || json!({ "o": drawn.ordinal }),
    );
}

/// Trace each stored explanation on the frame it is first seen in the store (`xrx`: its arrival,
/// since this runs after the drain) and on the frame it leaves [`ImpactPresence::Pending`]
/// (`xpres`). `seen` and `pending` hold only keys at or above the store's oldest, which retention
/// never readmits below.
fn trace_presence(
    store: Res<ShotExplanations>,
    time: Res<Time<Virtual>>,
    timeline: Res<LocalTimeline>,
    mut trace: Option<ResMut<ShotTrace>>,
    mut seen: Local<BTreeSet<ExplanationKey>>,
    mut pending: Local<BTreeSet<ExplanationKey>>,
) {
    let now = time.elapsed_secs_f64();
    let tick = timeline.tick().0;
    for (key, stored) in store.iter() {
        let kind = if seen.insert(*key) {
            if stored.presence == ImpactPresence::Pending {
                pending.insert(*key);
            }
            "xrx"
        } else if stored.presence != ImpactPresence::Pending && pending.remove(key) {
            "xpres"
        } else {
            continue;
        };
        crate::shot_trace::record(&mut trace, kind, tick, stored.explanation.shot, || {
            let mut row = explanation_fields(&stored.explanation);
            row["pres"] = presence_name(stored.presence).into();
            row["vs"] = now.into();
            row
        });
    }
    match store.iter().next() {
        Some((&oldest, _)) => {
            seen.retain(|key| *key >= oldest);
            pending.retain(|key| *key >= oldest);
        }
        None => {
            seen.clear();
            pending.clear();
        }
    }
}

/// An explanation's identity beyond its shot, for the trace: the struck life, the encounter, the
/// outcome a client draws as its impact, the authority's contact and completion ticks, and the
/// audience.
fn explanation_fields(explanation: &ShotExplanation) -> serde_json::Value {
    json!({
        "sc": explanation.struck.combatant.0,
        "sl": explanation.struck.life.0,
        "en": explanation.encounter,
        "o": explanation.outcome,
        "ct": explanation.contact_tick,
        "cpt": explanation.completion_tick,
        "aud": match explanation.audience {
            Audience::Shooter => "shooter",
            Audience::Victim => "victim",
            Audience::KilledVictim => "killed_victim",
        },
    })
}

fn presence_name(presence: ImpactPresence) -> &'static str {
    match presence {
        ImpactPresence::Pending => "pending",
        ImpactPresence::Presented => "presented",
        ImpactPresence::TimedOut => "timed_out",
    }
}

/// A new connection is a new match: its shots are not the last one's.
fn reset_shot_explanations(_connected: On<Add, Connected>, mut store: ResMut<ShotExplanations>) {
    *store = default();
}

/// The link ended: a corner explaining one of its shots closes with it.
fn close_corner(_ended: On<Remove, Connected>, mut commands: Commands) {
    commands.trigger(CloseCorner);
}

#[cfg(test)]
mod tests;
