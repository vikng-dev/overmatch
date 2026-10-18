//! The corner slot in play, and its two surfaces: the shooter's corner, each explanation of the
//! local player's own hit on another tank in the frame for its verdict ([`CornerKind::of`]), and
//! the victim glass, the player's own tank as it stands ([`glass`]). One owner, one slot: never two
//! surfaces at once.
//!
//! * **What opens the corner.** An explanation in the player's store ([`ShotExplanations`]) whose
//!   audience is [`Audience::Shooter`], fired by the player's combatant at another, admitted by
//!   the player's corner setting ([`PenCamCorner::opens`]).
//! * **What peeks the glass.** A victim projection of a shot on the player's current life, from
//!   any shooter, that tells something ([`glass::tells`]: a plate crossed, HP points lost, a
//!   consequence), admitted by [`VictimGlassPeek`]. It runs [`CornerKind::Victim`]'s turn.
//! * **When.** Either opens once its world impact is drawn ([`ImpactPresence::Presented`]); one
//!   released undrawn ([`ImpactPresence::TimedOut`]) is skipped, as the slot rides on the impact
//!   the player saw. Each explanation is considered once. It opens [`timeline::OPEN_LATENCY_S`]
//!   later by the replacement rule ([`Lineup`]): the newest event replaces the one on show, of
//!   either surface, once that one has shown [`timeline::MIN_SHOWN_S`]; one arriving sooner waits,
//!   and only the latest waiting is kept. A replacement keeps the frame open and crossfades
//!   ([`timeline::turn_presence`]); a replaced turn never resumes. The timings are the UX spec's,
//!   TUNED.
//! * **Which is newer.** The encounter's first contact, then its record's completion, identity
//!   breaking ties ([`Recency`]); never arrival, as the store's channel is unordered. One older
//!   than any the slot has admitted since it last held nothing, or than any admitted one still
//!   waiting on its impact, is dropped; of several presented together only the newest opens.
//! * **The hold.** While [`GlassHold`] holds (the scripted hold, `OVERMATCH_GLASS_HOLD`; its key
//!   is gone) the glass holds the slot, with or without a record and whatever the settings: it
//!   crossfades over a turn on show, which never resumes, and the slot admits nothing presented
//!   while it holds. Released, it closes over [`timeline::CLOSE_S`], and what is presented from the
//!   release on queues by the rule: a turn that starts before the close ends crossfades over the
//!   closing glass, one due later opens after it. Held again before the close ends, it holds on
//!   and drops what queued. A teardown other than its own close is final for the hold: it opens the
//!   glass again only once let go. TUNED.
//! * **What it draws.** The corner: the struck tank's stage view, of the tank file the record
//!   names, under the record's plan over the same tank asset the demo stages ([`TankAsset`]); a
//!   component the shooter's projection does not list is the base layout, untinted. The glass: the
//!   player's own tank file ([`TankSpecHandle`]) under [`glass::plan`], rebuilt whenever the latest
//!   record or the tank's live state changes.
//! * **Teardown.** The slot's entities exist from an arrival or a hold until the slot closes with
//!   nothing queued or held, a new life of the player's tank, its knockout or no tank, a change of
//!   either setting, or a [`CloseCorner`] (the network client's disconnect). Every one goes at
//!   once ([`Rig::teardown`]): the cameras, lights, stage view, UI and the images they own.
//!
//! No input reaches the corner. While the player's tank is knocked out the slot holds nothing and
//! its rig is the kill cam's ([`killcam`]).

mod glass;
mod killcam;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;

#[cfg(feature = "dev_tools")]
pub(crate) use self::glass::scripted_glass_plugin;
pub(crate) use self::glass::{GlassHold, OwnGlass};
use self::glass::{LiveState, LiveVolume};
pub(crate) use self::killcam::{GameDeathScreen, KillCamGate};
use super::hud::{self, CornerImages, Glyphs};
use super::look;
use super::plan::Plan;
use super::stage::{self, Stage};
use super::timeline::{self, CornerKind, Lineup};
use super::view::{self, StageView};
use super::{Corner, CornerRig, Exhibit, Exhibits, Knobs, Showing, Surface};
use crate::bake::TankBlueprint;
use crate::damage::{TankKnockedOut, TankVolumes};
use crate::damage_cam::PenetrationReport;
use crate::damage_cam::delivery::{ExplanationKey, ImpactPresence, ShotExplanations};
use crate::damage_cam::shapes::TankAsset;
use crate::damage_cam::wire::{Audience, ShotExplanation, TankLife};
use crate::settings::{PenCamCorner, Settings, VictimGlassPeek};
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

/// The slot's state and systems, without the materials (a headless test registers the materials'
/// assets alone and sets [`GlassHold`] itself).
fn wire(app: &mut App) {
    super::presentation(app);
    app.init_resource::<Slot>()
        .init_resource::<GlassHold>()
        .init_resource::<OwnGlass>()
        .init_resource::<glass::Glass>()
        .add_observer(close)
        .add_systems(
            Update,
            (
                glass::toggle,
                drive,
                killcam::drive.in_set(killcam::KillCamGateSet),
                killcam::give_way
                    .after(crate::state::PlayerInputSet)
                    .after(crate::overlay::OverlaySet::Toggle),
                // The glass reads this frame's kill cam run, so after its drive; it states its
                // mask need before the masks are owned.
                glass::drive,
                (view::prepare_stage, view::pose_stage, hud::layout)
                    .chain()
                    .run_if(resource_exists::<StageView>),
                look::own,
                super::present().run_if(resource_exists::<StageView>),
                glass::build,
                glass::animate,
                glass::follow_mesh,
            )
                .chain(),
        );
    killcam::wire(app);
}

/// What one turn shows.
#[derive(Clone)]
enum Content {
    /// The shooter's corner: one explanation's exhibit.
    Corner(Exhibit),
    /// The victim glass, built from the latest record and the live state while it shows.
    Glass,
}

/// One event queued for the slot.
#[derive(Clone)]
struct Queued {
    serial: usize,
    kind: CornerKind,
    vehicle: String,
    content: Content,
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

/// The own-tank key's hold of the glass.
#[derive(Clone, Copy)]
struct Hold {
    serial: usize,
    /// When it first showed, on the slot's clock; `None` while its stage view is not placed.
    start: Option<f32>,
    /// Whether it crossfaded over a turn on show.
    replacing: bool,
    /// When the key let go: it closes [`timeline::CLOSE_S`] later.
    released: Option<f32>,
}

/// What the glass's exhibit is built from; a change rebuilds it.
#[derive(Clone, PartialEq)]
struct GlassSource {
    turn: usize,
    record: Option<ExplanationKey>,
    live: LiveState,
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
    /// The settings it admits by.
    admits: Option<(PenCamCorner, VictimGlassPeek)>,
    /// Every stored explanation already considered.
    seen: BTreeSet<ExplanationKey>,
    lineup: Lineup<Queued>,
    /// The newest explanation admitted to the lineup since the slot last held nothing.
    newest: Option<Recency>,
    serials: usize,
    turn: Option<Turn>,
    hold: Option<Hold>,
    /// Whether the key held down through the slot's last forced close: that press opens nothing
    /// more, so a respawn, a knockout or a disconnect closes the glass for good.
    spent: bool,
    /// What the glass on show was built from.
    glass: Option<GlassSource>,
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

    /// Drop every queued, shown and held turn and the slot's clock.
    fn reset(&mut self) {
        self.lineup.clear();
        self.newest = None;
        self.turn = None;
        self.hold = None;
        self.glass = None;
        self.epoch = None;
    }

    /// The released `held` gives the slot to the lineup at `now`, its glass standing as the turn on
    /// show: a turn starting while the glass still draws crossfades over it.
    fn hand_over(&mut self, held: Hold, now: f32) {
        self.hold = None;
        self.turn = Some(Turn {
            serial: held.serial,
            kind: CornerKind::Victim,
            start: now,
            replacing: held.replacing,
        });
    }

    /// [`Self::reset`], and consider none of `store`'s explanations again, those still waiting on
    /// their impact included.
    fn forget(&mut self, store: Option<&ShotExplanations>) {
        self.reset();
        self.spent = true;
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

    /// The glass's exhibit for `turn`, of `vehicle` under `record` over `live`, rebuilt only when
    /// one of them changed.
    fn glass(
        &mut self,
        state: &mut Slot,
        turn: usize,
        record: Option<(ExplanationKey, &PenetrationReport)>,
        live: LiveState,
        vehicle: &str,
        blueprint: Option<&TankBlueprint>,
    ) {
        let source = GlassSource {
            turn,
            record: record.map(|(key, _)| key),
            live,
        };
        if state.glass.as_ref() == Some(&source) {
            return;
        }
        let Some(tank) = state.tank(vehicle, blueprint) else {
            self.exhibits.0.clear();
            return;
        };
        let report = record.map(|(_, report)| report);
        let servo_deg = report
            .map(|report| report.servo_deg.clone())
            .unwrap_or_default();
        let plan = glass::plan(report, &source.live, &tank.shapes(&servo_deg));
        self.exhibits.0 = vec![Exhibit {
            plan: Arc::new(plan),
            servo_deg,
        }];
        if state.glass.as_ref().is_some_and(|shown| shown.turn == turn) {
            // The same turn keeps the same stage key: only this rebuilds it.
            self.stage.invalidate();
        }
        state.glass = Some(source);
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

/// Queue each newly presented explanation the slot admits, hold the glass while the key is down,
/// run the replacement rule on the slot's clock, keep the rig for what is on show and write the
/// corner.
#[expect(
    clippy::too_many_arguments,
    reason = "the slot's clock, store, settings, blueprint and hold key are separate inputs; `Player` and `Rig` already bundle what belongs together"
)]
fn drive(
    time: Res<Time>,
    mut state: ResMut<Slot>,
    store: Option<Res<ShotExplanations>>,
    settings: Option<Res<Settings>>,
    blueprint: Option<Res<TankBlueprint>>,
    hold: Res<GlassHold>,
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
    let admits = (settings.pen_cam_corner, settings.victim_glass_peek);
    if life != state.life || Some(admits) != state.admits {
        if rig.exists(&state) {
            rig.teardown(&mut state);
        }
        state.forget(store);
        state.life = life;
        state.admits = Some(admits);
    }
    if !hold.0 {
        state.spent = false;
    }
    let Some(me) = state.life else {
        return;
    };
    let held_down = hold.0 && !state.spent;
    let latest = store.and_then(|store| glass::latest(store, me));
    let own = player
        .vehicle()
        .or_else(|| latest.and_then(|(_, stored)| stored.report.vehicle.clone()))
        .or_else(|| blueprint.map(|blueprint| blueprint.file.clone()));
    if let Some(own) = &own {
        state.preload(&rig.asset_server, own);
    }

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
            let candidate = if fired {
                entry.report.vehicle.clone().map(|vehicle| {
                    let admitted = settings.pen_cam_corner.opens(entry.report.impact.verdict);
                    (vehicle, admitted, false)
                })
            } else if glass::struck(entry, me) {
                own.clone().map(|vehicle| {
                    let admitted = settings.victim_glass_peek.on() && glass::tells(&entry.report);
                    (vehicle, admitted, true)
                })
            } else {
                None
            };
            let Some((vehicle, admitted, peek)) = candidate else {
                state.seen.insert(*key);
                continue;
            };
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
                ImpactPresence::Presented if admitted && !held_down => {
                    presented.push((recency, entry, vehicle, peek));
                }
                ImpactPresence::Presented => {}
            }
            state.seen.insert(*key);
        }
    }
    // Newest first: of explanations presented together only the newest opens.
    presented.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (recency, entry, vehicle, peek) in presented {
        if floor.is_some_and(|floor| recency < floor) {
            continue;
        }
        let (kind, content) = if peek {
            (CornerKind::Victim, Content::Glass)
        } else {
            let Some(tank) = state.tank(&vehicle, blueprint) else {
                continue;
            };
            let plan = Plan::new(&entry.report, &tank.shapes(&entry.report.servo_deg));
            let exhibit = Exhibit {
                servo_deg: entry.report.servo_deg.clone(),
                plan: Arc::new(plan),
            };
            (exhibit.plan.kind, Content::Corner(exhibit))
        };
        let now = state.clock(&time);
        state.serials += 1;
        state.newest = Some(recency);
        floor = Some(recency);
        let queued = Queued {
            serial: state.serials,
            kind,
            vehicle,
            content,
        };
        state.lineup.arrive(queued, now + timeline::OPEN_LATENCY_S);
    }

    let record = latest.map(|(key, stored)| (key, &stored.report));
    if held_down || state.hold.is_some() {
        let now = state.clock(&time);
        let slot = &mut *state;
        match &mut slot.hold {
            Some(held) if held_down => {
                if held.released.take().is_some() {
                    slot.lineup.clear();
                }
            }
            Some(held) => {
                held.released.get_or_insert(now);
            }
            None => {
                state.lineup.clear();
                state.turn = None;
                state.serials += 1;
                state.hold = Some(Hold {
                    serial: state.serials,
                    start: None,
                    replacing: false,
                    released: None,
                });
            }
        }
        let Some(held) = state.hold else {
            return;
        };
        if let Some(released) = held.released {
            state.lineup.advance(now, |_, _| {});
            let closed = held.start.is_none() || now - released >= timeline::CLOSE_S;
            if state.lineup.current().is_some() || closed && state.lineup.pending() {
                state.hand_over(held, now);
            } else if closed {
                rig.teardown(&mut state);
                state.reset();
                return;
            }
        }
    }
    if let Some(mut held) = state.hold {
        let now = state.clock(&time);
        let Some(vehicle) = own.as_deref() else {
            warn_once!("pen cam: the glass has no tank file for the player's tank");
            rig.corner.set_if_neq(Corner::default());
            return;
        };
        if !rig.ready(&mut state, vehicle) {
            // The stage cannot draw yet: the hold waits for it.
            rig.corner.set_if_neq(Corner::default());
            return;
        }
        let start = match held.start {
            Some(start) => start,
            None => {
                held.replacing = rig.corner.showing.is_some();
                held.start = Some(now);
                now
            }
        };
        state.hold = Some(held);
        rig.glass(
            &mut state,
            held.serial,
            record,
            player.live(),
            vehicle,
            blueprint,
        );
        let c = now - start;
        let (mut opacity, scale, fading) =
            timeline::turn_presence(c, f32::INFINITY, held.replacing);
        if let Some(released) = held.released {
            opacity *=
                1.0 - timeline::ease_exit(timeline::progress(now, released, timeline::CLOSE_S));
        }
        rig.corner.set_if_neq(Corner {
            showing: Some(Showing {
                record: 0,
                turn: held.serial,
                kind: CornerKind::Victim,
                c: c.min(CornerKind::Victim.hold_from()),
                opacity,
                scale,
                remaining: 1.0,
                surface: Surface::Corner,
            }),
            fading,
        });
        return;
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
        if let Content::Corner(exhibit) = &queued.content {
            rig.exhibits.0 = vec![exhibit.clone()];
        }
        debug!(
            "pen cam: the slot shows {:?} at {:.2} s",
            turn.kind,
            time.elapsed_secs_f64()
        );
    }
    state.turn = Some(turn);
    if let Content::Glass = queued.content {
        rig.glass(
            &mut state,
            turn.serial,
            record,
            player.live(),
            &queued.vehicle,
            blueprint,
        );
    }
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

/// Dev-only, offline: `OVERMATCH_GLASS_HOLD=<from>-<to>` holds the victim glass from `from` to `to`
/// app seconds (`Time` as `Update` sees it), for captures of the hold. Mounted only by the offline
/// composition.
#[cfg(feature = "dev_tools")]
pub(crate) fn scripted_hold_plugin(app: &mut App) {
    let Ok(window) = std::env::var("OVERMATCH_GLASS_HOLD") else {
        return;
    };
    let Some((from, to)) = window.split_once('-').and_then(|(from, to)| {
        Some((
            from.trim().parse::<f32>().ok()?,
            to.trim().parse::<f32>().ok()?,
        ))
    }) else {
        warn!("OVERMATCH_GLASS_HOLD={window}: expected <from>-<to> seconds");
        return;
    };
    info!("pen cam: the victim glass is held from {from} s to {to} s (OVERMATCH_GLASS_HOLD)");
    app.add_systems(
        Update,
        (move |time: Res<Time>, mut hold: ResMut<GlassHold>| {
            hold.set_if_neq(GlassHold((from..to).contains(&time.elapsed_secs())));
        })
        .before(drive),
    );
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
        ));
        mount(&mut app);
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

    /// A victim's projection never opens the shooter's corner: the struck player's peeks the
    /// glass, and one of another tank's life opens nothing.
    #[test]
    fn the_victims_projection_never_opens_the_corner() {
        for (combatant, expected) in [(STRUCK, &[CornerKind::Victim][..]), (SHOOTER, &[][..])] {
            let (mut app, player) = corner_app(Settings::default());
            app.world_mut().entity_mut(player).insert(combatant);
            app.update();
            let (_, victim, vocabulary, report) = explained("s1_through_crew", 1200);
            stage_ready(&mut app, &report);
            receive(&mut app, victim, &vocabulary);
            let kinds: Vec<CornerKind> = run(&mut app, 4.0).into_iter().map(|(_, k)| k).collect();
            assert_eq!(kinds, expected, "{combatant:?}");
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
    /// component of the fixture `s9_one_box_lost`'s tank at its full pool.
    fn glass_app(settings: Settings) -> (App, Entity) {
        let (mut app, player) = corner_app(settings);
        let spec = app.world().resource::<AssetServer>().load::<TankSpec>(OWN);
        app.world_mut()
            .entity_mut(player)
            .insert(TankSpecHandle(spec));
        for component in report("s9_one_box_lost").components {
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

    fn hold(app: &mut App, held: bool) {
        app.world_mut().resource_mut::<GlassHold>().0 = held;
    }

    fn showing(app: &App) -> Option<Showing> {
        app.world().resource::<Corner>().showing
    }

    /// The exhibit on show's plan.
    fn exhibit(app: &App) -> Arc<Plan> {
        app.world().resource::<Exhibits>().0[0].plan.clone()
    }

    /// `run`'s frames, without the turns.
    fn kinds(app: &mut App, seconds: f32) -> Vec<CornerKind> {
        run(app, seconds)
            .into_iter()
            .map(|(_, kind)| kind)
            .collect()
    }

    fn me() -> TankLife {
        TankLife {
            combatant: SHOOTER,
            life: CombatantLife(0),
        }
    }

    /// The glass on show tints by the tank's live state as it changes, in the same turn.
    #[test]
    fn the_glass_tints_by_the_live_state_as_it_changes() {
        let (mut app, _) = glass_app(Settings::default());
        stage_of(&mut app, OWN);
        hold(&mut app, true);
        assert_eq!(kinds(&mut app, 0.5), [CornerKind::Victim]);
        let pools = |app: &App| -> Vec<(String, u32, u32, bool)> {
            exhibit(app)
                .boxes
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
        let turn = showing(&app).unwrap().turn;
        let world = app.world_mut();
        let mut volumes = world.query::<&mut crate::ballistics::ComponentHealth>();
        let mut health = volumes
            .iter_mut(world)
            .find(|health| health.max > 1)
            .expect("fixture data: a component with a pool to lose points of");
        health.current -= 1;
        step(&mut app);
        let after = pools(&app);
        assert_ne!(before, after, "the live loss reaches the glass");
        assert_eq!(showing(&app).unwrap().turn, turn, "in the same turn");
    }

    /// The glass peeks for its turn only for a record that tells something, once its impact is
    /// drawn; a clean ricochet and a record released undrawn never peek.
    #[test]
    fn the_peek_opens_only_for_a_telling_record_once_presented() {
        let (mut app, _) = glass_app(Settings::default());
        stage_of(&mut app, OWN);
        let (_, ricochet, ricochet_vocabulary, _) = incoming("s3_ricochet", 1200);
        receive(&mut app, ricochet, &ricochet_vocabulary);
        assert!(
            app.world()
                .resource::<ShotExplanations>()
                .iter()
                .all(|(_, stored)| !glass::tells(&stored.report)),
            "fixture data: the ricochet tells nothing"
        );
        assert!(kinds(&mut app, 4.0).is_empty(), "a clean ricochet");

        let (_, crossing, vocabulary, _) = incoming("s1_through_crew", 1300);
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        app.world_mut().resource_mut::<ShotExplanations>().receive(
            crossing.clone(),
            &vocabulary,
            now,
        );
        assert!(
            kinds(&mut app, 2.0).is_empty(),
            "the impact is still to draw"
        );
        app.world_mut()
            .resource_mut::<ShotExplanations>()
            .drew(crossing.shot, crossing.outcome.unwrap());
        let mut frames = 0;
        for _ in 0..(4.0 / STEP_S) as usize {
            step(&mut app);
            if let Some(showing) = showing(&app) {
                assert_eq!(showing.kind, CornerKind::Victim);
                frames += 1;
            }
        }
        let shown_s = frames as f32 * STEP_S;
        assert!(
            (shown_s - CornerKind::Victim.duration()).abs() <= 2.0 * STEP_S,
            "the peek shows {shown_s} s"
        );
        assert_eq!(census(&mut app).0, 0, "and closes");

        let (_, lost, lost_vocabulary, _) = incoming("s9_one_box_lost", 1400);
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        let mut store = app.world_mut().resource_mut::<ShotExplanations>();
        store.receive(lost, &lost_vocabulary, now);
        store.expire(now + crate::damage_cam::delivery::IMPACT_TIMEOUT_S);
        assert!(kinds(&mut app, 4.0).is_empty(), "released undrawn");
    }

    /// Of the player's records the glass draws strictly the newest by recency, whatever it tells:
    /// a later clean ricochet replaces an earlier penetration, whichever arrives first, and the
    /// glass then draws the bounce and no crossing.
    #[test]
    fn a_later_ricochet_replaces_an_earlier_penetration() {
        let (mut app, _, _, _) = world_glass_app();
        let (_, crossing, vocabulary, _) = incoming("s1_through_crew", 1200);
        let (_, ricochet, ricochet_vocabulary, _) = incoming("s3_ricochet", 1300);
        assert!(ricochet.contact_tick > crossing.contact_tick);
        let latest = |app: &App| {
            glass::latest(app.world().resource::<ShotExplanations>(), me()).map(|(key, _)| key)
        };
        press_x(&mut app);
        receive(&mut app, crossing.clone(), &vocabulary);
        assert_eq!(latest(&app), Some(ExplanationKey::of(&crossing)));
        run(&mut app, 0.2);
        let plan = |app: &App| {
            let glass = app.world().resource::<glass::Glass>();
            let plan = glass.plan().expect("the glass is built");
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

        let (mut app, _, _, _) = world_glass_app();
        receive(&mut app, ricochet.clone(), &ricochet_vocabulary);
        receive(&mut app, crossing, &vocabulary);
        assert_eq!(
            latest(&app),
            Some(ExplanationKey::of(&ricochet)),
            "arrival never orders"
        );
    }

    /// The hold opens the glass with no record: the base look, no path, held open for as long as
    /// the key is down; let go, it closes and leaves nothing.
    #[test]
    fn the_hold_opens_the_glass_with_no_record() {
        let (mut app, _) = glass_app(Settings::default());
        let (_, _, entities) = census(&mut app);
        stage_of(&mut app, OWN);
        hold(&mut app, true);
        assert_eq!(kinds(&mut app, 10.0), [CornerKind::Victim]);
        let now = showing(&app).unwrap();
        assert_eq!(now.opacity, 1.0);
        assert_eq!(now.remaining, 1.0);
        assert!(now.c <= CornerKind::Victim.hold_from());
        let plan = exhibit(&app);
        assert!(plan.line.points.len() < 2, "no path");
        assert!(plan.cones.is_empty());
        hold(&mut app, false);
        run(&mut app, timeline::CLOSE_S + 3.0 * STEP_S);
        assert!(showing(&app).is_none());
        assert_eq!(census(&mut app), (0, 0, entities));
        assert!(default_image_kept(&app));
    }

    /// The hold takes the slot from a corner on show by the crossfade; a peek presented while it
    /// holds never takes it, and the corner never resumes once the hold lets go.
    #[test]
    fn the_hold_beats_a_showing_corner() {
        let (mut app, _) = glass_app(Settings::default());
        stage_of(&mut app, OWN);
        let (shooter, _, vocabulary, _) = explained("s1_through_crew", 1200);
        receive(&mut app, shooter, &vocabulary);
        let mut shown = kinds(&mut app, 1.0);
        assert_eq!(shown, [CornerKind::Glass]);
        hold(&mut app, true);
        step(&mut app);
        assert_eq!(showing(&app).unwrap().kind, CornerKind::Victim, "at once");
        assert!(
            app.world().resource::<Corner>().fading.is_some(),
            "crossfaded"
        );
        let turn = showing(&app).unwrap().turn;
        let (_, crossing, crossing_vocabulary, _) = incoming("s1_through_crew", 1300);
        receive(&mut app, crossing, &crossing_vocabulary);
        let held = run(&mut app, 2.0);
        assert!(
            held.iter().all(|&(t, _)| t == turn),
            "the hold keeps the slot"
        );
        hold(&mut app, false);
        shown.extend(kinds(&mut app, 6.0));
        shown.dedup();
        assert_eq!(shown, [CornerKind::Glass, CornerKind::Victim]);
        assert_eq!(census(&mut app).0, 0);
    }

    /// What is presented from the hold's release on, on the release frame or during the close,
    /// queues by the rule and shows once the glass has closed: the event itself, in its own turn,
    /// for its whole turn; with nothing presented the close ends empty.
    #[test]
    fn what_is_presented_from_the_release_on_shows_after_the_close() {
        let close_end = timeline::CLOSE_S + 2.0 * STEP_S;
        for delay in [None, Some(0.0), Some(timeline::CLOSE_S * 0.5)] {
            for peek in [false, true] {
                let case = (delay, peek);
                let (mut app, _) = glass_app(Settings::default());
                stage_of(&mut app, OWN);
                hold(&mut app, true);
                let held = run(&mut app, 1.0);
                assert_eq!(held.len(), 1, "{case:?}");
                let (hold_turn, _) = held[0];
                hold(&mut app, false);
                let Some(delay) = delay else {
                    run(&mut app, close_end);
                    assert_eq!(showing(&app), None, "the close ends empty");
                    let (rigs, insets, _) = census(&mut app);
                    assert_eq!((rigs, insets), (0, 0), "nothing left of the glass");
                    continue;
                };
                run(&mut app, delay);
                let (explanation, vocabulary, kind) = if peek {
                    let (_, victim, vocabulary, _) = incoming("s1_through_crew", 1300);
                    (victim, vocabulary, CornerKind::Victim)
                } else {
                    let (shooter, _, vocabulary, _) = explained("s1_through_crew", 1300);
                    (shooter, vocabulary, CornerKind::Glass)
                };
                receive(&mut app, explanation, &vocabulary);
                let mut after = Vec::new();
                for _ in 0..((close_end + timeline::OPEN_LATENCY_S) / STEP_S).ceil() as usize {
                    after.extend(run(&mut app, STEP_S));
                    if after.last().is_some_and(|&(turn, _)| turn != hold_turn) {
                        break;
                    }
                }
                let won = after.last().copied();
                assert!(
                    won.is_some_and(|(turn, shown)| turn != hold_turn && shown == kind),
                    "{case:?}: the event presented from the release on shows, got {after:?}"
                );
                if !peek {
                    assert_eq!(exhibit(&app).kind, kind, "{case:?}: its own exhibit");
                }
                let (turn, _) = won.unwrap();
                // Every stepped frame from its open to its end, two frames short of it, draws the
                // turn, visibly once it has begun to open.
                let first = showing(&app).map(|shown| shown.c);
                assert!(
                    first.is_some_and(|c| c < timeline::OPEN_S),
                    "{case:?}: caught at its open, got c = {first:?}"
                );
                for frame in 0.. {
                    let shown = showing(&app);
                    assert!(
                        shown.is_some_and(|shown| shown.turn == turn
                            && shown.kind == kind
                            && (shown.c <= 0.0 || shown.opacity > 0.0)),
                        "{case:?}: drawn on every frame of its turn, frame {frame}: {shown:?}"
                    );
                    if shown.is_some_and(|shown| shown.c >= kind.duration() - 2.0 * STEP_S) {
                        break;
                    }
                    assert!(
                        (frame as f32) * STEP_S < kind.duration(),
                        "{case:?}: c advances"
                    );
                    run(&mut app, STEP_S);
                }
                run(&mut app, 2.0);
                let (rigs, insets, _) = census(&mut app);
                assert_eq!((rigs, insets), (0, 0), "{case:?}: then the slot closes");
            }
        }
    }

    /// Between the corner and the peek the newer event takes the slot by the crossfade, either way
    /// round; an older one arriving later never shows.
    #[test]
    fn the_newer_event_takes_the_slot_between_peek_and_corner() {
        for (peek_tick, corner_tick, peek_first, expected) in [
            (
                1300,
                1200,
                false,
                &[CornerKind::Glass, CornerKind::Victim][..],
            ),
            (
                1200,
                1300,
                true,
                &[CornerKind::Victim, CornerKind::Glass][..],
            ),
            (1300, 1200, true, &[CornerKind::Victim][..]),
            (1200, 1300, false, &[CornerKind::Glass][..]),
        ] {
            let (mut app, _) = glass_app(Settings::default());
            stage_of(&mut app, OWN);
            let (shooter, _, shooter_vocabulary, _) = explained("s1_through_crew", corner_tick);
            let (_, victim, victim_vocabulary, _) = incoming("s1_through_crew", peek_tick);
            let mut arrivals = [(shooter, shooter_vocabulary), (victim, victim_vocabulary)];
            if peek_first {
                arrivals.reverse();
            }
            let mut shown = Vec::new();
            let mut faded = false;
            for (explanation, vocabulary) in arrivals {
                receive(&mut app, explanation, &vocabulary);
                for _ in 0..(1.0 / STEP_S) as usize {
                    shown.extend(run(&mut app, STEP_S));
                    faded |= app.world().resource::<Corner>().fading.is_some();
                }
            }
            shown.extend(run(&mut app, 6.0));
            shown.dedup();
            let kinds: Vec<CornerKind> = shown.iter().map(|(_, kind)| *kind).collect();
            let case = (peek_tick, corner_tick, peek_first);
            assert_eq!(kinds, expected, "{case:?}");
            assert_eq!(
                faded,
                expected.len() == 2,
                "{case:?}: the replacement crossfades"
            );
        }
    }

    /// The Shooter projection never opens the glass, whether the player fired it or was struck
    /// by it.
    #[test]
    fn the_shooter_projection_never_opens_the_glass() {
        for (combatant, corner) in [
            (SHOOTER, PenCamCorner::All),
            (SHOOTER, PenCamCorner::Off),
            (STRUCK, PenCamCorner::All),
        ] {
            let settings = Settings {
                pen_cam_corner: corner,
                ..Settings::default()
            };
            let (mut app, player) = glass_app(settings);
            app.world_mut().entity_mut(player).insert(combatant);
            app.update();
            stage_of(&mut app, OWN);
            let (shooter, _, vocabulary, _) = explained("s1_through_crew", 1200);
            receive(&mut app, shooter, &vocabulary);
            let shown = kinds(&mut app, 5.0);
            assert!(
                !shown.contains(&CornerKind::Victim),
                "{combatant:?}, {corner:?}: {shown:?}"
            );
        }
    }

    /// With the peek off a telling record peeks nothing, and the hold still opens the glass, as it
    /// does with the corner off.
    #[test]
    fn the_hold_works_with_the_peek_off() {
        for settings in [
            Settings {
                victim_glass_peek: VictimGlassPeek::Off,
                ..Settings::default()
            },
            Settings {
                pen_cam_corner: PenCamCorner::Off,
                victim_glass_peek: VictimGlassPeek::Off,
                ..Settings::default()
            },
        ] {
            let (mut app, _) = glass_app(settings);
            stage_of(&mut app, OWN);
            let (_, crossing, vocabulary, _) = incoming("s1_through_crew", 1200);
            receive(&mut app, crossing, &vocabulary);
            assert!(kinds(&mut app, 4.0).is_empty(), "no peek");
            hold(&mut app, true);
            assert_eq!(kinds(&mut app, 1.0), [CornerKind::Victim]);
            assert!(
                !exhibit(&app).crossings.is_empty(),
                "under the latest record"
            );
        }
    }

    /// The own tank's knockout, a new life, no tank and a disconnect each close the glass, held
    /// or peeking, and leave nothing of it; the press that held it opens nothing more.
    #[test]
    fn knockout_respawn_and_disconnect_close_the_glass_with_no_leaks() {
        for ending in 0..4 {
            for held in [true, false] {
                let (mut app, player) = glass_app(Settings::default());
                let (_, _, entities) = census(&mut app);
                let images = app.world().resource::<Assets<Image>>().len();
                stage_of(&mut app, OWN);
                if held {
                    hold(&mut app, true);
                } else {
                    let (_, crossing, vocabulary, _) = incoming("s1_through_crew", 1200);
                    receive(&mut app, crossing, &vocabulary);
                }
                assert_eq!(kinds(&mut app, 1.0), [CornerKind::Victim]);
                assert!(census(&mut app).0 > 0);
                let entities = match ending {
                    0 => {
                        app.world_mut().entity_mut(player).insert(TankKnockedOut {
                            reason: crate::damage::KnockoutReason::CrewLoss,
                        });
                        entities
                    }
                    1 => {
                        app.world_mut().entity_mut(player).insert(CombatantLife(1));
                        entities
                    }
                    2 => {
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
                let case = (ending, held);
                assert!(showing(&app).is_none(), "{case:?}");
                assert_eq!(census(&mut app), (0, 0, entities), "{case:?}");
                assert_eq!(app.world().resource::<Assets<Image>>().len(), images);
                assert!(!app.world().contains_resource::<StageView>());
                assert!(!app.world().contains_resource::<CornerImages>());
                assert!(!app.world().contains_resource::<Glyphs>());
                assert!(default_image_kept(&app));
                assert!(kinds(&mut app, 2.0).is_empty(), "{case:?}: nothing reopens");
            }
        }
    }

    // --- The own glass -------------------------------------------------------------------------

    /// [`glass_app`] with the player's tank as a live body — a rig whose hull and turret nodes
    /// carry one mesh each in a standard material, the turret's mesh node named after the
    /// fixture's first component — the player's camera drawing an image, and the keys. The app,
    /// the tank, its hull and turret meshes, and the camera.
    fn world_glass_app() -> (App, Entity, [Entity; 2], Entity) {
        use bevy::camera::{ImageRenderTarget, RenderTarget};
        use bevy::render::render_resource::TextureFormat;

        let (mut app, player) = glass_app(Settings {
            victim_glass_peek: VictimGlassPeek::Off,
            ..Settings::default()
        });
        app.insert_resource(Bindings::default())
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>();
        let component = report("s9_one_box_lost").components[0].name.clone();
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

    /// Press the own-glass key for one frame.
    fn press_x(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyX);
        step(app);
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::KeyX);
        keys.clear();
    }

    /// The glass's footprint: dressed meshes, its items, mask-set viewers, images and look
    /// materials stored, and every entity but the resources'.
    fn glass_census(app: &mut App) -> (usize, usize, Vec<Entity>, usize, usize, usize) {
        let viewers = app.world().resource::<Masks>().viewers();
        let images = app.world().resource::<Assets<Image>>().len();
        let looks = app.world().resource::<Assets<LookMaterial>>().len();
        let world = app.world_mut();
        let dressed = world
            .query_filtered::<(), With<glass::Dressed>>()
            .iter(world)
            .count();
        let items = world
            .query_filtered::<(), With<glass::GlassItem>>()
            .iter(world)
            .count();
        let all = world
            .query_filtered::<(), Without<bevy::ecs::resource::IsResource>>()
            .iter(world)
            .count();
        (dressed, items, viewers, images, looks, all)
    }

    /// Whether `mesh` wears the look: its look material on, its standard material off, and the
    /// glass's mask copy under it.
    fn wears_the_look(app: &mut App, mesh: Entity) -> bool {
        let copies: Vec<Entity> = {
            let world = app.world_mut();
            let mut query = world.query::<(&glass::MaskCopy, &ChildOf)>();
            query
                .iter(world)
                .filter(|(copy, parent)| copy.of() == mesh && parent.parent() == mesh)
                .map(|(_, parent)| parent.parent())
                .collect()
        };
        let entity = app.world().entity(mesh);
        entity.contains::<glass::Dressed>()
            && entity.contains::<MeshMaterial3d<LookMaterial>>()
            && !entity.contains::<MeshMaterial3d<StandardMaterial>>()
            && copies.len() == 1
    }

    /// X dresses the player's live tank — every one of its meshes wears the look, the turret's
    /// under the live servo node — and owns one mask set, the player's camera's; another tank's
    /// meshes are untouched, and the record's elements hang under the live hull.
    #[test]
    fn x_dresses_the_live_tank_for_the_players_camera() {
        let (mut app, player, meshes, camera) = world_glass_app();
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
        assert_eq!(glass_census(&mut app).0, 0, "off until the key");
        press_x(&mut app);
        run(&mut app, 0.2);
        assert!(app.world().resource::<OwnGlass>().on);
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
        let (dressed, items, viewers, ..) = glass_census(&mut app);
        assert_eq!(dressed, 2);
        assert!(
            items > 2,
            "the record's elements beyond the two mask copies"
        );
        assert_eq!(viewers, vec![camera], "the player's camera owns the masks");
        let hull = app.world().get::<crate::tank::Rig>(player).unwrap().hull;
        let world = app.world_mut();
        let under_hull = world
            .query_filtered::<&ChildOf, With<glass::GlassItem>>()
            .iter(world)
            .filter(|parent| parent.parent() == hull)
            .count();
        assert!(under_hull > 0, "the record hangs under the live hull");
    }

    /// Every deactivation — the key, a knockout, the gunner's optic, the tank gone — puts every
    /// mesh back in exactly the material it wore and leaves nothing of the glass: no tag, no
    /// item, no mask set, no image and no look material beyond what stood before.
    #[test]
    fn every_deactivation_restores_the_tank_exactly() {
        for ending in ["key", "knockout", "sight", "gone"] {
            let (mut app, player, meshes, _) = world_glass_app();
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
            let before = glass_census(&mut app);
            press_x(&mut app);
            run(&mut app, 0.2);
            let (dressed, items, ..) = glass_census(&mut app);
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
                    // The glass's own went with the tank; the rest is the tank's.
                    gone -= items;
                }
            }
            run(&mut app, 0.2);
            let after = glass_census(&mut app);
            assert_eq!((after.0, after.1), (0, 0), "{ending}: nothing of the glass");
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
            assert!(app.world().resource::<OwnGlass>().on == (ending != "key"));
        }
    }

    /// A corner that needs masks opening while the glass is on: two mask sets, each on its own
    /// viewer; the corner's close releases its own and the glass keeps the other, dressed
    /// throughout.
    #[test]
    fn the_corner_and_the_glass_each_own_their_viewers_masks() {
        let (mut app, _, _, camera) = world_glass_app();
        press_x(&mut app);
        run(&mut app, 0.2);
        assert_eq!(glass_census(&mut app).2, vec![camera]);
        stage_of(&mut app, OWN);
        let (shooter, _, vocabulary, report) = explained("s1_through_crew", 1200);
        let kind = CornerKind::of(report.impact.verdict);
        assert_eq!(
            kind,
            CornerKind::Glass,
            "fixture data: a frame that needs masks"
        );
        receive(&mut app, shooter, &vocabulary);
        let mut both = false;
        for _ in 0..(1.0 / STEP_S) as usize {
            step(&mut app);
            let mut viewers = glass_census(&mut app).2;
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
            }
            assert!(viewers.contains(&camera), "the glass keeps its set");
            assert_eq!(glass_census(&mut app).0, 2, "and stays dressed");
        }
        assert!(both, "two sets, each on its own viewer");
        run(&mut app, kind.duration() + 1.0);
        assert_eq!(census(&mut app).0, 0, "the corner closed");
        assert_eq!(glass_census(&mut app).2, vec![camera], "its set alone went");
        assert_eq!(glass_census(&mut app).0, 2);
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

    /// A knockout suspends the glass and X during the run it starts changes nothing; the glass
    /// returns on the respawn, the preference untouched.
    #[test]
    fn x_during_a_run_is_a_no_op_and_the_glass_returns_after_respawn() {
        let (mut app, player, _, _) = world_glass_app();
        press_x(&mut app);
        run(&mut app, 0.2);
        assert_eq!(glass_census(&mut app).0, 2);
        let (explanation, vocabulary) = killing("s1_through_crew");
        receive(&mut app, explanation, &vocabulary);
        app.world_mut().entity_mut(player).insert(TankKnockedOut {
            reason: crate::damage::KnockoutReason::CrewLoss,
        });
        step(&mut app);
        assert_eq!(glass_census(&mut app).0, 0, "suspended at the knockout");
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
            app.world().resource::<OwnGlass>().on,
            "the press changed nothing"
        );
        assert_eq!(glass_census(&mut app).0, 0);
        let mut tank = app.world_mut().entity_mut(player);
        tank.remove::<TankKnockedOut>();
        tank.insert(CombatantLife(1));
        run(&mut app, 0.2);
        assert!(!running(&app));
        assert_eq!(glass_census(&mut app).0, 2, "back on the new life");
    }
}
