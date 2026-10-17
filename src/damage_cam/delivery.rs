//! A shot explanation's way to its recipient: the vocabulary both ends fold, the recipient's store
//! of what arrived, and the single-player delivery.
//!
//! The authority builds each [`RecordedShot`] once ([`BuiltExplanation::build`]) and hands each
//! projection ([`BuiltExplanation::projections`]) to its combatant. Across the network that is
//! `net::shot_explanation`; in single-player it is [`offline_plugin`], the same two calls and the
//! same [`ShotExplanations::receive`], without the hop.
//!
//! The store holds what a recipient's views read: each explanation decoded once, keyed by its shot,
//! struck life and encounter, never by arrival. An explanation is released to a view once the
//! client has drawn its world impact ([`ImpactPresence::Presented`]) or has waited
//! [`IMPACT_TIMEOUT_S`] for it ([`ImpactPresence::TimedOut`]). Opening a view is the view's.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;

use super::PenetrationReport;
use super::recorder::RecordedShot;
use super::wire::{BuiltExplanation, CodecError, ShotExplanation, ShotVocabulary, TankLife};
use crate::bake::TankBlueprint;
use crate::ballistics::spall_cone::ConeShape;
use crate::rounds::RoundRegistry;
use crate::state::GameplaySet;
use crate::tank::Controlled;
use crate::{CombatantId, ShotClock, ShotId, WeaponClock};

/// How many explanations a recipient keeps, and how many drawn outcomes. TUNED: [`READ_WINDOW_S`]
/// of explanations arriving at 12.8 a second. A recipient receives one explanation per encounter of
/// every recorded round it fires and of every one that strikes it, so its load is data: the
/// combatants that can strike it and itself, × each one's recorded-round rate (one over its
/// weapon's single-fire reload), × the encounters a round can make (a perforating round meets more
/// than one tank, or one tank again). A load above RETAINED / [`READ_WINDOW_S`] evicts explanations
/// a view may still read; the store counts those and logs the first.
pub(crate) const RETAINED: usize = 128;

/// How long after its arrival an explanation may still be read, virtual seconds. TUNED: the
/// longest view that reads one, the kill cam (its phases in `pen_cam::killcam`), rounded up.
pub(crate) const READ_WINDOW_S: f64 = 10.0;

/// How long an explanation waits for its world impact to be drawn before it is released undrawn,
/// seconds of virtual time from its arrival. TUNED: the impact draws at the interpolation cursor,
/// a fraction of a second behind the authority, and a lost outcome resends within a round trip;
/// past a second the client's picture of the shot is missing, not late.
pub(crate) const IMPACT_TIMEOUT_S: f64 = 1.0;

/// [`ShotVocabulary`] from the tank blueprint, on every composition that has one.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PreUpdate,
        build_vocabulary
            .run_if(resource_exists::<TankBlueprint>)
            .run_if(not(resource_exists::<ShotVocabulary>)),
    );
}

/// Every vehicle this build spawns, the shipped round registry and the shipped cone shape.
fn build_vocabulary(blueprint: Res<TankBlueprint>, mut commands: Commands) {
    commands.insert_resource(ShotVocabulary::new(
        [(
            blueprint.file.as_str(),
            &*blueprint.geometry,
            &*blueprint.spec,
        )],
        RoundRegistry::shipped(),
        ConeShape::shipped(),
    ));
}

/// Where a stored explanation's world impact stands on this client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImpactPresence {
    /// Not yet drawn; a view waits.
    Pending,
    /// Drawn: this client has drawn the shot's authority outcome that is this encounter's impact
    /// ([`ShotExplanation::outcome`]); offline, the authority drew it.
    Presented,
    /// Not drawn within [`IMPACT_TIMEOUT_S`] of arrival: released undrawn, for good. A view that
    /// must show the shot (the kill cam) shows it; one that rides on the impact (the corner) may
    /// skip it.
    TimedOut,
}

/// A stored explanation's place: its shot, its struck life and its encounter, in tick order. Two
/// tanks one round struck share a shot and an encounter number; the struck life parts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ExplanationKey {
    fire_tick: u32,
    shooter: u64,
    weapon: u8,
    struck: u64,
    life: u32,
    encounter: u32,
}

impl ExplanationKey {
    pub(crate) fn of(explanation: &ShotExplanation) -> Self {
        let shot = explanation.shot;
        Self {
            fire_tick: shot.fire_tick,
            shooter: shot.combatant.0,
            weapon: shot.weapon,
            struck: explanation.struck.combatant.0,
            life: explanation.struck.life.0,
            encounter: explanation.encounter,
        }
    }

    fn shot(&self) -> ShotKey {
        (self.fire_tick, self.shooter, self.weapon)
    }
}

/// A shot, in tick order.
type ShotKey = (u32, u64, u8);

fn shot_key(shot: ShotId) -> ShotKey {
    (shot.fire_tick, shot.combatant.0, shot.weapon)
}

/// One received explanation and the report it decodes to.
#[derive(Clone, Debug)]
pub(crate) struct StoredExplanation {
    pub explanation: ShotExplanation,
    pub report: PenetrationReport,
    pub presence: ImpactPresence,
    /// Virtual seconds at arrival.
    arrived_s: f64,
}

/// What [`ShotExplanations::receive`] did with an explanation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Receipt {
    Stored,
    /// Already held, or past retention: nothing changed.
    Ignored,
    /// It does not decode against this vocabulary: dropped.
    Refused,
}

/// This recipient's explanations, at most [`RETAINED`], and the outcomes it has drawn, at most
/// [`RETAINED`], each a shot and its outcome's ordinal.
#[derive(Resource, Default)]
pub(crate) struct ShotExplanations {
    stored: BTreeMap<ExplanationKey, StoredExplanation>,
    drawn: BTreeSet<(ShotKey, u32)>,
    /// The highest key retention dropped; nothing at or below it is stored again.
    evicted: Option<ExplanationKey>,
    /// Explanations retention dropped within [`READ_WINDOW_S`] of their arrival.
    evicted_early: u64,
    refused: u64,
}

impl ShotExplanations {
    /// Store `explanation`, decoded against `vocabulary`, arrived at `now_s` (virtual seconds),
    /// presented once this client draws its impact. A duplicate, or an explanation at or below the
    /// last one retention dropped, changes nothing. One that does not decode is refused: counted,
    /// and the first refusal logged.
    pub(crate) fn receive(
        &mut self,
        explanation: ShotExplanation,
        vocabulary: &ShotVocabulary,
        now_s: f64,
    ) -> Receipt {
        let drawn = explanation
            .outcome
            .is_some_and(|ordinal| self.drawn.contains(&(shot_key(explanation.shot), ordinal)));
        self.admit(explanation, vocabulary, now_s, drawn)
    }

    /// [`Self::receive`] an explanation whose impact is already drawn: the authority's own, which
    /// draws every impact of its shells.
    pub(crate) fn receive_drawn(
        &mut self,
        explanation: ShotExplanation,
        vocabulary: &ShotVocabulary,
        now_s: f64,
    ) -> Receipt {
        self.admit(explanation, vocabulary, now_s, true)
    }

    fn admit(
        &mut self,
        explanation: ShotExplanation,
        vocabulary: &ShotVocabulary,
        now_s: f64,
        drawn: bool,
    ) -> Receipt {
        let key = ExplanationKey::of(&explanation);
        if self.stored.contains_key(&key) || self.evicted.is_some_and(|floor| key <= floor) {
            return Receipt::Ignored;
        }
        let report = match explanation.decode(vocabulary) {
            Ok(report) => report,
            Err(error) => {
                self.refuse(&error);
                return Receipt::Refused;
            }
        };
        let presence = if drawn {
            ImpactPresence::Presented
        } else {
            ImpactPresence::Pending
        };
        self.stored.insert(
            key,
            StoredExplanation {
                explanation,
                report,
                presence,
                arrived_s: now_s,
            },
        );
        while self.stored.len() > RETAINED {
            if let Some((evicted, stored)) = self.stored.pop_first() {
                self.evicted = self.evicted.max(Some(evicted));
                if now_s - stored.arrived_s < READ_WINDOW_S {
                    self.evict_early();
                }
            }
        }
        if self.stored.contains_key(&key) {
            Receipt::Stored
        } else {
            Receipt::Ignored
        }
    }

    /// Count an explanation dropped while a view may still read it; log the first.
    fn evict_early(&mut self) {
        if self.evicted_early == 0 {
            warn!(
                "pen cam: an explanation was dropped within {READ_WINDOW_S} s of its arrival, \
                 more than {RETAINED} arriving in that window; further drops are counted"
            );
        }
        self.evicted_early += 1;
    }

    /// Count a refusal; log the first.
    fn refuse(&mut self, error: &CodecError) {
        if self.refused == 0 {
            warn!(
                "pen cam: a shot explanation was refused ({error}); further refusals are counted"
            );
        }
        self.refused += 1;
    }

    /// This client drew `shot`'s authority outcome `ordinal`: the waiting explanation whose impact
    /// it is, if one has arrived, is presented; one arriving later is stored presented. Another
    /// encounter of the shot waits for its own outcome.
    pub(crate) fn drew(&mut self, shot: ShotId, ordinal: u32) {
        let key = shot_key(shot);
        self.drawn.insert((key, ordinal));
        while self.drawn.len() > RETAINED {
            self.drawn.pop_first();
        }
        for stored in self.stored.values_mut() {
            if stored.presence == ImpactPresence::Pending
                && stored.explanation.outcome == Some(ordinal)
                && ExplanationKey::of(&stored.explanation).shot() == key
            {
                stored.presence = ImpactPresence::Presented;
            }
        }
    }

    /// Release, undrawn, every explanation that has waited [`IMPACT_TIMEOUT_S`] by `now_s`.
    pub(crate) fn expire(&mut self, now_s: f64) {
        for stored in self.stored.values_mut() {
            if stored.presence == ImpactPresence::Pending
                && now_s - stored.arrived_s >= IMPACT_TIMEOUT_S
            {
                stored.presence = ImpactPresence::TimedOut;
            }
        }
    }

    /// The explanation at `key`.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "its readers, the pen cam's views, are not yet built"
        )
    )]
    pub(crate) fn get(&self, key: &ExplanationKey) -> Option<&StoredExplanation> {
        self.stored.get(key)
    }

    /// Every stored explanation, in tick order, whatever life it struck: the corner's, the kill
    /// cam's and the history's.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&ExplanationKey, &StoredExplanation)> {
        self.stored.iter()
    }

    /// The explanations of shots that struck `life`, in tick order: what a live view of that
    /// tank reads. A record of the combatant's earlier life is never among them.
    pub(crate) fn live(&self, life: TankLife) -> impl Iterator<Item = &StoredExplanation> {
        self.stored
            .values()
            .filter(move |stored| stored.explanation.struck == life)
    }

    /// How many explanations were refused.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "its readers, the pen cam's views, are not yet built"
        )
    )]
    pub(crate) fn refused(&self) -> u64 {
        self.refused
    }
}

/// Release, each frame, every explanation that has waited out [`IMPACT_TIMEOUT_S`].
pub(crate) fn expire_explanations(time: Res<Time<Virtual>>, mut store: ResMut<ShotExplanations>) {
    store.expire(time.elapsed_secs_f64());
}

/// Single-player: the shot clock, and every record explained to the player, without a network hop.
///
/// The clock is [`WeaponClock`]'s tick, published before gameplay as the network compositions
/// publish theirs, so every round the player's tank and its opponents fire carries a [`ShotId`].
/// Each tank the scenario spawns carries its combatant identity from spawn.
pub(crate) fn offline_plugin(app: &mut App) {
    app.init_resource::<ShotClock>()
        .init_resource::<ShotExplanations>()
        .add_message::<RecordedShot>()
        .add_systems(
            FixedUpdate,
            (
                publish_offline_shot_clock.before(GameplaySet),
                deliver_offline.after(GameplaySet),
            ),
        )
        .add_systems(Update, expire_explanations);
}

fn publish_offline_shot_clock(weapon: Res<WeaponClock>, mut shot: ResMut<ShotClock>) {
    shot.0 = weapon.0;
}

/// Build each record once and store the projection addressed to the player's combatant, its impact
/// drawn: the authority's own shell drew every impact on its tick, past a perforation too.
pub(crate) fn deliver_offline(
    mut records: MessageReader<RecordedShot>,
    vocabulary: Option<Res<ShotVocabulary>>,
    player: Query<&CombatantId, With<Controlled>>,
    time: Res<Time<Virtual>>,
    mut store: ResMut<ShotExplanations>,
) {
    let Some(vocabulary) = vocabulary else {
        if !records.is_empty() {
            warn_once!("pen cam: a shot was recorded before the shot vocabulary was built");
        }
        records.clear();
        return;
    };
    let now = time.elapsed_secs_f64();
    for recorded in records.read() {
        let built = match BuiltExplanation::build(&vocabulary, recorded) {
            Ok(built) => built,
            Err(error) => {
                warn!("pen cam: a recorded shot was not explained: {error}");
                continue;
            }
        };
        for (combatant, explanation) in built.projections() {
            if !player.iter().any(|&player| player == combatant) {
                continue;
            }
            store.receive_drawn(explanation, &vocabulary, now);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
