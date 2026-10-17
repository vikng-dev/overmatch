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
//! * **The hold.** While the own-tank key is down ([`GlassHold`],
//!   `command::Bindings::own_tank_glass`) the glass holds the slot, with or without a record and
//!   whatever the settings: it crossfades over a turn on show, which never resumes, and the
//!   slot admits nothing presented while it holds. Released, it closes over
//!   [`timeline::CLOSE_S`]. A teardown other than its own close is final for the press holding
//!   it: the key opens the glass again only once let go. TUNED.
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
//! No input reaches the corner; the hold key only opens the glass.

mod glass;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;

pub(crate) use self::glass::GlassHold;
use self::glass::{LiveState, LiveVolume};
use super::hud::{self, CornerImages, Glyphs};
use super::look::{MaskCamera, Masks};
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
use crate::tank::Controlled;
use crate::{CombatantId, CombatantLife};

/// The corner slot on a client composition: the stage's materials, the own-tank key and [`wire`].
pub fn plugin(app: &mut App) {
    super::materials(app);
    wire(app);
    app.add_systems(Update, glass::read_hold.before(drive));
}

/// Close the slot now and drop everything it holds.
#[derive(Event, Clone, Copy, Debug)]
pub(crate) struct CloseCorner;

/// The slot's state and systems, without the materials and the key (a headless test registers the
/// materials' assets alone and sets [`GlassHold`] itself).
fn wire(app: &mut App) {
    super::presentation(app);
    app.init_resource::<Slot>()
        .init_resource::<GlassHold>()
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

impl Slot {
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

    /// Despawn every entity of the corner and drop every image and resource it holds.
    fn teardown(&mut self, state: &mut Slot) {
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

/// The player's tank: who it is this life, its file and its components' live state.
#[derive(SystemParam)]
struct Player<'w, 's> {
    tank: Query<
        'w,
        's,
        (
            &'static CombatantId,
            &'static CombatantLife,
            Option<&'static TankVolumes>,
            Option<&'static TankSpecHandle>,
            Has<TankKnockedOut>,
        ),
        With<Controlled>,
    >,
    volumes: Query<'w, 's, LiveVolume>,
}

impl Player<'_, '_> {
    /// The tank's life, while it has one and is not knocked out.
    fn life(&self) -> Option<TankLife> {
        let (&combatant, &life, _, _, knocked_out) = self.tank.single().ok()?;
        (!knocked_out).then_some(TankLife { combatant, life })
    }

    /// The tank's file, as a record names it.
    fn vehicle(&self) -> Option<String> {
        let (_, _, _, spec, _) = self.tank.single().ok()?;
        let path = spec?.0.path()?;
        Some(path.path().to_string_lossy().into_owned())
    }

    fn live(&self) -> LiveState {
        self.tank
            .single()
            .ok()
            .and_then(|(_, _, volumes, _, _)| volumes)
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
#[allow(clippy::too_many_arguments)]
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
    let holding = held_down || state.hold.is_some_and(|held| held.released.is_none());

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
                ImpactPresence::Presented if admitted && !holding => {
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
        match &mut state.hold {
            Some(held) if held_down => held.released = None,
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
        let Some(mut held) = state.hold else {
            return;
        };
        if let Some(released) = held.released
            && (held.start.is_none() || now - released >= timeline::CLOSE_S)
        {
            rig.teardown(&mut state);
            state.reset();
            return;
        }
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
/// app seconds (`Time` as `Update` sees it), as the own-tank key would, for captures of the hold.
/// Mounted only by the offline composition.
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
            if (from..to).contains(&time.elapsed_secs()) {
                hold.0 = true;
            }
        })
        .after(glass::read_hold)
        .before(drive),
    );
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::damage_cam::PenetrationReport;
    use crate::damage_cam::delivery::tests::{SHOOTER, STRUCK, recorded, report, vocabulary};
    use crate::damage_cam::pen_cam::stage::StageCamera;
    use crate::damage_cam::wire::{BuiltExplanation, ShotVocabulary};

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
        stage_of(
            app,
            report
                .vehicle
                .as_deref()
                .expect("the record names its tank file"),
        );
    }

    /// A stage view of tank file `vehicle` standing in for its glb.
    fn stage_of(app: &mut App, vehicle: &str) {
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

    /// Of the player's records, a ricochet that tells nothing is the latest only while none that
    /// tells is stored; a newer one never displaces a crossing, and the glass draws the crossing's
    /// path.
    #[test]
    fn a_ricochet_never_displaces_a_crossing_as_latest() {
        let (mut app, _) = glass_app(Settings::default());
        let (_, crossing, vocabulary, _) = incoming("s1_through_crew", 1200);
        let (_, ricochet, ricochet_vocabulary, _) = incoming("s3_ricochet", 1300);
        let latest = |app: &App| {
            glass::latest(app.world().resource::<ShotExplanations>(), me()).map(|(key, _)| key)
        };
        receive(&mut app, ricochet.clone(), &ricochet_vocabulary);
        assert_eq!(latest(&app), Some(ExplanationKey::of(&ricochet)));
        receive(&mut app, crossing.clone(), &vocabulary);
        assert!(ricochet.contact_tick > crossing.contact_tick);
        assert_eq!(latest(&app), Some(ExplanationKey::of(&crossing)));
        run(&mut app, 4.0);
        stage_of(&mut app, OWN);
        hold(&mut app, true);
        run(&mut app, 0.5);
        let plan = exhibit(&app);
        assert_eq!(plan.kind, CornerKind::Victim);
        assert!(!plan.crossings.is_empty(), "the crossing's path");
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
}
