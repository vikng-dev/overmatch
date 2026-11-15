//! The game's damage-model view, game state only: offline replay evidence for corner shaders.
//! The shooter projection is encoded and decoded at plan admission, then stored at its cue.
//! No round flies and no simulated HP pool changes.

use bevy::prelude::*;
use serde::Deserialize;
use std::path::PathBuf;

use crate::ballistics::spall_cone::{BurstFields, ConeShape};
use crate::damage_cam::PenetrationReport;
use crate::damage_cam::delivery::ShotExplanations;
use crate::damage_cam::wire::{ShooterReplay, ShotVocabulary, TankLife};
use crate::rounds::RoundRegistry;
use crate::tank::{Controlled, Tank};
use crate::{CombatantId, CombatantLife, ShotClock, ShotId};

/// The game's damage-model replay payload, admitted against this build before play.
#[derive(Deserialize, Clone, Debug)]
#[serde(try_from = "PathBuf")]
pub(crate) struct ReplayReport {
    path: PathBuf,
    payload: ShooterReplay,
}

impl TryFrom<PathBuf> for ReplayReport {
    type Error = String;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        let refusal = |error| {
            format!(
                "corner.report {}: {error}; use a report from this build's tank and round vocabulary",
                path.display()
            )
        };
        let report = PenetrationReport::load(&path).map_err(refusal)?;
        let spec = crate::bake::selected_spec();
        let geometry = crate::bake::extract_tank_geometry(
            &crate::assets::asset_root().join(crate::tank::TANK_SIM_GLB_PATH),
            spec,
            &crate::substances::SubstanceRegistry::shipped(),
        )
        .map_err(|findings| refusal(crate::bake::render(&findings)))?;
        let vocabulary = ShotVocabulary::new(
            [(crate::tank::TANK_SPEC_PATH, &geometry, spec)],
            RoundRegistry::shipped(),
            ConeShape::shipped(),
            BurstFields::shipped(),
        );
        let payload = ShooterReplay::build(&vocabulary, &report)
            .map_err(|error| refusal(error.to_string()))?;
        Ok(Self { path, payload })
    }
}

#[derive(Resource)]
struct Replay {
    at: f32,
    report: Option<ReplayReport>,
}

pub(crate) fn offline_corner_plugin(app: &mut App) {
    let Some(cue) = super::plan().and_then(|plan| plan.corner.as_ref()) else {
        return;
    };
    install(app, cue);
}

fn install(app: &mut App, cue: &super::plan::CornerReplay) {
    info!(
        "capture: game's damage-model corner replay {} at {} s",
        cue.report.path.display(),
        cue.at
    );
    app.insert_resource(Replay {
        at: cue.at,
        report: Some(cue.report.clone()),
    })
    .add_systems(
        Update,
        replay.before(crate::damage_cam::pen_cam::game::PenCamSet),
    );
}

fn replay(
    time: Res<Time>,
    clock: Res<ShotClock>,
    mut cue: ResMut<Replay>,
    player: Query<&CombatantId, With<Controlled>>,
    opponent: Query<(&CombatantId, &CombatantLife), (With<Tank>, Without<Controlled>)>,
    mut store: ResMut<ShotExplanations>,
) {
    if time.elapsed_secs() < cue.at || cue.report.is_none() {
        return;
    }
    let Some(&shooter) = crate::tank::controlled_one(player.single()) else {
        return; // Loading has no possession; the replay waits for the offline duel.
    };
    let (&combatant, &life) = opponent.single().unwrap_or_else(|error| {
        super::plan::refuse(super::plan::PlanError::Field(format!(
            "corner: requires one opponent ({error}); use the offline duel"
        )))
    });
    let report = cue
        .report
        .take()
        .expect("an unfired replay holds its admitted payload");
    store.replay_drawn(
        report.payload,
        ShotId {
            combatant: shooter,
            weapon: 0,
            fire_tick: clock.0.raw(),
        },
        TankLife { combatant, life },
        time.elapsed_secs_f64(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage_cam::delivery::ImpactPresence;
    use crate::damage_cam::delivery::tests::{SHOOTER, STRUCK};
    use crate::damage_cam::wire::Audience;

    /// The game's damage-model capture must reach the real shooter's store once at its cue,
    /// already presented, so the corner's ordinary admission can show it without a world impact.
    #[test]
    fn the_replay_addresses_only_the_shooter_once_at_its_cue() {
        // Fixture: a positive cue independent of the shipped capture recipe.
        let cue = super::super::plan::CornerReplay {
            at: 2.0,
            report: ReplayReport::try_from(PathBuf::from("assets/dev/pen_cam/s8b_mg_stopped.ron"))
                .unwrap(),
        };
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ShotClock>()
            .init_resource::<ShotExplanations>();
        install(&mut app, &cue);
        app.world_mut().spawn((Controlled, SHOOTER));
        app.world_mut().spawn((Tank, STRUCK, CombatantLife(0)));
        app.update();
        assert_eq!(app.world().resource::<ShotExplanations>().iter().count(), 0);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(cue.at));
        app.update();
        app.update();
        let store = app.world().resource::<ShotExplanations>();
        let entries: Vec<_> = store.iter().collect();
        assert_eq!(entries.len(), 1);
        let entry = entries[0].1;
        assert_eq!(entry.explanation.audience, Audience::Shooter);
        assert_eq!(entry.explanation.shot.combatant, SHOOTER);
        assert_eq!(entry.explanation.struck.combatant, STRUCK);
        assert_eq!(entry.presence, ImpactPresence::Presented);
    }
}
