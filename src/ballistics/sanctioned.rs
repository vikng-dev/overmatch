//! The game's damage model: bounded, validated public paths awaiting cosmetic shells.
//! This buffer knows no scene geometry; duplicate authority sequences keep the first fact.

use super::path::{FlightOutcome, OutcomeError, ValidatedPath};
use crate::ShotId;
use bevy::prelude::*;

struct SanctionedShot {
    paths: Vec<ValidatedPath>,
    age: f32,
}

#[derive(Resource, Default)]
pub(crate) struct SanctionedShots {
    shots: std::collections::HashMap<ShotId, SanctionedShot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SanctionedInsert {
    Inserted,
    Duplicate,
    Refused(OutcomeError),
}

impl SanctionedShots {
    /// ASSUMED cosmetic retention, seconds; active shells copy paths before the buffer expires.
    pub(crate) const MAX_AGE_SECS: f32 = 3.0;
    /// DERIVED: the synchronized thirty-player automatic horizon fits below this power of two.
    const MAX_SHOTS: usize = 4096;

    pub(crate) fn insert(&mut self, path: FlightOutcome) -> SanctionedInsert {
        let path = match ValidatedPath::try_from(path) {
            Ok(path) => path,
            Err(error) => return SanctionedInsert::Refused(error),
        };
        if self.shots.get(&path.shot).is_some_and(|shot| {
            shot.paths
                .iter()
                .any(|known| known.sequence == path.sequence)
        }) {
            return SanctionedInsert::Duplicate;
        }
        if self.shots.len() >= Self::MAX_SHOTS && !self.shots.contains_key(&path.shot) {
            let oldest = self
                .shots
                .iter()
                .max_by(|(a_id, a), (b_id, b)| {
                    a.age.total_cmp(&b.age).then_with(|| {
                        (a_id.combatant.0, a_id.weapon, a_id.fire_tick).cmp(&(
                            b_id.combatant.0,
                            b_id.weapon,
                            b_id.fire_tick,
                        ))
                    })
                })
                .map(|(id, _)| *id)
                .expect("a full path buffer contains an oldest shot");
            self.shots.remove(&oldest);
        }
        let shot = self
            .shots
            .entry(path.shot)
            .or_insert_with(|| SanctionedShot {
                paths: Vec::new(),
                age: 0.0,
            });
        if shot.paths.len() >= super::resolve::MAX_STEP_CROSSINGS {
            return SanctionedInsert::Refused(OutcomeError::Capacity);
        }
        shot.age = 0.0;
        shot.paths.push(path);
        SanctionedInsert::Inserted
    }

    pub(crate) fn paths(&self, shot: ShotId) -> impl Iterator<Item = &ValidatedPath> {
        self.shots
            .get(&shot)
            .into_iter()
            .flat_map(|entry| &entry.paths)
    }

    #[cfg(test)]
    pub(crate) fn has_shot(&self, shot: ShotId) -> bool {
        self.shots.contains_key(&shot)
    }

    pub(crate) fn age(&mut self, dt: f32) {
        for shot in self.shots.values_mut() {
            shot.age += dt;
        }
        self.shots.retain(|_, shot| shot.age <= Self::MAX_AGE_SECS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's damage model: the public buffer holds a simultaneous automatic-fire horizon,
    /// and expiry removes old facts without changing paths already copied by an active shell.
    #[test]
    fn the_thirty_player_automatic_horizon_survives_until_expiry() {
        let mut buffer = SanctionedShots::default();
        // DERIVED: 750 rounds/minute * 3 seconds / 60 rounds up to 38 rounds per weapon.
        for combatant in 0..30 {
            for weapon in 0..2 {
                for fire_tick in 0..38 {
                    let shot = ShotId {
                        combatant: crate::CombatantId(combatant),
                        weapon,
                        fire_tick,
                    };
                    assert_eq!(
                        buffer.insert(super::super::path_fixture(shot, fire_tick)),
                        SanctionedInsert::Inserted
                    );
                }
            }
        }
        assert_eq!(buffer.shots.len(), 30 * 2 * 38);
        buffer.age(SanctionedShots::MAX_AGE_SECS);
        assert_eq!(buffer.shots.len(), 30 * 2 * 38);
        buffer.age(0.01); // Fixture step across the expiry boundary.
        assert!(buffer.shots.is_empty());
    }
}
