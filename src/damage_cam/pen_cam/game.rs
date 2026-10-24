//! The corner slot in play: the shooter's corner, each explanation of the local player's own hit
//! on another tank in the frame for its verdict ([`CornerKind::of`]). One owner, one slot. The
//! player's own tank is never a turn here: the x-ray ([`xray`]) dresses the live tank in the
//! look on the battlefield, its own presentation beside the slot, and the two show at once.
//!
//! * **What opens the corner.** An explanation in the player's store ([`ShotExplanations`]) whose
//!   audience is [`Audience::Shooter`], fired by the player's combatant at another, admitted by
//!   the player's corner setting ([`PenCamCorner::opens`]). A victim projection opens nothing.
//! * **When.** It opens once its world impact is drawn ([`ImpactPresence::Presented`]); one
//!   released undrawn ([`ImpactPresence::TimedOut`]) is skipped, as the slot rides on the impact
//!   the player saw. Each explanation is considered once. It opens [`timeline::OPEN_LATENCY_S`]
//!   later by the replacement rule ([`Lineup`]): the newest event replaces the one on show once
//!   that one has shown [`timeline::MIN_SHOWN_S`]; one arriving sooner waits, and only the latest
//!   waiting is kept. A replacement keeps the frame open and crossfades
//!   ([`timeline::turn_presence`]); a replaced turn never resumes. The timings are the UX spec's,
//!   TUNED.
//! * **Which is newer.** The encounter's first contact, then its record's completion, identity
//!   breaking ties ([`Recency`]); never arrival, as the store's channel is unordered. One older
//!   than any the slot has admitted since it last held nothing, or than any admitted one still
//!   waiting on its impact, is dropped; of several presented together only the newest opens.
//! * **What it draws.** The struck tank's stage view, of the tank file the record names, under
//!   the record's plan over the same tank asset the demo stages ([`TankAsset`]); a component the
//!   shooter's projection does not list is the base layout, untinted.
//! * **Teardown.** The slot's entities exist from an arrival until the slot closes with nothing
//!   queued, a new life of the player's tank, its knockout or no tank, a change of the corner
//!   setting, or a [`CloseCorner`] (the network client's disconnect). Every one goes at once
//!   ([`Rig::teardown`]): the cameras, lights, stage view, UI and the images they own; the
//!   corner's mask set follows a frame later (`look::own`).
//!
//! No input reaches the corner. While the player's tank is knocked out the slot holds nothing and
//! its rig is the kill cam's ([`killcam`]).

mod killcam;
mod xray;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;

pub(crate) use self::killcam::{GameDeathScreen, KillCamGate, KillCamGateSet};
pub(crate) use self::xray::XRay;
#[cfg(feature = "dev_tools")]
pub(crate) use self::xray::scripted_xray_plugin;
use self::xray::{LiveState, LiveVolume};
use super::hud::{self, CornerImages, Glyphs};
use super::look;
use super::plan::Plan;
use super::stage::{self, Stage};
use super::timeline::{self, CornerKind, Lineup};
use super::view::{self, StageView};
use super::{Corner, CornerRig, Exhibit, Exhibits, Knobs, Showing, Surface};
use crate::bake::TankBlueprint;
use crate::damage::{TankKnockedOut, TankVolumes};
use crate::damage_cam::delivery::{ExplanationKey, ImpactPresence, ShotExplanations};
use crate::damage_cam::shapes::TankAsset;
use crate::damage_cam::wire::{Audience, ShotExplanation, TankLife};
use crate::settings::{PenCamCorner, Settings};
use crate::spec::{TankSpec, TankSpecHandle};
use crate::tank::{Controlled, Rig as TankRig};
use crate::{CombatantId, CombatantLife};

/// The corner slot on a client composition: the stage's materials and [`wire`].
pub fn plugin(app: &mut App) {
    super::materials(app);
    wire(app);
}

/// Close the slot now and drop everything it holds.
#[derive(Event, Clone, Copy, Debug)]
pub(crate) struct CloseCorner;

/// A new connection: the store's shots are not the last match's, and neither is what the x-ray has
/// shown. Triggered by the network client with the store's reset.
#[derive(Event, Clone, Copy, Debug)]
pub(crate) struct NewMatch;

/// The slot's and the x-ray's state and systems, without the materials (a headless test registers
/// the materials' assets alone).
fn wire(app: &mut App) {
    super::presentation(app);
    app.init_resource::<Slot>()
        .init_resource::<XRay>()
        .init_resource::<xray::XRayState>()
        .add_observer(close)
        .add_observer(xray::forget_shown)
        .add_observer(xray::round_landed)
        .add_systems(
            Update,
            (
                xray::toggle,
                drive,
                killcam::drive.in_set(killcam::KillCamGateSet),
                killcam::give_way
                    .after(crate::state::PlayerInputSet)
                    .after(crate::overlay::OverlaySet::Toggle),
                // The x-ray reads this frame's kill cam run, so after its drive; it states its
                // mask need before the masks are owned.
                xray::drive,
                (view::prepare_stage, view::pose_stage, hud::layout)
                    .chain()
                    .run_if(resource_exists::<StageView>),
                look::own,
                super::present().run_if(resource_exists::<StageView>),
                xray::build,
                xray::animate,
                log_presence,
                xray::log_presence,
            )
                .chain(),
        )
        // A mask copy takes its shoe's rung in the frame the belt writes it.
        .add_systems(
            PostUpdate,
            xray::follow_mesh.after(crate::track::link_view::ShoeRungSelected),
        );
    killcam::wire(app);
}

/// Log each change of what the slot shows, on `Time<Real>` — the frame-cost stream's clock
/// (`src/frame_cost.rs`), so `scripts/perf/pencam_cost.py` can window that stream by these marks.
fn log_presence(
    corner: Res<Corner>,
    time: Res<Time<Real>>,
    mut shown: Local<Option<(CornerKind, Surface)>>,
) {
    let now = corner
        .showing
        .as_ref()
        .map(|showing| (showing.kind, showing.surface));
    if now == *shown {
        return;
    }
    let t = time.elapsed_secs_f64();
    match now {
        Some((kind, surface)) => {
            debug!("pen cam: the slot shows {kind:?} on {surface:?} at t={t:.3} s (Time<Real>)");
        }
        None => debug!("pen cam: the slot shows nothing at t={t:.3} s (Time<Real>)"),
    }
    *shown = now;
}

/// One explanation queued for the slot: its exhibit.
#[derive(Clone)]
struct Queued {
    serial: usize,
    kind: CornerKind,
    vehicle: String,
    exhibit: Exhibit,
}

/// The turn on show: its serial and frame, and when it started on the slot's clock.
#[derive(Clone, Copy)]
struct Turn {
    serial: usize,
    kind: CornerKind,
    start: f32,
    /// Whether it replaced a turn still open when it started.
    replacing: bool,
}

/// An explanation's place in time for replacement: its encounter's first contact, then its
/// record's completion, its identity breaking ties. A shot's later encounter is newer whichever
/// tank it struck.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Recency {
    contact_tick: u32,
    completion_tick: u32,
    key: ExplanationKey,
}

impl Recency {
    fn of(key: ExplanationKey, explanation: &ShotExplanation) -> Self {
        Self {
            contact_tick: explanation.contact_tick,
            completion_tick: explanation.completion_tick,
            key,
        }
    }
}

/// What the slot holds between frames.
#[derive(Resource, Default)]
pub(crate) struct Slot {
    /// The player's tank life the slot serves.
    life: Option<TankLife>,
    /// The setting it admits by.
    admits: Option<PenCamCorner>,
    /// Every stored explanation already considered.
    seen: BTreeSet<ExplanationKey>,
    lineup: Lineup<Queued>,
    /// The newest explanation admitted to the lineup since the slot last held nothing.
    newest: Option<Recency>,
    serials: usize,
    turn: Option<Turn>,
    /// Virtual seconds at the slot's clock zero, while it holds anything.
    epoch: Option<f64>,
    /// Whether the inset camera, its lights and the corner's UI exist.
    rigged: bool,
    /// Each tank file's asset, read once.
    tanks: HashMap<String, Arc<TankAsset>>,
    /// Each staged tank file's view assets, held loaded until the slot is closed by a
    /// [`CloseCorner`].
    loaded: HashMap<String, (Handle<WorldAsset>, Handle<TankSpec>)>,
}

/// The tank file `vehicle`'s asset, read once into `cache`: the blueprint's when it names that
/// file, else read from disk. `None`, warned once, when it cannot be read.
pub(super) fn tank_asset(
    cache: &mut HashMap<String, Arc<TankAsset>>,
    vehicle: &str,
    blueprint: Option<&TankBlueprint>,
) -> Option<Arc<TankAsset>> {
    if let Some(tank) = cache.get(vehicle) {
        return Some(tank.clone());
    }
    let tank = match blueprint.filter(|blueprint| blueprint.file == vehicle) {
        Some(blueprint) => TankAsset::of_blueprint(blueprint),
        None => match TankAsset::load(vehicle) {
            Ok(tank) => tank,
            Err(error) => {
                warn_once!("pen cam: cannot stage {vehicle}: {error}");
                return None;
            }
        },
    };
    let tank = Arc::new(tank);
    cache.insert(vehicle.to_owned(), tank.clone());
    Some(tank)
}

impl Slot {
    /// The tank file `vehicle`'s asset ([`tank_asset`]).
    fn tank(&mut self, vehicle: &str, blueprint: Option<&TankBlueprint>) -> Option<Arc<TankAsset>> {
        tank_asset(&mut self.tanks, vehicle, blueprint)
    }

    /// Hold `vehicle`'s view assets loaded, so a stage view of it can spawn by the time the slot
    /// opens.
    fn preload(&mut self, asset_server: &AssetServer, vehicle: &str) {
        if !self.loaded.contains_key(vehicle) {
            let assets = StageView::load(asset_server, vehicle);
            self.loaded.insert(vehicle.to_owned(), assets);
        }
    }

    /// Seconds on the slot's clock, starting it at zero if it is stopped.
    fn clock(&mut self, time: &Time) -> f32 {
        let epoch = *self.epoch.get_or_insert(time.elapsed_secs_f64());
        (time.elapsed_secs_f64() - epoch) as f32
    }

    /// Drop every queued and shown turn and the slot's clock.
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

/// The corner's entities and the presentation resources they feed. The corner's mask set is not
/// among them: `look::own` releases it once the corner no longer shows a frame that needs one.
#[derive(SystemParam)]
pub(crate) struct Rig<'w, 's> {
    commands: Commands<'w, 's>,
    asset_server: Res<'w, AssetServer>,
    images: ResMut<'w, Assets<Image>>,
    view: Option<Res<'w, StageView>>,
    stage: ResMut<'w, Stage>,
    corner: ResMut<'w, Corner>,
    exhibits: ResMut<'w, Exhibits>,
    entities: Query<'w, 's, (Entity, Option<&'static ChildOf>), With<CornerRig>>,
    rigged: Query<'w, 's, (), With<CornerRig>>,
}

impl Rig<'_, '_> {
    /// The inset, its lights and the corner's UI, and the stage view of `vehicle`, spawning what
    /// is missing. Whether the stage view is placed.
    fn ready(&mut self, state: &mut Slot, vehicle: &str) -> bool {
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

    /// Despawn every entity of the corner and drop every image and resource it holds. One under
    /// another of the corner's goes with it.
    fn teardown(&mut self, state: &mut Slot) {
        for (entity, parent) in &self.entities {
            if parent.is_none_or(|parent| !self.rigged.contains(parent.parent())) {
                self.commands.entity(entity).despawn();
            }
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
    fn exists(&self, state: &Slot) -> bool {
        state.rigged || self.view.is_some()
    }
}

/// The player's tank: who it is this life, its body, its file and its components' live state.
#[derive(SystemParam)]
struct Player<'w, 's> {
    tank: Query<
        'w,
        's,
        (
            Entity,
            &'static CombatantId,
            &'static CombatantLife,
            Option<&'static TankVolumes>,
            Option<&'static TankSpecHandle>,
            Option<&'static TankRig>,
            Has<TankKnockedOut>,
        ),
        With<Controlled>,
    >,
    volumes: Query<'w, 's, LiveVolume>,
}

impl Player<'_, '_> {
    /// The tank's life, while it has one and is not knocked out.
    fn life(&self) -> Option<TankLife> {
        let (_, &combatant, &life, _, _, _, knocked_out) = self.tank.single().ok()?;
        (!knocked_out).then_some(TankLife { combatant, life })
    }

    /// The tank's body — its entity and rig — while it has a life and is not knocked out.
    fn body(&self) -> Option<(Entity, &TankRig)> {
        let (entity, _, _, _, _, rig, knocked_out) = self.tank.single().ok()?;
        (!knocked_out).then_some(())?;
        Some((entity, rig?))
    }

    /// The tank's file, as a record names it.
    fn vehicle(&self) -> Option<String> {
        let (_, _, _, _, spec, _, _) = self.tank.single().ok()?;
        let path = spec?.0.path()?;
        Some(path.path().to_string_lossy().into_owned())
    }

    fn live(&self) -> LiveState {
        self.tank
            .single()
            .ok()
            .and_then(|(_, _, _, volumes, _, _, _)| volumes)
            .map(|volumes| LiveState::read(volumes, &self.volumes))
            .unwrap_or_default()
    }
}

fn close(
    _close: On<CloseCorner>,
    mut state: ResMut<Slot>,
    store: Option<Res<ShotExplanations>>,
    mut rig: Rig,
) {
    rig.teardown(&mut state);
    state.forget(store.as_deref());
    state.loaded.clear();
}

/// Queue each newly presented explanation the slot admits, run the replacement rule on the slot's
/// clock, keep the rig for what is on show and write the corner.
#[expect(
    clippy::too_many_arguments,
    reason = "the slot's clock, store, settings and blueprint are separate inputs; `Player` and `Rig` already bundle what belongs together"
)]
fn drive(
    time: Res<Time>,
    mut state: ResMut<Slot>,
    store: Option<Res<ShotExplanations>>,
    settings: Option<Res<Settings>>,
    blueprint: Option<Res<TankBlueprint>>,
    player: Player,
    mut knobs: ResMut<Knobs>,
    mut rig: Rig,
) {
    let settings = settings.as_deref().copied().unwrap_or_default();
    let reduce_motion = settings.reduce_motion.on();
    if knobs.reduce_motion != reduce_motion {
        knobs.reduce_motion = reduce_motion;
    }
    let store = store.as_deref();
    let blueprint = blueprint.as_deref();
    let life = player.life();
    let admits = settings.pen_cam_corner;
    if life != state.life || Some(admits) != state.admits {
        if rig.exists(&state) {
            rig.teardown(&mut state);
        }
        state.forget(store);
        state.life = life;
        state.admits = Some(admits);
    }
    let Some(me) = state.life else {
        return;
    };

    // Every newly presented explanation the slot admits, and the floor no admission may fall
    // below: the newest admitted since the slot last held nothing, or still waiting on its impact.
    let mut presented = Vec::new();
    let mut floor = state.newest;
    if let Some(store) = store {
        let stored: BTreeSet<ExplanationKey> = store.iter().map(|(key, _)| *key).collect();
        state.seen.retain(|key| stored.contains(key));
        for (key, entry) in store.iter() {
            if state.seen.contains(key) {
                continue;
            }
            let explanation = &entry.explanation;
            let fired = explanation.audience == Audience::Shooter
                && explanation.shot.combatant == me.combatant
                && explanation.struck.combatant != me.combatant;
            let Some(vehicle) = entry.report.vehicle.clone().filter(|_| fired) else {
                state.seen.insert(*key);
                continue;
            };
            let admitted = settings.pen_cam_corner.opens(entry.report.impact.verdict);
            // Loaded while the impact is still to draw.
            state.preload(&rig.asset_server, &vehicle);
            let recency = Recency::of(*key, explanation);
            match entry.presence {
                ImpactPresence::Pending => {
                    if admitted {
                        floor = floor.max(Some(recency));
                    }
                    continue;
                }
                ImpactPresence::TimedOut => {}
                ImpactPresence::Presented if admitted => {
                    presented.push((recency, entry, vehicle));
                }
                ImpactPresence::Presented => {}
            }
            state.seen.insert(*key);
        }
    }
    // Newest first: of explanations presented together only the newest opens.
    presented.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (recency, entry, vehicle) in presented {
        if floor.is_some_and(|floor| recency < floor) {
            continue;
        }
        let Some(tank) = state.tank(&vehicle, blueprint) else {
            continue;
        };
        let plan = Plan::new(&entry.report, &tank.shapes(&entry.report.servo_deg));
        let exhibit = Exhibit {
            servo_deg: entry.report.servo_deg.clone(),
            plan: Arc::new(plan),
        };
        let now = state.clock(&time);
        state.serials += 1;
        state.newest = Some(recency);
        floor = Some(recency);
        let queued = Queued {
            serial: state.serials,
            kind: exhibit.plan.kind,
            vehicle,
            exhibit,
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
        rig.exhibits.0 = vec![queued.exhibit];
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
    use crate::command::Bindings;
    use crate::damage_cam::PenetrationReport;
    use crate::damage_cam::delivery::tests::{SHOOTER, STRUCK, recorded, report, vocabulary};
    use crate::damage_cam::pen_cam::look::{LookMaterial, Masks};
    use crate::damage_cam::pen_cam::stage::StageCamera;
    use crate::damage_cam::wire::{BuiltExplanation, ShotVocabulary};

    /// The test's frame step (s), a 30 Hz frame: fixture data.
    pub(super) const STEP_S: f32 = 1.0 / 30.0;

    /// The corner's systems over a headless app, the player's tank (the shooter's combatant), an
    /// empty store and bevy's shared default image ([`default_image_kept`]), one frame run.
    fn corner_app(settings: Settings) -> (App, Entity) {
        corner_app_with(settings, |_| {})
    }

    /// [`corner_app`] with `mount` run among its plugins, before the app finishes.
    pub(super) fn corner_app_with(
        settings: Settings,
        mount: impl FnOnce(&mut App),
    ) -> (App, Entity) {
        let mut app = App::new();
        app.set_error_handler(bevy::ecs::error::ignore);
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            TransformPlugin,
            bevy::world_serialization::WorldSerializationPlugin,
        ));
        mount(&mut app);
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app.init_asset::<TankSpec>()
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
        stage_of(
            app,
            report
                .vehicle
                .as_deref()
                .expect("the record names its tank file"),
        );
    }

    /// A stage view of tank file `vehicle` standing in for its glb.
    pub(super) fn stage_of(app: &mut App, vehicle: &str) {
        let vehicle = vehicle.to_owned();
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
        encounter(name, fire_tick, STRUCK, fire_tick + 1, 0)
    }

    /// [`explained`] for an encounter of the shot fired on `fire_tick` with `struck`'s first life,
    /// opened on `contact_tick`, drawn at the shot's outcome `outcome`.
    fn encounter(
        name: &str,
        fire_tick: u32,
        struck: CombatantId,
        contact_tick: u32,
        outcome: u32,
    ) -> (
        ShotExplanation,
        ShotExplanation,
        ShotVocabulary,
        PenetrationReport,
    ) {
        hit(name, SHOOTER, fire_tick, struck, contact_tick, outcome)
    }

    /// `name`'s record of another tank's round fired on `fire_tick` striking the player's first
    /// life (the [`SHOOTER`] combatant) a tick later: the shooter's projection, the player's, and
    /// the vocabulary.
    fn incoming(
        name: &str,
        fire_tick: u32,
    ) -> (
        ShotExplanation,
        ShotExplanation,
        ShotVocabulary,
        PenetrationReport,
    ) {
        hit(name, STRUCK, fire_tick, SHOOTER, fire_tick + 1, 0)
    }

    /// [`encounter`] by `shooter`'s round.
    fn hit(
        name: &str,
        shooter: CombatantId,
        fire_tick: u32,
        struck: CombatantId,
        contact_tick: u32,
        outcome: u32,
    ) -> (
        ShotExplanation,
        ShotExplanation,
        ShotVocabulary,
        PenetrationReport,
    ) {
        let report = report(name);
        let vocabulary = vocabulary(&report);
        let struck = TankLife {
            combatant: struck,
            life: CombatantLife(0),
        };
        let mut recorded = recorded(&report, shooter, fire_tick, struck);
        recorded.contact_tick = contact_tick;
        recorded.completion_tick = contact_tick + 2;
        recorded.outcome = Some(outcome);
        let built = BuiltExplanation::build(&vocabulary, &recorded).unwrap();
        let mut projections = built.projections().into_iter().map(|(_, e)| e);
        let victim = projections.next().unwrap();
        let shooter = projections.next().unwrap();
        assert_eq!(shooter.audience, Audience::Shooter);
        assert_ne!(victim.audience, Audience::Shooter);
        (shooter, victim, vocabulary, report)
    }

    /// Store `explanation` with its impact drawn.
    pub(super) fn receive(
        app: &mut App,
        explanation: ShotExplanation,
        vocabulary: &ShotVocabulary,
    ) {
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        app.world_mut()
            .resource_mut::<ShotExplanations>()
            .receive_drawn(explanation, vocabulary, now);
    }

    /// Whether bevy's shared image, the one `ImagePlugin` keeps at `Handle::default()` for
    /// untextured UI, is still stored.
    pub(super) fn default_image_kept(app: &App) -> bool {
        app.world()
            .resource::<Assets<Image>>()
            .contains(&Handle::<Image>::default())
    }

    pub(super) fn step(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(STEP_S));
        app.update();
    }

    /// The corner's entities, inset cameras and every entity but the resources', in that order.
    pub(super) fn census(app: &mut App) -> (usize, usize, usize) {
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
            assert!(
                app.world().resource::<Exhibits>().0.len() <= 1,
                "never two surfaces"
            );
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
        let staged = census(&mut app);
        receive(&mut app, shooter, &vocabulary);
        assert!(run(&mut app, 2.0).is_empty());
        assert_eq!(census(&mut app), staged, "nothing spawned");

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

    /// A victim's projection never opens the corner: the slot admits no victim turn, whether the
    /// struck tank is the player's own or another's.
    #[test]
    fn the_victims_projection_never_opens_the_corner() {
        for combatant in [STRUCK, SHOOTER] {
            let (mut app, player) = corner_app(Settings::default());
            app.world_mut().entity_mut(player).insert(combatant);
            app.update();
            let (_, victim, vocabulary, report) = explained("s1_through_crew", 1200);
            stage_ready(&mut app, &report);
            receive(&mut app, victim, &vocabulary);
            assert!(run(&mut app, 4.0).is_empty(), "{combatant:?}");
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
        let (waiting, _, _, _) = explained("s1_through_crew", 1100);
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

    /// A newer explanation still waiting on its impact holds the order across the corner closing:
    /// an older one presented meanwhile never opens, and the newer opens once drawn.
    #[test]
    fn a_pending_newer_explanation_keeps_an_older_one_out_across_a_close() {
        let (mut app, _) = corner_app(Settings::default());
        let (a, _, a_vocabulary, report) = explained("s1_through_crew", 1200);
        let (b, _, b_vocabulary, _) = explained("s2_stopped_mantlet", 1400);
        let (c, _, c_vocabulary, _) = explained("s3_ricochet", 1100);
        stage_ready(&mut app, &report);
        receive(&mut app, a, &a_vocabulary);
        let mut shown = run(&mut app, CornerKind::Glass.duration());
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        app.world_mut()
            .resource_mut::<ShotExplanations>()
            .receive(b.clone(), &b_vocabulary, now);
        shown.extend(run(&mut app, 1.0));
        assert_eq!(census(&mut app).0, 0, "the first corner closed");
        stage_ready(&mut app, &report);
        receive(&mut app, c, &c_vocabulary);
        shown.extend(run(&mut app, 1.0));
        app.world_mut()
            .resource_mut::<ShotExplanations>()
            .drew(b.shot, b.outcome.unwrap());
        let drawn = shown.len();
        let after = run(&mut app, timeline::OPEN_LATENCY_S + 4.0 * STEP_S);
        assert_eq!(
            after.first().map(|(_, kind)| *kind),
            Some(CornerKind::Slice),
            "the newer opens once drawn, at once"
        );
        shown.extend(after);
        shown.extend(run(&mut app, 5.0));
        shown.dedup();
        let kinds: Vec<CornerKind> = shown.iter().map(|(_, kind)| *kind).collect();
        assert_eq!(
            kinds,
            [CornerKind::Glass, CornerKind::Slice],
            "drawn at {drawn}"
        );
    }

    /// Two encounters of one shot, a ricochet off one tank and the later contact with a tank of a
    /// lower id: the later contact is the one that shows, whichever arrives first.
    #[test]
    fn a_shots_later_encounter_shows_whichever_tank_it_struck() {
        const LOWER: CombatantId = CombatantId(STRUCK.0 - 1);
        assert!(LOWER != SHOOTER);
        for later_first in [true, false] {
            let (mut app, _) = corner_app(Settings::default());
            let (bounce, _, bounce_vocabulary, report) =
                encounter("s3_ricochet", 1200, STRUCK, 1201, 0);
            let (later, _, later_vocabulary, _) =
                encounter("s1_through_crew", 1200, LOWER, 1204, 1);
            stage_ready(&mut app, &report);
            let mut arrivals = [(bounce, bounce_vocabulary), (later, later_vocabulary)];
            if later_first {
                arrivals.reverse();
            }
            let mut shown = Vec::new();
            for (explanation, vocabulary) in arrivals {
                receive(&mut app, explanation, &vocabulary);
                shown.extend(run(&mut app, 1.0));
            }
            shown.extend(run(&mut app, 6.0));
            shown.dedup();
            let kinds: Vec<CornerKind> = shown.iter().map(|(_, kind)| *kind).collect();
            let expected: &[CornerKind] = if later_first {
                &[CornerKind::Glass]
            } else {
                &[CornerKind::Ricochet, CornerKind::Glass]
            };
            assert_eq!(kinds, expected, "later first: {later_first}");
        }
        let (mut app, _) = corner_app(Settings::default());
        let (bounce, _, bounce_vocabulary, report) =
            encounter("s3_ricochet", 1200, STRUCK, 1201, 0);
        let (later, _, later_vocabulary, _) = encounter("s1_through_crew", 1200, LOWER, 1204, 1);
        stage_ready(&mut app, &report);
        receive(&mut app, bounce, &bounce_vocabulary);
        receive(&mut app, later, &later_vocabulary);
        let kinds: Vec<CornerKind> = run(&mut app, 6.0).into_iter().map(|(_, k)| k).collect();
        assert_eq!(kinds, [CornerKind::Glass], "arriving together");
    }

    /// The player's tank file: fixture data, the file the fixtures' records name.
    const OWN: &str = "tiger_1/tiger_1.tank.ron";

    /// [`corner_app`] with the player's tank file and, as volumes of the player's tank, every
    /// component of the fixture `s9_breech_and_loader`'s tank at its full pool.
    fn xray_app(settings: Settings) -> (App, Entity) {
        let (mut app, player) = corner_app(settings);
        let spec = app.world().resource::<AssetServer>().load::<TankSpec>(OWN);
        app.world_mut()
            .entity_mut(player)
            .insert(TankSpecHandle(spec));
        for component in report("s9_breech_and_loader").components {
            app.world_mut().spawn((
                Name::new(component.name),
                crate::ballistics::ComponentHealth {
                    current: component.count,
                    max: component.count,
                },
                crate::damage::VolumeOf(player),
            ));
        }
        app.update();
        (app, player)
    }

    fn me() -> TankLife {
        TankLife {
            combatant: SHOOTER,
            life: CombatantLife(0),
        }
    }

    /// Of the player's records the x-ray draws strictly the newest by recency, whatever it tells:
    /// a later clean ricochet replaces an earlier penetration, whichever arrives first, and the
    /// x-ray then draws the bounce and no crossing.
    #[test]
    fn a_later_ricochet_replaces_an_earlier_penetration() {
        let (mut app, _, _, _) = world_xray_app();
        let (_, crossing, vocabulary, _) = incoming("s1_through_crew", 1200);
        let (_, ricochet, ricochet_vocabulary, _) = incoming("s3_ricochet", 1300);
        assert!(ricochet.contact_tick > crossing.contact_tick);
        let latest = |app: &App| {
            xray::latest(app.world().resource::<ShotExplanations>(), me()).map(|(key, _)| key)
        };
        press_x(&mut app);
        receive(&mut app, crossing.clone(), &vocabulary);
        assert_eq!(latest(&app), Some(ExplanationKey::of(&crossing)));
        run(&mut app, 0.2);
        let plan = |app: &App| {
            let xray = app.world().resource::<xray::XRayState>();
            let plan = xray.plan().expect("the x-ray is built");
            (plan.kind, !plan.crossings.is_empty(), plan.bounce.is_some())
        };
        assert_eq!(
            plan(&app),
            (CornerKind::Victim, true, false),
            "the crossing's path"
        );
        receive(&mut app, ricochet.clone(), &ricochet_vocabulary);
        assert_eq!(latest(&app), Some(ExplanationKey::of(&ricochet)));
        run(&mut app, 0.2);
        assert_eq!(plan(&app), (CornerKind::Victim, false, true), "the bounce");

        let (mut app, _, _, _) = world_xray_app();
        receive(&mut app, ricochet.clone(), &ricochet_vocabulary);
        receive(&mut app, crossing, &vocabulary);
        assert_eq!(
            latest(&app),
            Some(ExplanationKey::of(&ricochet)),
            "arrival never orders"
        );
    }

    // --- The x-ray -------------------------------------------------------------------------

    /// [`xray_app`] with the player's tank as a live body — a rig whose hull and turret nodes
    /// carry one mesh each in a standard material, the turret's mesh node named after the
    /// fixture's first component — the player's camera drawing an image, and the keys. The app,
    /// the tank, its hull and turret meshes, and the camera.
    fn world_xray_app() -> (App, Entity, [Entity; 2], Entity) {
        use bevy::camera::{ImageRenderTarget, RenderTarget};
        use bevy::render::render_resource::TextureFormat;

        let (mut app, player) = xray_app(Settings::default());
        app.insert_resource(Bindings::default())
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>();
        let component = report("s9_breech_and_loader").components[0].name.clone();
        let world = app.world_mut();
        let standard = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let hull = world
            .spawn((
                Name::new(view::HULL_NODE),
                Transform::IDENTITY,
                Visibility::default(),
                ChildOf(player),
            ))
            .id();
        let turret = world
            .spawn((
                Name::new("Turret"),
                Transform::IDENTITY,
                Visibility::default(),
                ChildOf(hull),
            ))
            .id();
        let gun = world.spawn((Transform::IDENTITY, ChildOf(turret))).id();
        let muzzle = world.spawn((Transform::IDENTITY, ChildOf(gun))).id();
        let mesh = |world: &mut World, parent: Entity, name: Option<String>| {
            let mut mesh = world.spawn((
                Mesh3d(Handle::default()),
                MeshMaterial3d(standard.clone()),
                Transform::IDENTITY,
                Visibility::default(),
                ChildOf(parent),
            ));
            if let Some(name) = name {
                mesh.insert(Name::new(name));
            }
            mesh.id()
        };
        let hull_mesh = mesh(world, hull, None);
        let turret_mesh = mesh(world, turret, Some(component));
        world.entity_mut(player).insert((
            crate::tank::Rig {
                hull,
                turret,
                gun,
                muzzle,
            },
            Transform::IDENTITY,
            Visibility::default(),
        ));
        let image = world
            .resource_mut::<Assets<Image>>()
            .add(Image::new_target_texture(
                64,
                32,
                TextureFormat::Rgba8UnormSrgb,
                None,
            ));
        let camera = world
            .spawn((
                crate::view::PlayerView,
                crate::render_policy::CameraProfile::BattlefieldThirdPerson,
                RenderTarget::Image(ImageRenderTarget {
                    handle: image,
                    scale_factor: 1.0,
                }),
                Transform::from_xyz(0.0, 3.0, 10.0),
            ))
            .id();
        app.update();
        (app, player, [hull_mesh, turret_mesh], camera)
    }

    /// Press the x-ray key for one frame.
    fn press_x(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyX);
        step(app);
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::KeyX);
        keys.clear();
    }

    /// The x-ray's footprint: dressed meshes, its items, mask-set viewers, images and look
    /// materials stored, and every entity but the resources'.
    fn xray_census(app: &mut App) -> (usize, usize, Vec<Entity>, usize, usize, usize) {
        let viewers = app.world().resource::<Masks>().viewers();
        let images = app.world().resource::<Assets<Image>>().len();
        let looks = app.world().resource::<Assets<LookMaterial>>().len();
        let world = app.world_mut();
        let dressed = world
            .query_filtered::<(), With<xray::Dressed>>()
            .iter(world)
            .count();
        let items = world
            .query_filtered::<(), With<xray::XRayItem>>()
            .iter(world)
            .count();
        let all = world
            .query_filtered::<(), Without<bevy::ecs::resource::IsResource>>()
            .iter(world)
            .count();
        (dressed, items, viewers, images, looks, all)
    }

    /// Whether `mesh` wears the look: its look material on, its standard material off, and the
    /// x-ray's mask copy under it.
    fn wears_the_look(app: &mut App, mesh: Entity) -> bool {
        let copies: Vec<Entity> = {
            let world = app.world_mut();
            let mut query = world.query::<(&xray::MaskCopy, &ChildOf)>();
            query
                .iter(world)
                .filter(|(copy, parent)| copy.of() == mesh && parent.parent() == mesh)
                .map(|(_, parent)| parent.parent())
                .collect()
        };
        let entity = app.world().entity(mesh);
        entity.contains::<xray::Dressed>()
            && entity.contains::<MeshMaterial3d<LookMaterial>>()
            && !entity.contains::<MeshMaterial3d<StandardMaterial>>()
            && copies.len() == 1
    }

    /// X dresses the player's live tank — every one of its meshes wears the look, the turret's
    /// under the live servo node — and owns one mask set, the player's camera's; another tank's
    /// meshes are untouched, and the record's elements hang under the live hull.
    #[test]
    fn x_dresses_the_live_tank_for_the_players_camera() {
        let (mut app, player, meshes, camera) = world_xray_app();
        let other = app
            .world_mut()
            .spawn((
                STRUCK,
                CombatantLife(0),
                Transform::IDENTITY,
                Visibility::default(),
            ))
            .id();
        let standard = app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(meshes[0])
            .unwrap()
            .0
            .clone();
        let other_mesh = app
            .world_mut()
            .spawn((
                Mesh3d(Handle::default()),
                MeshMaterial3d(standard),
                Transform::IDENTITY,
                Visibility::default(),
                ChildOf(other),
            ))
            .id();
        let (_, crossing, vocabulary, _) = incoming("s1_through_crew", 1200);
        receive(&mut app, crossing, &vocabulary);
        assert_eq!(xray_census(&mut app).0, 0, "off until the key");
        press_x(&mut app);
        run(&mut app, 0.2);
        assert!(app.world().resource::<XRay>().on);
        for mesh in meshes {
            assert!(wears_the_look(&mut app, mesh), "{mesh:?}");
        }
        let turret = app.world().get::<crate::tank::Rig>(player).unwrap().turret;
        assert_eq!(
            app.world().get::<ChildOf>(meshes[1]).map(ChildOf::parent),
            Some(turret),
            "the dressed turret mesh is the one under the live servo node"
        );
        assert!(!wears_the_look(&mut app, other_mesh));
        assert!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(other_mesh)
                .is_some()
        );
        let (dressed, items, viewers, ..) = xray_census(&mut app);
        assert_eq!(dressed, 2);
        assert!(
            items > 2,
            "the record's elements beyond the two mask copies"
        );
        assert_eq!(viewers, vec![camera], "the player's camera owns the masks");
        let hull = app.world().get::<crate::tank::Rig>(player).unwrap().hull;
        let world = app.world_mut();
        let under_hull = world
            .query_filtered::<&ChildOf, With<xray::XRayItem>>()
            .iter(world)
            .filter(|parent| parent.parent() == hull)
            .count();
        assert!(under_hull > 0, "the record hangs under the live hull");
    }

    /// Every deactivation — the key, a knockout, the gunner's optic, the tank gone — puts every
    /// mesh back in exactly the material it wore and leaves nothing of the x-ray: no tag, no
    /// item, no mask set, no image and no look material beyond what stood before.
    #[test]
    fn every_deactivation_restores_the_tank_exactly() {
        for ending in ["key", "knockout", "sight", "gone"] {
            let (mut app, player, meshes, _) = world_xray_app();
            let (_, crossing, vocabulary, _) = incoming("s1_through_crew", 1200);
            receive(&mut app, crossing, &vocabulary);
            // A struck component, so a tint hangs under a dressed mesh.
            {
                let world = app.world_mut();
                let mut volumes = world.query::<&mut crate::ballistics::ComponentHealth>();
                let mut health = volumes
                    .iter_mut(world)
                    .find(|health| health.max > 1)
                    .expect("fixture data: a component with a pool to lose points of");
                health.current -= 1;
            }
            let originals: Vec<AssetId<StandardMaterial>> = meshes
                .iter()
                .map(|mesh| {
                    app.world()
                        .get::<MeshMaterial3d<StandardMaterial>>(*mesh)
                        .unwrap()
                        .id()
                })
                .collect();
            let before = xray_census(&mut app);
            press_x(&mut app);
            run(&mut app, 0.2);
            let (dressed, items, ..) = xray_census(&mut app);
            assert_eq!(dressed, 2, "{ending}");
            assert!(items > 2, "{ending}");
            let mut gone = 0;
            match ending {
                "key" => press_x(&mut app),
                "knockout" => {
                    app.world_mut().entity_mut(player).insert(TankKnockedOut {
                        reason: crate::damage::KnockoutReason::CrewLoss,
                    });
                }
                "sight" => {
                    app.insert_resource(crate::sight::SightMode::Gunner);
                }
                _ => {
                    let world = app.world_mut();
                    gone = world
                        .query_filtered::<(), Without<bevy::ecs::resource::IsResource>>()
                        .iter(world)
                        .count();
                    world.despawn(player);
                    gone -= world
                        .query_filtered::<(), Without<bevy::ecs::resource::IsResource>>()
                        .iter(world)
                        .count();
                    // The x-ray's own went with the tank; the rest is the tank's.
                    gone -= items;
                }
            }
            run(&mut app, 0.2);
            let after = xray_census(&mut app);
            assert_eq!((after.0, after.1), (0, 0), "{ending}: nothing of the x-ray");
            assert!(after.2.is_empty(), "{ending}: no mask set");
            assert_eq!(after.3, before.3, "{ending}: images");
            assert_eq!(after.4, before.4, "{ending}: look materials");
            assert_eq!(after.5, before.5 - gone, "{ending}: entities");
            if ending != "gone" {
                for (mesh, original) in meshes.iter().zip(&originals) {
                    let entity = app.world().entity(*mesh);
                    assert_eq!(
                        entity
                            .get::<MeshMaterial3d<StandardMaterial>>()
                            .map(|m| m.id()),
                        Some(*original),
                        "{ending}: the same handle"
                    );
                    assert!(!entity.contains::<MeshMaterial3d<LookMaterial>>());
                }
            }
            assert!(app.world().resource::<XRay>().on == (ending != "key"));
        }
    }

    /// A corner that needs masks opening while the x-ray is on: two mask sets, each on its own
    /// viewer, their images distinct, and every mesh the x-ray dresses masked by the player's
    /// camera's set and no other; the corner's close releases its own and the x-ray keeps the
    /// other, dressed throughout.
    #[test]
    fn the_corner_and_the_xray_each_own_their_viewers_masks() {
        let (mut app, _, meshes, camera) = world_xray_app();
        press_x(&mut app);
        run(&mut app, 0.2);
        assert_eq!(xray_census(&mut app).2, vec![camera]);
        stage_of(&mut app, OWN);
        let (shooter, _, vocabulary, report) = explained("s1_through_crew", 1200);
        let kind = CornerKind::of(report.impact.verdict);
        assert_eq!(
            kind,
            CornerKind::Glass,
            "fixture data: a frame that needs masks"
        );
        receive(&mut app, shooter, &vocabulary);
        let (mut both, mut masked) = (false, false);
        for _ in 0..(1.0 / STEP_S) as usize {
            step(&mut app);
            let mut viewers = xray_census(&mut app).2;
            viewers.sort();
            let inset = app
                .world_mut()
                .query_filtered::<Entity, With<StageCamera>>()
                .iter(app.world())
                .next();
            if let Some(inset) = inset {
                let mut want = vec![camera, inset];
                want.sort();
                both |= viewers == want;
                let masks = app.world().resource::<Masks>();
                if let (Some(inset), Some(own)) = (masks.of(inset), masks.of(camera)) {
                    assert_ne!(inset.hull, own.hull, "two sets, two images");
                    for mesh in meshes {
                        let handle = app
                            .world()
                            .get::<MeshMaterial3d<LookMaterial>>(mesh)
                            .expect("dressed")
                            .0
                            .clone();
                        let material = app
                            .world()
                            .resource::<Assets<LookMaterial>>()
                            .get(&handle)
                            .expect("stored");
                        assert_eq!(
                            material.hull_mask.as_ref(),
                            Some(&own.hull),
                            "the camera's set"
                        );
                        assert_eq!(material.part_mask.as_ref(), Some(&own.part));
                    }
                    masked = true;
                }
            }
            assert!(viewers.contains(&camera), "the x-ray keeps its set");
            assert_eq!(xray_census(&mut app).0, 2, "and stays dressed");
        }
        assert!(both, "two sets, each on its own viewer");
        assert!(masked, "both sets existed at once");
        run(&mut app, kind.duration() + 1.0);
        assert_eq!(census(&mut app).0, 0, "the corner closed");
        assert_eq!(xray_census(&mut app).2, vec![camera], "its set alone went");
        assert_eq!(xray_census(&mut app).0, 2);
    }

    /// `name`'s record of another tank's round striking the player's first life and knocking it
    /// out: the player's projection and the vocabulary.
    fn killing(name: &str) -> (ShotExplanation, ShotVocabulary) {
        let mut report = report(name);
        report.killed = Some(crate::damage::KnockoutReason::CrewLoss);
        let vocabulary = vocabulary(&report);
        let mut recorded = recorded(&report, STRUCK, 1200, me());
        recorded.contact_tick = 1201;
        recorded.completion_tick = 1203;
        recorded.outcome = Some(0);
        let built = BuiltExplanation::build(&vocabulary, &recorded).unwrap();
        let victim = built
            .projections()
            .into_iter()
            .map(|(_, e)| e)
            .next()
            .unwrap();
        assert_ne!(victim.audience, Audience::Shooter);
        (victim, vocabulary)
    }

    /// With no record of the player's life the x-ray is the live tint alone: the base look on the
    /// tank, no path, no cone, no shell; the tint follows the live state in the same showing.
    #[test]
    fn with_no_record_the_xray_is_the_live_tint_alone() {
        let (mut app, _, meshes, _) = world_xray_app();
        press_x(&mut app);
        run(&mut app, 0.2);
        for mesh in meshes {
            assert!(wears_the_look(&mut app, mesh));
        }
        let (dressed, items, ..) = xray_census(&mut app);
        assert_eq!((dressed, items), (2, 2), "the mask copies and nothing else");
        let pools = |app: &App| -> Vec<(String, u32, u32, bool)> {
            let xray = app.world().resource::<xray::XRayState>();
            let plan = xray.plan().expect("built");
            assert!(plan.line.points.len() < 2, "no path");
            assert!(plan.cones.is_empty(), "no cone");
            plan.boxes
                .iter()
                .map(|b| (b.name.clone(), b.count, b.hp, b.lost))
                .collect()
        };
        let before = pools(&app);
        assert!(!before.is_empty());
        assert!(
            before
                .iter()
                .all(|(_, count, hp, lost)| hp == count && !lost)
        );
        let since = app.world().resource::<xray::XRayState>().since();
        {
            let world = app.world_mut();
            let mut volumes = world.query::<&mut crate::ballistics::ComponentHealth>();
            let mut health = volumes
                .iter_mut(world)
                .find(|health| health.max > 1)
                .expect("fixture data: a component with a pool to lose points of");
            health.current -= 1;
        }
        run(&mut app, 0.1);
        assert_ne!(pools(&app), before, "the live loss reaches the x-ray");
        assert_eq!(
            app.world().resource::<xray::XRayState>().since(),
            since,
            "in the same showing"
        );
        assert!(xray_census(&mut app).1 > 2, "the struck component's tint");
    }

    /// A record plays once, from the moment it first shows: a record that lands while the x-ray is
    /// off waits for X; X off and on, and the optic and back, continue its clock; a newer record
    /// plays from its own first showing.
    #[test]
    fn a_record_plays_once_from_its_first_showing() {
        let (mut app, _, _, _) = world_xray_app();
        let since = |app: &App| app.world().resource::<xray::XRayState>().since();
        let now = |app: &App| app.world().resource::<Time>().elapsed_secs_f64();
        let (first, first_vocabulary) = incoming_at("s1_through_crew", 1299, 1300, 1302);
        let landed = now(&app);
        receive(&mut app, first, &first_vocabulary);
        run(&mut app, 1.0);
        assert_eq!(since(&app), None, "off: nothing shows");
        press_x(&mut app);
        run(&mut app, 0.1);
        let shown = since(&app).expect("built");
        assert!(
            shown >= landed + 0.9,
            "the clock starts at X, not at the landing"
        );
        run(&mut app, 0.5);
        press_x(&mut app);
        run(&mut app, 0.2);
        assert_eq!(since(&app), None, "X off");
        press_x(&mut app);
        run(&mut app, 0.2);
        assert_eq!(since(&app), Some(shown), "X on again continues the clock");
        app.insert_resource(crate::sight::SightMode::Gunner);
        run(&mut app, 0.2);
        assert_eq!(since(&app), None, "the optic");
        app.insert_resource(crate::sight::SightMode::ThirdPerson);
        run(&mut app, 0.2);
        assert_eq!(since(&app), Some(shown), "and back continues the clock");
        let (second, second_vocabulary) = incoming_at("s3_ricochet", 1310, 1320, 1322);
        receive(&mut app, second, &second_vocabulary);
        run(&mut app, 0.2);
        let again = since(&app).expect("built");
        assert!(
            again > shown,
            "a newer record plays from its own first showing"
        );
    }

    /// The clock is kept per record, not for the last alone: the store evicts by fire tick while
    /// the x-ray picks by recency, so a newer record can leave the store before an older one that
    /// showed earlier; back as the latest, the older continues its clock. A connection forgets
    /// every clock with the store: the same key in the next match plays afresh.
    #[test]
    fn an_older_record_back_as_the_latest_continues_its_clock() {
        let (mut app, _, _, _) = world_xray_app();
        let since = |app: &App| app.world().resource::<xray::XRayState>().since();
        let latest = |app: &App| {
            xray::latest(app.world().resource::<ShotExplanations>(), me()).map(|(key, _)| key)
        };
        press_x(&mut app);
        // A fired later and struck earlier than B: B is the latest by recency and the first the
        // store evicts.
        let (a, a_vocabulary) = incoming_at("s1_through_crew", 1299, 1300, 1302);
        let a_key = ExplanationKey::of(&a);
        receive(&mut app, a, &a_vocabulary);
        run(&mut app, 0.2);
        let a_since = since(&app).expect("built");
        let (b, b_vocabulary) = incoming_at("s3_ricochet", 1290, 1310, 1312);
        receive(&mut app, b, &b_vocabulary);
        run(&mut app, 0.2);
        let b_since = since(&app).expect("built");
        assert!(b_since > a_since, "B shows after A");
        // Another life's records, fired after both, fill the store past its retention: B goes.
        let other = TankLife {
            combatant: SHOOTER,
            life: CombatantLife(1),
        };
        let filler = report("s9_breech_and_loader");
        let filler_vocabulary = vocabulary(&filler);
        for fire_tick in 0..(crate::damage_cam::delivery::RETAINED as u32 - 1) {
            let tick = 1300 + fire_tick;
            let explanation =
                projected(&filler, &filler_vocabulary, tick, tick + 1, tick + 3, other);
            receive(&mut app, explanation, &filler_vocabulary);
        }
        assert_eq!(latest(&app), Some(a_key), "B evicted, A the latest again");
        run(&mut app, 0.2);
        assert_eq!(since(&app), Some(a_since), "A continues its clock");

        // A new match, as the network client raises it with the store's reset, and the same key
        // again before the x-ray has seen a frame of the empty store.
        *app.world_mut().resource_mut::<ShotExplanations>() = default();
        app.world_mut().trigger(NewMatch);
        let (a, a_vocabulary) = incoming_at("s1_through_crew", 1299, 1300, 1302);
        assert_eq!(
            ExplanationKey::of(&a),
            a_key,
            "the same key in the next match"
        );
        receive(&mut app, a, &a_vocabulary);
        run(&mut app, 0.2);
        assert!(since(&app).expect("built") > b_since, "plays afresh");
    }

    /// A knockout suspends the x-ray and X during the run it starts changes nothing; the x-ray
    /// returns on the respawn, the preference untouched.
    #[test]
    fn x_during_a_run_is_a_no_op_and_the_xray_returns_after_respawn() {
        let (mut app, player, _, _) = world_xray_app();
        press_x(&mut app);
        run(&mut app, 0.2);
        assert_eq!(xray_census(&mut app).0, 2);
        let (explanation, vocabulary) = killing("s1_through_crew");
        receive(&mut app, explanation, &vocabulary);
        app.world_mut().entity_mut(player).insert(TankKnockedOut {
            reason: crate::damage::KnockoutReason::CrewLoss,
        });
        step(&mut app);
        assert_eq!(xray_census(&mut app).0, 0, "suspended at the knockout");
        let running = |app: &App| app.world().resource::<killcam::Victim>().running();
        for _ in 0..(1.0 / STEP_S) as usize {
            if running(&app) {
                break;
            }
            step(&mut app);
        }
        assert!(running(&app), "the run plays");
        press_x(&mut app);
        assert!(
            app.world().resource::<XRay>().on,
            "the press changed nothing"
        );
        assert_eq!(xray_census(&mut app).0, 0);
        let mut tank = app.world_mut().entity_mut(player);
        tank.remove::<TankKnockedOut>();
        tank.insert(CombatantLife(1));
        run(&mut app, 0.2);
        assert!(!running(&app));
        assert_eq!(xray_census(&mut app).0, 2, "back on the new life");
    }
    /// A new mask set for the player's camera (a resize) rebuilds the whole x-ray: every mesh is
    /// dressed again, in materials holding the new set's images, and the old set's are gone.
    #[test]
    fn a_new_mask_set_dresses_the_tank_afresh() {
        use bevy::camera::{ImageRenderTarget, RenderTarget};
        use bevy::render::render_resource::TextureFormat;

        let (mut app, _, meshes, camera) = world_xray_app();
        press_x(&mut app);
        run(&mut app, 0.2);
        let hull_mask = |app: &App, mesh: Entity| {
            let handle = app
                .world()
                .get::<MeshMaterial3d<LookMaterial>>(mesh)
                .expect("dressed")
                .0
                .clone();
            app.world()
                .resource::<Assets<LookMaterial>>()
                .get(&handle)
                .expect("stored")
                .hull_mask
                .clone()
        };
        let set_hull = |app: &App| {
            app.world()
                .resource::<Masks>()
                .of(camera)
                .unwrap()
                .hull
                .clone()
        };
        let old = set_hull(&app);
        assert_eq!(hull_mask(&app, meshes[0]), Some(old.clone()));

        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::new_target_texture(
                128,
                64,
                TextureFormat::Rgba8UnormSrgb,
                None,
            ));
        app.world_mut()
            .entity_mut(camera)
            .insert(RenderTarget::Image(ImageRenderTarget {
                handle: image,
                scale_factor: 1.0,
            }));
        run(&mut app, 0.2);
        let new = set_hull(&app);
        assert_ne!(new, old, "a new set");
        let (dressed, items, viewers, ..) = xray_census(&mut app);
        assert_eq!((dressed, items), (2, 2), "dressed afresh");
        assert_eq!(viewers, vec![camera]);
        for mesh in meshes {
            assert!(wears_the_look(&mut app, mesh));
            assert_eq!(
                hull_mask(&app, mesh),
                Some(new.clone()),
                "the new set's images"
            );
        }
        assert!(
            !app.world().resource::<Assets<Image>>().contains(&old),
            "the old set's images are gone"
        );
    }

    /// The x-ray's round is on the scene side of the draw order: once its model lands under the
    /// real spawner, each of its meshes keeps its own material and carries exactly one mask copy of
    /// its own mesh, in the part mask, drawn with the material every mask copy of the tank wears
    /// (the masks' material, uncut); a new mask set (a resize) draws it again on the new set. The
    /// x-ray relies on it: without the copy a field behind the round draws over it and the body's
    /// glass covers it, a copy in the hull mask bounds no field, and a copy on the old set reads
    /// images that are gone. Fixture cuboids stand in for the model: the order reads that a mesh
    /// is the round's, never its shape.
    #[test]
    fn the_xray_round_is_on_the_scene_side() {
        use bevy::camera::{ImageRenderTarget, RenderTarget};
        use bevy::render::render_resource::TextureFormat;

        use crate::render_policy::VisualScope;

        let (mut app, _, meshes, camera) = world_xray_app();
        app.register_type::<MeshMaterial3d<StandardMaterial>>();
        let world = app.world_mut();
        let cuboid = world
            .resource_mut::<Assets<Mesh>>()
            .add(Cuboid::new(0.1, 0.1, 0.3));
        let material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let mut model = World::new();
        let node = model.spawn(Transform::IDENTITY).id();
        let first = model
            .spawn((
                Mesh3d(cuboid.clone()),
                MeshMaterial3d(material.clone()),
                Transform::IDENTITY,
                ChildOf(node),
            ))
            .id();
        model.spawn((
            Mesh3d(cuboid.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_xyz(0.0, 0.0, 0.2),
            ChildOf(first),
        ));
        let model = world
            .resource_mut::<Assets<WorldAsset>>()
            .add(WorldAsset::new(model));
        world
            .resource_mut::<xray::XRayState>()
            .stand_in_shell(model);
        let (_, crossing, vocabulary, _) = incoming("s1_through_crew", 1200);
        receive(&mut app, crossing, &vocabulary);
        press_x(&mut app);
        run(&mut app, 0.2);
        let check = |app: &mut App, when: &str| {
            let tank_copy = {
                let world = app.world_mut();
                world
                    .query::<(&xray::MaskCopy, &MeshMaterial3d<LookMaterial>)>()
                    .iter(world)
                    .find(|(copy, _)| copy.of() == meshes[0])
                    .map(|(_, material)| material.0.clone())
                    .expect("the hull mesh's mask copy")
            };
            let world = app.world_mut();
            type Own = Option<&'static MeshMaterial3d<StandardMaterial>>;
            let round: Vec<(Entity, Option<AssetId<StandardMaterial>>)> = world
                .query_filtered::<(Entity, &Mesh3d, Own), Without<MeshMaterial3d<LookMaterial>>>()
                .iter(world)
                .filter(|(_, mesh, _)| mesh.0 == cuboid)
                .map(|(entity, _, own)| (entity, own.map(|m| m.id())))
                .collect();
            assert_eq!(round.len(), 2, "{when}: the model landed");
            let copies: Vec<(Entity, VisualScope, Handle<LookMaterial>)> = world
                .query::<(
                    &Mesh3d,
                    &ChildOf,
                    &VisualScope,
                    &MeshMaterial3d<LookMaterial>,
                )>()
                .iter(world)
                .filter(|(mesh, ..)| mesh.0 == cuboid)
                .map(|(_, parent, scope, material)| (parent.parent(), *scope, material.0.clone()))
                .collect();
            assert_eq!(copies.len(), round.len(), "{when}: one copy a mesh");
            for (mesh, own) in round {
                assert_eq!(own, Some(material.id()), "{when}: its own material");
                let mine: Vec<_> = copies.iter().filter(|copy| copy.0 == mesh).collect();
                assert_eq!(mine.len(), 1, "{when}");
                assert_eq!(mine[0].1, VisualScope::PEN_CAM_PART_MASK, "{when}");
                assert_eq!(mine[0].2, tank_copy, "{when}: the tank's copies' material");
            }
            let looks = world.resource::<Assets<LookMaterial>>();
            let set = world
                .resource::<Masks>()
                .of(camera)
                .cloned()
                .expect("the camera's set");
            assert_eq!(
                looks.get(&tank_copy).unwrap().params.mode,
                look::hull_mask(&set).params.mode
            );
            tank_copy
        };
        let old = check(&mut app, "built");

        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::new_target_texture(
                128,
                64,
                TextureFormat::Rgba8UnormSrgb,
                None,
            ));
        app.world_mut()
            .entity_mut(camera)
            .insert(RenderTarget::Image(ImageRenderTarget {
                handle: image,
                scale_factor: 1.0,
            }));
        run(&mut app, 0.2);
        let new = check(&mut app, "a new set");
        assert_ne!(new, old, "the new set's material");
    }

    /// A glTF primitive landing under a dressed tank is dressed once the LOD ladder has scanned
    /// it, and not before: the ladder binds the primitive's rungs off the handle the look takes.
    #[test]
    fn a_late_primitive_is_dressed_once_the_ladder_scanned_it() {
        let (mut app, player, meshes, _) = world_xray_app();
        press_x(&mut app);
        run(&mut app, 0.2);
        assert!(wears_the_look(&mut app, meshes[0]));
        let standard = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let hull = app.world().get::<crate::tank::Rig>(player).unwrap().hull;
        let late = app
            .world_mut()
            .spawn((
                bevy::gltf::GltfMeshName("Late".into()),
                Mesh3d(Handle::default()),
                MeshMaterial3d(standard),
                Transform::IDENTITY,
                Visibility::default(),
                ChildOf(hull),
            ))
            .id();
        run(&mut app, 0.2);
        assert!(
            !wears_the_look(&mut app, late),
            "left for the ladder's scan"
        );
        assert_eq!(xray_census(&mut app).0, 2);
        app.world_mut()
            .entity_mut(late)
            .insert(crate::geometry_lod::GeometryLodScanned);
        run(&mut app, 0.2);
        assert!(wears_the_look(&mut app, late), "dressed once scanned");
        assert_eq!(xray_census(&mut app).0, 3);
    }

    /// `name`'s record of another tank's round fired on `fire_tick` striking the player's first
    /// life, its encounter opened on `contact_tick` and completed on `completion_tick`: the
    /// player's projection and the vocabulary. The fire tick is the explanation's identity.
    fn incoming_at(
        name: &str,
        fire_tick: u32,
        contact_tick: u32,
        completion_tick: u32,
    ) -> (ShotExplanation, ShotVocabulary) {
        let report = report(name);
        let vocabulary = vocabulary(&report);
        let victim = projected(
            &report,
            &vocabulary,
            fire_tick,
            contact_tick,
            completion_tick,
            me(),
        );
        (victim, vocabulary)
    }

    /// [`incoming_at`] over a loaded `report` and its `vocabulary`, for a round striking `life`,
    /// which may be another tank's: the struck tank's projection.
    fn projected(
        report: &PenetrationReport,
        vocabulary: &ShotVocabulary,
        fire_tick: u32,
        contact_tick: u32,
        completion_tick: u32,
        life: TankLife,
    ) -> ShotExplanation {
        let mut recorded = recorded(report, STRUCK, fire_tick, life);
        recorded.contact_tick = contact_tick;
        recorded.completion_tick = completion_tick;
        recorded.outcome = Some(0);
        let built = BuiltExplanation::build(vocabulary, &recorded).unwrap();
        let victim = built
            .projections()
            .into_iter()
            .map(|(_, e)| e)
            .next()
            .unwrap();
        assert_ne!(victim.audience, Audience::Shooter);
        victim
    }

    /// The latest record is the newest by recency entire: of two encounters opened on the same
    /// tick the later completed is newer whatever their identities' order; of two completed
    /// together the greater identity; one still waiting on its impact is never the latest,
    /// however new.
    #[test]
    fn the_latest_record_is_the_newest_completed_whose_impact_is_drawn() {
        let (mut app, _, _, _) = world_xray_app();
        let latest = |app: &App| {
            xray::latest(app.world().resource::<ShotExplanations>(), me()).map(|(key, _)| key)
        };
        // The earlier completed has the greater identity: completion is read before it.
        let (early, early_vocabulary) = incoming_at("s1_through_crew", 1299, 1300, 1302);
        let (late, late_vocabulary) = incoming_at("s3_ricochet", 1298, 1300, 1305);
        assert!(ExplanationKey::of(&early) > ExplanationKey::of(&late));
        receive(&mut app, late.clone(), &late_vocabulary);
        receive(&mut app, early, &early_vocabulary);
        assert_eq!(
            latest(&app),
            Some(ExplanationKey::of(&late)),
            "the later completed of one contact tick"
        );
        let (tie_low, low_vocabulary) = incoming_at("s1_through_crew", 1310, 1320, 1322);
        let (tie_high, high_vocabulary) = incoming_at("s3_ricochet", 1311, 1320, 1322);
        receive(&mut app, tie_high.clone(), &high_vocabulary);
        receive(&mut app, tie_low, &low_vocabulary);
        assert_eq!(
            latest(&app),
            Some(ExplanationKey::of(&tie_high)),
            "completed together: the greater identity"
        );
        let (pending, pending_vocabulary) = incoming_at("s9_breech_and_loader", 1399, 1400, 1402);
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        app.world_mut().resource_mut::<ShotExplanations>().receive(
            pending.clone(),
            &pending_vocabulary,
            now,
        );
        assert_eq!(
            latest(&app),
            Some(ExplanationKey::of(&tie_high)),
            "a newer record still waiting on its impact is not the latest"
        );
        app.world_mut()
            .resource_mut::<ShotExplanations>()
            .drew(pending.shot, pending.outcome.unwrap());
        assert_eq!(
            latest(&app),
            Some(ExplanationKey::of(&pending)),
            "drawn, it is"
        );
    }

    /// A shoe's mask copy draws the rung the belt writes in the same frame: the copy follows the
    /// source's handle after the belt's write in `PostUpdate`, so a frame never renders the source
    /// on one rung and its mask on another. A swapper standing in for the belt is registered after
    /// the follower, on one thread, so only the ordering edge puts it first.
    #[test]
    fn a_shoes_mask_copy_takes_the_rung_the_belt_writes_in_the_frame() {
        #[derive(Resource)]
        struct Swap(Entity, Handle<Mesh>);

        fn swap(swap: Res<Swap>, mut meshes: Query<&mut Mesh3d>) {
            if let Ok(mut mesh) = meshes.get_mut(swap.0) {
                mesh.set_if_neq(Mesh3d(swap.1.clone()));
            }
        }

        let (mut app, _, meshes, _) = world_xray_app();
        press_x(&mut app);
        run(&mut app, 0.2);
        let source = meshes[0];
        let copy = {
            let world = app.world_mut();
            let mut query = world.query::<(Entity, &xray::MaskCopy)>();
            query
                .iter(world)
                .find(|(_, copy)| copy.of() == source)
                .map(|(entity, _)| entity)
                .expect("the source's mask copy")
        };
        let rung = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            ));
        app.insert_resource(Swap(source, rung.clone()));
        app.add_systems(
            PostUpdate,
            swap.in_set(crate::track::link_view::ShoeRungSelected),
        );
        app.edit_schedule(PostUpdate, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        step(&mut app);
        assert_eq!(app.world().get::<Mesh3d>(source).unwrap().0, rung);
        assert_eq!(
            app.world().get::<Mesh3d>(copy).unwrap().0,
            rung,
            "the copy took the rung in the frame it was written"
        );
    }

    /// Recency itself, read off the explanations: the encounter's first contact first, then its
    /// completion, then the explanation's identity — each only where the one before ties.
    #[test]
    fn recency_reads_contact_then_completion_then_identity() {
        let recency = |explanation: &ShotExplanation| {
            Recency::of(ExplanationKey::of(explanation), explanation)
        };
        let (base, _) = incoming_at("s1_through_crew", 1299, 1300, 1302);
        let (later_contact, _) = incoming_at("s3_ricochet", 1298, 1301, 1301);
        let (later_completion, _) = incoming_at("s3_ricochet", 1298, 1300, 1303);
        let (greater_identity, _) = incoming_at("s3_ricochet", 1300, 1300, 1302);
        assert!(
            recency(&later_contact) > recency(&base),
            "a later contact is newer, whatever completed first"
        );
        assert!(
            recency(&later_completion) > recency(&base),
            "of one contact tick, the later completed, whatever the identities"
        );
        assert!(ExplanationKey::of(&later_completion) < ExplanationKey::of(&base));
        assert!(
            recency(&greater_identity) > recency(&base),
            "completed together: the greater identity"
        );
        assert_eq!(recency(&base), recency(&base));
    }
}
