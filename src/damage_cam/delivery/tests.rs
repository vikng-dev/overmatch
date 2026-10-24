use super::*;
use crate::CombatantLife;
use crate::damage_cam::shapes::TankAsset;

/// Fixture identities: the shooter combatant 7, the struck combatant 9.
pub(crate) const SHOOTER: CombatantId = CombatantId(7);
pub(crate) const STRUCK: CombatantId = CombatantId(9);

/// A recorded scenario's report, by file stem.
pub(crate) fn report(name: &str) -> PenetrationReport {
    let path = crate::assets::asset_root().join(format!("dev/pen_cam/{name}.ron"));
    PenetrationReport::load(&path).unwrap()
}

/// The vocabulary of `report`'s vehicle, the shipped rounds and `cone`.
pub(crate) fn vocabulary_with(report: &PenetrationReport, cone: &ConeShape) -> ShotVocabulary {
    let file = report.vehicle.as_deref().unwrap();
    let asset = TankAsset::load(file).unwrap();
    ShotVocabulary::new(
        [(file, asset.geometry(), asset.spec())],
        RoundRegistry::shipped(),
        cone,
    )
}

pub(crate) fn vocabulary(report: &PenetrationReport) -> ShotVocabulary {
    vocabulary_with(report, ConeShape::shipped())
}

/// `report` as the authority records it: `shooter`'s round fired on `fire_tick` striking `life`,
/// contact a tick later, drawn at the shot's first outcome.
pub(crate) fn recorded(
    report: &PenetrationReport,
    shooter: CombatantId,
    fire_tick: u32,
    life: TankLife,
) -> RecordedShot {
    RecordedShot {
        tank: Entity::PLACEHOLDER,
        life: Some(life),
        shooter: None,
        shot: Some(ShotId {
            combatant: shooter,
            weapon: 0,
            fire_tick,
        }),
        encounter: 0,
        contact_tick: fire_tick + 1,
        completion_tick: fire_tick + 3,
        outcome: Some(0),
        hull: Some((Vec3::new(12.0, 0.8, -40.0), Quat::from_rotation_y(0.7))),
        report: report.clone(),
    }
}

fn life(life: u32) -> TankLife {
    TankLife {
        combatant: STRUCK,
        life: CombatantLife(life),
    }
}

/// The struck player's projection of `recorded`.
fn victim_projection(vocabulary: &ShotVocabulary, recorded: &RecordedShot) -> ShotExplanation {
    let built = BuiltExplanation::build(vocabulary, recorded).unwrap();
    let (combatant, explanation) = built.projections().into_iter().next().unwrap();
    assert_eq!(combatant, recorded.life.unwrap().combatant);
    explanation
}

/// The client draws `explanation`'s own impact.
fn draw(store: &mut ShotExplanations, explanation: &ShotExplanation) {
    store.drew(explanation.shot, explanation.outcome.unwrap());
}

fn presence(store: &ShotExplanations, explanation: &ShotExplanation) -> ImpactPresence {
    store
        .get(&ExplanationKey::of(explanation))
        .unwrap()
        .presence
}

/// The store as a view reads it: each explanation's key, its bytes and its presence, in key order.
fn snapshot(store: &ShotExplanations) -> Vec<(ExplanationKey, Vec<u8>, ImpactPresence)> {
    store
        .iter()
        .map(|(key, stored)| {
            (
                *key,
                postcard::to_allocvec(&stored.explanation).unwrap(),
                stored.presence,
            )
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
enum Step {
    /// Explanation `i` arrives.
    Arrive(usize),
    /// The client draws explanation `i`'s impact.
    Draw(usize),
}

/// Loss (a copy lost and resent last), reorder, duplicates, and an explanation arriving before or
/// after its impact all leave the store in one state.
#[test]
fn arrival_order_loss_and_duplicates_end_in_one_state() {
    let report = report("s1_through_crew");
    let vocabulary = vocabulary(&report);
    let explanations: Vec<ShotExplanation> = [1200, 1400, 1600]
        .into_iter()
        .map(|fire_tick| {
            victim_projection(&vocabulary, &recorded(&report, SHOOTER, fire_tick, life(1)))
        })
        .collect();
    let run = |steps: &[Step]| {
        let mut store = ShotExplanations::default();
        for (n, step) in steps.iter().enumerate() {
            match *step {
                Step::Arrive(i) => {
                    store.receive(explanations[i].clone(), &vocabulary, n as f64 * 0.01);
                }
                Step::Draw(i) => draw(&mut store, &explanations[i]),
            }
        }
        snapshot(&store)
    };
    use Step::{Arrive as A, Draw as D};
    let in_order = run(&[A(0), D(0), A(1), D(1), A(2), D(2)]);
    assert_eq!(in_order.len(), 3);
    assert!(
        in_order
            .iter()
            .all(|(_, _, presence)| *presence == ImpactPresence::Presented)
    );
    for (what, steps) in [
        ("impacts first", &[D(0), D(1), D(2), A(2), A(1), A(0)][..]),
        (
            "explanations first",
            &[A(0), A(1), A(2), D(2), D(1), D(0)][..],
        ),
        (
            "reordered with duplicates",
            &[A(2), A(0), D(1), A(2), A(1), D(0), A(0), D(2), A(1)][..],
        ),
        (
            "a lost copy resent last",
            &[A(0), D(0), D(1), A(2), D(2), A(1)][..],
        ),
    ] {
        assert_eq!(run(steps), in_order, "{what}");
    }
}

/// A record of an earlier life stays for the kill cam and history, and never reads as the live
/// view of the combatant's next life.
#[test]
fn a_respawned_life_never_reads_an_old_record_as_live() {
    let report = report("s1_through_crew");
    let vocabulary = vocabulary(&report);
    let mut store = ShotExplanations::default();
    let old = victim_projection(&vocabulary, &recorded(&report, SHOOTER, 1200, life(1)));
    assert_eq!(
        store.receive(old.clone(), &vocabulary, 0.0),
        Receipt::Stored
    );
    assert_eq!(store.live(life(2)).count(), 0);
    assert_eq!(store.live(life(1)).count(), 1);
    assert_eq!(store.iter().count(), 1);

    let new = victim_projection(&vocabulary, &recorded(&report, SHOOTER, 1300, life(2)));
    assert_eq!(
        store.receive(new.clone(), &vocabulary, 0.1),
        Receipt::Stored
    );
    let live: Vec<&StoredExplanation> = store.live(life(2)).collect();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].explanation, new);
    assert_eq!(
        store.get(&ExplanationKey::of(&old)).unwrap().explanation,
        old,
        "the earlier life's record is kept"
    );
}

/// An explanation folded against another vocabulary is refused and counted, never panicking and
/// never stored.
#[test]
fn a_fingerprint_mismatch_is_refused_without_panic() {
    let report = report("s1_through_crew");
    let vocabulary = vocabulary(&report);
    let other = vocabulary_with(&report, &ConeShape::shipped_at(128));
    assert_ne!(other.fingerprint(), vocabulary.fingerprint());
    let explanation = victim_projection(&vocabulary, &recorded(&report, SHOOTER, 1200, life(1)));
    let mut store = ShotExplanations::default();
    assert_eq!(
        store.receive(explanation.clone(), &other, 0.0),
        Receipt::Refused
    );
    assert_eq!(store.receive(explanation, &other, 0.1), Receipt::Refused);
    assert_eq!(store.refused(), 2);
    assert_eq!(store.iter().count(), 0);
}

/// An undrawn explanation is released at the timeout, for good; one drawn first never times out.
#[test]
fn an_undrawn_impact_times_out_once() {
    let report = report("s1_through_crew");
    let vocabulary = vocabulary(&report);
    let waiting = victim_projection(&vocabulary, &recorded(&report, SHOOTER, 1200, life(1)));
    let drawn = victim_projection(&vocabulary, &recorded(&report, SHOOTER, 1400, life(1)));
    let mut store = ShotExplanations::default();
    store.receive(waiting.clone(), &vocabulary, 10.0);
    store.receive(drawn.clone(), &vocabulary, 10.0);
    draw(&mut store, &drawn);
    store.expire(10.0 + IMPACT_TIMEOUT_S * 0.5);
    assert_eq!(presence(&store, &waiting), ImpactPresence::Pending);
    store.expire(10.0 + IMPACT_TIMEOUT_S);
    assert_eq!(presence(&store, &waiting), ImpactPresence::TimedOut);
    assert_eq!(presence(&store, &drawn), ImpactPresence::Presented);
    draw(&mut store, &waiting);
    assert_eq!(presence(&store, &waiting), ImpactPresence::TimedOut);
}

/// An encounter is presented by its own outcome only: neither another outcome of its shot (a bounce
/// off another tank before it, or the end the shot's flight came to there) nor the same outcome of
/// another shot presents it, before or after it arrives.
#[test]
fn only_the_encounters_own_outcome_presents_it() {
    let report = report("s1_through_crew");
    let vocabulary = vocabulary(&report);
    let mut after_a_bounce = recorded(&report, SHOOTER, 1200, life(1));
    after_a_bounce.outcome = Some(1);
    let explanation = victim_projection(&vocabulary, &after_a_bounce);
    let other_shot = ShotId {
        fire_tick: 1300,
        ..explanation.shot
    };
    for drawn_first in [false, true] {
        let mut store = ShotExplanations::default();
        if !drawn_first {
            store.receive(explanation.clone(), &vocabulary, 0.0);
        }
        store.drew(explanation.shot, 0);
        store.drew(explanation.shot, 2);
        store.drew(other_shot, 1);
        if drawn_first {
            store.receive(explanation.clone(), &vocabulary, 0.0);
        }
        assert_eq!(
            presence(&store, &explanation),
            ImpactPresence::Pending,
            "drawn first: {drawn_first}"
        );
        store.drew(explanation.shot, 1);
        assert_eq!(
            presence(&store, &explanation),
            ImpactPresence::Presented,
            "drawn first: {drawn_first}"
        );
    }
}

/// A round that perforates tank A and stops in tank B: a client's shell ends at A, the end the
/// authority sanctioned, so that draw presents A's explanation and never B's, which no client
/// draws; B's is released at the timeout.
#[test]
fn a_perforation_presents_only_the_tank_it_was_drawn_at() {
    let report = report("s1_through_crew");
    let vocabulary = vocabulary(&report);
    let a = victim_projection(&vocabulary, &recorded(&report, SHOOTER, 1200, life(1)));
    let mut into_b = recorded(&report, SHOOTER, 1200, life(1));
    into_b.life = Some(TankLife {
        combatant: CombatantId(11),
        life: CombatantLife(1),
    });
    into_b.contact_tick += 1;
    into_b.outcome = None;
    let b = victim_projection(&vocabulary, &into_b);
    assert_eq!(a.shot, b.shot);

    let mut store = ShotExplanations::default();
    store.receive(a.clone(), &vocabulary, 10.0);
    draw(&mut store, &a);
    store.receive(b.clone(), &vocabulary, 10.0);
    assert_eq!(presence(&store, &a), ImpactPresence::Presented);
    assert_eq!(presence(&store, &b), ImpactPresence::Pending);
    store.expire(10.0 + IMPACT_TIMEOUT_S * 0.5);
    assert_eq!(presence(&store, &b), ImpactPresence::Pending);
    store.expire(10.0 + IMPACT_TIMEOUT_S);
    assert_eq!(presence(&store, &a), ImpactPresence::Presented);
    assert_eq!(presence(&store, &b), ImpactPresence::TimedOut);
}

/// The store keeps the newest [`RETAINED`]; an evicted explanation arriving again changes nothing.
/// One dropped within [`READ_WINDOW_S`] of its arrival is counted; one dropped after is not.
#[test]
fn retention_keeps_the_newest_and_never_readmits() {
    let report = report("s1_through_crew");
    let vocabulary = vocabulary(&report);
    let explanations: Vec<ShotExplanation> = (0..RETAINED as u32 + 8)
        .map(|n| {
            victim_projection(
                &vocabulary,
                &recorded(&report, SHOOTER, 1000 + 10 * n, life(1)),
            )
        })
        .collect();
    let mut store = ShotExplanations::default();
    for explanation in explanations.iter().rev() {
        store.receive(explanation.clone(), &vocabulary, 0.0);
    }
    assert_eq!(store.iter().count(), RETAINED);
    assert_eq!(
        store.evicted_early, 1,
        "the first older arrival is dropped as it lands; the rest fall below the floor"
    );
    let first_kept = ExplanationKey::of(&explanations[8]);
    assert_eq!(store.iter().next().map(|(key, _)| *key), Some(first_kept));
    let before = snapshot(&store);
    assert_eq!(
        store.receive(explanations[3].clone(), &vocabulary, 0.0),
        Receipt::Ignored
    );
    assert_eq!(snapshot(&store), before);

    let mut store = ShotExplanations::default();
    for (n, explanation) in explanations.iter().enumerate() {
        store.receive(explanation.clone(), &vocabulary, n as f64 * READ_WINDOW_S);
    }
    assert_eq!(store.iter().count(), RETAINED);
    assert_eq!(
        store.evicted_early, 0,
        "each was dropped past its read window"
    );
}

/// Single-player: the player receives exactly its own projection of a recorded shot, through the
/// same build and projection the server sends, without a hop, its impact already drawn, past a
/// perforation too; a player who is neither party receives nothing. The offline clock is the weapon
/// clock's tick.
#[test]
fn offline_play_delivers_the_same_projections() {
    let report = report("s1_through_crew");
    let vocabulary = vocabulary(&report);
    let mut recorded_shot = recorded(&report, SHOOTER, 1200, life(1));
    recorded_shot.outcome = None;
    let built = BuiltExplanation::build(&vocabulary, &recorded_shot).unwrap();
    let projections = built.projections();
    assert_eq!(projections.len(), 2);

    for (player, expected) in [
        (STRUCK, Some(&projections[0].1)),
        (SHOOTER, Some(&projections[1].1)),
        (CombatantId(3), None),
    ] {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(offline_plugin);
        app.insert_resource(vocabulary.clone());
        app.insert_resource(WeaponClock(42));
        app.world_mut().spawn((player, Controlled));
        let mut recorded_shot = recorded(&report, SHOOTER, 1200, life(1));
        recorded_shot.outcome = None;
        app.world_mut().write_message(recorded_shot);
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<ShotClock>().0, 42);

        let store = app.world().resource::<ShotExplanations>();
        let stored: Vec<&StoredExplanation> = store.iter().map(|(_, stored)| stored).collect();
        match expected {
            Some(expected) => {
                assert_eq!(stored.len(), 1, "{player:?}");
                assert_eq!(
                    postcard::to_allocvec(&stored[0].explanation).unwrap(),
                    postcard::to_allocvec(expected).unwrap(),
                    "{player:?}"
                );
                assert_eq!(stored[0].presence, ImpactPresence::Presented);
                assert_eq!(stored[0].report.vehicle, report.vehicle);
            }
            None => assert!(stored.is_empty(), "{player:?}"),
        }
    }
}

/// A player firing as a [`FreeShooter`] at the tank it holds holds both combatants of one
/// encounter: offline delivery stores the shooter's projection and the victim's, one each, as the
/// corner reads the first and the x-ray the second. The armour sandbox's range relies on it.
#[test]
fn a_free_shooter_holding_its_target_keeps_both_projections() {
    let report = report("s1_through_crew");
    let vocabulary = vocabulary(&report);
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).add_plugins(offline_plugin);
    app.insert_resource(vocabulary);
    app.insert_resource(WeaponClock(0));
    app.world_mut().spawn((SHOOTER, FreeShooter));
    app.world_mut().spawn((STRUCK, Controlled));
    let mut recorded_shot = recorded(&report, SHOOTER, 1200, life(1));
    recorded_shot.outcome = None;
    app.world_mut().write_message(recorded_shot);
    app.world_mut().run_schedule(FixedUpdate);

    let store = app.world().resource::<ShotExplanations>();
    let audiences: Vec<Audience> = store
        .iter()
        .map(|(_, stored)| stored.explanation.audience)
        .collect();
    let to_shooter = audiences
        .iter()
        .filter(|&&audience| audience == Audience::Shooter)
        .count();
    assert_eq!(audiences.len(), 2, "{audiences:?}");
    assert_eq!(to_shooter, 1, "{audiences:?}");
}
