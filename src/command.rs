//! The command layer: raw device reads, translated through the player's [`Bindings`] into a
//! serializable per-tank [`TankCommand`] — the seam authoritative multiplayer hangs off. The sim
//! consumes only the command, never devices, so the same simulation runs from a local player, a
//! replayed tick, or (later) a network peer. Gathered once per render frame, before the fixed
//! loop, which is where input belongs when the sim runs on a fixed clock.

use bevy::ecs::lifecycle::{Add, Remove};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::damage::CrewStation;
use crate::firecontrol::Ranging;
use crate::state::GameplaySet;
use crate::tank::{Controlled, Tank};

/// One tick's worth of driver intent for one tank: plain data, serializable — exactly what a
/// client will send per tick under authoritative multiplayer. Carries the *target* values; the
/// response ramp toward them is vehicle feel and lives in the sim (`driving`), so a command is
/// replay-safe and an analog stick later just supplies a finer target.
#[derive(Component, Default, Clone, Copy, Serialize, Deserialize)]
pub struct TankCommand {
    /// Target throttle in [-1, 1]: forward/reverse drive.
    pub throttle: f32,
    /// Target steer in [-1, 1]: differential yaw, positive to the right.
    pub steer: f32,
    /// Fire the primary weapon. An *edge*: latched by `gather_commands` from the click, held
    /// until the first fixed tick consumes it (`consume_edges`) — so a click between ticks is
    /// neither lost (frame with zero ticks) nor doubled (frame with several).
    pub fire_primary: bool,
    /// Fire the secondary weapon(s). A *level*: true while the trigger is held; the MGs cycle on
    /// their own reload.
    pub fire_secondary: bool,
    /// The committed aim *intention*: a hull-local point every servo chases (ADR-0012's one aim
    /// point, moved onto the command). Hull-local so it rides with the tank (unstabilized WW2
    /// lay) and stays valid regardless of hull replication error. Absolute each command, like
    /// Quake/Source viewangles — a dropped packet loses nothing. `None` = no commitment yet;
    /// written by the per-view commit systems (`aim::commit_aim`, `sight::drive_gunner_aim`),
    /// not `gather_commands`.
    pub aim: Option<Vec3>,
    /// The player-dialed range (m). The sim lobs the bore above the aim intention by the range
    /// table's superelevation for this range — dial wrong and the shot falls short or long.
    pub range: f32,
    /// Start or cancel a crew swap. An *edge* like [`fire_primary`](Self::fire_primary): written
    /// by the crew bar's two-tap input, consumed by one fixed tick. The sim
    /// (`damage::apply_crew_swap_commands`) validates it against the tank's own seats — crew
    /// reassignment changes capabilities, so the server must own it.
    pub crew_swap: Option<CrewSwap>,
}

/// One crew-swap intent, in *stations* (semantic seat identity — stable on the wire, unlike
/// entity ids). `Start` begins the timed swap between two seats; `Cancel` aborts an in-flight one
/// (any crew-bar tap while a swap runs).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum CrewSwap {
    Start(CrewStation, CrewStation),
    Cancel,
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
    app.add_observer(attach_command)
        .add_observer(clear_command_on_release)
        .add_systems(
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
                .in_set(GameplaySet),
        );
}

/// Every tank carries a `TankCommand` from birth — zeroed until someone (local player now, a
/// network peer later) writes it.
fn attach_command(add: On<Add, Tank>, mut commands: Commands) {
    commands.entity(add.entity).insert(TankCommand::default());
}

/// Translate devices through the bindings into the controlled tank's command. The only place in
/// the sim path that reads a device.
fn gather_commands(
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
        if command.fire_primary || command.crew_swap.is_some() {
            command.fire_primary = false;
            command.crew_swap = None;
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
