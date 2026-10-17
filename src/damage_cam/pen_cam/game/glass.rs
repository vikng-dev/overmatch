//! The victim glass's two layers and its key: the player's own tank as its replicated state stands
//! ([`LiveState`]), under the latest record that struck it ([`latest`]).
//!
//! * **The tint reads live state only.** Each component's fill is its live HP points over its pool
//!   ([`LiveState::boxes`]), never the record's: the record supplies the path and the cones, and a
//!   component's dose or HP at the shot never reaches the glass. An unstruck component (no point
//!   lost) is the base layout, untinted.
//! * **The latest rule.** Of the victim records of the player's current life whose impact is no
//!   longer waiting, the newest that [`tells`] something; with none telling, the newest. A record
//!   that tells nothing (a clean ricochet off the face) therefore never displaces one that did.

use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

use super::super::plan::{BoxPlan, Plan};
use super::super::timeline::CornerKind;
use super::Recency;
use crate::ballistics::ComponentHealth;
use crate::command::Bindings;
use crate::damage::{Ammo, CrewStation, Dead, FunctionRole, TankVolumes};
use crate::damage_cam::delivery::{
    ExplanationKey, ImpactPresence, ShotExplanations, StoredExplanation,
};
use crate::damage_cam::shapes::TankShapes;
use crate::damage_cam::wire::{Audience, TankLife};
use crate::damage_cam::{PenetrationReport, Role};

/// Whether the own-tank key holds the victim glass open this frame.
#[derive(Resource, Default, Clone, Copy, PartialEq, Debug)]
pub(crate) struct GlassHold(pub(crate) bool);

/// Read [`Bindings::own_tank_glass`] into [`GlassHold`]: held while the key is down and the cursor
/// is captured, so a menu or a pause lets go of it.
pub(super) fn read_hold(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    bindings: Res<Bindings>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut hold: ResMut<GlassHold>,
) {
    let held =
        crate::state::cursor_locked(cursor) && bindings.own_tank_glass.pressed(&keys, &mouse);
    hold.set_if_neq(GlassHold(held));
}

/// One health-bearing volume of the player's tank as its replicated state stands.
#[derive(Clone, Debug, PartialEq)]
struct LiveBox {
    name: String,
    role: Role,
    current: u32,
    max: u32,
    dead: bool,
}

/// Every health-bearing volume of the player's tank, in the tank's volume order: its HP points
/// (`ComponentHealth`, which `NetCrew` writes on a network client) and its crew death.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct LiveState(Vec<LiveBox>);

#[derive(QueryData)]
pub(super) struct LiveVolume {
    name: &'static Name,
    health: &'static ComponentHealth,
    dead: Has<Dead>,
    crew: Has<CrewStation>,
    ammo: Has<Ammo>,
    function: Option<&'static FunctionRole>,
}

impl LiveState {
    pub(super) fn read(volumes: &TankVolumes, query: &Query<LiveVolume>) -> Self {
        Self(
            volumes
                .iter()
                .filter_map(|volume| query.get(volume).ok())
                .map(|volume| LiveBox {
                    name: volume.name.as_str().to_owned(),
                    role: if volume.crew {
                        Role::Crew
                    } else if volume.ammo {
                        Role::Ammo
                    } else if volume.function == Some(&FunctionRole::Engine) {
                        Role::Engine
                    } else {
                        Role::Module
                    },
                    current: volume.health.current,
                    max: volume.health.max,
                    dead: volume.dead,
                })
                .collect(),
        )
    }

    /// Each volume as the glass draws it, its shape from `shapes`: its pool and its live points,
    /// lost at none left or at its crewman's death. Nothing of a record.
    fn boxes(&self, shapes: &TankShapes) -> Vec<BoxPlan> {
        self.0
            .iter()
            .map(|live| {
                let shape = shapes.get(&live.name);
                BoxPlan {
                    name: live.name.clone(),
                    role: live.role,
                    center: shape.map(|shape| shape.center()),
                    heights: shape.map(|shape| {
                        let (lo, hi) = shape.bounds();
                        (lo.y, hi.y)
                    }),
                    count: live.max,
                    hp: live.current,
                    line: 0.0,
                    field: 0.0,
                    lost_hp: 0,
                    reached_at: None,
                    lost: live.dead || (live.max > 0 && live.current == 0),
                }
            })
            .collect()
    }
}

/// The glass's plan: `record`'s path and cones, or no round at all without one, over `live`'s
/// components, posed in `shapes`.
pub(super) fn plan(
    record: Option<&PenetrationReport>,
    live: &LiveState,
    shapes: &TankShapes,
) -> Plan {
    let boxes = live.boxes(shapes);
    match record {
        Some(report) => {
            let mut plan = Plan::new(report, shapes);
            plan.kind = CornerKind::Victim;
            plan.boxes = boxes;
            plan
        }
        None => Plan::bare(shapes, boxes),
    }
}

/// Whether a record tells the player something about their tank: the line put a chord into a
/// plate, a component lost HP points, or the shot had a consequence.
pub(super) fn tells(report: &PenetrationReport) -> bool {
    !report.plates.is_empty()
        || report.doses.iter().any(|dose| dose.lost > 0)
        || !report.consequences.is_empty()
}

/// Whether `stored` is a victim projection of a shot on `life`: the struck player's copy, never the
/// shooter's.
pub(super) fn struck(stored: &StoredExplanation, life: TankLife) -> bool {
    stored.explanation.audience != Audience::Shooter && stored.explanation.struck == life
}

/// The record the glass draws for `life`, by the latest rule (module doc).
pub(super) fn latest(
    store: &ShotExplanations,
    life: TankLife,
) -> Option<(ExplanationKey, &StoredExplanation)> {
    let mut newest = None;
    let mut newest_telling = None;
    for stored in store.live(life) {
        if !struck(stored, life) || stored.presence == ImpactPresence::Pending {
            continue;
        }
        let key = ExplanationKey::of(&stored.explanation);
        let recency = Recency::of(key, &stored.explanation);
        let candidate = Some((recency, key, stored));
        if newest.is_none_or(|(r, _, _)| recency > r) {
            newest = candidate;
        }
        if tells(&stored.report) && newest_telling.is_none_or(|(r, _, _)| recency > r) {
            newest_telling = candidate;
        }
    }
    newest_telling
        .or(newest)
        .map(|(_, key, stored)| (key, stored))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage_cam::delivery::tests::report;
    use crate::damage_cam::shapes::TankAsset;

    /// Each box's tint inputs: name, pool, points held, points lost, lost.
    fn tints(plan: &Plan) -> Vec<(String, u32, u32, u32, bool)> {
        plan.boxes
            .iter()
            .map(|b| (b.name.clone(), b.count, b.hp, b.lost_hp, b.lost))
            .collect()
    }

    /// The live state of `record`'s components as they stood at its shot.
    fn standing(record: &PenetrationReport) -> LiveState {
        LiveState(
            record
                .components
                .iter()
                .map(|component| LiveBox {
                    name: component.name.clone(),
                    role: component.role,
                    current: component.hp,
                    max: component.count,
                    dead: false,
                })
                .collect(),
        )
    }

    /// The tint is the live state's, never the record's: a record with other HP points, doses or
    /// consequences, or no record, tints the same; a live HP change tints differently.
    #[test]
    fn the_tint_follows_live_state_only() {
        let record = report("s9_one_box_lost");
        let shapes = TankAsset::load(record.vehicle.as_deref().unwrap())
            .unwrap()
            .shapes(&record.servo_deg);
        let live = standing(&record);
        assert!(
            live.0.iter().any(|b| b.max > 1),
            "fixture data: a component with a pool to lose points of"
        );
        let base = tints(&plan(Some(&record), &live, &shapes));

        let mut rewritten = record.clone();
        for component in &mut rewritten.components {
            component.hp = 0;
        }
        for dose in &mut rewritten.doses {
            dose.lost += 1;
        }
        rewritten.consequences.clear();
        assert_eq!(tints(&plan(Some(&rewritten), &live, &shapes)), base);
        assert_eq!(tints(&plan(None, &live, &shapes)), base);

        let mut hurt = live.clone();
        let b = hurt.0.iter_mut().find(|b| b.max > 1).unwrap();
        b.current = b.max - 1;
        assert_ne!(tints(&plan(Some(&record), &hurt, &shapes)), base);
        let mut killed = live.clone();
        killed.0[0].dead = true;
        assert_ne!(tints(&plan(Some(&record), &killed, &shapes)), base);
    }

    /// A component no point of which is lost draws untinted: the base layout.
    #[test]
    fn an_unstruck_component_is_the_base_layout() {
        let record = report("s9_one_box_lost");
        let shapes = TankAsset::load(record.vehicle.as_deref().unwrap())
            .unwrap()
            .shapes(&record.servo_deg);
        let mut live = standing(&record);
        for b in &mut live.0 {
            b.current = b.max;
        }
        for b in plan(Some(&record), &live, &shapes).boxes {
            assert!(!b.lost && b.pool_lost() == 0.0, "{}", b.name);
        }
    }
}
