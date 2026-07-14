//! Serializable tank commands and device-to-command translation.

use bevy::ecs::entity::{EntityMapper, MapEntities};
use bevy::ecs::lifecycle::Remove;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::damage::CrewStation;
use crate::firecontrol::Ranging;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::Controlled;

/// One tick's driver intent for a tank.
#[derive(Component, Default, Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Reflect)]
pub struct TankCommand {
    /// Target throttle in [-1, 1]: forward/reverse drive.
    pub throttle: f32,
    /// Target steer in [-1, 1]: differential yaw, positive to the right.
    pub steer: f32,
    /// Primary fire edge, latched until one fixed tick consumes it.
    pub fire_primary: bool,
    /// Secondary fire level. It is a consumable and must be attested for the current tick.
    pub fire_secondary: bool,
    /// Hull-local aim point chased by every servo; `None` means no commitment yet.
    pub aim: Option<Vec3>,
    /// Player-dialed range (m) for superelevation.
    pub range: f32,
    /// Crew-swap edge, validated against the tank's seats by simulation authority.
    pub crew_swap: Option<CrewSwap>,
    /// Respawn edge; authority validates that this tank is knocked out.
    pub respawn: bool,
    /// Tick this command was authored for.
    ///
    /// Invariant: the authority commits consumables only when this equals the input tick; levels
    /// and absolute intent may be held through an unattested gap.
    pub for_tick: u32,
}

impl TankCommand {
    /// Clear the complete edge set.
    ///
    /// Invariant: both normal consumption and unattested failure use this method.
    pub fn clear_edges(&mut self) {
        self.fire_primary = false;
        self.crew_swap = None;
        self.respawn = false;
    }

    /// Fail closed for every consumable: edges plus sustained secondary fire.
    ///
    /// Invariant: do not fold `fire_secondary` into [`clear_edges`](Self::clear_edges), which runs
    /// after every fixed tick.
    pub fn fail_consumables_closed(&mut self) {
        self.clear_edges();
        self.fire_secondary = false;
    }

    /// Whether any edge field is currently latched — the read counterpart to [`clear_edges`], so
    /// the edge set lives in exactly one place. [`consume_edges`] uses it to skip the mutable
    /// touch (and its change-detection churn) on a command with no edge to clear.
    pub fn has_edge(&self) -> bool {
        self.fire_primary || self.crew_swap.is_some() || self.respawn
    }
}

/// One crew-swap intent, in *stations* (semantic seat identity — stable on the wire, unlike
/// entity ids). `Start` begins the timed swap between two seats; `Cancel` aborts an in-flight one
/// (any crew-bar tap while a swap runs).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Reflect)]
pub enum CrewSwap {
    Start(CrewStation, CrewStation),
    Cancel,
}

// `TankCommand` has no `Entity` fields (`aim`/`range` are plain data, `crew_swap` addresses seats
// by `CrewStation`, not entity id) — lightyear's native input plugin requires `MapEntities` on the
// input type regardless, so this is a no-op, matching the examples' pattern for entity-less inputs.
impl MapEntities for TankCommand {
    fn map_entities<M: EntityMapper>(&mut self, _entity_mapper: &mut M) {}
}

/// The player's device→action map — pure data, no UI. A rebinding screen later just edits this
/// resource; nothing else in the game knows which physical key means "forward".
#[derive(Resource)]
pub struct Bindings {
    pub throttle: AxisKeys,
    pub steer: AxisKeys,
    pub fire_primary: ButtonBinding,
    pub fire_secondary: ButtonBinding,
}

/// A [-1, 1] axis from a key pair.
pub struct AxisKeys {
    pub pos: KeyCode,
    pub neg: KeyCode,
}

impl AxisKeys {
    fn value(&self, keys: &ButtonInput<KeyCode>) -> f32 {
        keys.pressed(self.pos) as i8 as f32 - keys.pressed(self.neg) as i8 as f32
    }
}

/// One bindable button — keyboard or mouse, so "fire" can live on either.
#[derive(Clone, Copy)]
pub enum ButtonBinding {
    Key(KeyCode),
    Mouse(MouseButton),
}

impl ButtonBinding {
    fn pressed(&self, keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>) -> bool {
        match *self {
            Self::Key(key) => keys.pressed(key),
            Self::Mouse(button) => mouse.pressed(button),
        }
    }

    fn just_pressed(&self, keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>) -> bool {
        match *self {
            Self::Key(key) => keys.just_pressed(key),
            Self::Mouse(button) => mouse.just_pressed(button),
        }
    }
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            throttle: AxisKeys {
                pos: KeyCode::KeyW,
                neg: KeyCode::KeyS,
            },
            steer: AxisKeys {
                pos: KeyCode::KeyD,
                neg: KeyCode::KeyA,
            },
            fire_primary: ButtonBinding::Mouse(MouseButton::Left),
            fire_secondary: ButtonBinding::Key(KeyCode::Space),
        }
    }
}

/// Systems that clear the commands' latched edges, at the end of each fixed tick. Sim systems
/// that consume an edge (`shooting::fire`) order themselves `.before(ConsumeCommandEdges)`, so
/// exactly one tick sees each click.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConsumeCommandEdges;

/// The command core, shared by every world that runs the sim (the game and the armor sandbox):
/// every tank carries a command, edges are consumed each tick, and losing `Controlled` zeroes the
/// command. No devices — the game adds those via [`plugin`]; the sandbox writes commands from its
/// own controls (the crew bar).
pub fn core_plugin(app: &mut App) {
    app.add_observer(clear_command_on_release).add_systems(
        FixedUpdate,
        consume_edges
            .in_set(ConsumeCommandEdges)
            .in_set(GameplaySet),
    );
}

/// Device gather — client-side: the only device→command translation. Requires [`core_plugin`]
/// (mounted by the sim side).
pub fn client_plugin(app: &mut App) {
    app.init_resource::<Bindings>()
        // Once per render frame, before the fixed loop runs its 0..N sim ticks — so every tick
        // this frame sees the same, freshest command, and edge inputs latch here without being
        // missed or doubled by the fixed clock.
        .add_systems(
            RunFixedMainLoop,
            gather_commands
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .in_set(PlayerInputSet)
                .in_set(GameplaySet),
        );
}

/// Translate devices through the bindings into the controlled tank's command. The only place in
/// the sim path that reads a device. `pub(crate)` so the other `BeforeFixedMainLoop` command
/// writers (`firecontrol::adjust_range`, `sight::drive_gunner_aim`) can pin an explicit order
/// against it — both share the `Ranging`/`TankCommand` it touches.
pub(crate) fn gather_commands(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    bindings: Res<Bindings>,
    ranging: Res<Ranging>,
    mut tanks: Query<&mut TankCommand, With<Controlled>>,
) {
    for mut command in &mut tanks {
        command.throttle = bindings.throttle.value(&keys);
        command.steer = bindings.steer.value(&keys);
        // `|=`: a click must survive frames the fixed loop skips, until a tick consumes it.
        command.fire_primary |= bindings.fire_primary.just_pressed(&keys, &mouse);
        command.fire_secondary = bindings.fire_secondary.pressed(&keys, &mouse);
        // The dial itself (`Ranging`, scroll in the optic) is client-side control state; the
        // command carries its absolute value. `aim` is written by the per-view commit systems.
        command.range = ranging.range;
    }
}

/// Clear the latched edges at the end of each fixed tick — the consuming half of the latch
/// contract described on [`TankCommand::fire_primary`].
fn consume_edges(mut tanks: Query<&mut TankCommand>) {
    for mut command in &mut tanks {
        // Read through the shared edge test first, so a command with no edge is never touched
        // mutably (no spurious change-detection); the field set itself lives in `clear_edges`.
        if command.has_edge() {
            command.clear_edges();
        }
    }
}

/// Zero the command when a tank loses `Controlled` (the Tab swap), so it doesn't drive on with
/// the last gathered input forever.
fn clear_command_on_release(remove: On<Remove, Controlled>, mut tanks: Query<&mut TankCommand>) {
    if let Ok(mut command) = tanks.get_mut(remove.entity) {
        *command = TankCommand::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::CrewStation;

    /// Every edge field is reported by [`TankCommand::has_edge`] and reset by
    /// [`TankCommand::clear_edges`] — the single-source-of-truth contract the edge set hangs off
    /// (`consume_edges` and the net input bridge both route through these two). A new edge added to
    /// one method but not the other fails this: `has_edge` would still report the field `clear_edges`
    /// left latched.
    #[test]
    fn clear_edges_resets_every_edge_has_edge_reports() {
        // Each latched-edge variant in isolation: has_edge true, then clear_edges makes it false.
        let edges: [fn(&mut TankCommand); 3] = [
            |c| c.fire_primary = true,
            |c| c.crew_swap = Some(CrewSwap::Start(CrewStation::Gunner, CrewStation::Loader)),
            |c| c.respawn = true,
        ];
        for set_edge in edges {
            let mut command = TankCommand::default();
            assert!(!command.has_edge(), "default command has no edge");
            set_edge(&mut command);
            assert!(command.has_edge(), "a latched edge is reported by has_edge");
            command.clear_edges();
            assert!(!command.has_edge(), "clear_edges resets the latched edge");
        }
    }

    /// `clear_edges` touches ONLY the edge fields — the levels and absolutes ride through untouched
    /// (the property `consume_edges` and the net input bridge both depend on). Note it leaves
    /// `fire_secondary` alone: that is a CONSUMABLE but not an EDGE, and folding it in here would
    /// kill sustained fire (`consume_edges` runs every tick). Only
    /// [`TankCommand::fail_consumables_closed`] clears both sets, and only on an unattested tick.
    #[test]
    fn clear_edges_preserves_levels_and_absolutes() {
        let mut command = TankCommand {
            throttle: 0.5,
            steer: -0.5,
            fire_secondary: true,
            aim: Some(Vec3::X),
            range: 800.0,
            fire_primary: true,
            crew_swap: Some(CrewSwap::Cancel),
            respawn: true,
            for_tick: 0,
        };
        command.clear_edges();
        assert_eq!(command.throttle, 0.5);
        assert_eq!(command.steer, -0.5);
        assert!(command.fire_secondary);
        assert_eq!(command.aim, Some(Vec3::X));
        assert_eq!(command.range, 800.0);
        assert!(!command.has_edge(), "all edges cleared");
    }
}
