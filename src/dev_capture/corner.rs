//! The game's damage-model view, game state only: offline replay evidence for corner shaders.
//! A report is parsed once at mount, then addressed to the current shooter and opponent through
//! the normal projection/store path. No round flies and no simulated HP pool changes.

use bevy::prelude::*;

use crate::damage_cam::PenetrationReport;
use crate::damage_cam::delivery::{Receipt, ShotExplanations};
use crate::damage_cam::recorder::RecordedShot;
use crate::damage_cam::wire::{BuiltExplanation, ShotVocabulary, TankLife};
use crate::tank::{Controlled, Tank};
use crate::{CombatantId, CombatantLife, ShotClock, ShotId};

#[derive(Resource)]
struct Replay {
    at: f32,
    report: Option<PenetrationReport>,
}

pub(crate) fn offline_corner_plugin(app: &mut App) {
    let Some(cue) = super::plan().and_then(|plan| plan.corner.as_ref()) else {
        return;
    };
    install(app, cue);
}

fn install(app: &mut App, cue: &super::plan::CornerReplay) {
    let report = PenetrationReport::load(&cue.report).unwrap_or_else(|error| {
        super::plan::refuse(super::plan::PlanError::Field(format!(
            "corner.report: {error}"
        )))
    });
    if report.synthetic || report.vehicle.is_none() {
        super::plan::refuse(super::plan::PlanError::Field(
            "corner.report: use a simulated report naming its vehicle".into(),
        ));
    }
    info!(
        "capture: game's damage-model corner replay {} at {} s",
        cue.report.display(),
        cue.at
    );
    app.insert_resource(Replay {
        at: cue.at,
        report: Some(report),
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
    vocabulary: Option<Res<ShotVocabulary>>,
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
        .expect("an unfired replay holds its parsed report");
    let recorded = RecordedShot {
        life: Some(TankLife { combatant, life }),
        shot: Some(ShotId {
            combatant: shooter,
            weapon: 0,
            fire_tick: clock.0.raw(),
        }),
        encounter: 0,
        contact_tick: clock.0.raw(),
        completion_tick: clock.0.raw(),
        outcome: None,
        hull: None,
        report,
    };
    let vocabulary = vocabulary.expect("the offline duel builds its vocabulary before possession");
    let built = BuiltExplanation::build(&vocabulary, &recorded).unwrap_or_else(|error| {
        super::plan::refuse(super::plan::PlanError::Field(format!(
            "corner.report: {error}; use a report from this build's tank and round vocabulary"
        )))
    });
    for (recipient, explanation) in built.projections() {
        if recipient == shooter {
            assert_eq!(
                store.receive_drawn(explanation, &vocabulary, time.elapsed_secs_f64()),
                Receipt::Stored,
                "a fresh offline replay decodes against the vocabulary that encoded it"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage_cam::delivery::ImpactPresence;
    use crate::damage_cam::delivery::tests::{SHOOTER, STRUCK, vocabulary};
    use crate::damage_cam::wire::Audience;

    /// The game's damage-model capture must reach the real shooter's store once at its cue,
    /// already presented, so the corner's ordinary admission can show it without a world impact.
    #[test]
    fn the_replay_addresses_only_the_shooter_once_at_its_cue() {
        let plan = super::super::plan::CapturePlan::parse(include_str!(
            "../../scripts/capture/pen-cam-slice.ron"
        ))
        .unwrap();
        let cue = plan.corner.unwrap();
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ShotClock>()
            .init_resource::<ShotExplanations>();
        install(&mut app, &cue);
        let words = vocabulary(app.world().resource::<Replay>().report.as_ref().unwrap());
        app.insert_resource(words);
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
