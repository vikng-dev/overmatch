//! The shooter's corner in play: each explanation of the local player's own hit on another tank
//! opens the corner in the frame for its verdict ([`CornerKind::of`]), plays it on the game clock
//! and closes it.
//!
//! * **What opens.** An explanation in the player's store ([`ShotExplanations`]) whose audience is
//!   [`Audience::Shooter`], fired by the player's combatant at another, once its world impact is
//!   drawn ([`ImpactPresence::Presented`]), and admitted by the player's corner setting
//!   ([`PenCamCorner::opens`]). One released undrawn ([`ImpactPresence::TimedOut`]) is skipped:
//!   the corner rides on the impact the player saw. Each explanation is considered once.
//! * **When.** [`timeline::OPEN_LATENCY_S`] after it is presented, by the replacement rule
//!   ([`Lineup`]): the newest record replaces the one on show once that one has shown
//!   [`timeline::MIN_SHOWN_S`]; one arriving sooner waits, and only the latest waiting is kept. A
//!   replacement keeps the frame open and crossfades ([`timeline::turn_presence`]). Never two
//!   corners. The timings are the UX spec's, TUNED.
//! * **Which is newer.** Explanation identity ([`ExplanationKey`], fire tick first), never
//!   arrival: the store's channel is unordered. One older than any record the corner has admitted
//!   since it last held nothing is dropped.
//! * **What it draws.** The struck tank's stage view, of the tank file the record names, and the
//!   record's plan over the same tank asset the demo stages ([`TankAsset`]); a component the
//!   shooter's projection does not list is drawn as the base layout, untinted.
//! * **Teardown.** The corner's entities exist from a record's arrival until the corner closes
//!   with nothing queued, a new life of the player's tank or no tank, the setting turning
//!   [`PenCamCorner::Off`], or a [`CloseCorner`] (the network client's disconnect). Every one goes
//!   at once ([`Rig::teardown`]): the cameras, lights, stage view, UI and the images they own.
//!
//! No input reaches the corner.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;

use super::hud::{self, CornerImages, Glyphs};
use super::look::{MaskCamera, Masks};
use super::plan::Plan;
use super::stage::{self, Stage};
use super::timeline::{self, CornerKind, Lineup};
use super::view::{self, StageView};
use super::{Corner, CornerRig, Exhibit, Exhibits, Knobs, Showing, Surface};
use crate::bake::TankBlueprint;
use crate::damage_cam::delivery::{ExplanationKey, ImpactPresence, ShotExplanations};
use crate::damage_cam::shapes::TankAsset;
use crate::damage_cam::wire::{Audience, TankLife};
use crate::settings::{PenCamCorner, Settings};
use crate::spec::TankSpec;
use crate::tank::Controlled;
use crate::{CombatantId, CombatantLife};

/// The shooter's corner on a client composition: the stage's materials and [`wire`].
pub fn plugin(app: &mut App) {
    super::materials(app);
    wire(app);
}

/// Close the corner now and drop everything it holds.
#[derive(Event, Clone, Copy, Debug)]
pub(crate) struct CloseCorner;

/// The corner's state and systems, without the materials (a headless test registers their assets
/// alone).
fn wire(app: &mut App) {
    super::presentation(app);
    app.init_resource::<ShooterCorner>()
        .add_observer(close)
        .add_systems(
            Update,
            (
                drive,
                (view::prepare_stage, view::pose_stage, super::present())
                    .chain()
                    .run_if(resource_exists::<StageView>),
            )
                .chain(),
        );
}

/// One record queued for the corner.
#[derive(Clone)]
struct Queued {
    serial: usize,
    kind: CornerKind,
    vehicle: String,
    exhibit: Exhibit,
}

/// The turn on show: its record's serial and frame, and when it started on the corner's clock.
#[derive(Clone, Copy)]
struct Turn {
    serial: usize,
    kind: CornerKind,
    start: f32,
    /// Whether it replaced a turn still open when it started.
    replacing: bool,
}

/// What the corner holds between frames.
#[derive(Resource, Default)]
pub(crate) struct ShooterCorner {
    /// The player's tank life the corner serves.
    life: Option<TankLife>,
    /// Every stored explanation already considered.
    seen: BTreeSet<ExplanationKey>,
    lineup: Lineup<Queued>,
    /// The newest explanation admitted to the lineup since the corner last held nothing.
    newest: Option<ExplanationKey>,
    serials: usize,
    turn: Option<Turn>,
    /// Virtual seconds at the corner's clock zero, while it holds anything.
    epoch: Option<f64>,
    /// Whether the inset camera, its lights and the corner's UI exist.
    rigged: bool,
    /// Each tank file's asset, read once.
    tanks: HashMap<String, Arc<TankAsset>>,
    /// Each staged tank file's view assets, held loaded until the corner is closed by a
    /// [`CloseCorner`].
    loaded: HashMap<String, (Handle<WorldAsset>, Handle<TankSpec>)>,
}

impl ShooterCorner {
    /// The tank file `vehicle`'s asset: the blueprint's when it names that file, else read from
    /// disk. `None`, warned once, when it cannot be read.
    fn tank(&mut self, vehicle: &str, blueprint: Option<&TankBlueprint>) -> Option<Arc<TankAsset>> {
        if let Some(tank) = self.tanks.get(vehicle) {
            return Some(tank.clone());
        }
        let tank = match blueprint.filter(|blueprint| blueprint.file == vehicle) {
            Some(blueprint) => TankAsset::of_blueprint(blueprint),
            None => match TankAsset::load(vehicle) {
                Ok(tank) => tank,
                Err(error) => {
                    warn_once!("pen cam: the corner cannot stage {vehicle}: {error}");
                    return None;
                }
            },
        };
        let tank = Arc::new(tank);
        self.tanks.insert(vehicle.to_owned(), tank.clone());
        Some(tank)
    }

    /// Drop every queued and shown record and the corner's clock.
    fn reset(&mut self) {
        self.lineup.clear();
        self.newest = None;
        self.turn = None;
        self.epoch = None;
    }

    /// [`Self::reset`], and consider none of `store`'s explanations again, those still waiting on
    /// their impact included.
    fn forget(&mut self, store: Option<&ShotExplanations>) {
        self.reset();
        self.seen = store
            .map(|store| store.iter().map(|(key, _)| *key).collect())
            .unwrap_or_default();
    }
}

/// The corner's entities and the presentation resources they feed.
#[derive(SystemParam)]
pub(crate) struct Rig<'w, 's> {
    commands: Commands<'w, 's>,
    asset_server: Res<'w, AssetServer>,
    images: ResMut<'w, Assets<Image>>,
    view: Option<Res<'w, StageView>>,
    stage: ResMut<'w, Stage>,
    corner: ResMut<'w, Corner>,
    exhibits: ResMut<'w, Exhibits>,
    masks: ResMut<'w, Masks>,
    entities: Query<'w, 's, Entity, (With<CornerRig>, Without<MaskCamera>)>,
}

impl Rig<'_, '_> {
    /// The inset, its lights and the corner's UI, and the stage view of `vehicle`, spawning what
    /// is missing. Whether the stage view is placed.
    fn ready(&mut self, state: &mut ShooterCorner, vehicle: &str) -> bool {
        if !state.rigged {
            *self.stage = Stage::default();
            stage::spawn_inset(
                &mut self.commands,
                &self.asset_server,
                &mut self.images,
                &mut self.stage,
            );
            hud::spawn_corner(&mut self.commands, &mut self.images);
            state.rigged = true;
        }
        match self.view.as_deref() {
            Some(view) if view.vehicle == vehicle => view.placed(),
            stale => {
                if let Some(old) = stale {
                    self.commands.entity(old.root).despawn();
                }
                self.stage.invalidate();
                let view = StageView::spawn(&mut self.commands, &self.asset_server, vehicle);
                self.commands.insert_resource(view);
                false
            }
        }
    }

    /// Despawn every entity of the corner and drop every image and resource it holds.
    fn teardown(&mut self, state: &mut ShooterCorner) {
        self.masks.release(&mut self.commands, &mut self.images);
        for entity in &self.entities {
            self.commands.entity(entity).despawn();
        }
        self.commands.remove_resource::<StageView>();
        self.commands.remove_resource::<CornerImages>();
        self.commands.remove_resource::<Glyphs>();
        *self.stage = Stage::default();
        self.corner.set_if_neq(Corner::default());
        self.exhibits.0.clear();
        state.rigged = false;
    }

    /// Whether anything of the corner exists.
    fn exists(&self, state: &ShooterCorner) -> bool {
        state.rigged || self.view.is_some()
    }
}

fn close(
    _close: On<CloseCorner>,
    mut state: ResMut<ShooterCorner>,
    store: Option<Res<ShotExplanations>>,
    mut rig: Rig,
) {
    rig.teardown(&mut state);
    state.forget(store.as_deref());
    state.loaded.clear();
}

/// Queue each newly presented shooter explanation, run the replacement rule on the corner's clock,
/// keep the rig for what is on show and write the corner.
#[allow(clippy::too_many_arguments)]
fn drive(
    time: Res<Time>,
    mut state: ResMut<ShooterCorner>,
    store: Option<Res<ShotExplanations>>,
    settings: Option<Res<Settings>>,
    blueprint: Option<Res<TankBlueprint>>,
    player: Query<(&CombatantId, &CombatantLife), With<Controlled>>,
    mut knobs: ResMut<Knobs>,
    mut rig: Rig,
) {
    let settings = settings.as_deref().copied().unwrap_or_default();
    let reduce_motion = settings.reduce_motion.on();
    if knobs.reduce_motion != reduce_motion {
        knobs.reduce_motion = reduce_motion;
    }
    let store = store.as_deref();
    let life = player
        .single()
        .ok()
        .map(|(&combatant, &life)| TankLife { combatant, life });
    if life != state.life || settings.pen_cam_corner == PenCamCorner::Off {
        if rig.exists(&state) {
            rig.teardown(&mut state);
        }
        state.forget(store);
        state.life = life;
    }
    let (Some(me), Some(store)) = (state.life, store) else {
        return;
    };

    let stored: BTreeSet<ExplanationKey> = store.iter().map(|(key, _)| *key).collect();
    state.seen.retain(|key| stored.contains(key));
    for (key, entry) in store.iter() {
        if state.seen.contains(key) {
            continue;
        }
        let explanation = &entry.explanation;
        let mine = explanation.audience == Audience::Shooter
            && explanation.shot.combatant == me.combatant
            && explanation.struck.combatant != me.combatant;
        let vehicle = entry.report.vehicle.as_deref().filter(|_| mine);
        if let Some(vehicle) = vehicle
            && !state.loaded.contains_key(vehicle)
        {
            // Loaded while the impact is still to draw, so the stage view can spawn by the time
            // the corner opens.
            let assets = StageView::load(&rig.asset_server, vehicle);
            state.loaded.insert(vehicle.to_owned(), assets);
        }
        if entry.presence == ImpactPresence::Pending {
            continue;
        }
        state.seen.insert(*key);
        let opens = entry.presence == ImpactPresence::Presented
            && settings.pen_cam_corner.opens(entry.report.impact.verdict);
        let Some(vehicle) = vehicle.filter(|_| opens).map(str::to_owned) else {
            continue;
        };
        if state.newest.is_some_and(|newest| *key < newest) {
            continue;
        }
        let Some(tank) = state.tank(&vehicle, blueprint.as_deref()) else {
            continue;
        };
        let plan = Plan::new(&entry.report, &tank.shapes(&entry.report.servo_deg));
        let epoch = *state.epoch.get_or_insert(time.elapsed_secs_f64());
        let now = (time.elapsed_secs_f64() - epoch) as f32;
        state.serials += 1;
        state.newest = Some(*key);
        let queued = Queued {
            serial: state.serials,
            kind: plan.kind,
            vehicle,
            exhibit: Exhibit {
                plan: Arc::new(plan),
                servo_deg: entry.report.servo_deg.clone(),
            },
        };
        state.lineup.arrive(queued, now + timeline::OPEN_LATENCY_S);
    }

    let Some(epoch) = state.epoch else {
        return;
    };
    let now = (time.elapsed_secs_f64() - epoch) as f32;
    state.lineup.advance(now, |_, _| {});
    let current = state.lineup.current().map(|(queued, start)| {
        let ended = now - start > queued.kind.duration();
        (queued.clone(), start, ended)
    });
    let ended = current.as_ref().is_none_or(|(_, _, ended)| *ended);
    if ended && !state.lineup.pending() {
        rig.teardown(&mut state);
        state.reset();
        return;
    }
    let showing = current.as_ref().filter(|(_, _, ended)| !ended);
    let wanted = showing
        .map(|(queued, _, _)| queued.vehicle.clone())
        .or_else(|| state.lineup.next().map(|next| next.vehicle.clone()));
    let Some(wanted) = wanted else {
        return;
    };
    let placed = rig.ready(&mut state, &wanted);
    let Some((queued, _, false)) = current else {
        rig.corner.set_if_neq(Corner::default());
        return;
    };
    if !placed {
        // The stage cannot draw yet: the turn waits for it.
        state.lineup.hold_until(now);
        rig.corner.set_if_neq(Corner::default());
        return;
    }
    let start = state.lineup.current().map_or(now, |(_, start)| start);
    let turn = match state.turn {
        Some(turn) if turn.serial == queued.serial => Turn { start, ..turn },
        previous => Turn {
            serial: queued.serial,
            kind: queued.kind,
            start,
            replacing: previous.is_some_and(|previous| {
                start < previous.start + previous.kind.duration() && rig.corner.showing.is_some()
            }),
        },
    };
    if state.turn.is_none_or(|shown| shown.serial != turn.serial) {
        rig.exhibits.0 = vec![queued.exhibit.clone()];
        debug!(
            "pen cam: the corner shows {:?} at {:.2} s",
            turn.kind,
            time.elapsed_secs_f64()
        );
    }
    state.turn = Some(turn);
    let duration = turn.kind.duration();
    let c = now - turn.start;
    let (opacity, scale, fading) = timeline::turn_presence(c, duration, turn.replacing);
    rig.corner.set_if_neq(Corner {
        showing: Some(Showing {
            record: 0,
            turn: turn.serial,
            kind: turn.kind,
            c,
            opacity,
            scale,
            remaining: 1.0 - (c / duration).clamp(0.0, 1.0),
            surface: Surface::Corner,
        }),
        fading,
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::damage_cam::PenetrationReport;
    use crate::damage_cam::delivery::tests::{SHOOTER, STRUCK, recorded, report, vocabulary};
    use crate::damage_cam::pen_cam::stage::StageCamera;
    use crate::damage_cam::wire::{BuiltExplanation, ShotExplanation, ShotVocabulary};

    /// The test's frame step (s), a 30 Hz frame: fixture data.
    const STEP_S: f32 = 1.0 / 30.0;

    /// The corner's systems over a headless app, the player's tank (the shooter's combatant), an
    /// empty store and bevy's shared default image ([`default_image_kept`]), one frame run.
    fn corner_app(settings: Settings) -> (App, Entity) {
        let mut app = App::new();
        app.set_error_handler(bevy::ecs::error::ignore);
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            TransformPlugin,
        ));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app.init_asset::<TankSpec>()
            .init_asset::<WorldAsset>()
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .init_asset::<super::super::material::GlowMaterial>()
            .init_asset::<super::super::material::OutlineMaterial>()
            .init_asset::<super::super::material::SliceMaterial>()
            .init_asset::<super::super::look::LookMaterial>()
            .init_resource::<Time>()
            .init_resource::<ShotExplanations>()
            .insert_resource(settings);
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .insert(&Handle::<Image>::default(), Image::default())
            .unwrap();
        wire(&mut app);
        let player = app
            .world_mut()
            .spawn((SHOOTER, CombatantLife(0), Controlled))
            .id();
        app.update();
        (app, player)
    }

    /// A stage view of `report`'s tank file standing in for its glb: the root and the hull node.
    fn stage_ready(app: &mut App, report: &PenetrationReport) {
        let vehicle = report
            .vehicle
            .clone()
            .expect("the record names its tank file");
        let path = crate::assets::asset_root().join(&vehicle);
        let spec: TankSpec =
            ron::de::from_str(&std::fs::read_to_string(path).expect("the tank file"))
                .expect("the tank file parses");
        let world = app.world_mut();
        let root = world
            .spawn((
                Transform::IDENTITY,
                Visibility::Hidden,
                view::ViewRoot::Stage,
                CornerRig,
            ))
            .id();
        world.spawn((
            Name::new(view::HULL_NODE),
            Transform::IDENTITY,
            ChildOf(root),
        ));
        let spec = world.resource_mut::<Assets<TankSpec>>().add(spec);
        let mut view = StageView::standing_in(spec, root);
        view.vehicle = vehicle;
        world.insert_resource(view);
    }

    /// `name`'s record fired by the shooter at the struck tank's first life on `fire_tick`: the
    /// shooter's projection, the victim's, and the vocabulary they decode against.
    fn explained(
        name: &str,
        fire_tick: u32,
    ) -> (
        ShotExplanation,
        ShotExplanation,
        ShotVocabulary,
        PenetrationReport,
    ) {
        let report = report(name);
        let vocabulary = vocabulary(&report);
        let struck = TankLife {
            combatant: STRUCK,
            life: CombatantLife(0),
        };
        let built =
            BuiltExplanation::build(&vocabulary, &recorded(&report, SHOOTER, fire_tick, struck))
                .unwrap();
        let mut projections = built.projections().into_iter().map(|(_, e)| e);
        let victim = projections.next().unwrap();
        let shooter = projections.next().unwrap();
        assert_eq!(shooter.audience, Audience::Shooter);
        assert_ne!(victim.audience, Audience::Shooter);
        (shooter, victim, vocabulary, report)
    }

    /// Store `explanation` with its impact drawn.
    fn receive(app: &mut App, explanation: ShotExplanation, vocabulary: &ShotVocabulary) {
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        app.world_mut()
            .resource_mut::<ShotExplanations>()
            .receive_drawn(explanation, vocabulary, now);
    }

    /// Whether bevy's shared image, the one `ImagePlugin` keeps at `Handle::default()` for
    /// untextured UI, is still stored.
    fn default_image_kept(app: &App) -> bool {
        app.world()
            .resource::<Assets<Image>>()
            .contains(&Handle::<Image>::default())
    }

    fn step(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(STEP_S));
        app.update();
    }

    /// The corner's entities, inset cameras and every entity but the resources', in that order.
    fn census(app: &mut App) -> (usize, usize, usize) {
        let world = app.world_mut();
        let rig = world
            .query_filtered::<(), With<CornerRig>>()
            .iter(world)
            .count();
        let insets = world
            .query_filtered::<(), With<StageCamera>>()
            .iter(world)
            .count();
        let all = world
            .query_filtered::<(), Without<bevy::ecs::resource::IsResource>>()
            .iter(world)
            .count();
        (rig, insets, all)
    }

    /// Run `seconds` of frames, collecting each shown turn's frame; at most one inset at a time.
    fn run(app: &mut App, seconds: f32) -> Vec<(usize, CornerKind)> {
        let mut shown = Vec::new();
        for _ in 0..(seconds / STEP_S).ceil() as usize {
            step(app);
            assert!(census(app).1 <= 1, "never two corners");
            if let Some(showing) = app.world().resource::<Corner>().showing {
                assert_eq!(showing.surface, Surface::Corner);
                if shown.last() != Some(&(showing.turn, showing.kind)) {
                    shown.push((showing.turn, showing.kind));
                }
            }
        }
        shown
    }

    /// The player's shooter explanation opens exactly one corner, in its verdict's frame, for its
    /// timeline's length, then closes it and leaves nothing of it.
    #[test]
    fn a_shooter_explanation_opens_one_corner_in_its_frame_and_closes_it() {
        for name in ["s1_through_crew", "s2_stopped_mantlet", "s3_ricochet"] {
            let (mut app, _) = corner_app(Settings::default());
            let (shooter, _, vocabulary, report) = explained(name, 1200);
            let kind = CornerKind::of(report.impact.verdict);
            stage_ready(&mut app, &report);
            receive(&mut app, shooter, &vocabulary);
            let mut frames = 0;
            let mut shown = Vec::new();
            for _ in 0..((kind.duration() + 1.0) / STEP_S) as usize {
                step(&mut app);
                if let Some(showing) = app.world().resource::<Corner>().showing {
                    frames += 1;
                    if shown.last() != Some(&showing.turn) {
                        shown.push(showing.turn);
                    }
                    assert_eq!(showing.kind, kind, "{name}");
                    assert_eq!(census(&mut app).1, 1, "{name}: one inset");
                }
            }
            assert_eq!(shown.len(), 1, "{name}: one turn");
            let shown_s = frames as f32 * STEP_S;
            assert!(
                (shown_s - kind.duration()).abs() <= 2.0 * STEP_S,
                "{name}: shown {shown_s} s of {} s",
                kind.duration()
            );
            assert!(app.world().resource::<Corner>().showing.is_none());
            assert_eq!(census(&mut app).0, 0, "{name}: nothing of the corner left");
            assert!(!app.world().contains_resource::<StageView>());
        }
    }

    /// `Off` opens nothing; `Through only` opens the penetration and not the non-penetration.
    #[test]
    fn the_setting_admits_by_verdict() {
        let off = Settings {
            pen_cam_corner: PenCamCorner::Off,
            ..Settings::default()
        };
        let (mut app, _) = corner_app(off);
        let (shooter, _, vocabulary, report) = explained("s1_through_crew", 1200);
        stage_ready(&mut app, &report);
        receive(&mut app, shooter, &vocabulary);
        assert!(run(&mut app, 2.0).is_empty());
        assert_eq!(census(&mut app).0, 0);

        let through = Settings {
            pen_cam_corner: PenCamCorner::ThroughOnly,
            ..Settings::default()
        };
        let (mut app, _) = corner_app(through);
        let (pen, _, pen_vocabulary, report) = explained("s1_through_crew", 1300);
        let (stopped, _, stopped_vocabulary, _) = explained("s2_stopped_mantlet", 1200);
        stage_ready(&mut app, &report);
        receive(&mut app, stopped, &stopped_vocabulary);
        receive(&mut app, pen, &pen_vocabulary);
        let kinds: Vec<CornerKind> = run(&mut app, 6.0).into_iter().map(|(_, k)| k).collect();
        assert_eq!(kinds, [CornerKind::Glass]);
    }

    /// A victim's projection opens nothing, whether the player is the struck tank or names its
    /// shooter.
    #[test]
    fn the_victims_projection_never_opens_the_corner() {
        for combatant in [STRUCK, SHOOTER] {
            let (mut app, player) = corner_app(Settings::default());
            app.world_mut().entity_mut(player).insert(combatant);
            app.update();
            let (_, victim, vocabulary, report) = explained("s1_through_crew", 1200);
            stage_ready(&mut app, &report);
            receive(&mut app, victim, &vocabulary);
            assert!(run(&mut app, 2.0).is_empty());
        }
    }

    /// A new life of the player's tank, no tank, and a disconnect each tear an open corner down:
    /// no entity, camera or image of it left.
    #[test]
    fn a_respawn_or_disconnect_mid_corner_leaves_nothing() {
        for ending in 0..3 {
            let (mut app, player) = corner_app(Settings::default());
            let (_, _, entities) = census(&mut app);
            let images = app.world().resource::<Assets<Image>>().len();
            let (shooter, _, vocabulary, report) = explained("s1_through_crew", 1200);
            stage_ready(&mut app, &report);
            receive(&mut app, shooter, &vocabulary);
            assert!(!run(&mut app, 1.0).is_empty(), "the corner opened");
            assert!(census(&mut app).0 > 0);
            let entities = match ending {
                0 => {
                    app.world_mut().entity_mut(player).insert(CombatantLife(1));
                    entities
                }
                1 => {
                    app.world_mut().despawn(player);
                    entities - 1
                }
                _ => {
                    app.world_mut().trigger(CloseCorner);
                    entities
                }
            };
            for _ in 0..3 {
                step(&mut app);
            }
            assert!(app.world().resource::<Corner>().showing.is_none());
            assert_eq!(census(&mut app), (0, 0, entities), "ending {ending}");
            assert_eq!(app.world().resource::<Assets<Image>>().len(), images);
            assert!(!app.world().contains_resource::<StageView>());
            assert!(!app.world().contains_resource::<CornerImages>());
            assert!(!app.world().contains_resource::<Glyphs>());
            // The explanation already shown never reopens.
            assert!(run(&mut app, 2.0).is_empty(), "ending {ending}");
        }
    }

    /// An explanation waits for its impact, across a corner closing; one released undrawn never
    /// opens.
    #[test]
    fn the_corner_rides_on_the_drawn_impact() {
        let (mut app, _) = corner_app(Settings::default());
        let (first, _, vocabulary, report) = explained("s1_through_crew", 1200);
        let (waiting, _, _, _) = explained("s1_through_crew", 1300);
        let (released, _, _, _) = explained("s1_through_crew", 1400);
        stage_ready(&mut app, &report);
        receive(&mut app, first, &vocabulary);
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        let mut store = app.world_mut().resource_mut::<ShotExplanations>();
        store.receive(released, &vocabulary, now);
        let later = now + crate::damage_cam::delivery::IMPACT_TIMEOUT_S;
        store.expire(later);
        store.receive(waiting.clone(), &vocabulary, later);
        assert_eq!(run(&mut app, 5.0).len(), 1, "only the drawn impact opens");
        assert_eq!(census(&mut app).0, 0, "the corner closed");

        stage_ready(&mut app, &report);
        app.world_mut()
            .resource_mut::<ShotExplanations>()
            .drew(waiting.shot, waiting.outcome.unwrap());
        assert_eq!(
            run(&mut app, 5.0).len(),
            1,
            "the waiting one opens once drawn"
        );
    }

    /// Two shots inside one turn's minimum: the second replaces the first once the first has
    /// shown its minimum, by a crossfade in the one corner.
    #[test]
    fn an_overlapping_shot_replaces_the_one_on_show() {
        let (mut app, _) = corner_app(Settings::default());
        let (first, _, vocabulary, report) = explained("s1_through_crew", 1200);
        let (second, _, ricochet_vocabulary, _) = explained("s3_ricochet", 1210);
        stage_ready(&mut app, &report);
        receive(&mut app, first, &vocabulary);
        step(&mut app);
        receive(&mut app, second, &ricochet_vocabulary);
        let mut faded = false;
        let mut shown = Vec::new();
        for _ in 0..(8.0 / STEP_S) as usize {
            step(&mut app);
            let corner = app.world().resource::<Corner>().clone();
            faded |= corner.fading.is_some();
            if let Some(showing) = corner.showing
                && shown.last() != Some(&showing.kind)
            {
                shown.push(showing.kind);
            }
        }
        assert_eq!(shown, [CornerKind::Glass, CornerKind::Ricochet]);
        assert!(faded, "the replacement crossfades");
    }

    /// Closing the corner of every frame, by its end or by a disconnect, leaves bevy's shared
    /// default image stored: a frame without masks owns no image to remove.
    #[test]
    fn closing_the_corner_keeps_the_shared_default_image() {
        for name in ["s1_through_crew", "s2_stopped_mantlet", "s3_ricochet"] {
            for disconnect in [false, true] {
                let (mut app, _) = corner_app(Settings::default());
                let (shooter, _, vocabulary, report) = explained(name, 1200);
                stage_ready(&mut app, &report);
                receive(&mut app, shooter, &vocabulary);
                assert!(!run(&mut app, 1.0).is_empty(), "{name}: the corner opened");
                if disconnect {
                    app.world_mut().trigger(CloseCorner);
                }
                run(&mut app, 5.0);
                assert_eq!(census(&mut app).0, 0, "{name}: the corner closed");
                assert!(default_image_kept(&app), "{name}, disconnect {disconnect}");
            }
        }
        let (mut app, _) = corner_app(Settings::default());
        app.world_mut().trigger(CloseCorner);
        step(&mut app);
        assert!(default_image_kept(&app), "a disconnect with nothing open");
    }

    /// A newer shot on show is not replaced by an older one whose explanation arrives later:
    /// order is the explanation's identity, never its arrival.
    #[test]
    fn an_older_explanation_arriving_late_never_replaces_a_newer_one() {
        let (mut app, _) = corner_app(Settings::default());
        let (newer, _, vocabulary, report) = explained("s1_through_crew", 1300);
        let (older, _, ricochet_vocabulary, _) = explained("s3_ricochet", 1200);
        stage_ready(&mut app, &report);
        receive(&mut app, newer, &vocabulary);
        let mut shown = run(
            &mut app,
            timeline::OPEN_LATENCY_S + timeline::MIN_SHOWN_S + 0.2,
        );
        assert_eq!(shown.len(), 1, "the newer shot is on show");
        receive(&mut app, older, &ricochet_vocabulary);
        shown.extend(run(&mut app, 8.0));
        shown.dedup();
        let kinds: Vec<CornerKind> = shown.into_iter().map(|(_, kind)| kind).collect();
        assert_eq!(kinds, [CornerKind::Glass]);
    }
}
