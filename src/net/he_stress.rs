//! The video game's damage model, game state only: unattended real HE flight into the map.
//! `scripts/perf/collapse.sh` owns `OVERMATCH_HE_STRESS=<bursts>:<period_s>`.
//! No tank, gun controls or player transport: `FireShell` enters the ordinary authority march.

use bevy::prelude::*;

use super::harness::{LeverError, env_text};
use crate::ballistics::{FireShell, ShellMarched, ShotSource};
use crate::construction::{Body, breakage::Breakage};
use crate::rounds::fuze::Trigger;
use crate::scatter::MapInstance;
use crate::state::GameplaySet;

/// Operator recipe consumed by the local and Edgegap runners.
const LEVER: &str = "OVERMATCH_HE_STRESS";

#[derive(Clone, Copy, Debug, PartialEq)]
struct Recipe {
    bursts: u32,
    period_s: f64,
}

impl Recipe {
    fn parse(value: Option<&str>) -> Result<Option<Self>, LeverError> {
        let Some(value) = value.map(str::trim) else {
            return Ok(None);
        };
        if value.is_empty()
            || value.eq_ignore_ascii_case("off")
            || value.eq_ignore_ascii_case("false")
        {
            return Ok(None);
        }
        let invalid = || {
            LeverError::invalid(
                LEVER,
                value,
                "<u32 bursts>:<finite positive period_s>, or off",
            )
        };
        let (count, period) = value.split_once(':').ok_or_else(invalid)?;
        let bursts = count.trim().parse().map_err(|_| invalid())?;
        let period_s: f64 = period.trim().parse().map_err(|_| invalid())?;
        if !(period_s.is_finite() && period_s > 0.0) {
            return Err(invalid());
        }
        Ok((bursts > 0).then_some(Self { bursts, period_s }))
    }
}

#[derive(Resource)]
struct HeStress {
    recipe: Recipe,
    round: crate::rounds::Round,
    fired: u32,
    burst: u32,
    next_s: f64,
}

pub(super) fn install_server(app: &mut App) -> Result<(), LeverError> {
    install_value(app, env_text(LEVER)?.as_deref())
}

fn install_value(app: &mut App, value: Option<&str>) -> Result<(), LeverError> {
    let Some(recipe) = Recipe::parse(value)? else {
        return Ok(());
    };
    // Registry order, then impact-fuzed burst: HE is selected by mechanism, never an asset name.
    let registry = crate::rounds::RoundRegistry::shipped();
    let key = registry
        .keys()
        .find(|key| {
            registry
                .get(key)
                .expect("a registry key resolves")
                .burst
                .is_some_and(|burst| matches!(burst.trigger, Trigger::Impact { .. }))
        })
        .expect("HE stress requires a shipped impact-fuzed burst round in assets/rounds");
    let round = crate::tank::WeaponRound::resolve(key)
        .expect("the shipped HE round must be chambered by a shipped gun")
        .round;
    info!("server: game destruction HE stress {value:?}, round={key}");
    app.insert_resource(HeStress {
        recipe,
        round,
        fired: 0,
        burst: 0,
        next_s: 0.0,
    })
    .add_observer(count_burst)
    .add_systems(FixedUpdate, fire.in_set(GameplaySet));
    Ok(())
}

fn fire(
    time: Res<Time>,
    mut stress: ResMut<HeStress>,
    constructions: Query<(&MapInstance, &Transform, &Breakage), With<Body>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    if stress.fired == stress.recipe.bursts || now < stress.next_s {
        return;
    }
    let (identity, pose, body) = constructions
        .iter()
        .min_by_key(|(id, ..)| id.ordinal)
        .expect("the loaded shipped map must contain a destructible construction for HE stress");
    let (min, max) = body.kind.bounds();
    // DERIVED: the bounds' centre in XZ, one full construction height above its roof. Vertical
    // flight crosses the roof, interior and ground without needing a tank or an aim solver.
    let centre = (min + max) * 0.5 * body.scale;
    let birth = pose.translation
        + pose.rotation * Vec3::new(centre.x, (max.y + max.y - min.y) * body.scale, centre.z);
    if stress.fired == 0 {
        info!(
            "server: game destruction HE target={} birth={birth:?}",
            identity.id
        );
    }
    commands.trigger(FireShell::of_round(
        &stress.round,
        birth,
        birth,
        Dir3::NEG_Y,
        crate::spec::FireMechanism::Single,
        false,
        None,
        None,
    ));
    stress.fired += 1;
    stress.next_s = now + stress.recipe.period_s;
    println!(
        "he_stress {{\"fired\":{},\"bursts\":{}}}",
        stress.fired, stress.burst
    );
}

fn count_burst(
    marched: On<ShellMarched>,
    sources: Query<(), With<ShotSource>>,
    mut stress: ResMut<HeStress>,
) {
    // Player rounds are outside this instrument's census; unkeyed tool rounds have no source.
    if marched.burst.is_some() && !sources.contains(marched.shell) {
        stress.burst += 1;
        println!(
            "he_stress {{\"fired\":{},\"bursts\":{}}}",
            stress.fired, stress.burst
        );
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::task::{Poll, Waker};
    use std::time::{Duration, Instant};

    use bevy::asset::io::{
        AssetReader, AssetReaderError, AssetSourceBuilder, AssetSourceId, PathStream, Reader,
        file::FileAssetReader,
    };

    use super::*;

    // Fixture: flight update budget; async asset admission has no update-count bound.
    const FLIGHT_UPDATES: usize = 512;

    /// Operators rely on refusal before startup and on an off installer mounting nothing.
    #[test]
    fn off_mounts_nothing_and_bad_values_refuse() {
        for off in [None, Some(""), Some("off"), Some("false"), Some("0:1")] {
            let mut app = App::new();
            let entities = app.world().entities().len();
            install_value(&mut app, off).unwrap();
            assert!(!app.world().contains_resource::<HeStress>());
            assert!(
                app.world()
                    .get_resource::<Schedules>()
                    .is_none_or(|s| !s.contains(FixedUpdate))
            );
            assert_eq!(app.world().entities().len(), entities, "no observer entity");
        }
        for bad in ["1", "1:0", "1:-1", "1:NaN", "1:inf", "1:2:3", "x:1", "-1:1"] {
            assert!(
                matches!(Recipe::parse(Some(bad)), Err(LeverError::Invalid { .. })),
                "{bad}"
            );
        }
        assert_eq!(
            Recipe::parse(Some(" 3 : 0.5 ")).unwrap(),
            Some(Recipe {
                bursts: 3,
                period_s: 0.5
            })
        );
    }

    /// The game's damage model: the empty-server recipe must fly and function real fuzes.
    /// A controlled support-removal deposit checks destruction delivery while Battle waits.
    #[test]
    fn armed_empty_server_flies_and_delivers_controlled_destruction() {
        assert_real_flight(App::new());
    }

    /// The game's damage model must still fire and burst when spec IO takes more updates than
    /// flight. The server's admission gate, rather than machine speed, starts this check's clock.
    #[test]
    fn asset_io_may_outlast_the_flight_update_budget() {
        let mut app = App::new();
        let delay = Arc::new(Mutex::new(ReadDelay {
            updates_left: FLIGHT_UPDATES + 1,
            waiting: None,
        }));
        let reader_delay = delay.clone();
        app.register_asset_source(
            AssetSourceId::Default,
            AssetSourceBuilder::new(move || {
                Box::new(DelayedSpecReader {
                    file: FileAssetReader::new(crate::assets::asset_root()),
                    delay: reader_delay.clone(),
                })
            }),
        )
        .add_systems(Update, move || {
            let mut delay = delay.lock().unwrap();
            delay.updates_left = delay.updates_left.saturating_sub(1);
            if delay.updates_left == 0
                && let Some(waiting) = delay.waiting.take()
            {
                waiting.wake();
            }
        });
        assert_real_flight(app);
    }

    fn assert_real_flight(mut app: App) {
        app.add_plugins(crate::server_plugin_floor())
            .add_plugins(super::super::physics::physics_plugins())
            .add_plugins(crate::SimPlugin)
            .add_plugins(lightyear::prelude::server::ServerPlugins {
                tick_duration: super::super::test_harness::TICK,
            })
            .add_plugins(super::super::plugin)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::ZERO,
            ))
            .add_systems(Startup, crate::tank::load_tank_sim_assets)
            .add_systems(Update, super::super::open_gameplay_gate);
        super::super::server::install_battle(
            &mut app,
            super::super::battle_mode::BattleMode::shipped(),
            super::super::test_harness::TICK,
        );
        // Fixture operator recipe: finite shots, not an obligation on emergent piece counts.
        install_value(&mut app, Some("3:0.25")).unwrap();
        super::super::test_harness::finish(&mut app);
        // The game's damage model starts its flight budget only after async spec IO and the
        // shipped admission gate finish. A frozen sim clock leaves the recipe unfired at entry.
        let started = Instant::now();
        loop {
            app.update();
            if *app
                .world()
                .resource::<State<crate::state::AppState>>()
                .get()
                == crate::state::AppState::Playing
            {
                break;
            }
            // Fixture: startup watchdog and IO-worker yield, independent of flight time.
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "server admission timed out before HE flight: state={:?}, spec {}={:?}",
                app.world()
                    .resource::<State<crate::state::AppState>>()
                    .get(),
                crate::tank::TANK_SPEC_PATH,
                app.world().resource::<AssetServer>().load_state(
                    &app.world()
                        .resource::<crate::tank::PendingTankAssets>()
                        .spec
                ),
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(app.world().resource::<HeStress>().fired, 0);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            super::super::test_harness::TICK,
        ));
        for _ in 0..FLIGHT_UPDATES {
            app.update();
            if app.world().resource::<HeStress>().burst == 3 {
                break;
            }
        }
        assert_eq!(app.world().resource::<HeStress>().fired, 3);
        assert_eq!(
            app.world().resource::<HeStress>().burst,
            3,
            "flight must function the real fuze"
        );
        let target = app
            .world_mut()
            .query_filtered::<&MapInstance, With<Body>>()
            .iter(app.world())
            .map(|id| id.ordinal)
            .min()
            .unwrap();
        let falls = app.world_mut().query_filtered::<&crate::construction::breakage::Breakage, Without<crate::construction::breakage::Piece>>()
            .iter(app.world()).filter(|b| b.construction == target)
            .flat_map(|b| b.live.state.fate.iter())
            .filter(|&&fate| fate == crate::construction::fracture::Fate::Fallen).count();
        println!(
            "MEASURED shipped game destruction: fired={} bursts={} target={target} falls={falls}",
            app.world().resource::<HeStress>().fired,
            app.world().resource::<HeStress>().burst,
        );
        assert_eq!(
            app.world_mut()
                .query::<&crate::tank::Tank>()
                .iter(app.world())
                .count(),
            0
        );
        let battle = app
            .world_mut()
            .query::<&super::super::battle::BattleState>()
            .single(app.world())
            .unwrap();
        assert!(matches!(
            battle.phase,
            super::super::battle::BattlePhase::WaitingForPlayer
        ));
        assert_controlled_destruction_delivery(&mut app);
    }

    /// The game's destruction ledger must receive support loss through the shipped observer and
    /// law schedule. DERIVED: work above V * crush empties the base's volume pool; the remaining
    /// column has no ground path and falls. No shipped round's potency or house strength gates it.
    fn assert_controlled_destruction_delivery(app: &mut App) {
        use crate::ballistics::{BallisticVolume, DeliveredLine, PenetrationMarks, VolumeCrossing};
        use crate::construction::breakage::{Event, Record};
        use crate::construction::fracture::Material;
        use crate::substances::SubstanceRegistry;
        use std::collections::BTreeSet;
        let kind = crate::construction::tests::column_construction();
        let used: BTreeSet<_> = app
            .world_mut()
            .query_filtered::<&MapInstance, With<Body>>()
            .iter(app.world())
            .map(|id| id.ordinal)
            .collect();
        let ordinal = (0..=u16::MAX)
            .find(|id| !used.contains(id))
            .expect("the controlled fixture needs an unused construction ordinal");
        // Fixture: y = 1000 m separates the column from the shipped map's contacts.
        let offset = Vec3::Y * 1000.0;
        let root = crate::construction::spawn(
            &mut app.world_mut().commands(),
            &kind,
            MapInstance {
                id: "stress_column_fixture".into(),
                ordinal,
            },
            Transform::from_translation(offset),
        );
        app.world_mut().flush();
        app.update();
        let volume = app
            .world_mut()
            .query::<(Entity, &ChildOf, &BallisticVolume)>()
            .iter(app.world())
            .find(|(_, parent, _)| parent.parent() == root)
            .unwrap()
            .0;
        // Fixture: twice the independently derived removal threshold of the unit base cube.
        let base = &kind.asset.chunks[0];
        let material = Material::of(
            SubstanceRegistry::shipped_static(),
            &kind.asset.materials[usize::from(base.material)],
        )
        .unwrap()
        .unwrap();
        let threshold = base.volume_m3 * material.crush;
        let entry = offset + Vec3::new(-0.5, 0.5, 0.0);
        let end = offset + Vec3::new(0.5, 0.5, 0.0);
        let shell = app.world_mut().spawn(PenetrationMarks::default()).id();
        assert!(
            app.world()
                .resource::<Record>()
                .entries
                .iter()
                .all(|entry| entry.construction != ordinal)
        );
        app.world_mut().trigger(ShellMarched {
            shell,
            marks_from: 0,
            cones_from: 0,
            burst: None,
            lines: vec![DeliveredLine {
                crossing: VolumeCrossing {
                    volume,
                    entry,
                    end,
                    chord: entry.distance(end),
                    cost: 0.0,
                    energy: 2.0 * threshold,
                    perforated: true,
                },
                contacts: vec![(entry, end)],
                momentum: Vec3::ZERO,
                entrance_momentum: Vec3::ZERO,
                entrance_energy: 0.0,
            }],
        });
        app.update();
        let record = app.world().resource::<Record>();
        assert!(
            record
                .entries
                .iter()
                .any(|entry| entry.construction == ordinal && entry.event == Event::Dust(0))
        );
        let fixture = crate::construction::breakage::Breakage::new(kind.clone(), ordinal, 1.0);
        let mut derived = crate::construction::breakage::Shape::intact(&kind.layout);
        for entry in record
            .entries
            .iter()
            .filter(|entry| entry.construction == ordinal)
        {
            derived.apply(entry.event, &kind.layout);
        }
        derived.derive_departures(&kind.layout, &fixture.live);
        let fallen: BTreeSet<_> = derived
            .fate
            .iter()
            .enumerate()
            .filter_map(|(chunk, fate)| {
                (*fate == crate::construction::fracture::Fate::Fallen).then_some(chunk as u16)
            })
            .collect();
        let unsupported: BTreeSet<_> = (1..kind.asset.chunks.len())
            .map(|c| u16::try_from(c).unwrap())
            .collect();
        assert_eq!(
            fallen, unsupported,
            "support loss must derive from the recorded causes in this update"
        );
        println!(
            "MEASURED controlled game destruction: threshold={threshold} J, delivered={} J, fallen={fallen:?}",
            2.0 * threshold
        );
        app.world_mut().despawn(shell);
    }

    struct ReadDelay {
        updates_left: usize,
        waiting: Option<Waker>,
    }

    /// A pending read yields its IO worker; unrelated tests and assets keep loading normally.
    struct DelayedSpecReader {
        file: FileAssetReader,
        delay: Arc<Mutex<ReadDelay>>,
    }

    impl AssetReader for DelayedSpecReader {
        async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
            if path == Path::new(crate::tank::TANK_SPEC_PATH) {
                poll_fn(|cx| {
                    let mut delay = self.delay.lock().unwrap();
                    if delay.updates_left == 0 {
                        Poll::Ready(())
                    } else {
                        delay.waiting = Some(cx.waker().clone());
                        Poll::Pending
                    }
                })
                .await;
            }
            self.file.read(path).await
        }

        async fn read_meta<'a>(
            &'a self,
            path: &'a Path,
        ) -> Result<impl Reader + 'a, AssetReaderError> {
            self.file.read_meta(path).await
        }

        async fn read_directory<'a>(
            &'a self,
            path: &'a Path,
        ) -> Result<Box<PathStream>, AssetReaderError> {
            self.file.read_directory(path).await
        }

        async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
            self.file.is_directory(path).await
        }
    }
}
