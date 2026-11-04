//! The video game's damage model, game state only: unattended real HE flight into the map.
//! `scripts/perf/collapse.sh` owns `OVERMATCH_HE_STRESS=<bursts>:<period_s>`.
//! No tank, gun controls or player transport: `FireShell` enters the ordinary authority march.

use bevy::prelude::*;

use super::harness::{LeverError, env_text};
use crate::ballistics::{FireShell, ShellMarched, ShotSource};
use crate::buildings::{BuildingBody, breakage::Breakage};
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
    buildings: Query<(&MapInstance, &Transform, &Breakage), With<BuildingBody>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    if stress.fired == stress.recipe.bursts || now < stress.next_s {
        return;
    }
    let (identity, pose, body) = buildings
        .iter()
        .min_by_key(|(id, ..)| id.ordinal)
        .expect("the loaded shipped map must contain a destructible building for HE stress");
    let (min, max) = body.kind.bounds();
    // DERIVED: the bounds' centre in XZ, one full building height above its roof. Vertical
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

    /// The operator's empty-server recipe must create real bursts and a nonempty destruction
    /// ledger with the shipped sim, even while the empty Battle waits for its first player.
    #[test]
    fn armed_empty_server_breaks_through_real_flight() {
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
        use crate::buildings::breakage::WallRecord;
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
            .query_filtered::<&MapInstance, With<BuildingBody>>()
            .iter(app.world())
            .map(|id| id.ordinal)
            .min()
            .unwrap();
        assert!(
            app.world()
                .resource::<WallRecord>()
                .entries
                .iter()
                .any(|entry| entry.building == target
                    && matches!(entry.event, crate::buildings::breakage::WallEvent::Fall(_))),
            "real deposits must free chunks of the selected building"
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
