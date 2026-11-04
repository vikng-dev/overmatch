//! The video game's destruction workload, game state only. `scripts/perf/collapse.sh --schedule`
//! owns `OVERMATCH_LOADTEST`: JSON naming map instances and wall times after combat admission.
//! Support-removal work is derived from each construction's own volume pools. The ordinary
//! deposit, law, launch and replication paths perform the break; this is an instrument, not fire.

use std::{collections::BTreeSet, time::Instant};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    battle::{BattlePhase, BattleState},
    harness::{LeverError, env_text},
};
use crate::{
    ballistics::{BallisticVolume, DeliveredLine, PenetrationMarks, ShellMarched, VolumeCrossing},
    construction::{
        Body, Element,
        breakage::{Breakage, Event, Record},
    },
    scatter::MapInstance,
    state::GameplaySet,
    tank::Tank,
};

const LEVER: &str = "OVERMATCH_LOADTEST";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Recipe {
    tanks: usize,
    events: Vec<Break>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Break {
    at_s: f64,
    instances: Vec<String>,
}

impl Recipe {
    fn parse(text: &str) -> Result<Self, LeverError> {
        let invalid = || {
            LeverError::invalid(
                LEVER,
                text,
                "JSON {tanks: positive count, events: [{at_s: finite nonnegative seconds in ascending order, instances: unique map IDs}]}; every instance occurs once",
            )
        };
        let recipe: Self = serde_json::from_str(text).map_err(|_| invalid())?;
        let mut ids = BTreeSet::new();
        if recipe.tanks == 0
            || recipe.events.is_empty()
            || recipe.events.iter().any(|event| {
                !event.at_s.is_finite()
                    || event.at_s < 0.0
                    || event.instances.is_empty()
                    || event
                        .instances
                        .iter()
                        .any(|id| id.is_empty() || !ids.insert(id))
            })
            || recipe
                .events
                .windows(2)
                .any(|pair| pair[0].at_s > pair[1].at_s)
        {
            return Err(invalid());
        }
        Ok(recipe)
    }
}

#[derive(Resource)]
pub(super) struct Loadtest {
    recipe: Recipe,
    state: RunState,
}

enum RunState {
    AwaitingAdmission,
    AwaitingCombat {
        targets: Vec<Vec<Target>>,
    },
    Running {
        started: Instant,
        /// Checked targets retain recipe event order; IDs are resolved only at admission.
        targets: Vec<Vec<Target>>,
        next: usize,
    },
}

struct Target {
    root: Entity,
    // No prior record entry means the static root and its initial element volumes still exist.
    lines: Vec<DeliveredLine>,
}

impl Loadtest {
    pub(super) fn json(
        &self,
        record: &Record,
        constructions: &Query<(&MapInstance, &Breakage), With<Body>>,
        now: Instant,
        period: std::time::Duration,
    ) -> Value {
        let targets: Vec<_> = constructions.iter().filter(|(id, _)| self.recipe.events.iter().any(|e| e.instances.contains(&id.id))).map(|(id, body)| {
            let falls = body.live.state.fate.iter().filter(|&&fate| fate == crate::construction::fracture::Fate::Fallen).count();
            let dust = record.entries.iter().filter(|entry| entry.construction == id.ordinal && matches!(entry.event, Event::Dust(_))).count();
            json!({"id": id.id, "falls": falls, "dust": dust, "chunks": body.kind.layout.chunks.len()})
        }).collect();
        let (elapsed, next) = match &self.state {
            RunState::AwaitingAdmission | RunState::AwaitingCombat { .. } => (None, 0),
            RunState::Running { started, next, .. } => {
                (Some(now.duration_since(*started).as_secs_f64()), *next)
            }
        };
        json!({"recipe": self.recipe, "elapsed_s": elapsed, "window_start_s": elapsed.map(|t| t - period.as_secs_f64()), "events_fired": next, "targets": targets})
    }
}

pub(super) fn install(app: &mut App) -> Result<(), LeverError> {
    let Some(text) = env_text(LEVER)? else {
        return Ok(());
    };
    install_value(app, &text)
}

fn install_value(app: &mut App, text: &str) -> Result<(), LeverError> {
    let recipe = Recipe::parse(text)?;
    app.insert_resource(Loadtest {
        recipe,
        state: RunState::AwaitingAdmission,
    })
    .add_systems(FixedUpdate, drive.in_set(GameplaySet));
    Ok(())
}

fn supports(body: &Breakage) -> BTreeSet<usize> {
    let anchored: BTreeSet<_> = body.kind.asset.anchors.iter().map(|a| a.chunk).collect();
    body.live
        .structure
        .chunks
        .iter()
        .enumerate()
        .filter(|(c, chunk)| chunk.buried || anchored.contains(&body.kind.layout.chunks[*c]))
        .map(|(c, _)| c)
        .collect()
}

fn drive(
    mut load: ResMut<Loadtest>,
    tanks: Query<(), With<Tank>>,
    battle: Query<&BattleState>,
    constructions: Query<(Entity, &MapInstance, &Transform, &Breakage), With<Body>>,
    volumes: Query<(Entity, &ChildOf, &Element), With<BallisticVolume>>,
    record: Res<Record>,
    mut commands: Commands,
) {
    let Loadtest { recipe, state } = &mut *load;
    if matches!(state, RunState::AwaitingAdmission) {
        // GameplaySet opens only after map admission: resolve operator IDs at its first tick.
        let mut targets = Vec::with_capacity(recipe.events.len());
        for event in &recipe.events {
            let mut resolved = Vec::with_capacity(event.instances.len());
            for id in &event.instances {
                let (root, identity, pose, body) = constructions
                    .iter()
                    .find(|(_, identity, ..)| identity.id == *id)
                    .unwrap_or_else(|| {
                        super::harness::refuse_start(LeverError::invalid(
                            LEVER,
                            id,
                            "a destructible map construction ID",
                        ))
                    });
                let supports = supports(body);
                if supports.is_empty()
                    || body
                        .kind
                        .asset
                        .anchors
                        .iter()
                        .any(|a| !body.kind.layout.chunks.contains(&a.chunk))
                {
                    super::harness::refuse_start::<()>(LeverError::invalid(
                        LEVER,
                        id,
                        "a construction whose ground supports are all breakable",
                    ));
                }
                let lines = supports
                    .iter()
                    .map(|&c| {
                        let chunk = &body.live.structure.chunks[c];
                        assert!(
                            body.live.state.standing(c),
                            "loadtest scheduled support must still stand; instance {} chunk {c}",
                            identity.id
                        );
                        let element = body.kind.layout.element_of[c];
                        let volume = volumes
                            .iter()
                            .find(|(_, parent, e)| parent.parent() == root && e.0 == element)
                            .expect("untouched scheduled support retains its element volume")
                            .0;
                        let at = pose.translation + pose.rotation * chunk.centroid;
                        // DERIVED: twice V * crush exceeds this scaled chunk's complete work pool.
                        // A zero-length line deposits at its centroid, without inventing a projectile.
                        let energy = 2.0 * chunk.volume * chunk.material.crush;
                        DeliveredLine {
                            crossing: VolumeCrossing {
                                volume,
                                entry: at,
                                end: at,
                                chord: 0.0,
                                cost: 0.0,
                                energy,
                                perforated: true,
                            },
                            contacts: vec![],
                            momentum: Vec3::ZERO,
                            entrance_momentum: Vec3::ZERO,
                            entrance_energy: 0.0,
                        }
                    })
                    .collect::<Vec<_>>();
                assert!(
                    !lines.is_empty(),
                    "admitted anchored construction has breakable supports"
                );
                resolved.push(Target { root, lines });
            }
            targets.push(resolved);
        }
        *state = RunState::AwaitingCombat { targets };
    }
    let combat = battle
        .iter()
        .any(|b| matches!(b.phase, BattlePhase::Combat { .. }));
    if let RunState::AwaitingCombat { targets } = state {
        // A tankless waiting server cannot start the workload clock. The runner's watchdog
        // refuses failed population/combat admission; map IDs are already checked.
        if tanks.iter().count() != recipe.tanks || !combat {
            return;
        }
        *state = RunState::Running {
            started: Instant::now(),
            targets: std::mem::take(targets),
            next: 0,
        };
        println!("loadtest {}", json!({"started": true, "recipe": recipe}));
    }
    if !combat {
        super::harness::refuse_start::<()>(LeverError::invalid(
            LEVER,
            "Battle ended",
            "one uninterrupted combat; shorten the observation/schedule",
        ));
    }
    let RunState::Running {
        started,
        targets,
        next,
    } = state
    else {
        unreachable!("pending admission returns before event dispatch");
    };
    let elapsed = started.elapsed().as_secs_f64();
    while let Some(event) = recipe.events.get(*next) {
        if elapsed < event.at_s {
            break;
        }
        // File order defines the cross-owner effect order. Every event due now is queued before
        // the normal FixedPostUpdate deposit consumer, so simultaneous breaks share one tick.
        for target in std::mem::take(&mut targets[*next]) {
            let (_, identity, _, _) = constructions
                .get(target.root)
                .expect("the current world retains each admitted construction root");
            if record
                .entries
                .iter()
                .any(|entry| entry.construction == identity.ordinal)
            {
                super::harness::refuse_start::<()>(LeverError::invalid(
                    LEVER,
                    &identity.id,
                    "a scheduled target untouched before its event; choose another map ID",
                ));
            }
            let shell = commands.spawn(PenetrationMarks::default()).id();
            commands.trigger(ShellMarched {
                shell,
                marks_from: 0,
                cones_from: 0,
                burst: None,
                lines: target.lines,
            });
            commands.entity(shell).despawn();
        }
        println!(
            "loadtest {}",
            json!({"event": next, "at_s": event.at_s, "actual_s": elapsed, "instances": event.instances})
        );
        *next += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Operators rely on an exact recipe: malformed timing, reused targets and unknown fields
    /// must refuse instead of silently changing the requested concurrency.
    #[test]
    fn recipes_refuse_ambiguous_or_malformed_workloads() {
        let good = r#"{"tanks":6,"events":[{"at_s":1,"instances":["a","b"]}]}"#;
        assert_eq!(Recipe::parse(good).unwrap().events[0].instances.len(), 2);
        for bad in [
            good.replace("\"tanks\":6", "\"tanks\":0"),
            good.replace("\"at_s\":1", "\"at_s\":-1"),
            good.replace("\"a\",\"b\"", "\"a\",\"a\""),
            good.replace("\"tanks\":6", "\"tanks\":6,\"typo\":1"),
        ] {
            assert!(Recipe::parse(&bad).is_err(), "{bad}");
        }
    }

    /// The game's destruction instrument must wait for population and combat, then feed the
    /// production deposit/law/launch path. Two independently placed columns lose support in one
    /// authority tick; removing the installed producer or its observer must break this witness.
    #[test]
    fn admitted_simultaneous_breaks_reach_the_production_ledger() {
        use avian3d::prelude::Gravity;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            crate::offline_physics(),
            crate::construction::breakage::sim_plugin,
            crate::debris::sim_plugin,
            crate::contact_transfer::sim_plugin,
            crate::ballistics::mount_march_probe,
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity::ZERO)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        install_value(
            &mut app,
            r#"{"tanks":1,"events":[{"at_s":0,"instances":["a","b"]}]}"#,
        )
        .unwrap();
        super::super::test_harness::finish(&mut app);
        let kind = crate::construction::tests::column_construction();
        for (ordinal, id) in [(0, "a"), (1, "b")] {
            // Fixture: separated columns above the map, using the fixture's admitted unit scale.
            crate::construction::spawn(
                &mut app.world_mut().commands(),
                &kind,
                MapInstance {
                    id: id.into(),
                    ordinal,
                },
                Transform::from_xyz(f32::from(ordinal) * 10.0, 1000.0, 0.0),
            );
        }
        app.world_mut().flush();
        let battle = app.world_mut().spawn(BattleState::default()).id();
        app.update();
        app.update();
        assert!(
            matches!(
                app.world().resource::<Loadtest>().state,
                RunState::AwaitingCombat { .. }
            ),
            "no tanks cannot start a workload"
        );
        app.world_mut().spawn(Tank);
        app.update();
        assert!(
            matches!(
                app.world().resource::<Loadtest>().state,
                RunState::AwaitingCombat { .. }
            ),
            "waiting is not combat"
        );
        app.world_mut().entity_mut(battle).insert(BattleState {
            number: 1,
            phase: BattlePhase::Combat {
                ends_at: crate::sim_tick::SimTick::new(1000),
                late_join: super::super::battle::LateJoin::Closed,
            },
        });
        app.update();
        assert!(matches!(
            app.world().resource::<Loadtest>().state,
            RunState::Running { next: 1, .. }
        ));
        let mut constructions = app
            .world_mut()
            .query_filtered::<(&MapInstance, &Breakage), With<Body>>();
        let record = app.world().resource::<Record>();
        let expected: BTreeSet<_> = (0..kind.asset.chunks.len())
            .filter(|&c| !kind.asset.anchors.iter().any(|a| a.chunk as usize == c))
            .map(|c| c as u16)
            .collect();
        let mut ticks = BTreeSet::new();
        for ordinal in [0, 1] {
            let (_, body) = constructions
                .iter(app.world())
                .find(|(id, _)| id.ordinal == ordinal)
                .unwrap();
            let mut shape = crate::construction::breakage::Shape::intact(&kind.layout);
            for entry in record
                .entries
                .iter()
                .filter(|entry| entry.construction == ordinal)
            {
                ticks.insert(entry.tick);
                shape.apply(entry.event, &kind.layout);
            }
            shape.derive_departures(&kind.layout, &body.live);
            let fallen = shape
                .fate
                .iter()
                .enumerate()
                .filter(|(_, fate)| **fate == crate::construction::fracture::Fate::Fallen)
                .map(|(c, _)| kind.layout.chunks[c] as u16)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                fallen, expected,
                "the producing update must publish causes for every unsupported chunk to depart"
            );
        }
        assert_eq!(
            ticks.len(),
            1,
            "concurrent breaks must share one authority tick"
        );
    }

    /// The game's destruction workload must remove buried supports as well as anchor faces;
    /// leaving either ground path behind changes a requested collapse into a partial break.
    #[test]
    fn support_selection_includes_buried_matter_without_an_anchor_face() {
        let kind = crate::construction::tests::column_construction();
        let mut body = Breakage::new(kind, 0, 1.0);
        // Fixture: a second ground-held chunk has no authored anchor face.
        let extra = body.live.structure.chunks.len() - 1;
        std::sync::Arc::make_mut(&mut body.live.structure).chunks[extra].buried = true;
        let selected = supports(&body);
        assert!(selected.contains(&extra));
        for anchor in &body.kind.asset.anchors {
            let law = body
                .kind
                .layout
                .chunks
                .iter()
                .position(|&c| c == anchor.chunk)
                .unwrap();
            assert!(
                selected.contains(&law),
                "authored anchors also support the construction"
            );
        }
    }
}
